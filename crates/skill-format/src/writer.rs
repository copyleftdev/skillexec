use std::collections::{BTreeSet, HashMap};

use crate::codec;
use crate::error::{Error, Result};
use crate::graph::{Abi, EdgeKind, Kind, NONE32, Tier, TrustClass, node_flags};
use crate::header::{HEADER_LEN, MAGIC, VERSION_MAJOR, VERSION_MINOR};
use crate::manifest::{
    DIR_ENTRY_LEN, MANIFEST_HDR_LEN, NODE_LEN, SECT_CAPS, SECT_DICTREF, SECT_EDGES, SECT_HASHES,
    SECT_NODES, SECT_REGIONS, SECT_ROUTING, SECT_SEGMENTS, SECT_STRINGS, SECT_ZSTD, manifest_flags,
};
use crate::routing;
use crate::subtree_hash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeId(usize);

/// What the writer is allowed to compress.
///
/// `Mapped` keeps every table readable in place, which is what "map, don't parse" was for, and
/// compresses only the payload regions -- the 68% of a container that a router never reads.
/// `Compact` also compresses the tables, trading zero-copy graph access for size. The choice is
/// the publisher's and is recorded in the file, so a reader never has to guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
    None,
    #[default]
    Mapped,
    Compact,
}

/// A capability a segment declares and the loader enforces. Kinds mirror `SPEC.md` §4.6.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cap {
    pub kind: u16,
    pub flags: u16,
    pub arg: String,
}

#[derive(Debug, Clone)]
pub struct SegmentSpec {
    pub abi: Abi,
    pub trust_class: TrustClass,
    pub caps: Vec<Cap>,
    pub mem_kib: u32,
    pub cpu_ms: u32,
    pub wall_ms: u32,
}

impl SegmentSpec {
    /// A segment that is identified but authorized for nothing. This is the right default for
    /// code lifted out of prose: it has an ABI and a hash, and no policy will run it.
    #[must_use]
    pub fn inert(abi: Abi) -> Self {
        Self {
            trust_class: if abi.is_portable() {
                TrustClass::Portable
            } else {
                TrustClass::HostTrusted
            },
            abi,
            caps: Vec::new(),
            mem_kib: 0,
            cpu_ms: 0,
            wall_ms: 0,
        }
    }
}

#[derive(Debug, Clone)]
struct BuildNode {
    kind: Kind,
    tier: Tier,
    must_understand: bool,
    depth: u8,
    role: u16,
    name: Option<String>,
    payload: Vec<u8>,
    parent: Option<usize>,
    edges: Vec<(EdgeKind, usize, u8, u16)>,
    segment: Option<SegmentSpec>,
}

/// Emits canonical bytes by construction: callers describe a tree, the builder chooses the
/// pre-order, the string ordering and the payload placement. There is no way to ask it for a
/// non-canonical file, which is what makes `serialize(parse(b)) == b` hold.
#[derive(Debug)]
pub struct Builder {
    name: String,
    desc: String,
    version: Option<String>,
    license: Option<String>,
    nodes: Vec<BuildNode>,
    profile: Profile,
    dict: Option<crate::Dictionary>,
}

impl Builder {
    #[must_use]
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            desc: description.into(),
            version: None,
            license: None,
            nodes: Vec::new(),
            profile: Profile::default(),
            dict: None,
        }
    }

    #[must_use]
    pub fn profile(mut self, p: Profile) -> Self {
        self.profile = p;
        self
    }

    /// A shared zstd dictionary. Readers must supply the same bytes; the file records its
    /// BLAKE3 so a mismatch is refused rather than silently decoded into nonsense.
    #[must_use]
    pub fn dictionary(mut self, dict: crate::Dictionary) -> Self {
        self.dict = Some(dict);
        self
    }

    #[must_use]
    pub fn version(mut self, v: impl Into<String>) -> Self {
        self.version = Some(v.into());
        self
    }

    #[must_use]
    pub fn license(mut self, v: impl Into<String>) -> Self {
        self.license = Some(v.into());
        self
    }

    pub fn root(
        &mut self,
        kind: Kind,
        tier: Tier,
        role: u16,
        payload: impl Into<Vec<u8>>,
    ) -> NodeId {
        self.push(kind, tier, role, payload, None, None, 0)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn child(
        &mut self,
        parent: NodeId,
        kind: Kind,
        tier: Tier,
        role: u16,
        name: Option<&str>,
        payload: impl Into<Vec<u8>>,
        depth: u8,
    ) -> NodeId {
        self.push(kind, tier, role, payload, Some(parent.0), name, depth)
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        kind: Kind,
        tier: Tier,
        role: u16,
        payload: impl Into<Vec<u8>>,
        parent: Option<usize>,
        name: Option<&str>,
        depth: u8,
    ) -> NodeId {
        self.nodes.push(BuildNode {
            kind,
            tier,
            must_understand: false,
            depth,
            role,
            name: name.map(ToOwned::to_owned),
            payload: payload.into(),
            parent,
            edges: Vec::new(),
            segment: None,
        });
        NodeId(self.nodes.len() - 1)
    }

    /// Adds a `Segment` node. The bytes are the node's payload; the spec supplies everything
    /// that is not derivable from them.
    pub fn segment(
        &mut self,
        parent: NodeId,
        name: Option<&str>,
        bytes: impl Into<Vec<u8>>,
        spec: SegmentSpec,
        depth: u8,
    ) -> NodeId {
        let id = self.push(
            Kind::Segment,
            Tier::OnDemand,
            0,
            bytes,
            Some(parent.0),
            name,
            depth,
        );
        self.nodes[id.0].segment = Some(spec);
        id
    }

    /// Role is descriptive and never drives dispatch, so a compiler may refine it after the
    /// node exists without changing how anything loads.
    pub fn set_role(&mut self, id: NodeId, role: u16) {
        self.nodes[id.0].role = role;
    }

    /// Replaces a node's payload after creation, so a compiler can open a section node before
    /// it has seen the prose that belongs to it.
    pub fn set_payload(&mut self, id: NodeId, payload: impl Into<Vec<u8>>) {
        self.nodes[id.0].payload = payload.into();
    }

    pub fn edge(&mut self, src: NodeId, kind: EdgeKind, dst: NodeId, ordinal: u8, label: u16) {
        self.nodes[src.0].edges.push((kind, dst.0, ordinal, label));
    }

    /// # Errors
    /// Returns an error if the described tree is not a single-rooted, tier-monotone pre-order
    /// tree, or if any table would exceed the format's 4 GiB addressing.
    ///
    /// # Panics
    /// Panics if a string reached here without having been interned, which would mean the
    /// builder's own string collection and its emission disagree.
    #[allow(clippy::too_many_lines)]
    pub fn build(self) -> Result<Vec<u8>> {
        let order = self.pre_order()?;
        if self.nodes.first().is_some_and(|r| r.tier != Tier::Routing) {
            return Err(Error::RootNotRouting);
        }
        let mut rank = vec![0usize; self.nodes.len()];
        for (new, &old) in order.iter().enumerate() {
            rank[old] = new;
        }

        for &old in &order {
            let n = &self.nodes[old];
            if let Some(p) = n.parent
                && n.tier < self.nodes[p].tier
            {
                return Err(Error::TierNotMonotone {
                    node: u32::try_from(rank[old]).unwrap_or(NONE32),
                    parent: u32::try_from(rank[p]).unwrap_or(NONE32),
                });
            }
        }

        // `name` and `description` are deliberately absent: they live in the routing block and
        // nowhere else, so the file still has exactly one encoding of each.
        let mut strs: BTreeSet<&str> = BTreeSet::new();
        for s in [&self.version, &self.license].into_iter().flatten() {
            strs.insert(s);
        }
        for n in &self.nodes {
            if let Some(s) = &n.name {
                strs.insert(s);
            }
            for c in n.segment.iter().flat_map(|sp| &sp.caps) {
                strs.insert(&c.arg);
            }
        }
        let strs: Vec<&str> = strs.into_iter().collect();
        let sidx = |s: &str| -> u32 {
            u32::try_from(strs.binary_search(&s).expect("interned")).expect("string count")
        };

        let mut hot: Vec<u8> = Vec::new();
        let mut cold: Vec<u8> = Vec::new();
        // Exact-blob dedup only. Substring dedup looks like a free win and is not: short
        // payloads occur inside longer ones constantly, which manufactures partially
        // overlapping ranges and makes the file ambiguous about who owns which bytes.
        let mut seen: HashMap<(bool, Vec<u8>), u32> = HashMap::new();
        let mut placed: Vec<(u32, u32, bool)> = vec![(0, 0, false); self.nodes.len()];
        for &old in &order {
            let n = &self.nodes[old];
            let is_cold = n.tier != Tier::Routing;
            let region = if is_cold { &mut cold } else { &mut hot };
            let off = if n.payload.is_empty() {
                0
            } else if let Some(&p) = seen.get(&(is_cold, n.payload.clone())) {
                p
            } else {
                let at = u32::try_from(region.len())
                    .map_err(|_| Error::OffsetOverflow { what: "payload" })?;
                region.extend_from_slice(&n.payload);
                seen.insert((is_cold, n.payload.clone()), at);
                at
            };
            placed[old] = (
                off,
                u32::try_from(n.payload.len())
                    .map_err(|_| Error::OffsetOverflow { what: "payload" })?,
                is_cold,
            );
        }

        let subtrees = self.hash_tree(&order);
        let mut hash_tbl: Vec<[u8; 32]> = Vec::new();
        let mut hash_idx = vec![NONE32; self.nodes.len()];
        for (rank_of, &old) in order.iter().enumerate() {
            // A Merkle tree does not store its interior nodes. A commitment is kept only where
            // something can be verified on its own: the root, every segment, and every point
            // where the disclosure tier steps up -- exactly the set of nodes a loader can fetch
            // without having already fetched more.
            let n = &self.nodes[old];
            let boundary = rank_of == 0
                || n.kind == Kind::Segment
                || n.parent.is_some_and(|p| n.tier > self.nodes[p].tier);
            if !boundary {
                continue;
            }
            let h = subtrees[old];
            let at = hash_tbl.iter().position(|x| *x == h).unwrap_or_else(|| {
                hash_tbl.push(h);
                hash_tbl.len() - 1
            });
            hash_idx[old] =
                u32::try_from(at).map_err(|_| Error::OffsetOverflow { what: "hashes" })?;
        }

        let mut edges: Vec<u8> = Vec::new();
        let mut edge_span = vec![(0u32, 0u16); self.nodes.len()];
        let mut edge_n = 0u32;
        for &old in &order {
            let n = &self.nodes[old];
            let start = edge_n;
            let mut sorted = n.edges.clone();
            sorted.sort_by_key(|(k, d, o, _)| (*k as u8, rank[*d], *o));
            for (kind, dst, ordinal, label) in sorted {
                edges.extend_from_slice(&u32::try_from(rank[dst]).unwrap_or(NONE32).to_le_bytes());
                edges.push(kind as u8);
                edges.push(ordinal);
                edges.extend_from_slice(&label.to_le_bytes());
                edge_n += 1;
            }
            edge_span[old] = (
                start,
                u16::try_from(edge_n - start)
                    .map_err(|_| Error::OffsetOverflow { what: "edges" })?,
            );
        }

        let mut nodes_sec: Vec<u8> = Vec::with_capacity(order.len() * NODE_LEN);
        for &old in &order {
            let n = &self.nodes[old];
            let (poff, plen, is_cold) = placed[old];
            let mut flags = 0u8;
            if n.must_understand {
                flags |= node_flags::MUST_UNDERSTAND;
            }
            if is_cold {
                flags |= node_flags::PAYLOAD_COLD;
            }
            nodes_sec.extend_from_slice(&[n.kind as u8, n.tier as u8, flags, n.depth]);
            nodes_sec.extend_from_slice(&n.role.to_le_bytes());
            nodes_sec.extend_from_slice(&edge_span[old].1.to_le_bytes());
            nodes_sec.extend_from_slice(&n.name.as_deref().map_or(NONE32, sidx).to_le_bytes());
            nodes_sec.extend_from_slice(&edge_span[old].0.to_le_bytes());
            nodes_sec.extend_from_slice(&poff.to_le_bytes());
            nodes_sec.extend_from_slice(&plen.to_le_bytes());
            nodes_sec.extend_from_slice(&hash_idx[old].to_le_bytes());
            nodes_sec.extend_from_slice(
                &n.parent
                    .map_or(NONE32, |p| u32::try_from(rank[p]).unwrap_or(NONE32))
                    .to_le_bytes(),
            );
        }

        let mut heap: Vec<u8> = Vec::new();
        heap.extend_from_slice(&u32::try_from(strs.len()).unwrap_or(0).to_le_bytes());
        let mut cur = 0u32;
        for s in &strs {
            heap.extend_from_slice(&cur.to_le_bytes());
            cur += u32::try_from(s.len()).map_err(|_| Error::OffsetOverflow { what: "strings" })?;
        }
        heap.extend_from_slice(&cur.to_le_bytes());
        for s in &strs {
            heap.extend_from_slice(s.as_bytes());
        }

        let mut caps_sec: Vec<u8> = Vec::new();
        let mut cap_count = 0u32;
        let mut segs_sec: Vec<u8> = Vec::new();
        let mut seg_count = 0u32;
        for &old in &order {
            let n = &self.nodes[old];
            let Some(spec) = &n.segment else { continue };
            let cap_off = cap_count;
            for c in &spec.caps {
                caps_sec.extend_from_slice(&c.kind.to_le_bytes());
                caps_sec.extend_from_slice(&c.flags.to_le_bytes());
                caps_sec.extend_from_slice(&sidx(&c.arg).to_le_bytes());
                cap_count += 1;
            }
            segs_sec.extend_from_slice(blake3::hash(&n.payload).as_bytes());
            segs_sec.extend_from_slice(
                &u32::try_from(n.payload.len())
                    .map_err(|_| Error::OffsetOverflow { what: "segment" })?
                    .to_le_bytes(),
            );
            segs_sec.extend_from_slice(&(spec.abi as u16).to_le_bytes());
            segs_sec.push(0);
            segs_sec.push(spec.trust_class as u8);
            segs_sec.extend_from_slice(&cap_off.to_le_bytes());
            segs_sec.extend_from_slice(
                &u16::try_from(spec.caps.len())
                    .map_err(|_| Error::OffsetOverflow { what: "caps" })?
                    .to_le_bytes(),
            );
            segs_sec.extend_from_slice(&0u16.to_le_bytes());
            segs_sec.extend_from_slice(&spec.mem_kib.to_le_bytes());
            segs_sec.extend_from_slice(&spec.cpu_ms.to_le_bytes());
            segs_sec.extend_from_slice(&spec.wall_ms.to_le_bytes());
            segs_sec.extend_from_slice(&NONE32.to_le_bytes());
            segs_sec.extend_from_slice(&NONE32.to_le_bytes());
            segs_sec.extend_from_slice(&[0u8; 28]);
            seg_count += 1;
        }

        let mut sections: Vec<(u16, Vec<u8>, u32)> = vec![
            (SECT_STRINGS, heap, u32::try_from(strs.len()).unwrap_or(0)),
            (
                SECT_NODES,
                nodes_sec,
                u32::try_from(order.len()).unwrap_or(0),
            ),
        ];
        if edge_n > 0 {
            sections.push((SECT_EDGES, edges, edge_n));
        }
        let mut hashes_sec = Vec::with_capacity(hash_tbl.len() * 32);
        for h in &hash_tbl {
            hashes_sec.extend_from_slice(h);
        }
        sections.push((
            SECT_HASHES,
            hashes_sec,
            u32::try_from(hash_tbl.len()).unwrap_or(0),
        ));
        if seg_count > 0 {
            sections.push((SECT_SEGMENTS, segs_sec, seg_count));
        }
        if cap_count > 0 {
            sections.push((SECT_CAPS, caps_sec, cap_count));
        }

        let dict = self.dict.as_ref();
        let compress_tables = self.profile == Profile::Compact;
        let compress_regions = self.profile != Profile::None;

        if let Some(d) = dict {
            sections.push((SECT_DICTREF, d.digest().to_vec(), 1));
        }

        let routing_block = routing::encode(&self.name, &self.desc)?;
        let mut routing_sec = Vec::with_capacity(36);
        routing_sec.extend_from_slice(
            &u32::try_from(routing_block.len())
                .map_err(|_| Error::OffsetOverflow { what: "routing" })?
                .to_le_bytes(),
        );
        routing_sec.extend_from_slice(blake3::hash(&routing_block).as_bytes());
        sections.push((SECT_ROUTING, routing_sec, 1));

        let mut hot_flag = 0u16;
        let mut cold_flag = 0u16;
        let hot_orig = u32::try_from(hot.len()).unwrap_or(0);
        let cold_orig = u32::try_from(cold.len()).unwrap_or(0);
        if compress_regions {
            // Only keep the compressed form when it actually wins. On a near-empty region zstd
            // frames cost more than they save, and a format that stores the larger of two
            // encodings has chosen ceremony over bytes.
            if let Ok(z) = codec::compress(&hot, dict)
                && z.len() < hot.len()
            {
                hot = z;
                hot_flag = manifest_flags::HOT_ZSTD;
            }
            if let Ok(z) = codec::compress(&cold, dict)
                && z.len() < cold.len()
            {
                cold = z;
                cold_flag = manifest_flags::COLD_ZSTD;
            }
        }
        if hot_flag | cold_flag != 0 {
            let mut rh = Vec::with_capacity(64);
            rh.extend_from_slice(blake3::hash(&hot).as_bytes());
            rh.extend_from_slice(blake3::hash(&cold).as_bytes());
            sections.push((SECT_REGIONS, rh, 2));
        }

        // (id, stored bytes, count, orig_len, flags)
        let sections: Vec<(u16, Vec<u8>, u32, u32, u16)> = sections
            .into_iter()
            .map(|(id, data, count)| {
                let orig = u32::try_from(data.len()).unwrap_or(0);
                let compressible = compress_tables
                    && matches!(
                        id,
                        SECT_STRINGS
                            | SECT_NODES
                            | SECT_HASHES
                            | SECT_EDGES
                            | SECT_SEGMENTS
                            | SECT_CAPS
                    );
                if compressible
                    && let Ok(z) = codec::compress(&data, dict)
                    && z.len() < data.len()
                {
                    return (id, z, count, orig, SECT_ZSTD);
                }
                (id, data, count, orig, 0)
            })
            .collect();

        let dir_len = DIR_ENTRY_LEN * sections.len();
        let mut body: Vec<u8> = Vec::new();
        let mut dir: Vec<u8> = Vec::new();
        for (id, data, count, orig, flags) in &sections {
            let off = u32::try_from(MANIFEST_HDR_LEN + dir_len + body.len())
                .map_err(|_| Error::OffsetOverflow { what: "section" })?;
            dir.extend_from_slice(&id.to_le_bytes());
            dir.extend_from_slice(&flags.to_le_bytes());
            dir.extend_from_slice(&off.to_le_bytes());
            dir.extend_from_slice(
                &u32::try_from(data.len())
                    .map_err(|_| Error::OffsetOverflow { what: "section" })?
                    .to_le_bytes(),
            );
            dir.extend_from_slice(&count.to_le_bytes());
            dir.extend_from_slice(&orig.to_le_bytes());
            dir.extend_from_slice(&0u32.to_le_bytes());
            body.extend_from_slice(data);
            while !body.len().is_multiple_of(8) {
                body.push(0);
            }
        }

        let manifest_off = u32::try_from(HEADER_LEN + routing_block.len())
            .map_err(|_| Error::OffsetOverflow { what: "manifest" })?;
        let manifest_len = u32::try_from(MANIFEST_HDR_LEN + dir_len + body.len())
            .map_err(|_| Error::OffsetOverflow { what: "manifest" })?;
        let hot_off = manifest_off + manifest_len;
        let cold_off = hot_off + u32::try_from(hot.len()).unwrap_or(0);
        let file_len = cold_off + u32::try_from(cold.len()).unwrap_or(0);

        let mut mhdr: Vec<u8> = Vec::with_capacity(MANIFEST_HDR_LEN);
        mhdr.extend_from_slice(&u16::try_from(sections.len()).unwrap_or(0).to_le_bytes());
        let mut mflags = hot_flag | cold_flag;
        if dict.is_some() {
            mflags |= manifest_flags::USES_DICT;
        }
        mhdr.extend_from_slice(&mflags.to_le_bytes());
        mhdr.extend_from_slice(&[0u8; 8]);
        mhdr.extend_from_slice(&self.version.as_deref().map_or(NONE32, &sidx).to_le_bytes());
        mhdr.extend_from_slice(&self.license.as_deref().map_or(NONE32, &sidx).to_le_bytes());
        mhdr.extend_from_slice(&u32::try_from(order.len()).unwrap_or(0).to_le_bytes());
        mhdr.extend_from_slice(&hot_off.to_le_bytes());
        mhdr.extend_from_slice(&u32::try_from(hot.len()).unwrap_or(0).to_le_bytes());
        mhdr.extend_from_slice(&cold_off.to_le_bytes());
        mhdr.extend_from_slice(&u32::try_from(cold.len()).unwrap_or(0).to_le_bytes());
        mhdr.extend_from_slice(&hot_orig.to_le_bytes());
        mhdr.extend_from_slice(&cold_orig.to_le_bytes());
        debug_assert_eq!(mhdr.len(), MANIFEST_HDR_LEN);

        let mut manifest = mhdr;
        manifest.extend_from_slice(&dir);
        manifest.extend_from_slice(&body);

        let mut out: Vec<u8> = Vec::with_capacity(file_len as usize);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&VERSION_MAJOR.to_le_bytes());
        out.extend_from_slice(&VERSION_MINOR.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&file_len.to_le_bytes());
        out.extend_from_slice(&manifest_off.to_le_bytes());
        out.extend_from_slice(&manifest_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(blake3::hash(&manifest).as_bytes());
        debug_assert_eq!(out.len(), HEADER_LEN);
        out.extend_from_slice(&routing_block);
        out.extend_from_slice(&manifest);
        out.extend_from_slice(&hot);
        out.extend_from_slice(&cold);
        Ok(out)
    }

    fn pre_order(&self) -> Result<Vec<usize>> {
        if self.nodes.is_empty() {
            return Ok(Vec::new());
        }
        let mut kids: Vec<Vec<usize>> = vec![Vec::new(); self.nodes.len()];
        let mut roots = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            match n.parent {
                Some(p) => kids[p].push(i),
                None => roots.push(i),
            }
        }
        if roots.len() != 1 || roots[0] != 0 {
            return Err(Error::RootHasParent);
        }
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut stack = vec![0usize];
        while let Some(n) = stack.pop() {
            out.push(n);
            for &c in kids[n].iter().rev() {
                stack.push(c);
            }
        }
        if out.len() != self.nodes.len() {
            return Err(Error::NotPreOrder(0));
        }
        Ok(out)
    }

    fn hash_tree(&self, order: &[usize]) -> Vec<[u8; 32]> {
        let mut kids: Vec<Vec<usize>> = vec![Vec::new(); self.nodes.len()];
        for (i, n) in self.nodes.iter().enumerate() {
            if let Some(p) = n.parent {
                kids[p].push(i);
            }
        }
        let mut out = vec![[0u8; 32]; self.nodes.len()];
        for &old in order.iter().rev() {
            let n = &self.nodes[old];
            let ch: Vec<[u8; 32]> = kids[old].iter().map(|&c| out[c]).collect();
            out[old] = subtree_hash(
                n.kind,
                n.tier,
                n.must_understand,
                n.depth,
                n.role,
                n.name.as_deref().unwrap_or(""),
                &n.payload,
                &ch,
            );
        }
        out
    }
}
