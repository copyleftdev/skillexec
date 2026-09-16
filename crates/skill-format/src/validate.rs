use crate::error::{Error, Result};
use crate::graph::{EdgeKind, Kind, NONE32, Node, Tier, TrustClass, node_flags};
use crate::manifest::Manifest;

/// One forward pass over the node table proving `GRAPH.md` §9 invariants 1–10.
pub(crate) fn graph(m: &Manifest<'_>) -> Result<Vec<Node>> {
    let n = m.node_count;
    let mut nodes = Vec::with_capacity(n as usize);
    for i in 0..n {
        nodes.push(m.node(i)?);
    }
    if n == 0 {
        return Ok(nodes);
    }

    if nodes[0].parent != NONE32 {
        return Err(Error::RootHasParent);
    }
    // Tier monotonicity only composes if the tree's root is the hottest node. Anything else
    // makes an `Applicability` child of the body unrepresentable, which is the shape every
    // real skill has.
    if nodes[0].tier != Tier::Routing {
        return Err(Error::RootNotRouting);
    }
    for i in 1..n as usize {
        let p = nodes[i].parent;
        if p == NONE32 {
            return Err(Error::NonRootWithoutParent(as_u32(i)));
        }
        // Pre-order storage makes acyclicity a comparison rather than a traversal.
        if p >= as_u32(i) {
            return Err(Error::ParentNotBefore {
                node: as_u32(i),
                parent: p,
            });
        }
        if !on_root_path(&nodes, as_u32(i - 1), p) {
            return Err(Error::NotPreOrder(as_u32(i)));
        }
        if nodes[i].tier < nodes[p as usize].tier {
            return Err(Error::TierNotMonotone {
                node: as_u32(i),
                parent: p,
            });
        }
    }

    check_strings_and_hashes(m, &nodes)?;
    check_edges(m, &nodes)?;
    check_payload_ranges(m, &nodes)?;
    check_segments(m, &nodes)?;
    m.check_string_order()?;
    Ok(nodes)
}

fn as_u32(v: usize) -> u32 {
    u32::try_from(v).unwrap_or(NONE32)
}

fn on_root_path(nodes: &[Node], from: u32, target: u32) -> bool {
    let mut cur = from;
    loop {
        if cur == target {
            return true;
        }
        let p = nodes[cur as usize].parent;
        if p == NONE32 {
            return false;
        }
        cur = p;
    }
}

fn check_strings_and_hashes(m: &Manifest<'_>, nodes: &[Node]) -> Result<()> {
    let strs = m.string_count();
    let hashes = m.hash_count();
    for idx in [m.name_idx, m.desc_idx] {
        if idx >= strs {
            return Err(Error::StringIndexOutOfRange(idx));
        }
    }
    for idx in [m.version_idx, m.license_idx] {
        if idx != NONE32 && idx >= strs {
            return Err(Error::StringIndexOutOfRange(idx));
        }
    }
    for node in nodes {
        if node.name_idx != NONE32 && node.name_idx >= strs {
            return Err(Error::StringIndexOutOfRange(node.name_idx));
        }
        if node.hash_idx >= hashes {
            return Err(Error::HashIndexOutOfRange(node.hash_idx));
        }
    }
    Ok(())
}

fn check_edges(m: &Manifest<'_>, nodes: &[Node]) -> Result<()> {
    let total = m.edge_count();
    let n = as_u32(nodes.len());
    let mut needs: Vec<Vec<u32>> = vec![Vec::new(); nodes.len()];
    // Every edge kind the loader has no choice about following. TLC showed that checking these
    // one relation at a time is strictly weaker than checking their union: `guards = {1->2}`
    // with `alt = {2->1}` is two acyclic relations and one infinite loop. ALT was not being
    // checked for cycles at all, so a pair of nodes could fall back to each other forever.
    let mut obligation: Vec<Vec<u32>> = vec![Vec::new(); nodes.len()];

    for (si, node) in nodes.iter().enumerate() {
        let src = as_u32(si);
        let end = node
            .edge_off
            .checked_add(u32::from(node.edge_cnt))
            .ok_or(Error::OffsetOverflow { what: "edge range" })?;
        if end > total {
            return Err(Error::RangeOutOfBounds {
                what: "edge range",
                off: node.edge_off,
                len: u32::from(node.edge_cnt),
            });
        }
        let mut alt_seen: Vec<u8> = Vec::new();
        for e in node.edge_off..end {
            let edge = m.edge(e)?;
            if edge.external && edge.kind != EdgeKind::Cites {
                return Err(Error::UnknownEdgeKind(edge.kind as u8));
            }
            if !edge.external && edge.dst >= n {
                return Err(Error::NodeIndexOutOfRange(edge.dst));
            }
            match edge.kind {
                EdgeKind::Seq => {
                    if edge.dst <= src {
                        return Err(Error::SeqNotForward { src, dst: edge.dst });
                    }
                }
                EdgeKind::Guards => {
                    if node.kind != Kind::Contract {
                        return Err(Error::GuardsFromNonContract(src));
                    }
                }
                EdgeKind::Needs => {
                    if edge.external {
                        return Err(Error::NeedsBadTarget { src, dst: edge.dst });
                    }
                    let k = nodes[edge.dst as usize].kind;
                    if !matches!(k, Kind::Binding | Kind::Resource | Kind::Segment) {
                        return Err(Error::NeedsBadTarget { src, dst: edge.dst });
                    }
                    needs[si].push(edge.dst);
                }
                EdgeKind::Alt => {
                    if alt_seen.contains(&edge.ordinal) {
                        return Err(Error::DuplicateAltOrdinal {
                            src,
                            ordinal: edge.ordinal,
                        });
                    }
                    alt_seen.push(edge.ordinal);
                }
                EdgeKind::Cites => {}
            }
            if edge.kind != EdgeKind::Cites && !edge.external {
                obligation[si].push(edge.dst);
            }
        }
    }
    cycle_free(&needs).map_err(Error::NeedsCycle)?;
    cycle_free(&obligation).map_err(Error::ObligationCycle)
}

fn cycle_free(adj: &[Vec<u32>]) -> core::result::Result<(), u32> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        White,
        Grey,
        Black,
    }
    let mut mark = vec![Mark::White; adj.len()];
    let mut stack: Vec<(u32, usize)> = Vec::new();

    for root in 0..adj.len() {
        if mark[root] != Mark::White {
            continue;
        }
        stack.push((as_u32(root), 0));
        mark[root] = Mark::Grey;
        while let Some((node, edge_i)) = stack.pop() {
            let ni = node as usize;
            if let Some(&next) = adj[ni].get(edge_i) {
                stack.push((node, edge_i + 1));
                match mark[next as usize] {
                    Mark::Grey => return Err(next),
                    Mark::White => {
                        mark[next as usize] = Mark::Grey;
                        stack.push((next, 0));
                    }
                    Mark::Black => {}
                }
            } else {
                mark[ni] = Mark::Black;
            }
        }
    }
    Ok(())
}

/// Identical ranges are legal — content-addressed payload dedup (`SPEC.md` §6) is exactly two
/// nodes pointing at one blob. Only *partial* overlap indicates a malformed file.
pub(crate) fn payload_spans(_m: &Manifest<'_>, nodes: &[Node]) -> Vec<(bool, u32, u32)> {
    nodes
        .iter()
        .filter(|n| n.payload_len != 0)
        .map(|n| {
            (
                n.flags & node_flags::PAYLOAD_COLD != 0,
                n.payload_off,
                n.payload_off.saturating_add(n.payload_len),
            )
        })
        .collect()
}

fn check_payload_ranges(m: &Manifest<'_>, nodes: &[Node]) -> Result<()> {
    let mut spans: Vec<(u8, u32, u32)> = Vec::new();
    for node in nodes {
        if node.payload_len == 0 {
            continue;
        }
        let cold = node.flags & node_flags::PAYLOAD_COLD != 0;
        let (_, region_len) = if cold { m.cold } else { m.hot };
        let end = node
            .payload_off
            .checked_add(node.payload_len)
            .ok_or(Error::OffsetOverflow { what: "payload" })?;
        if end > region_len {
            return Err(Error::RangeOutOfBounds {
                what: if cold { "cold payload" } else { "hot payload" },
                off: node.payload_off,
                len: node.payload_len,
            });
        }
        spans.push((u8::from(cold), node.payload_off, end));
    }
    spans.sort_unstable();
    for w in spans.windows(2) {
        let (ra, sa, ea) = w[0];
        let (rb, sb, eb) = w[1];
        if ra == rb && sb < ea && (sa, ea) != (sb, eb) {
            return Err(Error::RangeOverlap { a: sa, b: sb });
        }
    }
    Ok(())
}

/// Segment records are parallel-indexed to `Segment`-kind nodes in pre-order, so the counts must
/// agree exactly; there is no field linking one to the other and none is needed.
fn check_segments(m: &Manifest<'_>, nodes: &[Node]) -> Result<()> {
    let seg_nodes =
        u32::try_from(nodes.iter().filter(|n| n.kind == Kind::Segment).count()).unwrap_or(NONE32);
    if seg_nodes != m.segment_count() {
        return Err(Error::SegmentCountMismatch {
            nodes: seg_nodes,
            records: m.segment_count(),
        });
    }
    for i in 0..m.segment_count() {
        let s = m.segment(i)?;
        if s.trust_class == TrustClass::Portable && !s.abi.is_portable() {
            return Err(Error::PortableClaimOnNonWasm(i));
        }
    }
    Ok(())
}
