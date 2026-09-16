//! The routing block: every skill's name and description, contiguous, immediately after the
//! signature block.
//!
//! These fields decide whether a skill is loaded at all, and they were once *indices* into a
//! sorted string heap. Sorting scatters them, so reading them meant reading the manifest —
//! measured at 1,524 bytes per skill to retrieve about 200. They now live in one run of bytes at
//! an offset derived from the header, and nowhere else: keeping them out of the heap is what
//! preserves exactly one encoding per logical value.
//!
//! The block holds `count` entries, so one file can carry many skills. A bundle's node 0 is a
//! synthetic root and each skill is one of its children; `count = 1` with `root = 0` is an
//! ordinary single-skill file.
//!
//! ```text
//! 0x00  count u16
//! 0x02  reserved u16 (zero)
//! 0x04  count × { root u32, name_len u16, desc_len u16 }
//!       arena: name bytes then description bytes, per entry, in entry order
//!       zero padding to a multiple of 8
//! ```

use crate::error::{Error, Result};
use crate::raw::{all_zero, bytes_at, u16_at, u32_at};

/// `count`, `reserved`.
pub const HEADER_LEN: usize = 4;
/// `root`, `name_len`, `desc_len`.
pub const ENTRY_LEN: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    /// Index of the node this skill is rooted at. `0` for a single-skill file.
    pub root: u32,
    pub name: &'a str,
    pub description: &'a str,
}

#[derive(Debug, Clone)]
pub struct Routing<'a> {
    pub entries: Vec<Entry<'a>>,
    /// Stored length including the header, the entry table and padding.
    pub stored_len: usize,
}

/// Reads the block at `off`.
///
/// # Errors
/// Rejects a truncated block, a non-zero reserved field, or non-UTF-8 content.
pub fn read(bytes: &[u8], off: usize) -> Result<Routing<'_>> {
    let count = u16_at(bytes, off)? as usize;
    all_zero(bytes, off + 2, 2, "routing reserved 0x02")?;

    let arena = off + HEADER_LEN + ENTRY_LEN * count;
    let mut entries = Vec::with_capacity(count);
    let mut at = arena;
    for i in 0..count {
        let e = off + HEADER_LEN + ENTRY_LEN * i;
        let root = u32_at(bytes, e)?;
        let name_len = u16_at(bytes, e + 4)? as usize;
        let desc_len = u16_at(bytes, e + 6)? as usize;
        let name = bytes_at(bytes, at, name_len)?;
        let description = bytes_at(bytes, at + name_len, desc_len)?;
        at += name_len + desc_len;
        entries.push(Entry {
            root,
            name: core::str::from_utf8(name).map_err(|_| Error::RoutingNotUtf8)?,
            description: core::str::from_utf8(description).map_err(|_| Error::RoutingNotUtf8)?,
        });
    }
    Ok(Routing {
        entries,
        stored_len: (at - off).next_multiple_of(8),
    })
}

/// The exact bytes a writer emits for a set of skills.
///
/// # Errors
/// Rejects more than `u16::MAX` skills, or a name or description longer than `u16::MAX`.
pub fn encode(entries: &[(u32, String, String)]) -> Result<Vec<u8>> {
    let count = u16::try_from(entries.len()).map_err(|_| Error::RoutingTooLong)?;
    let mut out = Vec::with_capacity(HEADER_LEN + ENTRY_LEN * entries.len() + 256);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    for (root, name, desc) in entries {
        let n = u16::try_from(name.len()).map_err(|_| Error::RoutingTooLong)?;
        let d = u16::try_from(desc.len()).map_err(|_| Error::RoutingTooLong)?;
        out.extend_from_slice(&root.to_le_bytes());
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&d.to_le_bytes());
    }
    for (_, name, desc) in entries {
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(desc.as_bytes());
    }
    while !out.len().is_multiple_of(8) {
        out.push(0);
    }
    Ok(out)
}
