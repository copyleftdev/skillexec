use std::path::{Path, PathBuf};

use skill_format::{Abi, Builder, Cap, Kind, SegmentSpec, Tier};

use skill_mcp::library;

fn bundle_at(dir: &Path) -> PathBuf {
    let mut b = Builder::bundle();
    for (name, desc, body) in [
        (
            "reviewer",
            "Use when reviewing a diff for correctness.",
            "Read the diff first.\n",
        ),
        (
            "archivist",
            "Use when filing notes into a corpus.",
            "File by date.\n",
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
    // One skill that carries an executable fragment, so segment disclosure has something to say.
    let root = b.add_skill("deployer", "Use when shipping a build.");
    b.segment(
        root,
        Some("```sh"),
        &b"echo ship it"[..],
        SegmentSpec {
            caps: vec![Cap {
                kind: skill_format::cap_kind::STDIO,
                flags: 0,
                arg: String::new(),
            }],
            mem_kib: 1024,
            cpu_ms: 250,
            ..SegmentSpec::inert(Abi::Sh)
        },
        0,
    );
    let path = dir.join("test.skill");
    std::fs::write(&path, b.build().expect("build")).expect("write");
    path
}

#[test]
fn search_reads_routing_and_load_reads_a_body() {
    let dir = std::env::temp_dir().join(format!("skillmcp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = bundle_at(&dir);

    let lib = library::Library::open(&path, None).expect("open library");
    assert_eq!(lib.catalogue().len(), 3);

    // Search answers from the routing plane. An empty query lists everything.
    let all = lib.search("", 50).unwrap();
    assert_eq!(all.len(), 3);
    let hits = lib.search("diff correctness", 10).unwrap();
    assert_eq!(hits[0].name, "reviewer", "best match ranks first");

    // A miss is a miss, not a fuzzy match: this is a literal search by design.
    assert!(lib.search("quantum tunnelling", 10).unwrap().is_empty());

    // Loading is the first read that touches a body, and it renders only that skill.
    let md = lib.load("archivist").unwrap();
    assert!(md.contains("File by date."), "got {md:?}");
    assert!(
        !md.contains("Read the diff first."),
        "a bundle must not leak its neighbours"
    );

    assert!(lib.load("nonesuch").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn segments_report_what_was_declared_and_nothing_more() {
    let dir = std::env::temp_dir().join(format!("skillmcp-seg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = bundle_at(&dir);
    let lib = library::Library::open(&path, None).expect("open library");

    // Only the skill that has one, and only its own.
    assert!(lib.segments("reviewer").unwrap().is_empty());
    let segs = lib.segments("deployer").unwrap();
    assert_eq!(segs.len(), 1);
    let s = &segs[0];
    assert_eq!(s.abi, "sh");
    assert_eq!(
        s.trust_class, "HostTrusted",
        "a shell fragment is labelled, never contained"
    );
    assert_eq!(s.capabilities, vec!["stdio"]);
    assert_eq!((s.mem_kib, s.cpu_ms), (1024, 250));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_rare_term_outranks_common_ones() {
    // The live failure this encodes: "how do I make this UI less aggressive" ranked three
    // MCP-authoring skills above the one skill whose description says "aggressive".
    //
    // The corpus has to be big enough for the fix to mean anything. Ranking by inverse document
    // frequency says a term is worth what it rules out, and in a three-skill library "make" rules
    // out exactly as much as "aggressive" does. It is common words in a real library that the
    // weighting is there to discount, so the fixture has to contain some.
    let dir = std::env::temp_dir().join(format!("skillmcp-rank-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let mut b = Builder::bundle();
    let add = |b: &mut Builder, name: &str, desc: &str| {
        let root = b.add_skill(name, desc);
        b.child(
            root,
            Kind::Prose,
            Tier::Body,
            1,
            Some("b"),
            b"body\n".to_vec(),
            1,
        );
    };
    add(
        &mut b,
        "quieter",
        "Tones down visually aggressive or overstimulating designs.",
    );
    for i in 0..12 {
        add(
            &mut b,
            &format!("build-thing-{i}"),
            "Use when the user wants to make this, or asks how to make an integration with UI.",
        );
    }
    let path = dir.join("rank.skill");
    std::fs::write(&path, b.build().unwrap()).unwrap();

    let lib = library::Library::open(&path, None).unwrap();
    let hits = lib
        .search("how do I make this UI less aggressive", 5)
        .unwrap();
    assert_eq!(
        hits[0].name,
        "quieter",
        "got {:?}",
        hits.iter().map(|h| &h.name).collect::<Vec<_>>()
    );

    // A query of only short tokens reduces to no usable terms, and is treated as no query at
    // all rather than being given a fabricated ranking. Same answer as an empty query.
    let stopwords = lib.search("do I a an", 5).unwrap();
    let empty = lib.search("", 5).unwrap();
    assert_eq!(
        stopwords.iter().map(|h| &h.name).collect::<Vec<_>>(),
        empty.iter().map(|h| &h.name).collect::<Vec<_>>()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
