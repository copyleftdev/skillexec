//! The routing block: a skill's name and description, contiguous, immediately after the
//! signature block.
//!
//! These two fields decide whether a skill is loaded at all, and they were previously *indices*
//! into a sorted string heap. Sorting scatters them, so reading them meant touching the whole
//! manifest -- 2,330 bytes per skill measured across the corpus, to read about 200. They now
//! live in one run of bytes at an offset derived from the header, and nowhere else: keeping
//! them out of the heap is what preserves exactly one encoding per logical value.

use crate::error::{Error, Result};
use crate::raw::{bytes_at, u16_at};

/// `name_len u16`, `desc_len u16`, then the bytes, padded to 8.
pub const HEADER_LEN: usize = 4;

#[derive(Debug, Clone, Copy)]
pub struct Routing<'a> {
    pub name: &'a str,
    pub description: &'a str,
    /// Stored length including the 4-byte prefix and padding.
    pub stored_len: usize,
}

/// Reads the block at `off`.
///
/// # Errors
/// Rejects a truncated block or non-UTF-8 content.
pub fn read(bytes: &[u8], off: usize) -> Result<Routing<'_>> {
    let name_len = u16_at(bytes, off)? as usize;
    let desc_len = u16_at(bytes, off + 2)? as usize;
    let name = bytes_at(bytes, off + HEADER_LEN, name_len)?;
    let description = bytes_at(bytes, off + HEADER_LEN + name_len, desc_len)?;
    Ok(Routing {
        name: core::str::from_utf8(name).map_err(|_| Error::RoutingNotUtf8)?,
        description: core::str::from_utf8(description).map_err(|_| Error::RoutingNotUtf8)?,
        stored_len: (HEADER_LEN + name_len + desc_len).next_multiple_of(8),
    })
}

/// The exact bytes a writer emits for `name` and `description`.
///
/// # Errors
/// Rejects a name or description longer than `u16::MAX`.
pub fn encode(name: &str, description: &str) -> Result<Vec<u8>> {
    let n = u16::try_from(name.len()).map_err(|_| Error::RoutingTooLong)?;
    let d = u16::try_from(description.len()).map_err(|_| Error::RoutingTooLong)?;
    let mut out = Vec::with_capacity(HEADER_LEN + name.len() + description.len() + 7);
    out.extend_from_slice(&n.to_le_bytes());
    out.extend_from_slice(&d.to_le_bytes());
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(description.as_bytes());
    while !out.len().is_multiple_of(8) {
        out.push(0);
    }
    Ok(out)
}
