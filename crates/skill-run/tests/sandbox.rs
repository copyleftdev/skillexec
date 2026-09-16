use skill_format::{Abi, Builder, Cap, Kind, SegmentSpec, Skill, Tier, TrustClass, cap_kind};
use skill_run::{Policy, RunError, run};

const HELLO: &str = r#"
(module
  (import "wasi_snapshot_preview1" "fd_write"
    (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 100) "hi\n")
  (func (export "run")
    (i32.store (i32.const 0) (i32.const 100))
    (i32.store (i32.const 4) (i32.const 3))
    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 8)))))
"#;

const PURE: &str = r#"(module (func (export "run") (nop)))"#;
const SPIN: &str = r#"(module (func (export "run") (loop $l (br $l))))"#;
const BACKDOOR: &str = r#"
(module
  (import "evil" "backdoor" (func $b))
  (func (export "run") (call $b)))
"#;
const GROW: &str = r#"
(module
  (memory (export "memory") 1)
  (func (export "run")
    (if (i32.eq (memory.grow (i32.const 64)) (i32.const -1)) (then unreachable))))
"#;

fn build(src: &str, spec: SegmentSpec) -> Vec<u8> {
    let wasm = wat::parse_str(src).expect("wat");
    let mut b = Builder::new("sandbox", "Fixture for the capability gate.");
    let root = b.root(Kind::Prose, Tier::Routing, 0, Vec::new());
    b.segment(root, Some("seg"), wasm, spec, 0);
    b.build().expect("build")
}

fn spec_wall(caps: &[u16], mem_kib: u32, cpu_ms: u32, wall_ms: u32) -> SegmentSpec {
    SegmentSpec {
        caps: caps
            .iter()
            .map(|k| Cap {
                kind: *k,
                flags: 0,
                arg: String::new(),
            })
            .collect(),
        mem_kib,
        cpu_ms,
        wall_ms,
        ..SegmentSpec::inert(Abi::Wasm32Core)
    }
}

fn spec(caps: &[u16], mem_kib: u32, cpu_ms: u32) -> SegmentSpec {
    SegmentSpec {
        caps: caps
            .iter()
            .map(|k| Cap {
                kind: *k,
                flags: 0,
                arg: String::new(),
            })
            .collect(),
        mem_kib,
        cpu_ms,
        wall_ms: 1_000,
        ..SegmentSpec::inert(Abi::Wasm32Core)
    }
}

fn open(bytes: &[u8]) -> Skill<'_> {
    Skill::open(bytes, &skill_format::TrustPolicy::permissive()).expect("open")
}

#[test]
fn an_undeclared_import_is_refused_before_instantiation() {
    let bytes = build(HELLO, spec(&[], 256, 10));
    let s = open(&bytes);
    match run(&s, 0, "run", &Policy::default()) {
        Err(RunError::UndeclaredImport {
            module,
            name,
            needs,
        }) => {
            assert_eq!(module, "wasi_snapshot_preview1");
            assert_eq!(name, "fd_write");
            assert_eq!(needs, "stdio");
        }
        other => panic!("expected refusal, got {other:?}"),
    }
}

#[test]
fn the_same_module_runs_once_the_capability_is_declared() {
    let bytes = build(HELLO, spec(&[cap_kind::STDIO], 256, 10));
    let s = open(&bytes);
    let out = run(&s, 0, "run", &Policy::default()).expect("runs");
    assert_eq!(out.stdout, b"hi\n");
    assert_eq!(out.granted, vec!["stdio"]);
    assert!(out.fuel_used > 0);
}

#[test]
fn an_import_this_host_does_not_recognise_is_refused_not_ignored() {
    let bytes = build(
        BACKDOOR,
        spec(&[cap_kind::STDIO, cap_kind::FS_READ], 256, 10),
    );
    let s = open(&bytes);
    match run(&s, 0, "run", &Policy::default()) {
        Err(RunError::UndeclaredImport { module, needs, .. }) => {
            assert_eq!(module, "evil");
            assert_eq!(needs, "unknown");
        }
        other => panic!("expected refusal, got {other:?}"),
    }
}

#[test]
fn a_segment_with_no_imports_needs_no_capabilities() {
    let bytes = build(PURE, spec(&[], 256, 10));
    let s = open(&bytes);
    let out = run(&s, 0, "run", &Policy::default()).expect("runs");
    assert!(out.granted.is_empty());
}

#[test]
fn cpu_budget_is_enforced_by_fuel() {
    let bytes = build(SPIN, spec(&[], 256, 1));
    let s = open(&bytes);
    let err = run(&s, 0, "run", &Policy::default()).expect_err("must not spin forever");
    assert!(matches!(err, RunError::OutOfFuel), "got {err:?}");
}

#[test]
fn memory_budget_is_enforced() {
    let tight = build(GROW, spec(&[], 64, 10));
    let s = open(&tight);
    assert!(matches!(
        run(&s, 0, "run", &Policy::default()),
        Err(RunError::Trap(_))
    ));

    let roomy = build(GROW, spec(&[], 8 * 1024, 10));
    let s = open(&roomy);
    run(&s, 0, "run", &Policy::default()).expect("grows within its budget");
}

#[test]
fn a_segment_may_not_exceed_the_host_ceiling() {
    let bytes = build(PURE, spec(&[], 1024, 10));
    let s = open(&bytes);
    let policy = Policy {
        max_mem_kib: 128,
        ..Policy::default()
    };
    match run(&s, 0, "run", &policy) {
        Err(RunError::LimitExceedsPolicy {
            what,
            asked,
            ceiling,
        }) => {
            assert_eq!((what, asked, ceiling), ("mem_kib", 1024, 128));
        }
        other => panic!("expected refusal, got {other:?}"),
    }
}

#[test]
fn a_host_trusted_segment_is_refused_by_default() {
    let mut b = Builder::new("shell", "A shell script is labelled, never contained.");
    let root = b.root(Kind::Prose, Tier::Routing, 0, Vec::new());
    b.segment(
        root,
        Some("s"),
        &b"rm -rf /"[..],
        SegmentSpec::inert(Abi::Sh),
        0,
    );
    let bytes = b.build().unwrap();
    let s = open(&bytes);
    assert_eq!(
        s.manifest.segment(0).unwrap().trust_class,
        TrustClass::HostTrusted
    );
    assert!(matches!(
        run(&s, 0, "run", &Policy::default()),
        Err(RunError::NotPortable { .. })
    ));
}

#[test]
fn tampered_segment_bytes_are_refused_before_compilation() {
    let bytes = build(PURE, spec(&[], 256, 10));
    let cold = open(&bytes).manifest.cold.0 as usize;
    let mut tampered = bytes.clone();
    tampered[cold + 4] ^= 0xFF;
    let s = open(&tampered);
    assert!(matches!(
        run(&s, 0, "run", &Policy::default()),
        Err(RunError::IntegrityFailed(0))
    ));
}

#[test]
fn every_capability_a_run_grants_was_declared_in_the_file() {
    let bytes = build(HELLO, spec(&[cap_kind::STDIO], 256, 10));
    let s = open(&bytes);
    let rec = s.manifest.segment(0).unwrap();
    let declared: Vec<&str> = (0..u32::from(rec.cap_cnt))
        .map(|k| cap_kind::name(s.manifest.cap(rec.cap_off + k).unwrap().0))
        .collect();
    let out = run(&s, 0, "run", &Policy::default()).unwrap();
    assert_eq!(out.granted, declared);
}

#[test]
fn wall_clock_budget_is_enforced_independently_of_fuel() {
    // Give it far more fuel than it can burn in the wall budget, so only the clock can stop it.
    // If wall_ms is not enforced this still terminates -- on fuel, several seconds later -- and
    // fails on the error kind rather than hanging.
    let bytes = build(SPIN, spec_wall(&[], 256, 5_000, 50));
    let s = open(&bytes);
    let started = std::time::Instant::now();
    let err = run(&s, 0, "run", &Policy::default()).expect_err("must stop");
    assert!(
        matches!(err, RunError::Interrupted),
        "expected the wall clock to stop it, got {err:?} after {:?}",
        started.elapsed()
    );
}
