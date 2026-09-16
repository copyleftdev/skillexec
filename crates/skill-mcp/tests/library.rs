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
    assert_eq!(lib.catalogue().unwrap().len(), 3);

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
