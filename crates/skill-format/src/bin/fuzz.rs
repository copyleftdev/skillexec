//! A deliberately small, deliberately bounded parser fuzzer.
//!
//! Every knob that could consume the machine is capped here rather than delegated to a harness:
//! it is **single-threaded**, stops at whichever of an iteration count or a wall-clock deadline
//! comes first, and never hands the parser an input larger than `--max-bytes`. That last one is
//! the load-bearing bound — the validator allocates proportionally to lengths the *file*
//! declares, so capping the file caps the allocation. There is no thread pool and no `-jobs`
//! equivalent, so nothing can multiply the cost by core count.
//!
//! Failures are reproducible: the seed and the offending bytes are printed, and the same seed
//! replays the same sequence.

use std::panic::{self, AssertUnwindSafe};
use std::time::{Duration, Instant};

use skill_format::{Abi, Builder, Cap, EdgeKind, Kind, SegmentSpec, Skill, Tier, TrustPolicy};

const DEFAULT_ITERS: u64 = 20_000;
const DEFAULT_SECONDS: u64 = 10;
const DEFAULT_MAX_BYTES: usize = 64 * 1024;
const HARD_MAX_BYTES: usize = 1 << 20;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        usize::try_from(self.next() % n as u64).unwrap_or(0)
    }
}

struct Budget {
    iters: u64,
    deadline: Instant,
    max_bytes: usize,
    seed: u64,
}

fn parse_args() -> Budget {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |name: &str, default: u64| -> u64 {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let max_bytes = usize::try_from(get("--max-bytes", DEFAULT_MAX_BYTES as u64))
        .unwrap_or(DEFAULT_MAX_BYTES)
        .min(HARD_MAX_BYTES);
    Budget {
        iters: get("--iters", DEFAULT_ITERS),
        deadline: Instant::now() + Duration::from_secs(get("--seconds", DEFAULT_SECONDS)),
        max_bytes,
        seed: get("--seed", 0x5EED_1234),
    }
}

fn seeds() -> Vec<Vec<u8>> {
    let mut out = Vec::new();

    let mut b = Builder::new("demo", "Use when demonstrating the skill format.");
    let root = b.root(Kind::Prose, Tier::Routing, 1, Vec::new());
    b.child(
        root,
        Kind::Prose,
        Tier::Body,
        1,
        None,
        &b"Do the thing.\n"[..],
        1,
    );
    out.push(b.build().expect("minimal seed"));

    let mut b = Builder::new("rich", "Every kind and edge.").version("1.0.0");
    let root = b.root(Kind::Prose, Tier::Routing, 1, Vec::new());
    let body = b.child(
        root,
        Kind::Prose,
        Tier::Body,
        1,
        Some("body"),
        &b"Overview\n"[..],
        1,
    );
    b.child(
        root,
        Kind::Applicability,
        Tier::Routing,
        13,
        Some("when"),
        &b"when X"[..],
        2,
    );
    let pre = b.child(
        body,
        Kind::Contract,
        Tier::Body,
        10,
        Some("pre"),
        &b"needs"[..],
        2,
    );
    let s1 = b.child(
        body,
        Kind::Prose,
        Tier::Body,
        3,
        Some("s1"),
        &b"first"[..],
        3,
    );
    let s2 = b.child(
        body,
        Kind::Prose,
        Tier::Body,
        3,
        Some("s2"),
        &b"second"[..],
        3,
    );
    let tool = b.child(
        body,
        Kind::Binding,
        Tier::Body,
        14,
        Some("rg"),
        Vec::new(),
        0,
    );
    let doc = b.child(
        body,
        Kind::Resource,
        Tier::OnDemand,
        9,
        Some("ref"),
        &b"ref"[..],
        0,
    );
    let seg = b.segment(
        body,
        Some("```sh"),
        &b"echo hi"[..],
        SegmentSpec {
            caps: vec![Cap {
                kind: 1,
                flags: 0,
                arg: "example.com".into(),
            }],
            ..SegmentSpec::inert(Abi::Sh)
        },
        0,
    );
    b.edge(s1, EdgeKind::Seq, s2, 0, 0);
    b.edge(pre, EdgeKind::Guards, s1, 0, 0);
    b.edge(s1, EdgeKind::Needs, tool, 0, 0);
    b.edge(s2, EdgeKind::Needs, seg, 0, 0);
    b.edge(s2, EdgeKind::Alt, doc, 0, 0);
    b.edge(body, EdgeKind::Cites, doc, 0, 0);
    out.push(b.build().expect("rich seed"));
    out
}

const INTERESTING: [u32; 10] = [
    0,
    1,
    2,
    7,
    8,
    0x7FFF_FFFF,
    0x8000_0000,
    0xFFFF_FFF0,
    0xFFFF_FFFE,
    0xFFFF_FFFF,
];

fn mutate(rng: &mut Rng, base: &[u8], max_bytes: usize) -> Vec<u8> {
    let mut v = base.to_vec();
    let rounds = 1 + rng.below(4);
    for _ in 0..rounds {
        if v.is_empty() {
            break;
        }
        match rng.below(6) {
            0 => {
                let i = rng.below(v.len());
                v[i] ^= 1 << rng.below(8);
            }
            1 => {
                let i = rng.below(v.len());
                v[i] = (rng.next() & 0xFF) as u8;
            }
            2 => {
                // Aligned u32 overwrite with a boundary value: the offsets and lengths are
                // where a parser actually breaks, and random bytes rarely land on one.
                let i = rng.below(v.len() / 4) * 4;
                let val = INTERESTING[rng.below(INTERESTING.len())];
                if i + 4 <= v.len() {
                    v[i..i + 4].copy_from_slice(&val.to_le_bytes());
                }
            }
            3 => {
                let cut = rng.below(v.len());
                v.truncate(cut);
            }
            4 => {
                let n = rng.below(64);
                if v.len() + n <= max_bytes {
                    for _ in 0..n {
                        v.push((rng.next() & 0xFF) as u8);
                    }
                }
            }
            _ => {
                let a = rng.below(v.len());
                let b = rng.below(v.len());
                v.swap(a, b);
            }
        }
    }
    v.truncate(max_bytes);
    v
}

fn recommit(v: &mut [u8]) {
    if v.len() < 64 {
        return;
    }
    let off = u32::from_le_bytes(v[0x14..0x18].try_into().expect("4 bytes")) as usize;
    let len = u32::from_le_bytes(v[0x18..0x1C].try_into().expect("4 bytes")) as usize;
    let Some(end) = off.checked_add(len) else {
        return;
    };
    if end > v.len() {
        return;
    }
    let root = *blake3::hash(&v[off..end]).as_bytes();
    v[0x20..0x40].copy_from_slice(&root);
}

/// The whole contract under test: for any bytes at all, the reader returns `Ok` or `Err`, and a
/// file it accepted can be walked without panicking.
fn exercise(bytes: &[u8]) {
    let Ok(s) = Skill::open(bytes, &TrustPolicy::permissive()) else {
        return;
    };
    let _ = s.name();
    let _ = s.description();
    let _ = s.verify_all();
    for i in 0..u32::try_from(s.nodes.len()).unwrap_or(0) {
        let _ = s.payload(i);
        let _ = s.verified_payload(i);
        let _ = s.children_of(i);
    }
    for i in 0..s.manifest.segment_count() {
        let _ = s.manifest.segment(i);
    }
    for i in 0..s.manifest.edge_count() {
        let _ = s.manifest.edge(i);
    }
}

fn main() {
    let budget = parse_args();
    let corpus = seeds();
    let mut rng = Rng(budget.seed | 1);
    let mut accepted = 0u64;
    let mut done = 0u64;

    panic::set_hook(Box::new(|_| {}));
    let start = Instant::now();

    for i in 0..budget.iters {
        if Instant::now() >= budget.deadline {
            break;
        }
        done = i + 1;
        let base = &corpus[rng.below(corpus.len())];
        let mut input = mutate(&mut rng, base, budget.max_bytes);
        if rng.next().is_multiple_of(2) {
            recommit(&mut input);
        }
        if input.len() >= 0x14 {
            let len = u32::try_from(input.len()).unwrap_or(u32::MAX);
            if rng.next().is_multiple_of(2) {
                input[0x10..0x14].copy_from_slice(&len.to_le_bytes());
            }
        }

        let probe = input.clone();
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
            if Skill::open(&probe, &TrustPolicy::permissive()).is_ok() {
                exercise(&probe);
                return true;
            }
            false
        }));

        match outcome {
            Ok(true) => accepted += 1,
            Ok(false) => {}
            Err(_) => {
                let _ = panic::take_hook();
                eprintln!("PANIC at iteration {i} (seed {})", budget.seed);
                eprintln!("input ({} bytes):", input.len());
                for chunk in input.chunks(32) {
                    let hex = chunk.iter().fold(String::new(), |mut acc, b| {
                        use std::fmt::Write as _;
                        let _ = write!(acc, "{b:02x}");
                        acc
                    });
                    eprintln!("  {hex}");
                }
                std::process::exit(1);
            }
        }
    }

    let _ = panic::take_hook();
    println!(
        "fuzz ok: {done} inputs, {accepted} accepted, {:.1}s, seed {}, max {} bytes, 1 thread",
        start.elapsed().as_secs_f64(),
        budget.seed,
        budget.max_bytes
    );
}
