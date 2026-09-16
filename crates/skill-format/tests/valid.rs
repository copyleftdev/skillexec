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
    // The routing block sits ahead of the manifest, not inside it.
    assert!(
        s.header.routing_off() < s.header.manifest_off as usize,
        "routing must precede the manifest"
    );
    let entries = Skill::routing_view(&bytes).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "rich");
    assert!(entries[0].description.starts_with("Exercises"));
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
    assert_eq!(s.manifest.cold.2 as usize, "shared body text".len());
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
    // The digest is unchanged and the file is still rejected: the header commits to the
    // manifest root, which commits to every subtree hash. Signing 64 bytes signs the file.
    let s = Skill::open(&tampered, &TrustPolicy::permissive()).expect("structure survives");
    assert!(
        s.verify_all().is_err(),
        "a tampered payload must not verify"
    );
}

#[test]
fn commitments_are_stored_only_at_verification_boundaries() {
    use common::rich;
    let bytes = rich();
    let s = Skill::open(&bytes, &TrustPolicy::permissive()).unwrap();
    assert_ne!(
        s.nodes[0].hash_idx,
        u32::MAX,
        "the root must always be committed"
    );

    let stored = s.nodes.iter().filter(|n| n.hash_idx != u32::MAX).count();
    assert!(
        stored < s.nodes.len(),
        "interior nodes should carry no commitment"
    );

    // Every node still verifies, via the smallest committed subtree containing it.
    for i in 0..u32::try_from(s.nodes.len()).unwrap() {
        s.verified_payload(i)
            .expect("every node is reachable from some commitment");
    }
    s.verify_all().unwrap();
}

#[test]
fn an_interior_node_verifies_through_its_nearest_committed_ancestor() {
    use common::rich;
    let bytes = rich();
    let s = Skill::open(&bytes, &TrustPolicy::permissive()).unwrap();
    let interior = (0..u32::try_from(s.nodes.len()).unwrap())
        .find(|i| s.nodes[*i as usize].hash_idx == u32::MAX)
        .expect("rich has interior nodes");
    let at = s.commitment_for(interior).unwrap();
    assert_ne!(at, interior);
    assert_ne!(s.nodes[at as usize].hash_idx, u32::MAX);
}

#[test]
fn compression_preserves_canonicality_and_content() {
    use skill_format::{Builder, Profile};
    let body = "Compressible prose, repeated. ".repeat(300);
    for profile in [Profile::None, Profile::Mapped, Profile::Compact] {
        let mut b = Builder::new("z", "Compression fixture.").profile(profile);
        let root = b.root(Kind::Prose, Tier::Routing, 0, Vec::new());
        b.child(
            root,
            Kind::Prose,
            Tier::Body,
            0,
            Some("body"),
            body.clone().into_bytes(),
            1,
        );
        let bytes = b.build().unwrap();
        let s = Skill::open(&bytes, &TrustPolicy::permissive()).unwrap();
        assert_eq!(
            s.payload(1).unwrap(),
            body.as_bytes(),
            "{profile:?} lost content"
        );
        s.verify_all().unwrap();
        if profile != Profile::None {
            assert!(bytes.len() < body.len(), "{profile:?} did not compress");
        }
    }
}

#[test]
fn a_dictionary_file_is_refused_without_the_dictionary() {
    use skill_format::{Builder, Error as E, Profile};
    let dict =
        skill_format::Dictionary::new("Compressible prose, repeated. ".repeat(40).as_bytes());
    let body = "Compressible prose, repeated. ".repeat(300);
    let mut b = Builder::new("d", "Needs a dictionary.")
        .profile(Profile::Compact)
        .dictionary(dict.clone());
    let root = b.root(Kind::Prose, Tier::Routing, 0, Vec::new());
    b.child(
        root,
        Kind::Prose,
        Tier::Body,
        0,
        Some("body"),
        body.clone().into_bytes(),
        1,
    );
    let bytes = b.build().unwrap();

    assert!(matches!(
        Skill::open(&bytes, &TrustPolicy::permissive()),
        Err(E::MissingDictionary | E::DeclaredLenMismatch { .. })
    ));

    let wrong = skill_format::Dictionary::new(b"not the dictionary");
    assert!(Skill::open_with(&bytes, &TrustPolicy::permissive(), Some(&wrong)).is_err());

    let s = Skill::open_with(&bytes, &TrustPolicy::permissive(), Some(&dict)).expect("opens");
    assert_eq!(s.payload(1).unwrap(), body.as_bytes());
    s.verify_all().unwrap();
}

#[test]
fn routing_reads_only_the_front_of_the_file() {
    use common::rich;
    let bytes = rich();
    let s = Skill::open(&bytes, &TrustPolicy::permissive()).unwrap();
    let r = skill_format::routing::read(&bytes, s.header.routing_off()).unwrap();

    // Everything routing needs lives before the manifest begins.
    let touched = s.header.routing_off() + r.stored_len;
    assert!(touched <= s.header.manifest_off as usize);
    assert_eq!(r.entries[0].name, "rich");
    assert_eq!(Skill::routing_view(&bytes).unwrap()[0].name, "rich");

    // And the name is not duplicated in the string heap.
    for i in 0..s.manifest.string_count() {
        assert_ne!(
            s.manifest.string(i).unwrap(),
            "rich",
            "name leaked into the heap"
        );
    }
}

#[test]
fn a_tampered_routing_block_is_refused_at_open() {
    use common::rich;
    let mut bytes = rich();
    let off = {
        let s = Skill::open(&bytes, &TrustPolicy::permissive()).unwrap();
        s.header.routing_off()
    };
    bytes[off + 4] ^= 0x20;
    assert!(matches!(
        Skill::open(&bytes, &TrustPolicy::permissive()),
        Err(Error::RoutingHashMismatch)
    ));
    // The fast path still reads it, which is exactly why it is a hint and not a verdict.
    assert!(Skill::routing_view(&bytes).is_ok());
}

#[test]
fn a_bundle_carries_several_skills_in_one_file() {
    use skill_format::Builder;
    let mut b = Builder::bundle();
    for (name, desc, body) in [
        (
            "reviewer",
            "Use when reviewing a diff.",
            "Read the diff first.\n",
        ),
        ("archivist", "Use when filing notes.", "File by date.\n"),
        (
            "gardener",
            "Use when pruning a corpus.",
            "Prune, then measure.\n",
        ),
    ] {
        let root = b.add_skill(name, desc);
        b.child(
            root,
            Kind::Prose,
            Tier::Body,
            1,
            Some("body"),
            body.as_bytes().to_vec(),
            1,
        );
    }
    let bytes = b.build().unwrap();

    // Routing lists all three without opening the file properly, and without touching a body.
    let entries = Skill::routing_view(&bytes).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[1].name, "archivist");
    assert!(
        entries.iter().all(|e| e.root != 0),
        "each skill roots at its own node"
    );

    let s = Skill::open(&bytes, &TrustPolicy::permissive()).unwrap();
    s.verify_all().unwrap();
    for e in &s.skills().unwrap() {
        let kids = s.children_of(e.root);
        assert_eq!(kids.len(), 1, "{} should have one body", e.name);
        assert!(!s.verified_payload(kids[0]).unwrap().is_empty());
    }
}

#[test]
fn a_single_skill_file_is_a_bundle_of_one() {
    use common::rich;
    let bytes = rich();
    let s = Skill::open(&bytes, &TrustPolicy::permissive()).unwrap();
    let skills = s.skills().unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].root, 0, "a lone skill roots at node 0");
    assert_eq!(skills[0].name, s.name().unwrap());
}

#[test]
fn a_routing_entry_may_not_point_below_the_top_level() {
    use common::{recommit, rich};
    let original = rich();
    let mut bytes = original.clone();
    let off = Skill::open(&bytes, &TrustPolicy::permissive())
        .unwrap()
        .header
        .routing_off();
    let rlen = skill_format::routing::read(&bytes, off).unwrap().stored_len;
    let before = *blake3::hash(&original[off..off + rlen]).as_bytes();

    // Repoint the entry at a grandchild, then re-commit exactly as a publisher would: fix the
    // routing hash the manifest carries, then the manifest root the header carries. Without
    // both, the digest check rejects it first and the graph rule is never reached.
    bytes[off + 4..off + 8].copy_from_slice(&3u32.to_le_bytes());
    let after = *blake3::hash(&bytes[off..off + rlen]).as_bytes();
    let at = bytes
        .windows(32)
        .position(|w| w == before)
        .expect("the manifest commits to the routing block");
    bytes[at..at + 32].copy_from_slice(&after);
    recommit(&mut bytes);

    assert!(matches!(
        Skill::open(&bytes, &TrustPolicy::permissive()),
        Err(Error::RoutingRootNotTopLevel(3))
    ));
}

#[test]
fn embeddings_ride_next_to_the_routing_plane() {
    use skill_format::{Builder, embed};
    let vecs = vec![vec![1.0f32, 0.0, 0.0], vec![0.0, 1.0, 0.0]];
    let mut b = Builder::bundle();
    for (n, d) in [("alpha", "first"), ("beta", "second")] {
        let root = b.add_skill(n, d);
        b.child(
            root,
            Kind::Prose,
            Tier::Body,
            1,
            Some("b"),
            b"body\n".to_vec(),
            1,
        );
    }
    let bytes = b.vectors(vecs.clone()).build().unwrap();

    // The read a semantic router does: header, routing block, embedding block, stop.
    let (entries, emb) = Skill::routing_view_with_embeddings(&bytes).unwrap();
    let emb = emb.expect("the file carries vectors");
    assert_eq!(entries.len(), 2);
    assert_eq!((emb.dims, emb.count), (3, 2));
    assert_eq!(emb.get(0).unwrap(), &vecs[0][..]);

    // Similarity discriminates, and is computed rather than assumed to be normalised.
    assert!((embed::cosine(emb.get(0).unwrap(), &vecs[0]) - 1.0).abs() < 1e-6);
    assert!(embed::cosine(emb.get(0).unwrap(), &vecs[1]).abs() < 1e-6);

    Skill::open(&bytes, &TrustPolicy::permissive())
        .unwrap()
        .verify_all()
        .unwrap();
}

#[test]
fn a_file_without_vectors_reads_as_having_none() {
    use common::rich;
    let bytes = rich();
    let (_, emb) = Skill::routing_view_with_embeddings(&bytes).unwrap();
    assert!(emb.is_none(), "absence is not an error");
}

#[test]
fn a_tampered_embedding_block_is_refused() {
    use common::recommit;
    use skill_format::Builder;
    let mut b = Builder::bundle();
    let root = b.add_skill("alpha", "first");
    b.child(
        root,
        Kind::Prose,
        Tier::Body,
        1,
        Some("b"),
        b"body\n".to_vec(),
        1,
    );
    let mut bytes = b.vectors(vec![vec![1.0f32, 2.0, 3.0]]).build().unwrap();

    let off = {
        let s = Skill::open(&bytes, &TrustPolicy::permissive()).unwrap();
        let r = skill_format::routing::read(&bytes, s.header.routing_off()).unwrap();
        s.header.routing_off() + r.stored_len
    };
    bytes[off + 8] ^= 0xFF;
    recommit(&mut bytes);
    assert!(matches!(
        Skill::open(&bytes, &TrustPolicy::permissive()),
        Err(Error::EmbeddingHashMismatch)
    ));
}

#[test]
fn a_vector_count_that_disagrees_with_the_skills_is_rejected() {
    use skill_format::Builder;
    let mut b = Builder::bundle();
    for (n, d) in [("alpha", "first"), ("beta", "second")] {
        let root = b.add_skill(n, d);
        b.child(
            root,
            Kind::Prose,
            Tier::Body,
            1,
            Some("b"),
            b"x\n".to_vec(),
            1,
        );
    }
    assert!(matches!(
        b.vectors(vec![vec![1.0f32, 0.0]]).build(),
        Err(Error::EmbeddingCountMismatch {
            entries: 2,
            vectors: 1
        })
    ));
}
