use crate::error::{Error, Result};
use crate::graph::{
    Abi, CONTAINS_TAG, EDGE_EXTERNAL, Edge, EdgeKind, Kind, Node, Segment, Tier, TrustClass,
};
use crate::raw::{all_zero, bytes_at, hash_at, u8_at, u16_at, u32_at};

pub const MANIFEST_HDR_LEN: usize = 48;
pub const DIR_ENTRY_LEN: usize = 16;
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

const SECT_MUST_UNDERSTAND: u16 = 1 << 0;

#[derive(Debug, Clone, Copy)]
struct SectionRef {
    off: u32,
    len: u32,
    count: u32,
}

#[derive(Debug)]
pub struct Manifest<'a> {
    raw: &'a [u8],
    pub name_idx: u32,
    pub desc_idx: u32,
    pub version_idx: u32,
    pub license_idx: u32,
    pub node_count: u32,
    pub hot: (u32, u32),
    pub cold: (u32, u32),
    strings: Option<SectionRef>,
    nodes: Option<SectionRef>,
    edges: Option<SectionRef>,
    hashes: Option<SectionRef>,
    segments: Option<SectionRef>,
    caps: Option<SectionRef>,
}

impl<'a> Manifest<'a> {
    pub(crate) fn parse(raw: &'a [u8]) -> Result<Self> {
        if raw.len() < MANIFEST_HDR_LEN {
            return Err(Error::Truncated {
                off: 0,
                need: MANIFEST_HDR_LEN,
            });
        }
        all_zero(raw, 0x28, 8, "manifest reserved 0x28")?;
        let sect_count = u16_at(raw, 0x00)?;

        let mut m = Self {
            raw,
            name_idx: u32_at(raw, 0x04)?,
            desc_idx: u32_at(raw, 0x08)?,
            version_idx: u32_at(raw, 0x0C)?,
            license_idx: u32_at(raw, 0x10)?,
            node_count: u32_at(raw, 0x14)?,
            hot: (u32_at(raw, 0x18)?, u32_at(raw, 0x1C)?),
            cold: (u32_at(raw, 0x20)?, u32_at(raw, 0x24)?),
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
            let sect = SectionRef {
                off: u32_at(raw, off + 0x04)?,
                len: u32_at(raw, off + 0x08)?,
                count: u32_at(raw, off + 0x0C)?,
            };
            let end = sect
                .off
                .checked_add(sect.len)
                .ok_or(Error::OffsetOverflow { what: "section" })?;
            if end as usize > raw.len() {
                return Err(Error::RangeOutOfBounds {
                    what: "section",
                    off: sect.off,
                    len: sect.len,
                });
            }
            if !sect.off.is_multiple_of(8) {
                return Err(Error::SectionMisaligned(id));
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
            *slot = Some(sect);
        }

        m.check_fixed(SECT_NODES, m.nodes, NODE_LEN)?;
        m.check_fixed(SECT_EDGES, m.edges, EDGE_LEN)?;
        m.check_fixed(SECT_HASHES, m.hashes, 32)?;
        m.check_fixed(SECT_SEGMENTS, m.segments, SEGMENT_LEN)?;
        m.check_fixed(SECT_CAPS, m.caps, CAP_LEN)?;

        if m.node_count != m.nodes.map_or(0, |s| s.count) {
            return Err(Error::SectionCountMismatch { id: SECT_NODES });
        }
        Ok(m)
    }

    fn check_fixed(&self, id: u16, s: Option<SectionRef>, unit: usize) -> Result<()> {
        let Some(s) = s else { return Ok(()) };
        let unit_u32 = u32::try_from(unit).expect("record sizes are small");
        if !s.len.is_multiple_of(unit_u32) {
            return Err(Error::SectionLenNotMultiple {
                id,
                len: s.len,
                unit,
            });
        }
        if s.len / unit_u32 != s.count {
            return Err(Error::SectionCountMismatch { id });
        }
        Ok(())
    }

    #[must_use]
    pub fn string_count(&self) -> u32 {
        self.strings.map_or(0, |s| s.count)
    }

    #[must_use]
    pub fn edge_count(&self) -> u32 {
        self.edges.map_or(0, |s| s.count)
    }

    #[must_use]
    pub fn hash_count(&self) -> u32 {
        self.hashes.map_or(0, |s| s.count)
    }

    #[must_use]
    pub fn segment_count(&self) -> u32 {
        self.segments.map_or(0, |s| s.count)
    }

    pub fn string(&self, idx: u32) -> Result<&'a str> {
        let s = self.strings.ok_or(Error::StringIndexOutOfRange(idx))?;
        if idx >= s.count {
            return Err(Error::StringIndexOutOfRange(idx));
        }
        let base = s.off as usize;
        let table = base + 4;
        let i = idx as usize;
        let start = u32_at(self.raw, table + 4 * i)? as usize;
        let end = u32_at(self.raw, table + 4 * (i + 1))? as usize;
        if end < start {
            return Err(Error::StringIndexOutOfRange(idx));
        }
        let heap = table + 4 * (s.count as usize + 1);
        let raw = bytes_at(self.raw, heap + start, end - start)?;
        core::str::from_utf8(raw).map_err(|_| Error::StringIndexOutOfRange(idx))
    }

    pub fn hash(&self, idx: u32) -> Result<[u8; 32]> {
        let s = self.hashes.ok_or(Error::HashIndexOutOfRange(idx))?;
        if idx >= s.count {
            return Err(Error::HashIndexOutOfRange(idx));
        }
        hash_at(self.raw, s.off as usize + 32 * idx as usize)
    }

    pub fn node(&self, idx: u32) -> Result<Node> {
        let s = self.nodes.ok_or(Error::NodeIndexOutOfRange(idx))?;
        if idx >= s.count {
            return Err(Error::NodeIndexOutOfRange(idx));
        }
        let o = s.off as usize + NODE_LEN * idx as usize;
        Ok(Node {
            kind: Kind::from_u8(u8_at(self.raw, o)?)?,
            tier: Tier::from_u8(u8_at(self.raw, o + 1)?)?,
            flags: u8_at(self.raw, o + 2)?,
            depth: u8_at(self.raw, o + 3)?,
            role: u16_at(self.raw, o + 4)?,
            edge_cnt: u16_at(self.raw, o + 6)?,
            name_idx: u32_at(self.raw, o + 8)?,
            edge_off: u32_at(self.raw, o + 12)?,
            payload_off: u32_at(self.raw, o + 16)?,
            payload_len: u32_at(self.raw, o + 20)?,
            hash_idx: u32_at(self.raw, o + 24)?,
            parent: u32_at(self.raw, o + 28)?,
        })
    }

    pub fn edge(&self, idx: u32) -> Result<Edge> {
        let s = self.edges.ok_or(Error::NodeIndexOutOfRange(idx))?;
        if idx >= s.count {
            return Err(Error::NodeIndexOutOfRange(idx));
        }
        let o = s.off as usize + EDGE_LEN * idx as usize;
        let tag = u8_at(self.raw, o + 4)?;
        if tag & !EDGE_EXTERNAL == CONTAINS_TAG {
            return Err(Error::ContainsInEdgeTable(idx));
        }
        Ok(Edge {
            dst: u32_at(self.raw, o)?,
            kind: EdgeKind::from_u8(tag & !EDGE_EXTERNAL)?,
            external: tag & EDGE_EXTERNAL != 0,
            ordinal: u8_at(self.raw, o + 5)?,
            label: u16_at(self.raw, o + 6)?,
        })
    }

    pub fn segment(&self, idx: u32) -> Result<Segment> {
        let s = self.segments.ok_or(Error::SegmentIndexOutOfRange(idx))?;
        if idx >= s.count {
            return Err(Error::SegmentIndexOutOfRange(idx));
        }
        let o = s.off as usize + SEGMENT_LEN * idx as usize;
        all_zero(self.raw, o + 0x4C, 20, "segment reserved 0x4C")?;
        Ok(Segment {
            root: hash_at(self.raw, o)?,
            off: u32_at(self.raw, o + 0x20)?,
            len: u32_at(self.raw, o + 0x24)?,
            orig_len: u32_at(self.raw, o + 0x28)?,
            abi: Abi::from_u16(u16_at(self.raw, o + 0x2C)?)?,
            codec: u8_at(self.raw, o + 0x2E)?,
            trust_class: match u8_at(self.raw, o + 0x2F)? {
                0 => TrustClass::Portable,
                _ => TrustClass::HostTrusted,
            },
            cap_off: u32_at(self.raw, o + 0x30)?,
            cap_cnt: u16_at(self.raw, o + 0x34)?,
            flags: u16_at(self.raw, o + 0x36)?,
            mem_kib: u32_at(self.raw, o + 0x38)?,
            cpu_ms: u32_at(self.raw, o + 0x3C)?,
            wall_ms: u32_at(self.raw, o + 0x40)?,
            dict_idx: u32_at(self.raw, o + 0x44)?,
            signer_idx: u32_at(self.raw, o + 0x48)?,
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
