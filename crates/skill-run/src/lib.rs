//! Runs a `.skill` wasm segment under exactly the capabilities it declared, and no others.
//!
//! The enforcement point is **linking, not calling**. A wasm module can only reach the host
//! through its imports, so refusing to satisfy an import the segment did not declare makes the
//! capability unreachable rather than merely guarded — there is no call site left to check.
//!
//! What this is not: a WASI host. Granted capabilities are backed by minimal implementations
//! (`stdio` captures output; the rest report unsupported). Wiring full WASI, and supporting
//! `wasm32-wasip2` components rather than core modules, is future work. The gate is real; the
//! world behind it is a stub.

use std::sync::{Arc, Mutex};

use skill_format::{Abi, Kind, Skill, TrustClass, cap_kind};
use wasmtime::{Config, Engine, Linker, Module, ResourceLimiter, Store, StoreLimits};

mod imports;
pub use imports::required_capability;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    NoSuchSegment(u32),
    /// The stored bytes do not match the signed BLAKE3 root. `SPEC.md` §5 step 7.
    IntegrityFailed(u32),
    /// The format can label a shell script; it cannot contain one. Refusing is the honest
    /// answer, and a policy has to opt in explicitly to get anything else.
    NotPortable {
        abi: &'static str,
        trust: &'static str,
    },
    UnsupportedAbi(&'static str),
    /// The module imports something the segment never declared. Refused before instantiation.
    UndeclaredImport {
        module: String,
        name: String,
        needs: &'static str,
    },
    /// The segment asked for more than the policy's ceiling.
    LimitExceedsPolicy {
        what: &'static str,
        asked: u32,
        ceiling: u32,
    },
    Compile(String),
    Instantiate(String),
    NoSuchEntry(String),
    /// The segment burned its declared `cpu_ms`. Distinguished from a trap because a budget
    /// that was too small is an operator's problem, and a trap is the author's.
    OutOfFuel,
    /// The segment outran its declared `wall_ms`.
    Interrupted,
    Trap(String),
}

impl core::fmt::Display for RunError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for RunError {}

/// Ceilings the host imposes regardless of what a segment requests. A segment asking for more
/// is refused rather than silently clamped: a skill that needs 4 GB should fail loudly on a
/// host that will not give it, not run in a way its author never tested.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub allow_host_trusted: bool,
    pub max_mem_kib: u32,
    pub max_cpu_ms: u32,
    pub max_wall_ms: u32,
    /// Fuel per millisecond of declared CPU budget. Fuel counts instructions, not time, so this
    /// is a calibration constant and not a promise about wall clock.
    pub fuel_per_ms: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            allow_host_trusted: false,
            max_mem_kib: 64 * 1024,
            max_cpu_ms: 5_000,
            max_wall_ms: 10_000,
            fuel_per_ms: 1_000_000,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub stdout: Vec<u8>,
    pub fuel_used: u64,
    pub granted: Vec<&'static str>,
}

struct HostState {
    limits: StoreLimits,
    stdout: Arc<Mutex<Vec<u8>>>,
}

impl ResourceLimiter for HostState {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        self.limits.memory_growing(current, desired, maximum)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        self.limits.table_growing(current, desired, maximum)
    }
}

fn abi_name(abi: Abi) -> &'static str {
    match abi {
        Abi::Wasm32Wasip2 => "wasm32-wasip2",
        Abi::Sh => "sh",
        Abi::Python3 => "python3",
        Abi::Node => "node",
        Abi::Native => "native",
        Abi::Wasm32Core => "wasm32-core",
    }
}

fn classify_trap(e: &wasmtime::Error) -> RunError {
    match e.downcast_ref::<wasmtime::Trap>() {
        Some(wasmtime::Trap::OutOfFuel) => RunError::OutOfFuel,
        Some(wasmtime::Trap::Interrupt) => RunError::Interrupted,
        _ => RunError::Trap(e.to_string()),
    }
}

/// Maps segment-record index to the node that carries its bytes. The two are parallel-indexed
/// in pre-order (`SPEC.md` §10.4), so this is a count, not a lookup table.
fn segment_node(skill: &Skill<'_>, seg_idx: u32) -> Option<u32> {
    let mut seen = 0u32;
    for (i, n) in skill.nodes.iter().enumerate() {
        if n.kind == Kind::Segment {
            if seen == seg_idx {
                return u32::try_from(i).ok();
            }
            seen += 1;
        }
    }
    None
}

/// Runs a segment and returns what it produced.
///
/// # Errors
/// Refuses a segment whose bytes fail integrity, whose trust class the policy does not admit,
/// whose limits exceed the policy ceiling, or which imports a capability it never declared.
pub fn run(
    skill: &Skill<'_>,
    seg_idx: u32,
    entry: &str,
    policy: &Policy,
) -> Result<Outcome, RunError> {
    let rec = skill
        .manifest
        .segment(seg_idx)
        .map_err(|_| RunError::NoSuchSegment(seg_idx))?;
    let node = segment_node(skill, seg_idx).ok_or(RunError::NoSuchSegment(seg_idx))?;

    let code = skill
        .verified_payload(node)
        .map_err(|_| RunError::IntegrityFailed(seg_idx))?;

    if rec.trust_class != TrustClass::Portable && !policy.allow_host_trusted {
        return Err(RunError::NotPortable {
            abi: abi_name(rec.abi),
            trust: "HostTrusted",
        });
    }
    if !matches!(rec.abi, Abi::Wasm32Core) {
        return Err(RunError::UnsupportedAbi(abi_name(rec.abi)));
    }

    let declared: Vec<u16> = (0..u32::from(rec.cap_cnt))
        .filter_map(|k| skill.manifest.cap(rec.cap_off + k).ok())
        .map(|c| c.0)
        .collect();

    for (what, asked, ceiling) in [
        ("mem_kib", rec.mem_kib, policy.max_mem_kib),
        ("cpu_ms", rec.cpu_ms, policy.max_cpu_ms),
        ("wall_ms", rec.wall_ms, policy.max_wall_ms),
    ] {
        if asked > ceiling {
            return Err(RunError::LimitExceedsPolicy {
                what,
                asked,
                ceiling,
            });
        }
    }

    let mut config = Config::new();
    config.consume_fuel(true);
    config.epoch_interruption(true);
    let engine = Engine::new(&config).map_err(|e| RunError::Compile(e.to_string()))?;
    let module = Module::new(&engine, code).map_err(|e| RunError::Compile(e.to_string()))?;

    // The gate. Every import is checked against the declared set before anything is
    // instantiated, so an undeclared capability has no call site to reach.
    for imp in module.imports() {
        let needs = required_capability(imp.module(), imp.name());
        match needs {
            Some(kind) if declared.contains(&kind) => {}
            _ => {
                return Err(RunError::UndeclaredImport {
                    module: imp.module().to_string(),
                    name: imp.name().to_string(),
                    needs: needs.map_or("unknown", cap_kind::name),
                });
            }
        }
    }

    let mem_bytes = (rec.mem_kib.max(1) as usize).saturating_mul(1024);
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let state = HostState {
        limits: wasmtime::StoreLimitsBuilder::new()
            .memory_size(mem_bytes)
            .build(),
        stdout: Arc::clone(&stdout),
    };

    let mut store = Store::new(&engine, state);
    store.limiter(|s| &mut s.limits);
    let fuel = u64::from(rec.cpu_ms.max(1)).saturating_mul(policy.fuel_per_ms);
    store
        .set_fuel(fuel)
        .map_err(|e| RunError::Instantiate(e.to_string()))?;
    store.set_epoch_deadline(1);

    let mut linker: Linker<HostState> = Linker::new(&engine);
    imports::provide(&mut linker, &declared).map_err(|e| RunError::Instantiate(e.to_string()))?;

    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(|e| RunError::Instantiate(e.to_string()))?;
    let func = instance
        .get_typed_func::<(), ()>(&mut store, entry)
        .map_err(|_| RunError::NoSuchEntry(entry.to_string()))?;

    let call = func.call(&mut store, ());
    let used = fuel.saturating_sub(store.get_fuel().unwrap_or(0));
    call.map_err(|e| classify_trap(&e))?;

    let out = stdout
        .lock()
        .map_or_else(|e| e.into_inner().clone(), |g| g.clone());
    Ok(Outcome {
        stdout: out,
        fuel_used: used,
        granted: declared.iter().map(|k| cap_kind::name(*k)).collect(),
    })
}
