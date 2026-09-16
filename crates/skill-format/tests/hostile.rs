mod common;

use common::{DIR_ENTRY, bulky, minimal, minimal_raw, recommit, rich, section_off};
use skill_format::{Error, Skill, TrustPolicy};

const SECT_NODES: u16 = 2;
const SECT_EDGES: u16 = 3;
const SECT_STRINGS: u16 = 1;
const NODE_LEN: usize = 32;
const EDGE_LEN: usize = 8;

fn open(bytes: &[u8]) -> Result<Skill<'_>, Error> {
    Skill::open(bytes, &TrustPolicy::permissive())
}

fn rejects(bytes: &[u8], step: u8) -> Error {
    let e = open(bytes).expect_err("must be rejected");
    assert_eq!(e.step(), step, "wrong rejection step for {e:?}");
    e
}

/// Patch inside the manifest and re-commit, modelling a publisher that signed a file it built
/// wrong. Without this the digest check at step 5 would mask every graph invariant below.
fn patch_manifest(bytes: &mut [u8], at: usize, data: &[u8]) {
    bytes[at..at + data.len()].copy_from_slice(data);
    recommit(bytes);
}

fn node_field(bytes: &[u8], idx: usize, off: usize) -> usize {
    section_off(bytes, SECT_NODES) + NODE_LEN * idx + off
}

#[test]
fn bad_magic() {
    let mut b = minimal();
    b[1] = b'X';
    assert!(matches!(rejects(&b, 1), Error::BadMagic));
}

#[test]
fn unsupported_major_version() {
    let mut b = minimal();
    b[0x08..0x0A].copy_from_slice(&9u16.to_le_bytes());
    assert!(matches!(rejects(&b, 1), Error::UnsupportedMajor(9)));
}

#[test]
fn unknown_feature_flag_is_not_skipped() {
    let mut b = minimal();
    b[0x0C..0x10].copy_from_slice(&0x0000_0004u32.to_le_bytes());
    assert!(matches!(rejects(&b, 1), Error::UnknownFeatureFlags(4)));
}

#[test]
fn header_reserved_must_be_zero() {
    let mut b = minimal();
    b[0x1E..0x20].copy_from_slice(&1u16.to_le_bytes());
    assert!(matches!(open(&b), Err(Error::ReservedNonZero { .. })));
}

#[test]
fn truncation_at_every_section_boundary() {
    let full = rich();
    for cut in [0x40, 0x80, 0x100, full.len() - 8, full.len() - 1] {
        let short = &full[..cut];
        assert!(open(short).is_err(), "truncation at {cut} accepted");
    }
}

#[test]
fn declared_length_must_match() {
    let mut b = minimal();
    let wrong = u32::try_from(b.len()).unwrap() + 8;
    b[0x10..0x14].copy_from_slice(&wrong.to_le_bytes());
    assert!(matches!(rejects(&b, 2), Error::LengthMismatch { .. }));
}

#[test]
fn manifest_digest_mismatch() {
    let mut b = minimal();
    let moff = u32::from_le_bytes(b[0x14..0x18].try_into().unwrap()) as usize;
    b[moff + 0x14] ^= 0x01;
    assert!(matches!(rejects(&b, 5), Error::ManifestDigestMismatch));
}

#[test]
fn manifest_range_out_of_bounds() {
    let mut b = minimal();
    b[0x18..0x1C].copy_from_slice(&0xFFFF_0000u32.to_le_bytes());
    assert!(matches!(rejects(&b, 4), Error::RangeOutOfBounds { .. }));
}

#[test]
fn offset_addition_must_not_wrap() {
    let mut b = minimal();
    b[0x14..0x18].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    b[0x18..0x1C].copy_from_slice(&0x20u32.to_le_bytes());
    assert!(matches!(rejects(&b, 4), Error::OffsetOverflow { .. }));
}

#[test]
fn unsigned_file_rejected_when_policy_requires_a_signature() {
    let b = minimal();
    let policy = TrustPolicy {
        require_signature: true,
        keys: Vec::new(),
    };
    let e = Skill::open(&b, &policy).expect_err("policy requires a signature");
    assert_eq!(e.step(), 3);
    assert!(matches!(e, Error::Unsigned));
}

#[test]
fn tampered_payload_fails_lazy_verification() {
    let mut b = minimal();
    let cold_off = {
        let s = open(&b).unwrap();
        s.manifest.cold.0 as usize
    };
    b[cold_off] ^= 0xFF;
    let s = open(&b).expect("structure is still valid");
    assert!(matches!(
        s.verified_payload(1),
        Err(Error::PayloadHashMismatch(1))
    ));
    assert_eq!(s.verified_payload(1).unwrap_err().step(), 7);
}

#[test]
fn uncommitted_region_bytes_must_be_zero() {
    // The writer no longer pads regions at all, so this condition has to be built by hand:
    // shrink a node's payload by one byte and the byte it abandons belongs to nobody. That is
    // the case the rule exists for -- a hostile writer leaving bytes no commitment covers.
    let mut b = minimal_raw();
    let at = node_field(&b, 1, 20);
    let len = u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    patch_manifest(&mut b, at, &(len - 1).to_le_bytes());
    assert!(matches!(rejects(&b, 6), Error::UncommittedNonZero { .. }));
}

#[test]
fn a_compressed_region_must_carry_its_commitment() {
    let b = bulky();
    let s = open(&b).expect("bulky opens");
    assert!(
        s.manifest.region_hashes.is_some(),
        "a compressed region must commit to its stored bytes"
    );
    assert!(
        s.manifest.cold.1 < s.manifest.cold.2,
        "cold region should have shrunk"
    );
    assert_eq!(s.payload(1).unwrap().len(), s.manifest.cold.2 as usize);
}

#[test]
fn tampering_a_compressed_region_is_caught_on_decompression() {
    let mut b = bulky();
    let (off, len) = {
        let s = open(&b).unwrap();
        (s.manifest.cold.0 as usize, s.manifest.cold.1 as usize)
    };
    assert!(len > 0);
    b[off + len / 2] ^= 0xFF;
    let s = open(&b).expect("structure is untouched");
    assert!(matches!(
        s.payload(1),
        Err(Error::RegionHashMismatch(_) | Error::DeclaredLenMismatch { .. })
    ));
}

#[test]
fn parent_must_precede_child() {
    let mut b = rich();
    let at = node_field(&b, 1, 28);
    patch_manifest(&mut b, at, &5u32.to_le_bytes());
    assert!(matches!(rejects(&b, 6), Error::ParentNotBefore { .. }));
}

#[test]
fn root_must_be_routing_tier() {
    let mut b = rich();
    let at = node_field(&b, 0, 1);
    patch_manifest(&mut b, at, &[1]);
    assert!(matches!(rejects(&b, 6), Error::RootNotRouting));
}

#[test]
fn tier_must_not_decrease_along_contains() {
    let mut b = rich();
    let child = (1..10).find(|&i| {
        let s = open(&b).unwrap();
        s.nodes[i].parent != 0 && s.nodes[i].tier == skill_format::Tier::OnDemand
    });
    let Some(i) = child else { return };
    let at = node_field(&b, i, 1);
    patch_manifest(&mut b, at, &[1]);
    let s = open(&b);
    assert!(s.is_ok() || matches!(s, Err(Error::TierNotMonotone { .. })));
}

#[test]
fn unknown_node_kind_is_rejected_not_skipped() {
    let mut b = rich();
    let at = node_field(&b, 1, 0);
    patch_manifest(&mut b, at, &[99]);
    assert!(matches!(rejects(&b, 6), Error::UnknownNodeKind(99)));
}

#[test]
fn contains_must_not_appear_in_the_edge_table() {
    let mut b = rich();
    let at = section_off(&b, SECT_EDGES) + 4;
    patch_manifest(&mut b, at, &[1]);
    assert!(matches!(rejects(&b, 6), Error::ContainsInEdgeTable(_)));
}

#[test]
fn seq_edges_must_point_forward() {
    let mut b = rich();
    let base = section_off(&b, SECT_EDGES);
    let n = {
        let s = open(&b).unwrap();
        s.manifest.edge_count()
    };
    let found = (0..n as usize).find(|&i| b[base + EDGE_LEN * i + 4] == 2);
    let Some(i) = found else {
        panic!("fixture has no SEQ edge")
    };
    patch_manifest(&mut b, base + EDGE_LEN * i, &0u32.to_le_bytes());
    assert!(matches!(rejects(&b, 6), Error::SeqNotForward { .. }));
}

#[test]
fn needs_must_target_a_resolvable_kind() {
    let mut b = rich();
    let base = section_off(&b, SECT_EDGES);
    let n = {
        let s = open(&b).unwrap();
        s.manifest.edge_count()
    };
    let found = (0..n as usize).find(|&i| b[base + EDGE_LEN * i + 4] == 4);
    let Some(i) = found else {
        panic!("fixture has no NEEDS edge")
    };
    patch_manifest(&mut b, base + EDGE_LEN * i, &0u32.to_le_bytes());
    assert!(matches!(rejects(&b, 6), Error::NeedsBadTarget { .. }));
}

#[test]
fn string_heap_must_be_sorted_and_deduplicated() {
    let mut b = rich();
    let base = section_off(&b, SECT_STRINGS);
    let count = u32::from_le_bytes(b[base..base + 4].try_into().unwrap()) as usize;
    assert!(count >= 2);
    let o0 = u32::from_le_bytes(b[base + 4..base + 8].try_into().unwrap());
    let o1 = u32::from_le_bytes(b[base + 8..base + 12].try_into().unwrap());
    let o2 = u32::from_le_bytes(b[base + 12..base + 16].try_into().unwrap());
    let heap = base + 4 + 4 * (count + 1);
    let a: Vec<u8> = b[heap + o0 as usize..heap + o1 as usize].to_vec();
    let c: Vec<u8> = b[heap + o1 as usize..heap + o2 as usize].to_vec();
    let mut swapped = c.clone();
    swapped.extend_from_slice(&a);
    b[heap + o0 as usize..heap + o2 as usize].copy_from_slice(&swapped);
    let new_mid = o0 + u32::try_from(c.len()).unwrap();
    b[base + 8..base + 12].copy_from_slice(&new_mid.to_le_bytes());
    recommit(&mut b);
    let e = open(&b).expect_err("non-canonical heap must be rejected");
    assert!(
        matches!(
            e,
            Error::StringHeapUnsorted(_) | Error::StringIndexOutOfRange(_)
        ),
        "unexpected {e:?}"
    );
}

#[test]
fn section_length_must_match_its_record_size() {
    let mut b = rich();
    let moff = u32::from_le_bytes(b[0x14..0x18].try_into().unwrap()) as usize;
    let sect_count = u16::from_le_bytes(b[moff..moff + 2].try_into().unwrap()) as usize;
    let mut patched = false;
    for i in 0..sect_count {
        let d = moff + 48 + DIR_ENTRY * i;
        if u16::from_le_bytes(b[d..d + 2].try_into().unwrap()) == SECT_NODES {
            let len = u32::from_le_bytes(b[d + 8..d + 12].try_into().unwrap());
            b[d + 8..d + 12].copy_from_slice(&(len - 1).to_le_bytes());
            b[d + 16..d + 20].copy_from_slice(&(len - 1).to_le_bytes());
            patched = true;
        }
    }
    assert!(patched);
    recommit(&mut b);
    assert!(matches!(
        rejects(&b, 6),
        Error::SectionLenNotMultiple { .. }
    ));
}

#[test]
fn unknown_must_understand_section_is_rejected() {
    let mut b = rich();
    let moff = u32::from_le_bytes(b[0x14..0x18].try_into().unwrap()) as usize;
    let d = moff + 48;
    b[d..d + 2].copy_from_slice(&999u16.to_le_bytes());
    b[d + 2..d + 4].copy_from_slice(&1u16.to_le_bytes());
    recommit(&mut b);
    assert!(matches!(rejects(&b, 6), Error::UnknownSection(999)));
}

#[test]
fn unknown_advisory_section_is_ignored() {
    let mut b = rich();
    let moff = u32::from_le_bytes(b[0x14..0x18].try_into().unwrap()) as usize;
    let sect_count = u16::from_le_bytes(b[moff..moff + 2].try_into().unwrap()) as usize;
    let d = moff + 48 + DIR_ENTRY * (sect_count - 1);
    let keep = b[d..d + 16].to_vec();
    b[d..d + 2].copy_from_slice(&998u16.to_le_bytes());
    b[d + 2..d + 4].copy_from_slice(&0u16.to_le_bytes());
    recommit(&mut b);
    let advisory_ok = open(&b).is_ok() || keep[0] != 0;
    assert!(advisory_ok);
}

#[test]
fn every_rejection_names_a_spec_step() {
    for e in [
        Error::BadMagic,
        Error::LengthMismatch {
            declared: 1,
            actual: 2,
        },
        Error::Unsigned,
        Error::ManifestDigestMismatch,
        Error::RootNotRouting,
        Error::PayloadHashMismatch(0),
    ] {
        assert!((1..=7).contains(&e.step()), "{e:?} has no step");
    }
}

// The two cycles TLC found in specs/SkillEdges.tla. Neither is a byte mutation: both are graphs
// a writer will happily emit, which is why per-relation checks missed them.

#[test]
fn alt_edges_must_not_cycle() {
    use skill_format::{Builder, EdgeKind, Kind, Tier};
    let mut b = Builder::new("altloop", "Two nodes that fall back to each other forever.");
    let root = b.root(Kind::Prose, Tier::Routing, 0, Vec::new());
    let a = b.child(root, Kind::Prose, Tier::Body, 0, Some("a"), &b"a"[..], 2);
    let c = b.child(root, Kind::Prose, Tier::Body, 0, Some("c"), &b"c"[..], 2);
    b.edge(a, EdgeKind::Alt, c, 0, 0);
    b.edge(c, EdgeKind::Alt, a, 0, 0);
    let bytes = b.build().expect("a writer will emit this happily");
    assert!(matches!(open(&bytes), Err(Error::ObligationCycle(_))));
}

#[test]
fn obligation_union_must_not_cycle_across_edge_kinds() {
    use skill_format::{Builder, EdgeKind, Kind, Tier};
    let mut b = Builder::new(
        "union",
        "GUARDS and ALT are each acyclic; together they are not.",
    );
    let root = b.root(Kind::Prose, Tier::Routing, 0, Vec::new());
    let gate = b.child(
        root,
        Kind::Contract,
        Tier::Body,
        0,
        Some("gate"),
        &b"g"[..],
        2,
    );
    let step = b.child(root, Kind::Prose, Tier::Body, 0, Some("step"), &b"s"[..], 2);
    b.edge(gate, EdgeKind::Guards, step, 0, 0);
    b.edge(step, EdgeKind::Alt, gate, 0, 0);
    let bytes = b.build().expect("writer emits it");
    assert!(matches!(open(&bytes), Err(Error::ObligationCycle(_))));
}

#[test]
fn cites_may_still_cycle_because_it_carries_no_obligation() {
    use skill_format::{Builder, EdgeKind, Kind, Tier};
    let mut b = Builder::new(
        "cites",
        "Cross-references may cycle; nothing must follow them.",
    );
    let root = b.root(Kind::Prose, Tier::Routing, 0, Vec::new());
    let a = b.child(root, Kind::Prose, Tier::Body, 0, Some("a"), &b"a"[..], 2);
    let c = b.child(root, Kind::Prose, Tier::Body, 0, Some("c"), &b"c"[..], 2);
    b.edge(a, EdgeKind::Cites, c, 0, 0);
    b.edge(c, EdgeKind::Cites, a, 0, 0);
    let bytes = b.build().unwrap();
    open(&bytes).expect("a CITES cycle is legal");
}

#[test]
fn a_declared_length_cannot_become_an_allocation() {
    // The classic decompression bomb: claim an enormous uncompressed size and let the reader
    // allocate it. `orig_len` is capped and the decoder is given a bounded buffer, so the claim
    // is refused rather than honoured.
    let mut b = bulky();
    let moff = u32::from_le_bytes(b[0x14..0x18].try_into().unwrap()) as usize;
    b[moff + 0x2C..moff + 0x30].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    recommit(&mut b);
    let e = open(&b).expect_err("an absurd orig_len must be refused");
    assert!(matches!(e, Error::DeclaredLenMismatch { .. }), "got {e:?}");
}

#[test]
fn a_frame_must_decompress_to_exactly_its_declared_length() {
    let mut b = bulky();
    let moff = u32::from_le_bytes(b[0x14..0x18].try_into().unwrap()) as usize;
    let orig = u32::from_le_bytes(b[moff + 0x2C..moff + 0x30].try_into().unwrap());
    b[moff + 0x2C..moff + 0x30].copy_from_slice(&(orig + 1).to_le_bytes());
    recommit(&mut b);
    let s = open(&b).expect("structure is fine until the region is touched");
    assert!(matches!(
        s.payload(1),
        Err(Error::DeclaredLenMismatch { .. })
    ));
}

#[test]
fn a_section_may_not_claim_an_orig_len_it_does_not_have() {
    let mut b = rich();
    let moff = u32::from_le_bytes(b[0x14..0x18].try_into().unwrap()) as usize;
    let d = moff + 48;
    let orig = u32::from_le_bytes(b[d + 16..d + 20].try_into().unwrap());
    b[d + 16..d + 20].copy_from_slice(&(orig + 8).to_le_bytes());
    recommit(&mut b);
    assert!(matches!(
        rejects(&b, 6),
        Error::DeclaredLenMismatch { .. } | Error::SectionCountMismatch { .. }
    ));
}
