use skill_format::{Builder, Kind, Profile, Skill, Tier, TrustPolicy};

fn main() {
    // The worked example in SPEC.md §7 is uncompressed on purpose: a hexdump of a zstd frame
    // teaches nothing about this format.
    let mut b =
        Builder::new("demo", "Use when demonstrating the skill format.").profile(Profile::None);
    let root = b.root(Kind::Prose, Tier::Routing, 1, &b""[..]);
    b.child(
        root,
        Kind::Prose,
        Tier::Body,
        1,
        None,
        &b"Do the thing.\n"[..],
        1,
    );
    let bytes = b.build().expect("builds");

    let s = Skill::open(&bytes, &TrustPolicy::permissive()).expect("opens");
    s.verify_all().expect("verifies");
    eprintln!(
        "len={} manifest={}..{} nodes={} hot={:?} cold={:?}",
        bytes.len(),
        s.header.manifest_off,
        s.header.manifest_off + s.header.manifest_len,
        s.nodes.len(),
        s.manifest.hot,
        s.manifest.cold
    );
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let mut cols = String::new();
        for (j, byte) in chunk.iter().enumerate() {
            use std::fmt::Write as _;
            let _ = write!(cols, "{byte:02x} ");
            if j == 7 {
                cols.push(' ');
            }
        }
        println!("{:08x}  {cols:<50}", i * 16);
    }
}
