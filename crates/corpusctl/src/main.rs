//! Builds a `SKILL.md` corpus: supervised git clones, pruning, and BLAKE3 deduplication.
//!
//! This replaces a pile of shell that was the single largest source of defects while assembling
//! the held-out corpus — `find` not following symlinks, `xargs` running a command against stdin
//! on empty input, `pgrep -f` matching the shell that invoked it (three times), a wait loop
//! deadlocked because the watcher's own command line contained the pattern being watched for,
//! and `grep` buffering that discarded two completed runs. None of those are domain problems.
//!
//! Where the speed actually comes from: hashing 428,000 files in-process with BLAKE3, which uses
//! AVX2 on this machine, instead of spawning 428,000 `md5sum` processes. The rest of the work is
//! `unlink` and `getdents` syscalls, where SIMD is not the lever and parallelism is — bounded
//! parallelism, because the box is shared.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rayon::prelude::*;

const SKILL: &str = "SKILL.md";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map_or("help", String::as_str);
    let jobs = flag(&args, "--jobs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(8usize);

    // Explicit, modest, and never `nproc`. This box has 64 cores and other tenants.
    rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .build_global()
        .expect("thread pool");

    match cmd {
        "clone" => clone(&args),
        "prune" => prune(&args),
        "build" => build(&args),
        "run" => run(&args),
        _ => eprintln!(
            "usage:\n  \
             corpusctl clone --list <tsv> --dest <dir> [--jobs N] [--budget-gb N] [--timeout N] [--max-kb N]\n  \
             corpusctl prune --dest <dir> --sidecar <tsv> [--jobs N]\n  \
             corpusctl build --dest <dir> --out <txt> [--exclude <txt>] [--jobs N]\n  \
             corpusctl run   --list <txt> --out <dir> [--skillc <path>] [--dict <path>]"
        ),
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn need(args: &[String], name: &str) -> String {
    flag(args, name).unwrap_or_else(|| {
        eprintln!("missing {name}");
        std::process::exit(2);
    })
}

/// Every `SKILL.md` under `root`, skipping VCS metadata.
fn skill_files(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.file_name() != ".git")
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && e.file_name() == SKILL)
        .map(walkdir::DirEntry::into_path)
        .collect()
}

fn dir_size_bytes(root: &Path) -> u64 {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|e| e.metadata().ok())
        .filter(std::fs::Metadata::is_file)
        .map(|m| m.len())
        .sum()
}

// ---------------------------------------------------------------------------- budget

/// How many times a clone waits for another clone's reservation to reconcile before giving up its
/// turn. The wait is bounded at `ADMIT_TRIES × ADMIT_BACKOFF`, so contention costs time and never
/// a spin; a repo that never gets in is reported as past budget and the run moves on.
const ADMIT_TRIES: u32 = 4;
const ADMIT_BACKOFF: Duration = Duration::from_millis(250);

/// The disk ceiling a clone run holds itself to.
///
/// The ceiling is charged, not sampled. Walking the whole destination tree costs more than a clone
/// does, so it is walked once here — which is also how a resumed run counts what is already on
/// disk — and every clone afterwards keeps the total current by measuring only the tree it wrote.
///
/// `used` is what admission spends: bytes on disk plus every reservation in flight. `charged` is
/// bytes on disk alone. They are separate because a refusal means two different things, and
/// treating them alike is what would let a modest budget stop a run that had barely started.
struct Budget {
    ceiling: u64,
    used: AtomicU64,
    charged: AtomicU64,
    full: AtomicBool,
}

impl Budget {
    fn new(root: &Path, gb: u64) -> Self {
        let on_disk = dir_size_bytes(root);
        Self {
            ceiling: gb.saturating_mul(1024 * 1024 * 1024),
            used: AtomicU64::new(on_disk),
            charged: AtomicU64::new(on_disk),
            full: AtomicBool::new(false),
        }
    }

    /// Whether the bytes on disk have reached the ceiling, which is terminal for the run.
    fn is_full(&self) -> bool {
        self.full.load(Ordering::Relaxed)
    }

    /// Holds `bytes` against the ceiling for the caller, or refuses.
    ///
    /// Reserving before the clone rather than checking after it is the point of the whole
    /// structure: a check on the current total says nothing about the clones already running, so
    /// with N workers it can be N repos stale. A reservation is held by the clone that will spend
    /// it, so the ceiling holds however many run at once.
    fn admit(&self, bytes: u64) -> bool {
        for _ in 0..ADMIT_TRIES {
            // Bytes on disk alone at the ceiling: the run really is done, and every later repo
            // can stop without measuring anything.
            if self.charged.load(Ordering::Relaxed) >= self.ceiling {
                self.full.store(true, Ordering::Relaxed);
                return false;
            }
            if self.used.fetch_add(bytes, Ordering::Relaxed) + bytes <= self.ceiling {
                return true;
            }
            // The shortfall is other clones' reservations rather than spent bytes, so this is
            // contention. Wait for one of them to reconcile instead of declaring the budget gone.
            self.used.fetch_sub(bytes, Ordering::Relaxed);
            std::thread::sleep(ADMIT_BACKOFF);
        }
        false
    }

    /// Gives back a reservation whose clone never happened.
    fn release(&self, reserved: u64) {
        self.used.fetch_sub(reserved, Ordering::Relaxed);
    }

    /// Turns a reservation into the bytes it actually cost.
    ///
    /// Returns `false` when those bytes carry the total past the ceiling, and the caller must then
    /// remove what it wrote. A reservation for a repo the listing gave no size for is a guess, and
    /// keeping a repo that beat its guess would leave a ceiling that holds everywhere except where
    /// it was tested.
    fn commit(&self, reserved: u64, actual: u64) -> bool {
        // Charge before releasing, so the total never dips below what is on disk and a concurrent
        // clone cannot be admitted against bytes that are already spent.
        self.used.fetch_add(actual, Ordering::Relaxed);
        self.used.fetch_sub(reserved, Ordering::Relaxed);
        if self.charged.fetch_add(actual, Ordering::Relaxed) + actual > self.ceiling {
            self.charged.fetch_sub(actual, Ordering::Relaxed);
            self.used.fetch_sub(actual, Ordering::Relaxed);
            self.full.store(true, Ordering::Relaxed);
            return false;
        }
        true
    }
}

// ---------------------------------------------------------------------------- clone

/// Shallow-clones each repo, then prunes it to its skills before moving on.
///
/// The child is supervised through its own `Child` handle — polled for completion and killed by
/// handle on timeout. Nothing here matches process names, which is what made the shell version
/// able to kill its own supervisor.
fn clone(args: &[String]) {
    let list = need(args, "--list");
    let dest = PathBuf::from(need(args, "--dest"));
    let budget_gb: u64 = flag(args, "--budget-gb")
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let timeout = Duration::from_secs(
        flag(args, "--timeout")
            .and_then(|v| v.parse().ok())
            .unwrap_or(180),
    );
    let max_kb: u64 = flag(args, "--max-kb")
        .and_then(|v| v.parse().ok())
        .unwrap_or(400_000);
    let sidecar = dest.parent().unwrap_or(Path::new(".")).join("siblings.tsv");

    fs::create_dir_all(&dest).expect("dest");
    let listing = fs::read_to_string(&list).expect("read list");
    let repos: Vec<(String, u64)> = listing
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let name = f.next()?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let kb = f.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            Some((name, kb))
        })
        .collect();

    let cloned = AtomicU64::new(0);
    let skipped = AtomicU64::new(0);
    let failed = AtomicU64::new(0);
    let over_budget = AtomicU64::new(0);
    let started = Instant::now();
    let budget = Budget::new(&dest, budget_gb);

    repos.par_iter().for_each(|(full, kb)| {
        if budget.is_full() {
            over_budget.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if *kb > 0 && *kb > max_kb {
            skipped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let name = full.replace('/', "__");
        let path = dest.join(&name);
        if path.join(".done").exists() {
            skipped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let _ = fs::remove_dir_all(&path);

        // A listing without a size reserves `max_kb`, which is the most a repo is allowed to cost
        // anyway; `commit` below replaces the guess with what the pruned tree really occupies.
        let reserved = if *kb > 0 { *kb } else { max_kb }.saturating_mul(1024);
        if !budget.admit(reserved) {
            over_budget.fetch_add(1, Ordering::Relaxed);
            return;
        }

        if !git_clone(full, &path, timeout) {
            let _ = fs::remove_dir_all(&path);
            budget.release(reserved);
            failed.fetch_add(1, Ordering::Relaxed);
            return;
        }

        let _ = fs::remove_dir_all(path.join(".git"));
        prune_repo(&path, &sidecar);
        let _ = fs::create_dir_all(&path);
        let _ = fs::File::create(path.join(".done"));
        if !budget.commit(reserved, dir_size_bytes(&path)) {
            let _ = fs::remove_dir_all(&path);
            over_budget.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let n = cloned.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_multiple_of(100) {
            println!("  cloned {n} in {:?}", started.elapsed());
        }
    });

    println!(
        "clone: {} new, {} skipped, {} failed, {} past budget, {:?}",
        cloned.load(Ordering::Relaxed),
        skipped.load(Ordering::Relaxed),
        failed.load(Ordering::Relaxed),
        over_budget.load(Ordering::Relaxed),
        started.elapsed()
    );
}

/// Runs one `git clone` under a wall-clock deadline.
///
/// The child is owned and killed through its own handle. The shell version matched process
/// names, which is how it managed to kill its own supervisor three separate times.
fn git_clone(full: &str, path: &Path, timeout: Duration) -> bool {
    let url = format!("https://github.com/{full}.git");
    let Ok(mut child) = Command::new("git")
        .args(["clone", "--quiet", "--depth", "1", "--single-branch", &url])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => return false,
        }
    }
}

// ---------------------------------------------------------------------------- run

/// Runs the experiment suite, one child per step, each writing straight to its own file.
///
/// No pipes: filtering a long run through `grep` buffers the whole thing, so a child that dies
/// late discards results it had already computed. That cost two completed runs.
fn run(args: &[String]) {
    let list = need(args, "--list");
    let out = PathBuf::from(flag(args, "--out").unwrap_or_else(|| "results".into()));
    let skillc = flag(args, "--skillc")
        .unwrap_or_else(|| "/home/ops/.cargo-target/release/skillc".to_string());
    let dict = flag(args, "--dict").unwrap_or_else(|| "full.dict".to_string());
    fs::create_dir_all(&out).expect("out dir");

    let mut steps: Vec<(&str, Vec<String>)> = vec![
        ("01-roles", vec!["roles".into(), list.clone()]),
        (
            "02-structure",
            vec![
                "corpus".into(),
                list.clone(),
                "999999".into(),
                "--profile".into(),
                "none".into(),
            ],
        ),
        ("03-cas", vec!["cas".into(), list.clone()]),
        (
            "04-dict",
            vec!["dict".into(), list.clone(), dict.clone(), "112640".into()],
        ),
    ];
    for p in ["none", "mapped", "compact"] {
        steps.push((
            Box::leak(format!("05-size-{p}").into_boxed_str()),
            vec![
                "corpus".into(),
                list.clone(),
                "999999".into(),
                "--profile".into(),
                p.into(),
            ],
        ));
    }
    for p in ["mapped", "compact"] {
        steps.push((
            Box::leak(format!("06-size-{p}-dict").into_boxed_str()),
            vec![
                "corpus".into(),
                list.clone(),
                "999999".into(),
                "--profile".into(),
                p.into(),
                "--dict".into(),
                dict.clone(),
            ],
        ));
        steps.push((
            Box::leak(format!("07-route-{p}").into_boxed_str()),
            vec![
                "route".into(),
                list.clone(),
                "--profile".into(),
                p.into(),
                "--dict".into(),
                dict.clone(),
            ],
        ));
    }

    for (name, argv) in steps {
        let started = Instant::now();
        let path = out.join(format!("{name}.txt"));
        let Ok(fh) = fs::File::create(&path) else {
            continue;
        };
        let status = Command::new(&skillc)
            .args(&argv)
            .stdout(Stdio::from(fh))
            .stderr(Stdio::null())
            .status();
        let ok = status.is_ok_and(|s| s.success());
        println!(
            "{name:<22} {:>8?}  {}",
            started.elapsed(),
            if ok { "ok" } else { "FAILED" }
        );
    }
    let _ = fs::File::create(out.join("COMPLETE"));
}

// ---------------------------------------------------------------------------- prune

/// Records each skill's sibling directories, then deletes everything that is not a `SKILL.md`.
///
/// A skill repository is almost entirely not skills: 315 unpruned repos were 40 GB, of which the
/// skills are a few hundred megabytes.
fn prune_repo(repo: &Path, sidecar: &Path) {
    let files = skill_files(repo);
    if files.is_empty() {
        return;
    }
    let repo_name = repo
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut rows = String::new();
    for f in &files {
        let Some(d) = f.parent() else { continue };
        let sibs: Vec<String> = fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let rel = d.strip_prefix(repo).unwrap_or(d).to_string_lossy();
        let _ = writeln!(rows, "{repo_name}\t{rel}\t{}", sibs.join(","));
    }
    if let Ok(mut fh) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(sidecar)
    {
        let _ = fh.write_all(rows.as_bytes());
    }

    let keep: HashSet<PathBuf> = files.into_iter().collect();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for e in walkdir::WalkDir::new(repo)
        .into_iter()
        .filter_map(Result::ok)
    {
        let p = e.path();
        if e.file_type().is_dir() {
            dirs.push(p.to_path_buf());
        } else if !keep.contains(p) {
            let _ = fs::remove_file(p);
        }
    }
    // Deepest first, so a directory is only tried once its children are gone.
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in dirs {
        let _ = fs::remove_dir(&d);
    }
}

fn prune(args: &[String]) {
    let dest = PathBuf::from(need(args, "--dest"));
    let sidecar =
        PathBuf::from(flag(args, "--sidecar").unwrap_or_else(|| "siblings.tsv".to_string()));
    let repos: Vec<PathBuf> = fs::read_dir(&dest)
        .expect("dest")
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .filter(|p| !p.join(".done").exists())
        .collect();

    let started = Instant::now();
    let n = AtomicU64::new(0);
    repos.par_iter().for_each(|p| {
        prune_repo(p, &sidecar);
        let _ = fs::create_dir_all(p);
        let _ = fs::File::create(p.join(".done"));
        n.fetch_add(1, Ordering::Relaxed);
    });
    println!(
        "prune: {} repos in {:?}",
        n.load(Ordering::Relaxed),
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------- build

/// Deduplicates by content and subtracts an exclusion corpus, so "held out" means held out.
fn build(args: &[String]) {
    let dest = PathBuf::from(need(args, "--dest"));
    let out = PathBuf::from(need(args, "--out"));
    let exclude = flag(args, "--exclude");
    let started = Instant::now();

    let files = skill_files(&dest);
    println!("found        {}", files.len());

    // BLAKE3 in-process. The shell version spawned one `md5sum` per file; at this scale that is
    // most of the runtime and none of the work.
    let mut hashed: Vec<([u8; 32], PathBuf)> = files
        .par_iter()
        .filter_map(|p| {
            fs::read(p)
                .ok()
                .map(|b| (*blake3::hash(&b).as_bytes(), p.clone()))
        })
        .collect();
    hashed.par_sort_unstable_by(|a, b| a.1.cmp(&b.1));

    let mut seen: HashSet<[u8; 32]> = HashSet::with_capacity(hashed.len());
    let mut distinct: Vec<([u8; 32], PathBuf)> = Vec::with_capacity(hashed.len());
    for (h, p) in hashed {
        if seen.insert(h) {
            distinct.push((h, p));
        }
    }
    println!("distinct     {}", distinct.len());

    let mut kept = distinct;
    if let Some(ex) = exclude {
        let listing = fs::read_to_string(&ex).unwrap_or_default();
        let paths: Vec<&str> = listing.lines().filter(|l| !l.is_empty()).collect();
        let excluded: HashSet<[u8; 32]> = paths
            .par_iter()
            .filter_map(|p| fs::read(p).ok().map(|b| *blake3::hash(&b).as_bytes()))
            .collect();
        let before = kept.len();
        kept.retain(|(h, _)| !excluded.contains(h));
        println!("also in A    {}", before - kept.len());
    }

    let mut w = std::io::BufWriter::new(fs::File::create(&out).expect("out"));
    let base = fs::canonicalize(&dest).unwrap_or_else(|_| dest.clone());
    for (_, p) in &kept {
        // Absolute, because the list is consumed from other working directories. A relative
        // list silently produced "files 0" the first time this was run from elsewhere.
        let abs = base.join(p.strip_prefix(&dest).unwrap_or(p));
        let _ = writeln!(w, "{}", abs.display());
    }
    let _ = w.flush();

    let repos: HashMap<String, usize> = kept.iter().fold(HashMap::new(), |mut m, (_, p)| {
        // Strip the dest as it was given, not its canonical form: the walk produced paths in
        // the caller's spelling, and mixing the two counts every repo as one.
        let rel = p.strip_prefix(&dest).unwrap_or(p);
        if let Some(r) = rel.components().next() {
            *m.entry(r.as_os_str().to_string_lossy().into_owned())
                .or_insert(0) += 1;
        }
        m
    });
    println!("held out     {}", kept.len());
    println!("repos        {}", repos.len());
    println!("elapsed      {:?}", started.elapsed());
}

// ---------------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::{ADMIT_BACKOFF, ADMIT_TRIES, Budget};
    use std::path::Path;
    use std::sync::atomic::Ordering;

    const GB: u64 = 1024 * 1024 * 1024;

    /// An empty directory that exists, so `Budget::new` starts from a walk of nothing rather than
    /// from a walk that failed. Tagged per test because these all run in one process.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("corpusctl-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("scratch");
            Self(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_reservation_that_fits_is_admitted_and_one_that_does_not_is_refused() {
        let d = Scratch::new("fits");
        let b = Budget::new(d.path(), 1);
        assert!(b.admit(GB / 2));
        assert!(b.admit(GB / 2));
        // Nothing is on disk yet, so this is contention rather than a spent budget: refused
        // after the bounded wait, and the run is not declared over.
        assert!(!b.admit(1));
        assert!(!b.is_full());
    }

    #[test]
    fn a_refused_reservation_is_not_left_charged() {
        let d = Scratch::new("no-leak");
        let b = Budget::new(d.path(), 1);
        assert!(b.admit(GB));
        assert!(!b.admit(1));
        b.release(GB);
        // The refused attempt must not have leaked its reservation, or this would not fit.
        assert!(b.admit(GB));
    }

    #[test]
    fn committing_replaces_the_guess_with_what_was_really_used() {
        let d = Scratch::new("commit");
        let b = Budget::new(d.path(), 1);
        let guess = GB / 2;
        assert!(b.admit(guess));
        assert!(b.commit(guess, 1024));
        assert_eq!(b.charged.load(Ordering::Relaxed), 1024);
        assert_eq!(b.used.load(Ordering::Relaxed), 1024);
        // The guess is back, so a whole further budget's worth still fits.
        assert!(b.admit(GB - 1024));
    }

    #[test]
    fn a_repo_that_beats_its_guess_is_refused_rather_than_kept() {
        let d = Scratch::new("beats-guess");
        let b = Budget::new(d.path(), 1);
        let guess = GB / 2;
        assert!(b.admit(guess));
        // Cloned bigger than the listing implied, and past the ceiling.
        assert!(!b.commit(guess, GB + 1));
        assert_eq!(b.charged.load(Ordering::Relaxed), 0);
        assert_eq!(b.used.load(Ordering::Relaxed), 0);
        assert!(b.is_full(), "past the ceiling on disk is terminal");
    }

    #[test]
    fn a_spent_budget_is_terminal_and_a_contended_one_is_not() {
        let d = Scratch::new("terminal");
        let b = Budget::new(d.path(), 1);
        assert!(b.admit(GB));
        assert!(b.commit(GB, GB));
        assert!(!b.admit(1));
        assert!(b.is_full(), "bytes on disk reached the ceiling");
    }

    #[test]
    fn refusal_is_bounded_in_time() {
        let d = Scratch::new("bounded");
        let b = Budget::new(d.path(), 1);
        assert!(b.admit(GB));
        let t = std::time::Instant::now();
        assert!(!b.admit(1));
        // Bounded above by the retry schedule, and below it: a refusal that returned instantly
        // would mean contention was never waited out.
        assert!(t.elapsed() >= ADMIT_BACKOFF, "gave up without waiting");
        assert!(
            t.elapsed() < ADMIT_BACKOFF * (ADMIT_TRIES + 2),
            "waited longer than the schedule allows: {:?}",
            t.elapsed()
        );
    }

    #[test]
    fn concurrent_admissions_never_exceed_the_ceiling() {
        let d = Scratch::new("concurrent");
        let b = Budget::new(d.path(), 1);
        let each = GB / 8;
        // Sixty-four workers against a budget holding eight: whatever the interleaving, the
        // reservations granted must not add up past the ceiling. This is the property the old
        // every-64-clones sample could not state.
        let granted = std::thread::scope(|s| {
            let hs: Vec<_> = (0..64)
                .map(|_| s.spawn(|| u64::from(b.admit(each))))
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).sum::<u64>()
        });
        assert_eq!(
            granted, 8,
            "granted {granted} reservations of an eighth each"
        );
        assert!(b.used.load(Ordering::Relaxed) <= b.ceiling);
    }

    #[test]
    fn a_resumed_run_counts_what_is_already_on_disk() {
        let d = Scratch::new("resumed");
        std::fs::write(d.path().join("already"), vec![0u8; 4096]).expect("write");
        let b = Budget::new(d.path(), 1);
        assert_eq!(b.charged.load(Ordering::Relaxed), 4096);
        assert!(
            !b.admit(GB),
            "the resumed bytes must leave no room for a full budget"
        );
    }

    #[test]
    fn dir_size_of_a_missing_directory_is_zero_rather_than_a_panic() {
        assert_eq!(
            super::dir_size_bytes(Path::new("/nonexistent/for/this/test")),
            0
        );
    }
}
