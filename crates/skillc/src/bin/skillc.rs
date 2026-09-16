// A corpus reporting tool. Percentage and quantile arithmetic is deliberately floating point
// and deliberately lossy, and the report function is long because a report is a long list of
// lines; neither is worth contorting the code to avoid.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines,
    clippy::many_single_char_names
)]

use std::path::{Path, PathBuf};

use skill_format::{Dictionary, Profile};
use skill_format::{Skill, TrustPolicy};
use skillc::{compile, compile_with, md, render};

#[derive(Default)]
struct Corpus {
    seen: u32,
    compiled: u32,
    verified: u32,
    exact: u32,
    normalized: u32,
    failures: Vec<(String, String)>,
    src_bytes: u64,
    bin_bytes: u64,
    routing_bytes: u64,
    manifest_span: u64,
    manifest_b: u64,
    nodes_b: u64,
    hashes_b: u64,
    edges_b: u64,
    hot_b: u64,
    cold_b: u64,
    nodes: Vec<u32>,
    segments: u32,
    clamps: u32,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("corpus") => {
            let list = args.get(1).map_or("-", String::as_str);
            let limit: usize = args
                .get(2)
                .and_then(|s| s.parse().ok())
                .unwrap_or(usize::MAX);
            let profile = match flag(&args, "--profile").as_deref() {
                Some("none") => Profile::None,
                Some("compact") => Profile::Compact,
                _ => Profile::Mapped,
            };
            let dict = flag(&args, "--dict")
                .map(|p| Dictionary::new(&std::fs::read(p).expect("read dictionary")));
            corpus(list, limit, profile, dict.as_ref());
        }
        Some("route") => {
            let list = args.get(1).map_or("-", String::as_str);
            let profile = match flag(&args, "--profile").as_deref() {
                Some("none") => Profile::None,
                Some("compact") => Profile::Compact,
                _ => Profile::Mapped,
            };
            let dict = flag(&args, "--dict")
                .map(|p| Dictionary::new(&std::fs::read(p).expect("read dictionary")));
            route_bench(list, profile, dict.as_ref());
        }
        Some("dict") => {
            let list = args.get(1).map_or("-", String::as_str);
            let out = args.get(2).map_or("corpus.dict", String::as_str);
            let max: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(112_640);
            train_dict(list, out, max);
        }
        Some(path) => one(Path::new(path)),
        None => eprintln!(
            "usage: skillc <SKILL.md>\n       skillc corpus <list> [limit] [--profile P] [--dict F]\n       skillc dict <list> <out.dict> [max-bytes]"
        ),
    }
}

/// Answers the question a size table cannot: what does it cost to decide whether a skill is
/// relevant? For the container that is two string reads at a fixed offset. For compressed
/// Markdown it is a full decompression of every candidate, because frontmatter is at the front
/// of a stream that has to be decoded from the start.
fn route_bench(list: &str, profile: Profile, dict: Option<&Dictionary>) {
    let listing = std::fs::read_to_string(list).expect("read list");
    let mut containers: Vec<Vec<u8>> = Vec::new();
    let mut zstd_md: Vec<Vec<u8>> = Vec::new();
    let mut raw_md: Vec<String> = Vec::new();

    for line in listing.lines() {
        let path = PathBuf::from(line);
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let doc = md::parse(&src);
        let Ok((bytes, _)) = compile_with(&doc, &stem(&path), profile, dict) else {
            continue;
        };
        let Ok(z) = skill_format::codec::compress(src.as_bytes(), dict) else {
            continue;
        };
        containers.push(bytes);
        zstd_md.push(z);
        raw_md.push(src);
    }
    let n = containers.len();

    let t0 = std::time::Instant::now();
    let mut acc = 0usize;
    for c in &containers {
        if let Ok((name, desc)) = Skill::routing_view(c) {
            acc += name.len() + desc.len();
        }
    }
    let container_ns = t0.elapsed().as_nanos() / n.max(1) as u128;

    let t1 = std::time::Instant::now();
    let mut acc2 = 0usize;
    for (i, z) in zstd_md.iter().enumerate() {
        let cap = raw_md[i].len();
        if let Ok(plain) = skill_format::codec::decompress(z, cap, dict) {
            let text = String::from_utf8_lossy(&plain);
            let doc = md::parse(&text);
            acc2 +=
                doc.get("name").map_or(0, str::len) + doc.get("description").map_or(0, str::len);
        }
    }
    let zstd_ns = t1.elapsed().as_nanos() / n.max(1) as u128;

    let csize: usize = containers.iter().map(Vec::len).sum();
    let zsize: usize = zstd_md.iter().map(Vec::len).sum();
    println!(
        "profile          {profile:?}{}",
        if dict.is_some() { " + dict" } else { "" }
    );
    println!("skills           {n}");
    println!("container        {csize:>12} B total, routing {container_ns:>6} ns/skill");
    println!("zstd markdown    {zsize:>12} B total, routing {zstd_ns:>6} ns/skill");
    println!(
        "ratio            container is {:.2}x the bytes and {:.1}x the routing speed",
        csize as f64 / zsize.max(1) as f64,
        zstd_ns as f64 / container_ns.max(1) as f64
    );
    assert_eq!(acc > 0, acc2 > 0, "both paths must read something");
}

/// Trains on the COLD regions of compiled containers rather than on the Markdown, because
/// those are the bytes a dictionary will actually be asked to help with.
fn train_dict(list: &str, out: &str, max: usize) {
    let listing = std::fs::read_to_string(list).expect("read list");
    let mut samples: Vec<Vec<u8>> = Vec::new();
    for line in listing.lines() {
        let path = PathBuf::from(line);
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let doc = md::parse(&src);
        let Ok((bytes, _)) = compile_with(&doc, &stem(&path), Profile::None, None) else {
            continue;
        };
        let Ok(s) = Skill::open(&bytes, &TrustPolicy::permissive()) else {
            continue;
        };
        if let Ok(region) = s.region(true)
            && region.len() > 256
        {
            samples.push(region.to_vec());
        }
    }
    let total: usize = samples.iter().map(Vec::len).sum();
    let dict = skill_format::codec::train(&samples, max).expect("train");
    std::fs::write(out, &dict).expect("write dictionary");
    println!(
        "trained {} bytes from {} samples ({:.1} MB of payload) -> {out}",
        dict.len(),
        samples.len(),
        total as f64 / 1.048_576e6
    );
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn one(path: &Path) {
    let src = std::fs::read_to_string(path).expect("read");
    let doc = md::parse(&src);
    let fallback = stem(path);
    let (bytes, st) = compile(&doc, &fallback).expect("compile");
    let s = Skill::open(&bytes, &TrustPolicy::permissive()).expect("open");
    s.verify_all().expect("verify");
    let back = render(&s).expect("render");
    if back != src {
        let a: Vec<&str> = src.lines().collect();
        let c: Vec<&str> = back.lines().collect();
        for i in 0..a.len().max(c.len()) {
            let (x, y) = (a.get(i).copied(), c.get(i).copied());
            if x != y {
                println!("  line {}: src {x:?}\n           out {y:?}", i + 1);
                break;
            }
        }
    }
    println!(
        "{}: {} src -> {} bin, {} nodes, {} segments, {} clamps, round-trip {}",
        path.display(),
        src.len(),
        bytes.len(),
        s.nodes.len(),
        st.segments,
        st.tier_clamps,
        if back == src { "exact" } else { "differs" }
    );
}

fn stem(path: &Path) -> String {
    path.parent()
        .and_then(Path::file_name)
        .map_or_else(|| "skill".into(), |s| s.to_string_lossy().into_owned())
}

fn corpus(list: &str, limit: usize, profile: Profile, dict: Option<&Dictionary>) {
    let listing = std::fs::read_to_string(list).expect("read list");
    let mut c = Corpus::default();

    for line in listing.lines().take(limit) {
        let path = PathBuf::from(line);
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        c.seen += 1;
        c.src_bytes += src.len() as u64;

        let doc = md::parse(&src);
        let (bytes, st) = match compile_with(&doc, &stem(&path), profile, dict) {
            Ok(v) => v,
            Err(e) => {
                record(&mut c, line, &format!("compile: {e:?}"));
                continue;
            }
        };
        c.compiled += 1;
        c.bin_bytes += bytes.len() as u64;
        c.segments += st.segments;
        c.clamps += st.tier_clamps;

        let s = match Skill::open_with(&bytes, &TrustPolicy::permissive(), dict) {
            Ok(s) => s,
            Err(e) => {
                record(&mut c, line, &format!("open: {e:?}"));
                continue;
            }
        };
        if let Err(e) = s.verify_all() {
            record(&mut c, line, &format!("verify: {e:?}"));
            continue;
        }
        c.verified += 1;
        // What routing actually has to touch: the header plus the routing block, not the
        // whole manifest.
        if let Ok(r) = skill_format::routing::read(&bytes, s.header.routing_off()) {
            c.routing_bytes += (s.header.routing_off() + r.stored_len) as u64;
        }
        c.manifest_span += u64::from(s.header.manifest_off + s.header.manifest_len);
        c.nodes_b += u64::try_from(s.nodes.len() * 32).unwrap_or(0);
        c.hashes_b += u64::from(s.manifest.hash_count()) * 32;
        c.edges_b += u64::from(s.manifest.edge_count()) * 8;
        c.hot_b += u64::from(s.manifest.hot.1);
        c.cold_b += u64::from(s.manifest.cold.1);
        c.manifest_b += u64::from(s.header.manifest_len);
        c.nodes
            .push(u32::try_from(s.nodes.len()).unwrap_or(u32::MAX));

        match render(&s) {
            Ok(back) if back == src => {
                c.exact += 1;
                c.normalized += 1;
            }
            Ok(back) if norm(&back) == norm(&src) => c.normalized += 1,
            Ok(_) => record(&mut c, line, "round-trip: differs"),
            Err(e) => record(&mut c, line, &format!("render: {e:?}")),
        }
    }

    c.nodes.sort_unstable();
    let pct = |n: u32| f64::from(n) * 100.0 / f64::from(c.seen.max(1));
    println!(
        "profile          {profile:?}{}",
        if dict.is_some() { " + dict" } else { "" }
    );
    println!("files            {}", c.seen);
    println!("compiled         {} ({:.2}%)", c.compiled, pct(c.compiled));
    println!("verified         {} ({:.2}%)", c.verified, pct(c.verified));
    println!("round-trip exact {} ({:.2}%)", c.exact, pct(c.exact));
    println!(
        "round-trip norm  {} ({:.2}%)",
        c.normalized,
        pct(c.normalized)
    );
    println!("segments lifted  {}", c.segments);
    println!("tier clamps      {}", c.clamps);
    if !c.nodes.is_empty() {
        let q = |f: f64| c.nodes[((c.nodes.len() as f64 - 1.0) * f) as usize];
        println!(
            "nodes/skill      med {} p95 {} max {}",
            q(0.5),
            q(0.95),
            c.nodes[c.nodes.len() - 1]
        );
    }
    println!(
        "routing touches  {} B/skill avg (header + routing block)",
        c.routing_bytes / u64::from(c.verified.max(1))
    );
    println!(
        "manifest span    {} B/skill avg (what routing used to have to read)",
        c.manifest_span / u64::from(c.verified.max(1))
    );
    println!(
        "size             {:.1} MB src -> {:.1} MB bin ({:+.1}%)",
        c.src_bytes as f64 / 1.048_576e6,
        c.bin_bytes as f64 / 1.048_576e6,
        (c.bin_bytes as f64 / c.src_bytes.max(1) as f64 - 1.0) * 100.0
    );

    let mb = |v: u64| v as f64 / 1.048_576e6;
    println!("\nstored bytes (MB):");
    println!("  manifest (tables)  {:.1}", mb(c.manifest_b));
    println!("  payload HOT        {:.1}", mb(c.hot_b));
    println!("  payload COLD       {:.1}", mb(c.cold_b));
    println!("logical table sizes, before any compression (MB):");
    println!("  node records       {:.1}", mb(c.nodes_b));
    println!("  subtree hashes     {:.1}", mb(c.hashes_b));
    println!("  edge records       {:.1}", mb(c.edges_b));

    let mut kinds: Vec<(String, usize)> = Vec::new();
    for (_, why) in &c.failures {
        let key = why
            .split(&['{', '('])
            .next()
            .unwrap_or(why)
            .trim()
            .to_string();
        match kinds.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => kinds.push((key, 1)),
        }
    }
    kinds.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    if !kinds.is_empty() {
        println!("\nfailure modes:");
        for (k, n) in kinds.iter().take(12) {
            println!("  {n:6}  {k}");
        }
        println!("\nfirst 5 failing files:");
        for (p, why) in c.failures.iter().take(5) {
            println!("  {p}\n      {why}");
        }
    }
}

fn record(c: &mut Corpus, path: &str, why: &str) {
    c.failures.push((path.to_string(), why.to_string()));
}

fn norm(s: &str) -> String {
    s.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}
