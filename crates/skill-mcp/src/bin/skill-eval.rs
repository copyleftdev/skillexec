//! Sweeps the lexical/semantic blend against a labelled query set.
//!
//! The weight was 0.6 because 0.6 felt right, which is not a reason. This measures it.
//!
//! Every accuracy is reported with a Wilson interval, because fifty queries cannot distinguish
//! differences of a few points and a bare percentage invites the reader to think otherwise.

use std::path::PathBuf;

use skill_mcp::library::Library;

struct Case {
    query: String,
    targets: Vec<String>,
}

/// Wilson score interval: the honest one for a proportion at this sample size, where the normal
/// approximation is wrong at the ends and wrong in a direction that flatters whatever you ran.
fn wilson(hits: usize, total: usize) -> (f64, f64, f64) {
    if total == 0 {
        return (0.0, 0.0, 0.0);
    }
    #[allow(clippy::cast_precision_loss)]
    let (observed, n) = (hits as f64, total as f64);
    let rate = observed / n;
    let z = 1.96_f64;
    let denom = 1.0 + z * z / n;
    let centre = (rate + z * z / (2.0 * n)) / denom;
    let half = z * ((rate * (1.0 - rate) / n) + z * z / (4.0 * n * n)).sqrt() / denom;
    (rate, (centre - half).max(0.0), (centre + half).min(1.0))
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let lib_path = PathBuf::from(args.first().cloned().unwrap_or_else(|| {
        std::env::var("SKILL_LIBRARY").unwrap_or_else(|_| "library.skill".into())
    }));
    let set_path = args.get(1).cloned().unwrap_or_else(|| "queries.tsv".into());

    let cases: Vec<Case> = std::fs::read_to_string(&set_path)?
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (q, t) = l.split_once('\t')?;
            Some(Case {
                query: q.trim().to_string(),
                targets: t.split(',').map(|s| s.trim().to_string()).collect(),
            })
        })
        .collect();
    anyhow::ensure!(!cases.is_empty(), "no labelled queries in {set_path}");

    let lib = Library::open(&lib_path, None)?;
    println!(
        "{} queries against {} skills\n",
        cases.len(),
        lib.catalogue()?.len()
    );
    println!(
        "{:>6}  {:>8}  {:>8}  {:>6}  95% CI on top-1",
        "weight", "top-1", "top-3", "MRR"
    );

    let mut best = (0.0f32, -1.0f64);
    for step in 0..=10 {
        #[allow(clippy::cast_precision_loss)]
        let w = step as f32 / 10.0;
        let (mut top1, mut top3, mut rr) = (0usize, 0usize, 0.0f64);
        for c in &cases {
            let hits = lib.search_weighted(&c.query, 10, w)?;
            let rank = hits.iter().position(|h| c.targets.contains(&h.name));
            match rank {
                Some(0) => {
                    top1 += 1;
                    top3 += 1;
                    rr += 1.0;
                }
                Some(i) => {
                    if i < 3 {
                        top3 += 1;
                    }
                    #[allow(clippy::cast_precision_loss)]
                    {
                        rr += 1.0 / (i + 1) as f64;
                    }
                }
                None => {}
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let n = cases.len() as f64;
        let (p1, lo, hi) = wilson(top1, cases.len());
        #[allow(clippy::cast_precision_loss)]
        let p3 = top3 as f64 / n;
        let mrr = rr / n;
        println!(
            "{w:>6.1}  {:>7.1}%  {:>7.1}%  {mrr:>6.3}  [{:.1}%, {:.1}%]",
            p1 * 100.0,
            p3 * 100.0,
            lo * 100.0,
            hi * 100.0
        );
        if mrr > best.1 {
            best = (w, mrr);
        }
    }
    println!("\nbest MRR at weight {:.1} ({:.3})", best.0, best.1);
    Ok(())
}
