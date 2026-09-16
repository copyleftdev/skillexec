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
use std::sync::atomic::{AtomicU64, Ordering};
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
        _ => eprintln!(
            "usage:\n  \
             corpusctl clone --list <tsv> --dest <dir> [--jobs N] [--budget-gb N] [--timeout N] [--max-kb N]\n  \
             corpusctl prune --dest <dir> --sidecar <tsv> [--jobs N]\n  \
             corpusctl build --dest <dir> --out <txt> [--exclude <txt>] [--jobs N]"
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

fn dir_size_gb(root: &Path) -> u64 {
    let bytes: u64 = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|e| e.metadata().ok())
        .filter(std::fs::Metadata::is_file)
        .map(|m| m.len())
        .sum();
    bytes / (1024 * 1024 * 1024)
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
    // Sampled rather than recomputed per repo: walking a 20 GB tree for every clone costs more
    // than the clone does.
    let budget_hit = std::sync::atomic::AtomicBool::new(false);

    repos.par_iter().for_each(|(full, kb)| {
        if budget_hit.load(Ordering::Relaxed) {
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

        let done = cloned.load(Ordering::Relaxed);
        if done.is_multiple_of(64) && dir_size_gb(&dest) >= budget_gb {
            budget_hit.store(true, Ordering::Relaxed);
            return;
        }

        if !git_clone(full, &path, timeout) {
            let _ = fs::remove_dir_all(&path);
            failed.fetch_add(1, Ordering::Relaxed);
            return;
        }

        let _ = fs::remove_dir_all(path.join(".git"));
        prune_repo(&path, &sidecar);
        let _ = fs::create_dir_all(&path);
        let _ = fs::File::create(path.join(".done"));
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
