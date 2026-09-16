//! Reference implementation of the `.skill` binary container.
//!
//! Layout and verification order are normative in `docs/SPEC.md`; the graph model it encodes is
//! in `docs/GRAPH.md`. The ordering of [`Skill::open`] is the security argument: steps 1–5 of
//! `SPEC.md` §5 touch no variable-length data, so the parser only ever runs on bytes already
//! proven to be the publisher's.

pub mod codec;
pub mod error;
pub mod graph;
pub mod header;
pub mod manifest;
mod raw;
pub mod sig;
mod validate;
pub mod writer;

pub use codec::Dictionary;
pub use error::{Error, Result};
pub use graph::{Abi, Edge, EdgeKind, Kind, Node, Segment, Tier, TrustClass, cap_kind};
pub use header::Header;
pub use manifest::Manifest;
pub use sig::{SigEntry, TrustPolicy};
pub use writer::{Builder, Cap, NodeId, Profile, SegmentSpec};

use graph::{NONE32, node_flags};
use manifest::manifest_flags;
use std::sync::OnceLock;

const NODE_DOMAIN: &[u8] = b"skill.v1.node\0";

/// Canonical subtree commitment. Domain-separated and length-prefixed at every variable field so
/// that no two distinct graphs can collide by re-partitioning the same bytes.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn subtree_hash(
    kind: Kind,
    tier: Tier,
    must_understand: bool,
    depth: u8,
    role: u16,
    name: &str,
    payload: &[u8],
    children: &[[u8; 32]],
) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(NODE_DOMAIN);
    h.update(&[kind as u8, tier as u8, u8::from(must_understand), depth]);
    h.update(&role.to_le_bytes());
    h.update(&(name.len() as u64).to_le_bytes());
    h.update(name.as_bytes());
    h.update(&(payload.len() as u64).to_le_bytes());
    h.update(payload);
    h.update(&(children.len() as u64).to_le_bytes());
    for c in children {
        h.update(c);
    }
    *h.finalize().as_bytes()
}

#[derive(Debug)]
pub struct Skill<'a> {
    bytes: &'a [u8],
    pub header: Header,
    pub manifest: Manifest<'a>,
    pub nodes: Vec<Node>,
    pub signatures: Vec<SigEntry>,
    children: Vec<Vec<u32>>,
    dict: Option<&'a Dictionary>,
    /// Decompressed lazily and at most once. A compressed COLD region is never touched by
    /// routing, which is the whole reason the tiers are separate regions in the first place.
    hot: OnceLock<Vec<u8>>,
    cold: OnceLock<Vec<u8>>,
}

impl<'a> Skill<'a> {
    /// `SPEC.md` §5 steps 1–6.
    /// # Errors
    /// Rejects at the first failing step of `SPEC.md` §5; [`Error::step`] reports which.
    pub fn open(bytes: &'a [u8], policy: &TrustPolicy) -> Result<Self> {
        Self::open_with(bytes, policy, None)
    }

    /// As [`Skill::open`], with a shared zstd dictionary. A file that names a dictionary it was
    /// not given is refused rather than read wrong.
    ///
    /// # Errors
    /// As [`Skill::open`], plus [`Error::MissingDictionary`] and [`Error::WrongDictionary`].
    pub fn open_with(
        bytes: &'a [u8],
        policy: &TrustPolicy,
        dict: Option<&'a Dictionary>,
    ) -> Result<Self> {
        let header = Header::parse(bytes)?;
        let signatures = sig::verify(bytes, header.sig_count, policy)?;

        let end = header
            .manifest_off
            .checked_add(header.manifest_len)
            .ok_or(Error::OffsetOverflow { what: "manifest" })?;
        if end > header.file_len {
            return Err(Error::RangeOutOfBounds {
                what: "manifest",
                off: header.manifest_off,
                len: header.manifest_len,
            });
        }
        let raw = raw::bytes_at(
            bytes,
            header.manifest_off as usize,
            header.manifest_len as usize,
        )?;
        if blake3::hash(raw).as_bytes() != &header.manifest_root {
            return Err(Error::ManifestDigestMismatch);
        }

        let manifest = Manifest::parse(raw, dict)?;
        if let Some(want) = manifest.dict_hash {
            let have = dict.ok_or(Error::MissingDictionary)?;
            if have.digest() != want {
                return Err(Error::WrongDictionary);
            }
        }
        for (region, what) in [(manifest.hot, "hot region"), (manifest.cold, "cold region")] {
            let e = region
                .0
                .checked_add(region.1)
                .ok_or(Error::OffsetOverflow { what })?;
            if e > header.file_len {
                return Err(Error::RangeOutOfBounds {
                    what,
                    off: region.0,
                    len: region.1,
                });
            }
            if region.2 > manifest::MAX_SECTION_BYTES {
                return Err(Error::DeclaredLenMismatch { what });
            }
        }

        let nodes = validate::graph(&manifest)?;
        let spans = validate::payload_spans(&manifest, &nodes);
        for (cold, region, name, compressed) in [
            (
                false,
                manifest.hot,
                "hot region",
                manifest.flags & manifest_flags::HOT_ZSTD != 0,
            ),
            (
                true,
                manifest.cold,
                "cold region",
                manifest.flags & manifest_flags::COLD_ZSTD != 0,
            ),
        ] {
            if compressed {
                // There is nothing to inspect in place: the stored bytes are a zstd frame, and
                // every one of them is covered by the region hash checked at decompression.
                if manifest.region_hashes.is_none() {
                    return Err(Error::UncommittedRegion(name));
                }
                continue;
            }
            let mine: Vec<(u32, u32)> = spans
                .iter()
                .filter(|s| s.0 == cold)
                .map(|s| (s.1, s.2))
                .collect();
            check_region_padding(bytes, (region.0, region.1), &mine, name)?;
        }
        let mut children = vec![Vec::new(); nodes.len()];
        for (i, n) in nodes.iter().enumerate().skip(1) {
            children[n.parent as usize].push(u32::try_from(i).unwrap_or(NONE32));
        }
        Ok(Self {
            bytes,
            header,
            manifest,
            nodes,
            signatures,
            children,
            dict,
            hot: OnceLock::new(),
            cold: OnceLock::new(),
        })
    }

    /// # Errors
    /// Rejects a manifest whose name index does not resolve.
    pub fn name(&self) -> Result<&str> {
        self.manifest.string(self.manifest.name_idx)
    }

    /// # Errors
    /// Rejects a manifest whose description index does not resolve.
    pub fn description(&self) -> Result<&str> {
        self.manifest.string(self.manifest.desc_idx)
    }

    /// Reading the routing plane never touches a body. This is the whole point of the tier
    /// split, and it is a property of the layout rather than of caller discipline.
    /// # Errors
    /// Rejects a malformed header or manifest. Performs no signature check: callers that
    /// need one open the file properly.
    pub fn routing_view(bytes: &'a [u8], dict: Option<&'a Dictionary>) -> Result<(String, String)> {
        let header = Header::parse(bytes)?;
        let raw = raw::bytes_at(
            bytes,
            header.manifest_off as usize,
            header.manifest_len as usize,
        )?;
        let m = Manifest::parse(raw, dict)?;
        Ok((
            m.string(m.name_idx)?.to_owned(),
            m.string(m.desc_idx)?.to_owned(),
        ))
    }

    #[must_use]
    pub fn children_of(&self, idx: u32) -> &[u32] {
        self.children.get(idx as usize).map_or(&[], Vec::as_slice)
    }

    /// The decompressed bytes of one payload region, decompressing on first touch.
    ///
    /// # Errors
    /// Rejects a region whose stored bytes do not match the committed hash, or whose frame does
    /// not decompress to exactly the declared length.
    pub fn region(&self, cold: bool) -> Result<&[u8]> {
        let (region, flag, cell, which) = if cold {
            (
                self.manifest.cold,
                manifest_flags::COLD_ZSTD,
                &self.cold,
                "cold region",
            )
        } else {
            (
                self.manifest.hot,
                manifest_flags::HOT_ZSTD,
                &self.hot,
                "hot region",
            )
        };
        let stored = raw::bytes_at(self.bytes, region.0 as usize, region.1 as usize)?;
        if self.manifest.flags & flag == 0 {
            return Ok(stored);
        }
        if let Some(v) = cell.get() {
            return Ok(v);
        }
        let pair = self
            .manifest
            .region_hashes
            .ok_or(Error::UncommittedRegion(which))?;
        let want = if cold { pair.1 } else { pair.0 };
        if blake3::hash(stored).as_bytes() != &want {
            return Err(Error::RegionHashMismatch(which));
        }
        let out = codec::decompress(stored, region.2 as usize, self.dict)
            .map_err(|_| Error::DeclaredLenMismatch { what: which })?;
        if out.len() != region.2 as usize {
            return Err(Error::DeclaredLenMismatch { what: which });
        }
        Ok(cell.get_or_init(|| out))
    }

    /// Raw payload bytes. Unverified by design: callers that care use [`Skill::verified_payload`].
    /// # Errors
    /// Rejects an out-of-range node index or a payload range outside its region.
    pub fn payload(&self, idx: u32) -> Result<&[u8]> {
        let n = *self
            .nodes
            .get(idx as usize)
            .ok_or(Error::NodeIndexOutOfRange(idx))?;
        if n.payload_len == 0 {
            return Ok(&[]);
        }
        let region = self.region(n.flags & node_flags::PAYLOAD_COLD != 0)?;
        raw::bytes_at(region, n.payload_off as usize, n.payload_len as usize)
    }

    /// `SPEC.md` §5 step 7: lazy verification against the signed subtree commitment.
    /// # Errors
    /// Rejects a payload whose recomputed subtree hash differs from the signed
    /// commitment (`SPEC.md` §5 step 7).
    pub fn verified_payload(&self, idx: u32) -> Result<&[u8]> {
        let at = self.commitment_for(idx)?;
        let want = self.manifest.hash(self.nodes[at as usize].hash_idx)?;
        if self.compute_subtree(at)? != want {
            return Err(Error::PayloadHashMismatch(idx));
        }
        self.payload(idx)
    }

    /// The nearest node at or above `idx` that carries a stored commitment. Interior nodes have
    /// none, so verifying one means verifying the smallest committed subtree containing it.
    ///
    /// # Errors
    /// Rejects an out-of-range index, or a tree whose root carries no commitment.
    pub fn commitment_for(&self, idx: u32) -> Result<u32> {
        let mut cur = idx;
        loop {
            let n = *self
                .nodes
                .get(cur as usize)
                .ok_or(Error::NodeIndexOutOfRange(idx))?;
            if n.hash_idx != NONE32 {
                return Ok(cur);
            }
            if n.parent == NONE32 {
                return Err(Error::RootHasNoCommitment);
            }
            cur = n.parent;
        }
    }

    /// # Errors
    /// Rejects an out-of-range node index or an unresolvable name or payload.
    pub fn compute_subtree(&self, idx: u32) -> Result<[u8; 32]> {
        let n = *self
            .nodes
            .get(idx as usize)
            .ok_or(Error::NodeIndexOutOfRange(idx))?;
        let kids: Vec<[u8; 32]> = self
            .children_of(idx)
            .iter()
            .map(|&c| self.compute_subtree(c))
            .collect::<Result<_>>()?;
        let name = if n.name_idx == NONE32 {
            ""
        } else {
            self.manifest.string(n.name_idx)?
        };
        Ok(subtree_hash(
            n.kind,
            n.tier,
            n.flags & node_flags::MUST_UNDERSTAND != 0,
            n.depth,
            n.role,
            name,
            self.payload(idx)?,
            &kids,
        ))
    }

    /// Verifies every node's stored commitment. Linear; used by the conformance suite and by
    /// publishers, not on the hot path.
    /// # Errors
    /// Rejects the first node whose stored commitment does not match its subtree.
    pub fn verify_all(&self) -> Result<()> {
        for i in 0..u32::try_from(self.nodes.len()).unwrap_or(0) {
            if self.nodes[i as usize].hash_idx == NONE32 {
                continue;
            }
            let want = self.manifest.hash(self.nodes[i as usize].hash_idx)?;
            if self.compute_subtree(i)? != want {
                return Err(Error::PayloadHashMismatch(i));
            }
        }
        Ok(())
    }
}

/// Bytes inside a payload region that no node claims are committed to by nothing, so they are
/// a malleability channel: flip them and every signature still verifies. Requiring them to be
/// zero is what closes it. Found by a test that tampered with the last byte of a padded file
/// and watched it validate.
fn check_region_padding(
    bytes: &[u8],
    region: (u32, u32),
    spans: &[(u32, u32)],
    name: &'static str,
) -> Result<()> {
    let (base, len) = region;
    let mut covered = vec![false; len as usize];
    for &(s, e) in spans {
        for c in covered
            .get_mut(s as usize..e as usize)
            .ok_or(Error::RangeOutOfBounds {
                what: name,
                off: s,
                len: e - s,
            })?
        {
            *c = true;
        }
    }
    for (i, c) in covered.iter().enumerate() {
        if !c && bytes[base as usize + i] != 0 {
            return Err(Error::UncommittedNonZero {
                region: name,
                off: u32::try_from(i).unwrap_or(NONE32),
            });
        }
    }
    Ok(())
}
