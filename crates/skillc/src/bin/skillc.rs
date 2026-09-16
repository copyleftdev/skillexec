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

use skill_format::{Skill, TrustPolicy};
use skillc::{compile, md, render};

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
            corpus(list, limit);
        }
        Some(path) => one(Path::new(path)),
        None => eprintln!("usage: skillc <SKILL.md> | skillc corpus <list-file> [limit]"),
    }
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

fn corpus(list: &str, limit: usize) {
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
        let (bytes, st) = match compile(&doc, &stem(&path)) {
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

        let s = match Skill::open(&bytes, &TrustPolicy::permissive()) {
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
        c.routing_bytes += u64::from(s.header.manifest_off + s.header.manifest_len);
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
        "routing plane    {:.1} MB of {:.1} MB bin ({:.1}% of it), {} B/skill avg",
        c.routing_bytes as f64 / 1.048_576e6,
        c.bin_bytes as f64 / 1.048_576e6,
        c.routing_bytes as f64 * 100.0 / c.bin_bytes.max(1) as f64,
        c.routing_bytes / u64::from(c.verified.max(1))
    );
    println!(
        "size             {:.1} MB src -> {:.1} MB bin ({:+.1}%)",
        c.src_bytes as f64 / 1.048_576e6,
        c.bin_bytes as f64 / 1.048_576e6,
        (c.bin_bytes as f64 / c.src_bytes.max(1) as f64 - 1.0) * 100.0
    );

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
