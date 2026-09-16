use crate::error::{Error, Result};
use crate::graph::{
    Abi, CONTAINS_TAG, EDGE_EXTERNAL, Edge, EdgeKind, Kind, Node, Segment, Tier, TrustClass,
};
use crate::raw::{all_zero, bytes_at, hash_at, u8_at, u16_at, u32_at};

pub const MANIFEST_HDR_LEN: usize = 48;
pub const DIR_ENTRY_LEN: usize = 24;
pub const NODE_LEN: usize = 32;
pub const EDGE_LEN: usize = 8;
pub const SEGMENT_LEN: usize = 96;
pub const CAP_LEN: usize = 8;

pub const SECT_STRINGS: u16 = 1;
pub const SECT_NODES: u16 = 2;
pub const SECT_EDGES: u16 = 3;
pub const SECT_HASHES: u16 = 4;
pub const SECT_SEGMENTS: u16 = 5;
pub const SECT_CAPS: u16 = 6;
pub const SECT_EXTREFS: u16 = 7;
pub const SECT_SRCSPANS: u16 = 8;
pub const SECT_DICTREF: u16 = 9;
pub const SECT_REGIONS: u16 = 10;
pub const SECT_ROUTING: u16 = 11;

const SECT_MUST_UNDERSTAND: u16 = 1 << 0;
pub const SECT_ZSTD: u16 = 1 << 1;

/// A decompressed section may not exceed this. `orig_len` is attacker-controlled, and a
/// declared length is an allocation request until something bounds it.
pub const MAX_SECTION_BYTES: u32 = 64 << 20;

pub mod manifest_flags {
    pub const HOT_ZSTD: u16 = 1 << 0;
    pub const COLD_ZSTD: u16 = 1 << 1;
    pub const USES_DICT: u16 = 1 << 2;
}

/// Section bytes, borrowed when stored raw and owned when they had to be decompressed. Keeping
/// the borrowed case really borrowed is the point: an uncompressed file is still read in place.
#[derive(Debug)]
enum Sect<'a> {
    Borrowed(&'a [u8], u32),
    Owned(Vec<u8>, u32),
}

impl Sect<'_> {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Borrowed(b, _) => b,
            Self::Owned(v, _) => v,
        }
    }

    fn count(&self) -> u32 {
        match self {
            Self::Borrowed(_, c) | Self::Owned(_, c) => *c,
        }
    }
}

#[derive(Debug)]
pub struct Manifest<'a> {
    pub version_idx: u32,
    pub license_idx: u32,
    pub node_count: u32,
    pub flags: u16,
    /// `(offset, stored_len, uncompressed_len)`.
    pub hot: (u32, u32, u32),
    pub cold: (u32, u32, u32),
    pub dict_hash: Option<[u8; 32]>,
    /// `(stored_len, BLAKE3)` of the routing block. The block lives outside the manifest, so
    /// without this it would be committed by nothing.
    pub routing: Option<(u32, [u8; 32])>,
    pub region_hashes: Option<([u8; 32], [u8; 32])>,
    strings: Option<Sect<'a>>,
    nodes: Option<Sect<'a>>,
    edges: Option<Sect<'a>>,
    hashes: Option<Sect<'a>>,
    segments: Option<Sect<'a>>,
    caps: Option<Sect<'a>>,
}

fn inflate(src: &[u8], orig_len: u32, dict: Option<&crate::Dictionary>) -> Result<Vec<u8>> {
    if orig_len > MAX_SECTION_BYTES {
        return Err(Error::DeclaredLenMismatch {
            what: "section orig_len too large",
        });
    }
    let out = crate::codec::decompress(src, orig_len as usize, dict).map_err(|_| {
        Error::DeclaredLenMismatch {
            what: "zstd section",
        }
    })?;
    if out.len() != orig_len as usize {
        return Err(Error::DeclaredLenMismatch {
            what: "section orig_len",
        });
    }
    Ok(out)
}

impl<'a> Manifest<'a> {
    pub(crate) fn parse(raw: &'a [u8], dict: Option<&crate::Dictionary>) -> Result<Self> {
        if raw.len() < MANIFEST_HDR_LEN {
            return Err(Error::Truncated {
                off: 0,
                need: MANIFEST_HDR_LEN,
            });
        }
        let sect_count = u16_at(raw, 0x00)?;
        // 0x04..0x0C held name_idx and desc_idx before the routing block replaced them.
        all_zero(raw, 0x04, 8, "manifest reserved 0x04")?;
        let mut m = Self {
            version_idx: u32_at(raw, 0x0C)?,
            license_idx: u32_at(raw, 0x10)?,
            node_count: u32_at(raw, 0x14)?,
            flags: u16_at(raw, 0x02)?,
            hot: (u32_at(raw, 0x18)?, u32_at(raw, 0x1C)?, u32_at(raw, 0x28)?),
            cold: (u32_at(raw, 0x20)?, u32_at(raw, 0x24)?, u32_at(raw, 0x2C)?),
            dict_hash: None,
            routing: None,
            region_hashes: None,
            strings: None,
            nodes: None,
            edges: None,
            hashes: None,
            segments: None,
            caps: None,
        };

        for i in 0..sect_count as usize {
            let off = MANIFEST_HDR_LEN + DIR_ENTRY_LEN * i;
            let id = u16_at(raw, off)?;
            let flags = u16_at(raw, off + 0x02)?;
            let s_off = u32_at(raw, off + 0x04)?;
            let s_len = u32_at(raw, off + 0x08)?;
            let count = u32_at(raw, off + 0x0C)?;
            let orig_len = u32_at(raw, off + 0x10)?;
            all_zero(raw, off + 0x14, 4, "section reserved 0x14")?;

            let end = s_off
                .checked_add(s_len)
                .ok_or(Error::OffsetOverflow { what: "section" })?;
            if end as usize > raw.len() {
                return Err(Error::RangeOutOfBounds {
                    what: "section",
                    off: s_off,
                    len: s_len,
                });
            }
            if !s_off.is_multiple_of(8) {
                return Err(Error::SectionMisaligned(id));
            }
            let compressed = flags & SECT_ZSTD != 0;
            if !compressed && orig_len != s_len {
                return Err(Error::DeclaredLenMismatch {
                    what: "section orig_len",
                });
            }

            if id == SECT_REGIONS {
                m.region_hashes = Some((
                    hash_at(raw, s_off as usize)?,
                    hash_at(raw, s_off as usize + 32)?,
                ));
                continue;
            }

            if m.absorb_sidecar(raw, id, s_off)? {
                continue;
            }
            let slot = match id {
                SECT_STRINGS => &mut m.strings,
                SECT_NODES => &mut m.nodes,
                SECT_EDGES => &mut m.edges,
                SECT_HASHES => &mut m.hashes,
                SECT_SEGMENTS => &mut m.segments,
                SECT_CAPS => &mut m.caps,
                SECT_EXTREFS | SECT_SRCSPANS => continue,
                other => {
                    if flags & SECT_MUST_UNDERSTAND != 0 {
                        return Err(Error::UnknownSection(other));
                    }
                    continue;
                }
            };
            if slot.is_some() {
                return Err(Error::DuplicateSection(id));
            }
            let stored = bytes_at(raw, s_off as usize, s_len as usize)?;
            *slot = Some(if compressed {
                Sect::Owned(inflate(stored, orig_len, dict)?, count)
            } else {
                Sect::Borrowed(stored, count)
            });
        }

        Self::check_fixed(SECT_NODES, m.nodes.as_ref(), NODE_LEN)?;
        Self::check_fixed(SECT_EDGES, m.edges.as_ref(), EDGE_LEN)?;
        Self::check_fixed(SECT_HASHES, m.hashes.as_ref(), 32)?;
        Self::check_fixed(SECT_SEGMENTS, m.segments.as_ref(), SEGMENT_LEN)?;
        Self::check_fixed(SECT_CAPS, m.caps.as_ref(), CAP_LEN)?;

        if m.node_count != m.nodes.as_ref().map_or(0, Sect::count) {
            return Err(Error::SectionCountMismatch { id: SECT_NODES });
        }
        Ok(m)
    }

    /// Sections that are a single fixed record rather than a table, and that the rest of the
    /// parser reads as scalars. Returns whether `id` was one of them.
    fn absorb_sidecar(&mut self, raw: &'a [u8], id: u16, off: u32) -> Result<bool> {
        let at = off as usize;
        match id {
            SECT_DICTREF => self.dict_hash = Some(hash_at(raw, at)?),
            SECT_ROUTING => self.routing = Some((u32_at(raw, at)?, hash_at(raw, at + 4)?)),
            SECT_REGIONS => {
                self.region_hashes = Some((hash_at(raw, at)?, hash_at(raw, at + 32)?));
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn check_fixed(id: u16, s: Option<&Sect<'_>>, unit: usize) -> Result<()> {
        let Some(s) = s else { return Ok(()) };
        let len = u32::try_from(s.bytes().len()).unwrap_or(u32::MAX);
        let unit_u32 = u32::try_from(unit).expect("record sizes are small");
        if !len.is_multiple_of(unit_u32) {
            return Err(Error::SectionLenNotMultiple { id, len, unit });
        }
        if len / unit_u32 != s.count() {
            return Err(Error::SectionCountMismatch { id });
        }
        Ok(())
    }

    #[must_use]
    pub fn string_count(&self) -> u32 {
        self.strings.as_ref().map_or(0, Sect::count)
    }

    #[must_use]
    pub fn edge_count(&self) -> u32 {
        self.edges.as_ref().map_or(0, Sect::count)
    }

    #[must_use]
    pub fn hash_count(&self) -> u32 {
        self.hashes.as_ref().map_or(0, Sect::count)
    }

    #[must_use]
    pub fn segment_count(&self) -> u32 {
        self.segments.as_ref().map_or(0, Sect::count)
    }

    /// # Errors
    /// Rejects an out-of-range index, a reversed offset pair, or non-UTF-8 bytes.
    pub fn string(&self, idx: u32) -> Result<&str> {
        let s = self
            .strings
            .as_ref()
            .ok_or(Error::StringIndexOutOfRange(idx))?;
        if idx >= s.count() {
            return Err(Error::StringIndexOutOfRange(idx));
        }
        let b = s.bytes();
        let i = idx as usize;
        let start = u32_at(b, 4 + 4 * i)? as usize;
        let end = u32_at(b, 4 + 4 * (i + 1))? as usize;
        if end < start {
            return Err(Error::StringIndexOutOfRange(idx));
        }
        let heap = 4 + 4 * (s.count() as usize + 1);
        let raw = bytes_at(b, heap + start, end - start)?;
        core::str::from_utf8(raw).map_err(|_| Error::StringIndexOutOfRange(idx))
    }

    /// # Errors
    /// Rejects an index past the end of the hash table.
    pub fn hash(&self, idx: u32) -> Result<[u8; 32]> {
        let s = self
            .hashes
            .as_ref()
            .ok_or(Error::HashIndexOutOfRange(idx))?;
        if idx >= s.count() {
            return Err(Error::HashIndexOutOfRange(idx));
        }
        hash_at(s.bytes(), 32 * idx as usize)
    }

    /// # Errors
    /// Rejects an out-of-range index or a record naming an unknown kind or tier.
    pub fn node(&self, idx: u32) -> Result<Node> {
        let s = self.nodes.as_ref().ok_or(Error::NodeIndexOutOfRange(idx))?;
        if idx >= s.count() {
            return Err(Error::NodeIndexOutOfRange(idx));
        }
        let b = s.bytes();
        let o = NODE_LEN * idx as usize;
        Ok(Node {
            kind: Kind::from_u8(u8_at(b, o)?)?,
            tier: Tier::from_u8(u8_at(b, o + 1)?)?,
            flags: u8_at(b, o + 2)?,
            depth: u8_at(b, o + 3)?,
            role: u16_at(b, o + 4)?,
            edge_cnt: u16_at(b, o + 6)?,
            name_idx: u32_at(b, o + 8)?,
            edge_off: u32_at(b, o + 12)?,
            payload_off: u32_at(b, o + 16)?,
            payload_len: u32_at(b, o + 20)?,
            hash_idx: u32_at(b, o + 24)?,
            parent: u32_at(b, o + 28)?,
        })
    }

    /// # Errors
    /// Rejects an out-of-range index, an unknown edge kind, or the reserved `CONTAINS` tag,
    /// which may never appear in the edge table (`SPEC.md` §10.1).
    pub fn edge(&self, idx: u32) -> Result<Edge> {
        let s = self.edges.as_ref().ok_or(Error::NodeIndexOutOfRange(idx))?;
        if idx >= s.count() {
            return Err(Error::NodeIndexOutOfRange(idx));
        }
        let b = s.bytes();
        let o = EDGE_LEN * idx as usize;
        let tag = u8_at(b, o + 4)?;
        if tag & !EDGE_EXTERNAL == CONTAINS_TAG {
            return Err(Error::ContainsInEdgeTable(idx));
        }
        Ok(Edge {
            dst: u32_at(b, o)?,
            kind: EdgeKind::from_u8(tag & !EDGE_EXTERNAL)?,
            external: tag & EDGE_EXTERNAL != 0,
            ordinal: u8_at(b, o + 5)?,
            label: u16_at(b, o + 6)?,
        })
    }

    /// Returns `(kind, flags, arg_idx)` for one capability record.
    ///
    /// # Errors
    /// Rejects an index past the end of the capability table.
    pub fn cap(&self, idx: u32) -> Result<(u16, u16, u32)> {
        let s = self
            .caps
            .as_ref()
            .ok_or(Error::SegmentIndexOutOfRange(idx))?;
        if idx >= s.count() {
            return Err(Error::SegmentIndexOutOfRange(idx));
        }
        let b = s.bytes();
        let o = CAP_LEN * idx as usize;
        Ok((u16_at(b, o)?, u16_at(b, o + 2)?, u32_at(b, o + 4)?))
    }

    /// # Errors
    /// Rejects an out-of-range index, an unknown ABI, or a non-zero reserved field.
    pub fn segment(&self, idx: u32) -> Result<Segment> {
        let s = self
            .segments
            .as_ref()
            .ok_or(Error::SegmentIndexOutOfRange(idx))?;
        if idx >= s.count() {
            return Err(Error::SegmentIndexOutOfRange(idx));
        }
        let b = s.bytes();
        let o = SEGMENT_LEN * idx as usize;
        all_zero(b, o + 0x44, 28, "segment reserved 0x44")?;
        Ok(Segment {
            root: hash_at(b, o)?,
            orig_len: u32_at(b, o + 0x20)?,
            abi: Abi::from_u16(u16_at(b, o + 0x24)?)?,
            codec: u8_at(b, o + 0x26)?,
            trust_class: match u8_at(b, o + 0x27)? {
                0 => TrustClass::Portable,
                _ => TrustClass::HostTrusted,
            },
            cap_off: u32_at(b, o + 0x28)?,
            cap_cnt: u16_at(b, o + 0x2C)?,
            flags: u16_at(b, o + 0x2E)?,
            mem_kib: u32_at(b, o + 0x30)?,
            cpu_ms: u32_at(b, o + 0x34)?,
            wall_ms: u32_at(b, o + 0x38)?,
            dict_idx: u32_at(b, o + 0x3C)?,
            signer_idx: u32_at(b, o + 0x40)?,
        })
    }

    /// Sorted-and-deduplicated is a canonicality requirement, not an optimisation: it is what
    /// makes string equality an integer comparison and the byte encoding unique.
    pub(crate) fn check_string_order(&self) -> Result<()> {
        let mut prev: Option<&str> = None;
        for i in 0..self.string_count() {
            let s = self.string(i)?;
            if prev.is_some_and(|p| p.as_bytes() >= s.as_bytes()) {
                return Err(Error::StringHeapUnsorted(i));
            }
            prev = Some(s);
        }
        Ok(())
    }
}
