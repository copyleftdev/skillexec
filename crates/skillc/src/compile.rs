use skill_format::{Abi, Builder, Dictionary, Kind, NodeId, Profile, Result, SegmentSpec, Tier};

use crate::classify::{
    self, ROLE_CONTINUATION, ROLE_FENCE, ROLE_FENCE_UNCLOSED, ROLE_FRONTMATTER, ROLE_INTENT,
};
use crate::md::{Block, Document};

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub nodes: u32,
    pub segments: u32,
    pub fences: u32,
    pub tier_clamps: u32,
    pub headings: u32,
}

/// Compiles a parsed `SKILL.md` into container bytes.
///
/// # Errors
/// Propagates any violation the writer detects while canonicalising the graph.
///
/// # Panics
/// Panics only if the heading stack loses its root, which the loop maintains as an invariant.
#[allow(clippy::too_many_lines)]
pub fn compile(doc: &Document, fallback_name: &str) -> Result<(Vec<u8>, Stats)> {
    compile_with(doc, fallback_name, Profile::default(), None)
}

/// Compiles with an explicit compression profile and optional shared dictionary.
///
/// # Errors
/// Propagates any violation the writer detects while canonicalising the graph.
///
/// # Panics
/// Panics only if the heading stack loses its root, which the loop maintains as an invariant.
#[allow(clippy::too_many_lines)]
pub fn compile_with(
    doc: &Document,
    fallback_name: &str,
    profile: Profile,
    dict: Option<&Dictionary>,
) -> Result<(Vec<u8>, Stats)> {
    let name = doc.get("name").unwrap_or(fallback_name);
    let desc = doc.get("description").unwrap_or("");
    let mut b = Builder::new(name, desc).profile(profile);
    if let Some(d) = dict {
        b = b.dictionary(d.clone());
    }
    if let Some(v) = doc.get("version") {
        b = b.version(v);
    }
    if let Some(l) = doc.get("license") {
        b = b.license(l);
    }

    let root = b.root(Kind::Prose, Tier::Routing, ROLE_INTENT, Vec::new());
    let st = build_into(&mut b, root, doc);
    let bytes = b.build()?;
    Ok((bytes, st))
}

/// Builds one skill's subtree under `root`, which the caller has already created.
///
/// Separated out so a bundle can hang several of these off one synthetic root; a single-skill
/// file is the same code with `root` being node 0.
///
/// # Panics
/// Panics only if the heading stack loses its root, which the loop maintains as an invariant.
#[allow(clippy::too_many_lines)]
pub fn build_into(b: &mut Builder, root: NodeId, doc: &Document) -> Stats {
    let mut st = Stats::default();

    if !doc.fm_raw.is_empty() {
        b.child(
            root,
            Kind::Applicability,
            Tier::Routing,
            ROLE_FRONTMATTER,
            None,
            doc.fm_raw.as_bytes().to_vec(),
            0,
        );
    }

    // (heading level, node, tier) — the stack that turns a flat block list back into the tree
    // the headings already describe.
    let mut stack: Vec<(u8, NodeId, Tier)> = vec![(0, root, Tier::Routing)];
    let mut pending_payload: Option<NodeId> = None;
    let mut saw_fence_in_section = false;

    for block in &doc.blocks {
        match block {
            Block::Heading { level, text } => {
                st.headings += 1;
                while stack.len() > 1 && stack.last().is_some_and(|(l, _, _)| *l >= *level) {
                    stack.pop();
                }
                let (_, parent, parent_tier) = *stack.last().expect("root is never popped");
                let (kind, mut tier, role) = classify::heading(text);
                if tier < parent_tier {
                    tier = parent_tier;
                    st.tier_clamps += 1;
                }
                let id = b.child(parent, kind, tier, role, Some(text), Vec::new(), *level);
                stack.push((*level, id, tier));
                pending_payload = Some(id);
                saw_fence_in_section = false;
                st.nodes += 1;
            }
            Block::Text(t) => {
                let (_, current, tier) = *stack.last().expect("root");
                if saw_fence_in_section || pending_payload.is_none() {
                    // Prose never belongs in the routing plane, even directly under the root.
                    let t_tier = if tier > Tier::Body { tier } else { Tier::Body };
                    b.child(
                        current,
                        Kind::Prose,
                        t_tier,
                        ROLE_CONTINUATION,
                        None,
                        t.as_bytes().to_vec(),
                        0,
                    );
                    st.nodes += 1;
                } else {
                    b.set_payload(current, t.as_bytes().to_vec());
                    pending_payload = None;
                }
            }
            Block::Fence {
                open,
                close,
                lang,
                code,
            } => {
                st.fences += 1;
                saw_fence_in_section = true;
                let (_, current, tier) = *stack.last().expect("root");
                let role = if close.is_some() {
                    ROLE_FENCE
                } else {
                    ROLE_FENCE_UNCLOSED
                };
                // One interned string holds both delimiter lines, newline-separated. They are
                // presentation, not content, and belong nowhere near the payload that gets
                // hashed and executed.
                let label = match close {
                    Some(c) => format!("{open}\n{c}"),
                    None => open.clone(),
                };
                if let Some(abi) = classify::fence_abi(lang) {
                    let id = b.segment(
                        current,
                        Some(&label),
                        code.as_bytes().to_vec(),
                        SegmentSpec::inert(abi),
                        0,
                    );
                    b.set_role(id, role);
                    st.segments += 1;
                } else {
                    let t = if tier > Tier::OnDemand {
                        tier
                    } else {
                        Tier::OnDemand
                    };
                    b.child(
                        current,
                        Kind::Resource,
                        t,
                        role,
                        Some(&label),
                        code.as_bytes().to_vec(),
                        0,
                    );
                }
                st.nodes += 1;
            }
        }
    }

    st.nodes += 1;
    st
}

/// Compiles many documents into one file, each a top-level skill.
///
/// # Errors
/// Propagates any violation the writer detects while canonicalising the graph.
pub fn bundle(
    docs: &[(String, String, Document)],
    profile: Profile,
    dict: Option<&Dictionary>,
) -> Result<(Vec<u8>, Stats)> {
    let mut b = Builder::bundle().profile(profile);
    if let Some(d) = dict {
        b = b.dictionary(d.clone());
    }
    let mut total = Stats::default();
    for (name, desc, doc) in docs {
        let root = b.add_skill(name.clone(), desc.clone());
        let st = build_into(&mut b, root, doc);
        total.nodes += st.nodes;
        total.segments += st.segments;
        total.fences += st.fences;
        total.tier_clamps += st.tier_clamps;
        total.headings += st.headings;
    }
    Ok((b.build()?, total))
}

#[must_use]
pub fn abi_name(abi: Abi) -> &'static str {
    match abi {
        Abi::Wasm32Wasip2 => "wasm32-wasip2",
        Abi::Sh => "sh",
        Abi::Python3 => "python3",
        Abi::Node => "node",
        Abi::Native => "native",
        Abi::Wasm32Core => "wasm32-core",
    }
}
