use skill_format::{Skill, TrustPolicy};
use skillc::{compile, md, render};

fn roundtrip(src: &str) -> String {
    let doc = md::parse(src);
    let (bytes, _) = compile(&doc, "fixture").expect("compile");
    let s = Skill::open(&bytes, &TrustPolicy::permissive()).expect("open");
    s.verify_all().expect("verify");
    render(&s).expect("render")
}

fn exact(src: &str) {
    assert_eq!(roundtrip(src), src, "round-trip differed");
}

#[test]
fn frontmatter_and_headings() {
    exact("---\nname: demo\ndescription: A demo.\n---\n\nPreamble.\n\n## When to use\n\nWhen X.\n");
}

#[test]
fn heading_whitespace_is_preserved() {
    exact("---\nname: w\ndescription: d\n---\n\n##  Two spaces\n\nBody.\n");
}

#[test]
fn indented_fence_does_not_leak_headings() {
    let src = "---\nname: f\ndescription: d\n---\n\n- item:\n\n  ```python\n  # not a heading\n  x = 1\n  ```\n\nAfter.\n";
    exact(src);
    let doc = md::parse(src);
    let (bytes, st) = compile(&doc, "f").unwrap();
    assert_eq!(
        st.segments, 1,
        "the indented python fence must become a segment"
    );
    assert_eq!(st.headings, 0, "nothing inside the fence is a heading");
    let s = Skill::open(&bytes, &TrustPolicy::permissive()).unwrap();
    assert_eq!(s.manifest.segment_count(), 1);
}

#[test]
fn closing_fence_indentation_is_stored_not_derived() {
    exact("---\nname: c\ndescription: d\n---\n\n  ```sh\necho hi\n```\n\nEnd.\n");
}

#[test]
fn unterminated_fence_gains_no_closing_marker() {
    exact("---\nname: u\ndescription: d\n---\n\n## Tail\n\n```bash\necho unterminated\n");
}

#[test]
fn text_after_a_fence_keeps_its_position() {
    exact("---\nname: o\ndescription: d\n---\n\n## S\n\nBefore.\n\n```json\n{}\n```\n\nAfter.\n");
}

#[test]
fn data_fences_do_not_become_segments() {
    let doc = md::parse("---\nname: d\ndescription: d\n---\n\n```json\n{\"a\":1}\n```\n");
    let (_, st) = compile(&doc, "d").unwrap();
    assert_eq!(st.fences, 1);
    assert_eq!(st.segments, 0, "json is data, not an executable segment");
}

#[test]
fn lifted_segments_are_authorized_for_nothing() {
    let doc = md::parse("---\nname: s\ndescription: d\n---\n\n```bash\nrm -rf /\n```\n");
    let (bytes, _) = compile(&doc, "s").unwrap();
    let s = Skill::open(&bytes, &TrustPolicy::permissive()).unwrap();
    let seg = s.manifest.segment(0).unwrap();
    assert_eq!(
        seg.cap_cnt, 0,
        "code lifted from prose declares no capabilities"
    );
    assert_eq!(seg.trust_class, skill_format::TrustClass::HostTrusted);
    assert_eq!(seg.abi, skill_format::Abi::Sh);
}

#[test]
fn a_skill_with_no_frontmatter_still_compiles() {
    exact("# Title\n\nBody text.\n");
}

#[test]
fn empty_input_compiles() {
    exact("");
}

// The three frontmatter shapes a 128k-file held-out corpus produced, and nothing smaller did.

#[test]
fn an_empty_frontmatter_block_survives() {
    exact("---\n---\nname: x\ndescription: d\n---\n\nBody.\n");
}

#[test]
fn a_file_that_is_only_frontmatter_gains_no_extra_delimiter() {
    exact("---\nname: only\ndescription: d\n---\n");
}

#[test]
fn a_stray_rule_below_the_frontmatter_is_left_alone() {
    exact("---\nname: x\ndescription: d\n---\n\nIntro.\n\n---\n\nMore.\n");
}

#[test]
fn frontmatter_is_stored_verbatim_not_reconstructed() {
    let src = "---\nname: v\ndescription: d\n---\n\nBody.\n";
    let doc = md::parse(src);
    assert!(
        doc.fm_raw.starts_with("---\n") && doc.fm_raw.ends_with("---\n"),
        "the block must include its own delimiters"
    );
    assert_eq!(doc.get("name"), Some("v"));
}

#[test]
fn an_unterminated_frontmatter_block_is_not_frontmatter() {
    exact("---\nname: x\nno closing delimiter\n");
}

#[test]
fn folded_block_scalars_are_read_not_taken_literally() {
    // `description: >-` with indented continuation lines is common in real skills. Reading the
    // marker as the value left 25 of a 192-skill library with no description, and therefore no
    // routing signal at all.
    let src = "---\nname: s\ndescription: >-\n  First line of the description\n  and its continuation.\n---\n\nBody.\n";
    let doc = md::parse(src);
    assert_eq!(
        doc.get("description"),
        Some("First line of the description and its continuation.")
    );
    exact(src);
}

#[test]
fn literal_block_scalars_keep_their_newlines() {
    let src = "---\nname: s\ndescription: |\n  one\n  two\n---\n\nBody.\n";
    assert_eq!(md::parse(src).get("description"), Some("one\ntwo"));
    exact(src);
}

/// Compiles several sources into one bundle and renders each back out of it.
///
/// `render_from` with a non-zero root had no fidelity test: the whole-file `render` was measured
/// at 100% normalized round-trip across both corpora, but pulling one skill out of a bundle was
/// only ever asserted not to leak its neighbours. Composing an organization out of an existing
/// library depends on this being exact, so it is asserted rather than assumed.
fn bundle_roundtrip(srcs: &[&str]) -> Vec<String> {
    let docs: Vec<(String, String, md::Document)> = srcs
        .iter()
        .enumerate()
        .map(|(i, src)| {
            let doc = md::parse(src);
            let name = doc.get("name").unwrap_or("fixture").to_string();
            let desc = doc.get("description").unwrap_or("").to_string();
            assert_eq!(name, format!("s{i}"), "fixtures are named in order");
            (name, desc, doc)
        })
        .collect();
    let (bytes, _) = skillc::bundle(&docs, skill_format::Profile::default(), None).expect("bundle");
    let s = Skill::open(&bytes, &TrustPolicy::permissive()).expect("open");
    s.verify_all().expect("verify");

    let entries = Skill::routing_view(&bytes).expect("routing");
    entries
        .iter()
        .map(|e| skillc::render_from(&s, e.root).expect("render subtree"))
        .collect()
}

#[test]
fn a_skill_rendered_out_of_a_bundle_is_byte_identical_to_its_source() {
    let srcs = [
        "---\nname: s0\ndescription: The first.\n---\n\nPreamble.\n\n## When to use\n\nWhen X.\n",
        "---\nname: s1\ndescription: The second.\n---\n\n##  Two spaces\n\nBody.\n\n```sh\necho hi\n```\n",
        "---\nname: s2\ndescription: The third.\n---\n\n- item:\n\n  ```python\n  # not a heading\n  x = 1\n  ```\n\nAfter.\n",
    ];
    for (i, (got, want)) in bundle_roundtrip(&srcs).iter().zip(srcs).enumerate() {
        assert_eq!(got, want, "skill {i} differed coming out of the bundle");
    }
}

#[test]
fn a_bundled_skill_renders_the_same_text_as_it_would_alone() {
    // The stronger claim: bundling changes nothing about a skill's rendered form, so an
    // organization can be composed out of a library without altering what its members say.
    let srcs = [
        "---\nname: s0\ndescription: One.\n---\n\nAlpha.\n",
        "---\nname: s1\ndescription: Two.\n---\n\n## Heading\n\nBeta.\n",
    ];
    for (bundled, src) in bundle_roundtrip(&srcs).iter().zip(srcs) {
        assert_eq!(
            bundled,
            &roundtrip(src),
            "bundling altered the rendered text"
        );
    }
}
