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
