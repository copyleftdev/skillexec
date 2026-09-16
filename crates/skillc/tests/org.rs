//! What `skillc org` must refuse, and what it must emit.
//!
//! The approval-cycle test is the load-bearing one. `specs/SkillOrg.tla` shows the container's
//! node-level cycle check accepts every organization ever written, deadlocked or not, because
//! `GUARDS` sources are gate nodes and `GUARDS` targets are member roots and those sets never
//! overlap. If this layer does not reject a circular approval chain, nothing does.

use std::collections::BTreeMap;

use skill_format::{EdgeKind, Kind, Profile, Skill, TrustPolicy};
use skillc::classify::{ROLE_ORG_ARTIFACT, ROLE_ORG_CHARTER, ROLE_ORG_GATE, ROLE_ORG_MEMBER};
use skillc::{md, org};

fn docs(ids: &[&str]) -> BTreeMap<String, md::Document> {
    ids.iter()
        .map(|id| {
            let src =
                format!("---\nname: {id}\ndescription: The {id} role.\n---\n\nWhat {id} does.\n");
            ((*id).to_string(), md::parse(&src))
        })
        .collect()
}

fn build(src: &str) -> Result<Vec<u8>, org::Error> {
    let m = org::parse(src)?;
    let ids: Vec<&str> = m.members.iter().map(|x| x.id.as_str()).collect();
    org::compile(&m, &docs(&ids), Profile::default(), None, Vec::new())
}

/// A gate is "evaluated before entry" (`GRAPH.md` §3), so it runs *ahead* of what it gates. A
/// review that happens after implementing therefore gates the stage that follows it — `ship` —
/// and never `implement`, which it would otherwise have to approve before that work began.
const PIPELINE: &str = r#"
name = "three-role"
description = "A pipeline with one gate."

[[member]]
id = "implement"
skill = "implement"
artifact = "A diff."

[[member]]
id = "review"
skill = "review"
gate = "Refuses a diff that changes behaviour without a test."

[[member]]
id = "ship"
skill = "ship"

[graph]
seq    = [["implement", "review"], ["review", "ship"]]
guards = [["review", "ship"]]
needs  = [["review", "implement"]]
"#;

#[test]
fn a_circular_approval_chain_does_not_compile() {
    let src = r#"
name = "deadlock"
[[member]]
id = "a"
skill = "a"
gate = "Refuses until C approves."
[[member]]
id = "c"
skill = "c"
gate = "Refuses until A approves."
[graph]
guards = [["a", "c"], ["c", "a"]]
"#;
    let e = build(src).expect_err("a ring of approvals must not compile");
    let msg = e.to_string();
    assert!(
        matches!(e, org::Error::ApprovalCycle(_)),
        "wrong error: {msg}"
    );
    assert!(
        msg.contains('a') && msg.contains('c'),
        "cycle unnamed: {msg}"
    );
    assert!(
        msg.contains("waits on"),
        "the error must say who waits on whom: {msg}"
    );
}

#[test]
fn the_same_container_accepts_that_cycle_when_built_by_hand() {
    // Not a defect, and recorded here so it stays a decision. The node-level obligation check is
    // answering a different question; see `specs/SkillOrg.tla` and the module doc on `org.rs`.
    use skill_format::{Builder, Tier};
    let mut b = Builder::bundle();
    let a = b.add_skill("a", "first");
    let ag = b.child(
        a,
        Kind::Contract,
        Tier::Body,
        0,
        Some("gate"),
        Vec::new(),
        0,
    );
    let c = b.add_skill("c", "second");
    let cg = b.child(
        c,
        Kind::Contract,
        Tier::Body,
        0,
        Some("gate"),
        Vec::new(),
        0,
    );
    b.edge(ag, EdgeKind::Guards, c, 0, 0);
    b.edge(cg, EdgeKind::Guards, a, 0, 0);
    let bytes = b
        .build()
        .expect("the container accepts what the org compiler refuses");
    Skill::open(&bytes, &TrustPolicy::permissive())
        .expect("open")
        .verify_all()
        .expect("verify");
}

#[test]
fn a_pipeline_that_loops_does_not_compile() {
    let src = r#"
name = "loop"
[[member]]
id = "a"
skill = "a"
[[member]]
id = "c"
skill = "c"
[graph]
seq = [["a", "c"], ["c", "a"]]
"#;
    assert!(matches!(
        build(src).expect_err("a looping pipeline must not compile"),
        org::Error::SeqCycle(_)
    ));
}

#[test]
fn members_are_ordered_so_every_seq_edge_runs_forward() {
    // Declared backwards on purpose: `review` first, though the pipeline runs into it. SEQ must
    // have dst > src in pre-order, so the compiler has to reorder rather than fail.
    let src = r#"
name = "reordered"
[[member]]
id = "review"
skill = "review"
gate = "Refuses."
[[member]]
id = "implement"
skill = "implement"
[graph]
seq = [["implement", "review"]]
"#;
    let bytes = build(src).expect("the compiler reorders instead of refusing");
    let entries = Skill::routing_view(&bytes).expect("routing");
    let names: Vec<&str> = entries.iter().map(|e| e.name).collect();
    assert_eq!(names, ["reordered", "implement", "review"]);
}

#[test]
fn guards_is_lowered_from_the_source_gate_and_needs_onto_the_target_artifact() {
    let bytes = build(PIPELINE).expect("compile");
    let s = Skill::open(&bytes, &TrustPolicy::permissive()).expect("open");
    s.verify_all().expect("verify");

    let mut guards = 0;
    let mut needs = 0;
    for (i, n) in s.nodes.iter().enumerate() {
        for e in n.edge_off..n.edge_off + u32::from(n.edge_cnt) {
            let edge = s.manifest.edge(e).expect("edge");
            match edge.kind {
                EdgeKind::Guards => {
                    guards += 1;
                    assert_eq!(
                        s.nodes[i].kind,
                        Kind::Contract,
                        "a gate must be a Contract, or the container rejects it"
                    );
                    assert_eq!(s.nodes[i].role, ROLE_ORG_GATE);
                    assert_eq!(s.nodes[edge.dst as usize].role, ROLE_ORG_MEMBER);
                }
                EdgeKind::Needs => {
                    needs += 1;
                    assert_eq!(
                        s.nodes[edge.dst as usize].kind,
                        Kind::Binding,
                        "NEEDS targets an artifact, never a member"
                    );
                    assert_eq!(s.nodes[edge.dst as usize].role, ROLE_ORG_ARTIFACT);
                }
                _ => {}
            }
        }
    }
    assert_eq!((guards, needs), (1, 1));
}

#[test]
fn the_organization_is_routing_entry_zero() {
    let bytes = build(PIPELINE).expect("compile");
    let entries = Skill::routing_view(&bytes).expect("routing");
    assert_eq!(entries[0].name, "three-role");
    assert_eq!(entries[0].description, "A pipeline with one gate.");

    let s = Skill::open(&bytes, &TrustPolicy::permissive()).expect("open");
    assert_eq!(s.nodes[entries[0].root as usize].role, ROLE_ORG_CHARTER);
    for e in &entries[1..] {
        assert_eq!(s.nodes[e.root as usize].role, ROLE_ORG_MEMBER);
    }
}

#[test]
fn rework_is_expressible_as_cites_because_seq_cannot_run_backwards() {
    let src = r#"
name = "rework"
[[member]]
id = "implement"
skill = "implement"
[[member]]
id = "review"
skill = "review"
gate = "Refuses."
[graph]
seq   = [["implement", "review"]]
cites = [["review", "implement"]]
"#;
    let bytes = build(src).expect("CITES may point backwards, and may cycle");
    let s = Skill::open(&bytes, &TrustPolicy::permissive()).expect("open");
    s.verify_all().expect("verify");
    let cites = s
        .nodes
        .iter()
        .flat_map(|n| n.edge_off..n.edge_off + u32::from(n.edge_cnt))
        .filter(|&e| s.manifest.edge(e).expect("edge").kind == EdgeKind::Cites)
        .count();
    assert_eq!(cites, 1);
}

#[test]
fn guarding_without_declaring_a_gate_is_refused() {
    let src = r#"
name = "no-gate"
[[member]]
id = "a"
skill = "a"
[[member]]
id = "c"
skill = "c"
[graph]
guards = [["a", "c"]]
"#;
    let e = build(src).expect_err("a gate that says nothing refuses nothing");
    assert!(matches!(e, org::Error::MissingGate { .. }), "{e}");
    assert!(e.to_string().contains("say what it refuses"), "{e}");
}

#[test]
fn needing_a_member_that_hands_on_nothing_is_refused() {
    let src = r#"
name = "no-artifact"
[[member]]
id = "a"
skill = "a"
[[member]]
id = "c"
skill = "c"
[graph]
needs = [["a", "c"]]
"#;
    assert!(matches!(
        build(src).expect_err("NEEDS must have an artifact to target"),
        org::Error::MissingArtifact { .. }
    ));
}

#[test]
fn an_edge_naming_a_member_that_does_not_exist_is_named_not_ignored() {
    let src = r#"
name = "typo"
[[member]]
id = "implement"
skill = "implement"
[graph]
seq = [["implement", "reveiw"]]
"#;
    let e = build(src).expect_err("a misspelled member must not vanish");
    assert!(e.to_string().contains("reveiw"), "{e}");
    assert!(e.to_string().contains("seq"), "the edge kind is named: {e}");
}

#[test]
fn a_dotted_endpoint_means_the_same_member() {
    let dotted = PIPELINE
        .replace(
            r#"guards = [["review", "ship"]]"#,
            r#"guards = [["review.gate", "ship"]]"#,
        )
        .replace(
            r#"needs  = [["review", "implement"]]"#,
            r#"needs  = [["review", "implement.artifact"]]"#,
        );
    assert_eq!(
        build(&dotted).expect("dotted form compiles"),
        build(PIPELINE).expect("bare form compiles"),
        "the two spellings must produce the same bytes"
    );
}

#[test]
fn a_misspelled_manifest_key_is_refused_rather_than_ignored() {
    let src = r#"
name = "typo"
[[member]]
id = "a"
skill = "a"
guaards = "Refuses."
"#;
    assert!(matches!(
        org::parse(src).expect_err("an unknown field must not compile to silence"),
        org::Error::Parse(_)
    ));
}

#[test]
fn an_organization_with_no_members_is_refused() {
    assert!(matches!(
        build("name = \"empty\"\n").expect_err("nothing to organize"),
        org::Error::NoMembers
    ));
}

#[test]
fn the_same_manifest_compiles_to_the_same_bytes() {
    // Ties in the topological order are broken by declaration order, so a manifest is
    // reproducible rather than dependent on hash iteration.
    assert_eq!(build(PIPELINE).expect("a"), build(PIPELINE).expect("b"));
}
