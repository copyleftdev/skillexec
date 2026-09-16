use std::collections::BTreeSet;

use crate::error::{Error, Result};
use crate::graph::{EdgeKind, Kind, NONE32, Tier, node_flags};
use crate::header::{HEADER_LEN, MAGIC, VERSION_MAJOR, VERSION_MINOR};
use crate::manifest::{
    DIR_ENTRY_LEN, MANIFEST_HDR_LEN, NODE_LEN, SECT_EDGES, SECT_HASHES, SECT_NODES, SECT_STRINGS,
};
use crate::subtree_hash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeId(usize);

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
        }
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

    /// # Errors
    /// Never; the returned id is always valid for this builder.
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
        });
        NodeId(self.nodes.len() - 1)
    }

    pub fn edge(&mut self, src: NodeId, kind: EdgeKind, dst: NodeId, ordinal: u8, label: u16) {
        self.nodes[src.0].edges.push((kind, dst.0, ordinal, label));
    }

    /// # Errors
    /// Returns an error if the described tree is not a single-rooted, tier-monotone pre-order
    /// tree, or if any table would exceed the format's 4 GiB addressing.
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

        let mut strs: BTreeSet<&str> = BTreeSet::new();
        strs.insert(&self.name);
        strs.insert(&self.desc);
        for s in [&self.version, &self.license].into_iter().flatten() {
            strs.insert(s);
        }
        for n in &self.nodes {
            if let Some(s) = &n.name {
                strs.insert(s);
            }
        }
        let strs: Vec<&str> = strs.into_iter().collect();
        let sidx = |s: &str| -> u32 {
            u32::try_from(strs.binary_search(&s).expect("interned")).expect("string count")
        };

        let mut hot: Vec<u8> = Vec::new();
        let mut cold: Vec<u8> = Vec::new();
        let mut placed: Vec<(u32, u32, bool)> = vec![(0, 0, false); self.nodes.len()];
        for &old in &order {
            let n = &self.nodes[old];
            let is_cold = n.tier != Tier::Routing;
            let region = if is_cold { &mut cold } else { &mut hot };
            let off = if n.payload.is_empty() {
                0
            } else if let Some(p) = find_sub(region, &n.payload) {
                u32::try_from(p).map_err(|_| Error::OffsetOverflow { what: "payload" })?
            } else {
                let at = region.len();
                region.extend_from_slice(&n.payload);
                u32::try_from(at).map_err(|_| Error::OffsetOverflow { what: "payload" })?
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
        let mut hash_idx = vec![0u32; self.nodes.len()];
        for &old in &order {
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

        let dir_len = DIR_ENTRY_LEN * sections.len();
        let mut body: Vec<u8> = Vec::new();
        let mut dir: Vec<u8> = Vec::new();
        for (id, data, count) in &sections {
            let off = u32::try_from(MANIFEST_HDR_LEN + dir_len + body.len())
                .map_err(|_| Error::OffsetOverflow { what: "section" })?;
            dir.extend_from_slice(&id.to_le_bytes());
            dir.extend_from_slice(&0u16.to_le_bytes());
            dir.extend_from_slice(&off.to_le_bytes());
            dir.extend_from_slice(
                &u32::try_from(data.len())
                    .map_err(|_| Error::OffsetOverflow { what: "section" })?
                    .to_le_bytes(),
            );
            dir.extend_from_slice(&count.to_le_bytes());
            body.extend_from_slice(data);
            while !body.len().is_multiple_of(8) {
                body.push(0);
            }
        }

        let manifest_off = u32::try_from(HEADER_LEN).expect("const");
        let manifest_len = u32::try_from(MANIFEST_HDR_LEN + dir_len + body.len())
            .map_err(|_| Error::OffsetOverflow { what: "manifest" })?;
        while !hot.len().is_multiple_of(8) {
            hot.push(0);
        }
        while !cold.len().is_multiple_of(8) {
            cold.push(0);
        }
        let hot_off = manifest_off + manifest_len;
        let cold_off = hot_off + u32::try_from(hot.len()).unwrap_or(0);
        let file_len = cold_off + u32::try_from(cold.len()).unwrap_or(0);

        let mut mhdr: Vec<u8> = Vec::with_capacity(MANIFEST_HDR_LEN);
        mhdr.extend_from_slice(&u16::try_from(sections.len()).unwrap_or(0).to_le_bytes());
        mhdr.extend_from_slice(&0u16.to_le_bytes());
        mhdr.extend_from_slice(&sidx(&self.name).to_le_bytes());
        mhdr.extend_from_slice(&sidx(&self.desc).to_le_bytes());
        mhdr.extend_from_slice(&self.version.as_deref().map_or(NONE32, &sidx).to_le_bytes());
        mhdr.extend_from_slice(&self.license.as_deref().map_or(NONE32, &sidx).to_le_bytes());
        mhdr.extend_from_slice(&u32::try_from(order.len()).unwrap_or(0).to_le_bytes());
        mhdr.extend_from_slice(&hot_off.to_le_bytes());
        mhdr.extend_from_slice(&u32::try_from(hot.len()).unwrap_or(0).to_le_bytes());
        mhdr.extend_from_slice(&cold_off.to_le_bytes());
        mhdr.extend_from_slice(&u32::try_from(cold.len()).unwrap_or(0).to_le_bytes());
        mhdr.extend_from_slice(&[0u8; 8]);
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

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}
