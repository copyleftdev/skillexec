mod common;

use common::{minimal, reserialize, rich};
use skill_format::{Error, Header, Kind, Skill, Tier, TrustPolicy};

#[test]
fn minimal_file_opens_and_verifies() {
    let bytes = minimal();
    let s = Skill::open(&bytes, &TrustPolicy::permissive()).unwrap();
    assert_eq!(s.name().unwrap(), "demo");
    assert_eq!(
        s.description().unwrap(),
        "Use when demonstrating the skill format."
    );
    assert_eq!(s.nodes.len(), 2);
    assert_eq!(s.nodes[0].kind, Kind::Prose);
    assert_eq!(s.nodes[0].tier, Tier::Routing);
    assert_eq!(s.verified_payload(1).unwrap(), b"Do the thing.\n");
    s.verify_all().unwrap();
}

#[test]
fn rich_file_exercises_every_kind() {
    let bytes = rich();
    let s = Skill::open(&bytes, &TrustPolicy::permissive()).unwrap();
    s.verify_all().unwrap();
    let kinds: Vec<Kind> = s.nodes.iter().map(|n| n.kind).collect();
    for k in [
        Kind::Applicability,
        Kind::Prose,
        Kind::Contract,
        Kind::Segment,
        Kind::Resource,
        Kind::Binding,
    ] {
        assert!(kinds.contains(&k), "missing {k:?}");
    }
    let tiers: Vec<Tier> = s.nodes.iter().map(|n| n.tier).collect();
    for t in [Tier::Routing, Tier::Body, Tier::OnDemand] {
        assert!(tiers.contains(&t), "missing {t:?}");
    }
}

#[test]
fn canonicality_serialize_parse_is_identity() {
    for bytes in [minimal(), rich()] {
        assert_eq!(reserialize(&bytes), bytes, "serialize(parse(b)) != b");
    }
}

#[test]
fn routing_plane_precedes_every_payload() {
    let bytes = rich();
    let s = Skill::open(&bytes, &TrustPolicy::permissive()).unwrap();
    let manifest_end = s.header.manifest_off + s.header.manifest_len;
    assert!(
        manifest_end <= s.manifest.hot.0,
        "payload regions must follow the manifest"
    );
    let (name, desc) = Skill::routing_view(&bytes).unwrap();
    assert_eq!(name, "rich");
    assert!(desc.starts_with("Exercises"));
}

#[test]
fn identical_payloads_are_stored_once() {
    use skill_format::{Builder, EdgeKind};
    let mut b = Builder::new("dedup", "Two nodes, one blob.");
    let root = b.root(Kind::Prose, Tier::Routing, 0, &b""[..]);
    let a = b.child(
        root,
        Kind::Prose,
        Tier::Body,
        0,
        Some("a"),
        &b"shared body text"[..],
        2,
    );
    let twin = b.child(
        root,
        Kind::Prose,
        Tier::Body,
        0,
        Some("twin"),
        &b"shared body text"[..],
        2,
    );
    b.edge(a, EdgeKind::Cites, twin, 0, 0);
    let bytes = b.build().unwrap();
    let s = Skill::open(&bytes, &TrustPolicy::permissive()).unwrap();
    assert_eq!(s.nodes[1].payload_off, s.nodes[2].payload_off);
    assert_eq!(
        s.manifest.cold.1 as usize,
        "shared body text".len().next_multiple_of(8)
    );
}

#[test]
fn tier_must_not_decrease_along_contains() {
    use skill_format::Builder;
    let mut b = Builder::new("bad", "Child is hotter than its parent.");
    let root = b.root(Kind::Prose, Tier::Routing, 0, &b""[..]);
    let mid = b.child(
        root,
        Kind::Prose,
        Tier::OnDemand,
        0,
        Some("m"),
        &b"x"[..],
        2,
    );
    b.child(mid, Kind::Prose, Tier::Body, 0, Some("c"), &b"y"[..], 3);
    assert!(matches!(b.build(), Err(Error::TierNotMonotone { .. })));
}

#[test]
fn signing_digest_covers_only_the_header() {
    let bytes = minimal();
    let mut tampered = bytes.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xFF;
    assert_eq!(
        Header::signing_digest(&bytes),
        Header::signing_digest(&tampered)
    );
    // Padding is committed by the region check, not by any subtree hash.
    assert!(matches!(
        Skill::open(&tampered, &TrustPolicy::permissive()),
        Err(Error::UncommittedNonZero { .. })
    ));
}
