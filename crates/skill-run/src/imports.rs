//! The import → capability mapping, and minimal hosts for the capabilities that are granted.
//!
//! Anything not in this table is `None`, which the gate treats as a refusal. An unrecognised
//! import is not harmless-by-default: a host that shrugs at imports it does not understand is a
//! host with no capability model at all.

use std::sync::{Arc, Mutex};

use skill_format::cap_kind;
use wasmtime::{Caller, Extern, Linker};

use crate::HostState;

const WASI: &str = "wasi_snapshot_preview1";
const SKILL: &str = "skill";

/// `Some(FREE)` marks an import that needs no capability at all.
pub const FREE: u16 = 0;

const ERRNO_NOTSUP: i32 = 58;

/// Guest pointers arrive as `i32` because that is what wasm32 has, but they are addresses.
/// Reinterpreting the bits is correct; sign-extending them is not.
fn guest_ptr(v: i32) -> usize {
    v.cast_unsigned() as usize
}

/// The capability an import requires, or `None` if this host does not recognise it.
///
/// # Panics
/// Never; the match is total.
#[must_use]
#[allow(clippy::match_same_arms)]
pub fn required_capability(module: &str, name: &str) -> Option<u16> {
    match (module, name) {
        (WASI, "proc_exit") | (SKILL, "yield") => Some(FREE),
        (WASI, "fd_write" | "fd_read" | "fd_close" | "fd_fdstat_get") => Some(cap_kind::STDIO),
        (WASI, "path_open" | "fd_prestat_get" | "fd_prestat_dir_name" | "fd_readdir") => {
            Some(cap_kind::FS_READ)
        }
        (WASI, "path_create_directory" | "path_unlink_file" | "fd_allocate") => {
            Some(cap_kind::FS_WRITE)
        }
        (WASI, "environ_get" | "environ_sizes_get" | "args_get" | "args_sizes_get") => {
            Some(cap_kind::ENV)
        }
        (WASI, "clock_time_get" | "clock_res_get") => Some(cap_kind::CLOCK),
        (WASI, "random_get") => Some(cap_kind::RAND),
        (WASI, n) if n.starts_with("sock_") => Some(cap_kind::NET_HOST),
        _ => None,
    }
}

/// Registers hosts for the declared capabilities only. Anything undeclared is simply never
/// defined, so instantiation fails on the missing import rather than at a call site.
#[allow(clippy::too_many_lines)]
pub(crate) fn provide(linker: &mut Linker<HostState>, declared: &[u16]) -> wasmtime::Result<()> {
    linker.func_wrap(WASI, "proc_exit", |_: Caller<'_, HostState>, _code: i32| {})?;

    if declared.contains(&cap_kind::STDIO) {
        linker.func_wrap(
            WASI,
            "fd_write",
            |mut caller: Caller<'_, HostState>,
             fd: i32,
             iovs: i32,
             iovs_len: i32,
             nwritten: i32|
             -> i32 {
                let Some(Extern::Memory(mem)) = caller.get_export("memory") else {
                    return ERRNO_NOTSUP;
                };
                let sink: Arc<Mutex<Vec<u8>>> = Arc::clone(&caller.data().stdout);
                let mut total: u32 = 0;
                let mut staged: Vec<u8> = Vec::new();

                for i in 0..iovs_len.max(0) {
                    let base = guest_ptr(iovs) + guest_ptr(i) * 8;
                    let mut hdr = [0u8; 8];
                    if mem.read(&mut caller, base, &mut hdr).is_err() {
                        return ERRNO_NOTSUP;
                    }
                    let ptr = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
                    let len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
                    let mut buf = vec![0u8; len];
                    if mem.read(&mut caller, ptr, &mut buf).is_err() {
                        return ERRNO_NOTSUP;
                    }
                    total = total.saturating_add(u32::try_from(len).unwrap_or(0));
                    staged.extend_from_slice(&buf);
                }

                // fd 1 and 2 are captured; anything else is reported unsupported rather than
                // silently discarded, so a segment cannot believe it wrote to a file.
                if fd != 1 && fd != 2 {
                    return ERRNO_NOTSUP;
                }
                if let Ok(mut g) = sink.lock() {
                    g.extend_from_slice(&staged);
                }
                if mem
                    .write(&mut caller, guest_ptr(nwritten), &total.to_le_bytes())
                    .is_err()
                {
                    return ERRNO_NOTSUP;
                }
                0
            },
        )?;
        for name in ["fd_read", "fd_close", "fd_fdstat_get"] {
            linker.func_wrap(
                WASI,
                name,
                |_: Caller<'_, HostState>, _: i32, _: i32, _: i32, _: i32| ERRNO_NOTSUP,
            )?;
        }
    }

    // Declared but unimplemented capabilities link and then report unsupported. The segment is
    // authorized; this host simply has nothing behind the door yet.
    for (kind, names) in [
        (
            cap_kind::FS_READ,
            &[
                "path_open",
                "fd_prestat_get",
                "fd_prestat_dir_name",
                "fd_readdir",
            ][..],
        ),
        (
            cap_kind::FS_WRITE,
            &["path_create_directory", "path_unlink_file", "fd_allocate"][..],
        ),
        (
            cap_kind::ENV,
            &[
                "environ_get",
                "environ_sizes_get",
                "args_get",
                "args_sizes_get",
            ][..],
        ),
        (cap_kind::CLOCK, &["clock_time_get", "clock_res_get"][..]),
        (cap_kind::RAND, &["random_get"][..]),
    ] {
        if !declared.contains(&kind) {
            continue;
        }
        for name in names {
            let n = (*name).to_string();
            if linker
                .func_wrap(WASI, &n, |_: Caller<'_, HostState>, _: i32, _: i32| {
                    ERRNO_NOTSUP
                })
                .is_err()
            {
                linker.func_wrap(
                    WASI,
                    &n,
                    |_: Caller<'_, HostState>, _: i32, _: i32, _: i32, _: i32| ERRNO_NOTSUP,
                )?;
            }
        }
    }
    Ok(())
}
