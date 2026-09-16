// Shared by both test binaries; each uses a subset, so unused-here is expected.
#![allow(dead_code)]

use skill_format::{Abi, Builder, Cap, EdgeKind, Kind, SegmentSpec, Skill, Tier, TrustPolicy};

pub const ROLE_INTENT: u16 = 1;
pub const ROLE_STEP: u16 = 2;
pub const ROLE_PITFALL: u16 = 3;

/// An uncompressed build, for the tests that reason about bytes in place.
#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn minimal_raw() -> Vec<u8> {
    let mut b = Builder::new("demo", "Use when demonstrating the skill format.")
        .profile(skill_format::Profile::None);
    let root = b.root(Kind::Prose, Tier::Routing, ROLE_INTENT, &b""[..]);
    b.child(
        root,
        Kind::Prose,
        Tier::Body,
        ROLE_INTENT,
        None,
        &b"Do the thing.\n"[..],
        1,
    );
    b.build().expect("minimal builds")
}

#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn minimal() -> Vec<u8> {
    let mut b = Builder::new("demo", "Use when demonstrating the skill format.");
    let root = b.root(Kind::Prose, Tier::Routing, ROLE_INTENT, &b""[..]);
    b.child(
        root,
        Kind::Prose,
        Tier::Body,
        ROLE_INTENT,
        None,
        &b"Do the thing.\n"[..],
        1,
    );
    b.build().expect("minimal builds")
}

/// Exercises every node kind, every edge kind and all three tiers.
#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn rich() -> Vec<u8> {
    let mut b = Builder::new("rich", "Exercises every kind and tier.").version("1.2.3");
    let root = b.root(Kind::Prose, Tier::Routing, ROLE_INTENT, &b""[..]);
    let body = b.child(
        root,
        Kind::Prose,
        Tier::Body,
        ROLE_INTENT,
        Some("overview"),
        &b"Overview.\n"[..],
        1,
    );
    let when = b.child(
        root,
        Kind::Applicability,
        Tier::Routing,
        0,
        Some("when"),
        &b"when X\n"[..],
        2,
    );
    let pre = b.child(
        body,
        Kind::Contract,
        Tier::Body,
        0,
        Some("prereqs"),
        &b"needs Y\n"[..],
        2,
    );
    let s1 = b.child(
        body,
        Kind::Prose,
        Tier::Body,
        ROLE_STEP,
        Some("step 1"),
        &b"first\n"[..],
        3,
    );
    let s2 = b.child(
        body,
        Kind::Prose,
        Tier::Body,
        ROLE_STEP,
        Some("step 2"),
        &b"second\n"[..],
        3,
    );
    let tool = b.child(
        body,
        Kind::Binding,
        Tier::Body,
        0,
        Some("ripgrep"),
        &b""[..],
        0,
    );
    let doc = b.child(
        body,
        Kind::Resource,
        Tier::OnDemand,
        0,
        Some("ref.md"),
        &b"long reference\n"[..],
        0,
    );
    let seg = b.segment(
        body,
        Some("run.wasm"),
        &b"\0asm"[..],
        SegmentSpec {
            caps: vec![Cap {
                kind: 1,
                flags: 0,
                arg: "example.com".into(),
            }],
            mem_kib: 4096,
            cpu_ms: 500,
            wall_ms: 1000,
            ..SegmentSpec::inert(Abi::Wasm32Wasip2)
        },
        0,
    );
    let trap = b.child(
        body,
        Kind::Prose,
        Tier::OnDemand,
        ROLE_PITFALL,
        Some("pitfalls"),
        &b"do not\n"[..],
        2,
    );

    b.edge(s1, EdgeKind::Seq, s2, 0, 0);
    b.edge(pre, EdgeKind::Guards, s1, 0, 0);
    b.edge(s1, EdgeKind::Needs, tool, 0, 0);
    b.edge(s2, EdgeKind::Needs, seg, 0, 0);
    b.edge(s2, EdgeKind::Alt, trap, 0, 0);
    b.edge(s2, EdgeKind::Alt, doc, 1, 0);
    b.edge(body, EdgeKind::Cites, doc, 0, 0);
    let _ = when;
    b.build().expect("rich builds")
}

/// A skill whose body is large enough that zstd actually wins. Real skills are 5.5 KB at the
/// median, so this is the ordinary case, not an extreme one.
#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn bulky() -> Vec<u8> {
    let body = "The quick brown fox jumps over the lazy dog. ".repeat(200);
    let mut b = Builder::new("bulky", "A body worth compressing.");
    let root = b.root(Kind::Prose, Tier::Routing, ROLE_INTENT, &b""[..]);
    b.child(
        root,
        Kind::Prose,
        Tier::Body,
        ROLE_INTENT,
        Some("body"),
        body.into_bytes(),
        1,
    );
    b.build().expect("bulky builds")
}

/// Rebuilds a `Builder` from a parsed file. `serialize(parse(b)) == b` only holds if the
/// reader recovered every canonical decision the writer made.
#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn reserialize(bytes: &[u8]) -> Vec<u8> {
    let s = Skill::open(bytes, &TrustPolicy::permissive()).expect("reopen");
    let mut b = Builder::new(s.name().unwrap(), s.description().unwrap());
    if s.manifest.version_idx != u32::MAX {
        b = b.version(s.manifest.string(s.manifest.version_idx).unwrap());
    }
    if s.manifest.license_idx != u32::MAX {
        b = b.license(s.manifest.string(s.manifest.license_idx).unwrap());
    }
    let mut ids = Vec::new();
    let mut seg_seen = 0u32;
    for (i, n) in s.nodes.iter().enumerate() {
        let payload = s.payload(u32::try_from(i).unwrap()).unwrap().to_vec();
        let id = if i == 0 {
            b.root(n.kind, n.tier, n.role, payload)
        } else if n.kind == Kind::Segment {
            let name =
                (n.name_idx != u32::MAX).then(|| s.manifest.string(n.name_idx).unwrap().to_owned());
            let rec = s.manifest.segment(seg_seen).unwrap();
            let caps = (0..u32::from(rec.cap_cnt))
                .map(|k| {
                    let c = s.manifest.cap(rec.cap_off + k).unwrap();
                    Cap {
                        kind: c.0,
                        flags: c.1,
                        arg: s.manifest.string(c.2).unwrap().to_owned(),
                    }
                })
                .collect();
            seg_seen += 1;
            b.segment(
                ids[n.parent as usize],
                name.as_deref(),
                payload,
                SegmentSpec {
                    abi: rec.abi,
                    trust_class: rec.trust_class,
                    caps,
                    mem_kib: rec.mem_kib,
                    cpu_ms: rec.cpu_ms,
                    wall_ms: rec.wall_ms,
                },
                n.depth,
            )
        } else {
            let name =
                (n.name_idx != u32::MAX).then(|| s.manifest.string(n.name_idx).unwrap().to_owned());
            b.child(
                ids[n.parent as usize],
                n.kind,
                n.tier,
                n.role,
                name.as_deref(),
                payload,
                n.depth,
            )
        };
        ids.push(id);
    }
    for (i, n) in s.nodes.iter().enumerate() {
        for e in n.edge_off..n.edge_off + u32::from(n.edge_cnt) {
            let edge = s.manifest.edge(e).unwrap();
            b.edge(
                ids[i],
                edge.kind,
                ids[edge.dst as usize],
                edge.ordinal,
                edge.label,
            );
        }
    }
    b.build().expect("rebuild")
}

/// A publisher that signs a file it built incorrectly. Recomputing the commitments is what
/// forces the hostile corpus past steps 1–5 and onto the validator, which is where graph
/// invariants must hold on their own.
#[allow(clippy::missing_panics_doc)]
pub fn recommit(bytes: &mut [u8]) {
    let moff = u32::from_le_bytes(bytes[0x14..0x18].try_into().unwrap()) as usize;
    let mlen = u32::from_le_bytes(bytes[0x18..0x1C].try_into().unwrap()) as usize;
    let root = *blake3::hash(&bytes[moff..moff + mlen]).as_bytes();
    bytes[0x20..0x40].copy_from_slice(&root);
}

/// Section directory entries are 24 bytes since compression added `orig_len`.
pub const DIR_ENTRY: usize = 24;

#[must_use]
#[allow(clippy::missing_panics_doc)]
pub fn section_off(bytes: &[u8], id: u16) -> usize {
    let moff = u32::from_le_bytes(bytes[0x14..0x18].try_into().unwrap()) as usize;
    let n = u16::from_le_bytes(bytes[moff..moff + 2].try_into().unwrap()) as usize;
    for i in 0..n {
        let d = moff + 48 + DIR_ENTRY * i;
        if u16::from_le_bytes(bytes[d..d + 2].try_into().unwrap()) == id {
            return moff + u32::from_le_bytes(bytes[d + 4..d + 8].try_into().unwrap()) as usize;
        }
    }
    panic!("section {id} absent");
}
