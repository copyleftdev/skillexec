//! The embedding block: one vector per skill, immediately after the routing block.
//!
//! Embeddings are routing data, so they live where routing data lives — at an offset derived
//! from the header, ahead of the manifest. Putting them in a manifest section would have made
//! semantic search parse the manifest, which is the exact cost `SPEC.md` §3.1 exists to avoid.
//!
//! The block is self-describing through its magic, so a reader can tell whether a file carries
//! embeddings without consulting anything else, and a file without them is simply a file whose
//! next bytes are the manifest.
//!
//! ```text
//! 0x00  magic[4] = "SEMB"
//! 0x04  dims u16
//! 0x06  kind u8   (0 = f32)
//! 0x07  reserved u8 (zero)
//! 0x08  count × dims × 4 bytes, little-endian f32, in routing-entry order
//!       zero padding to a multiple of 8
//! ```
//!
//! Vectors are stored as written and are **not** normalised by this layer; a reader that wants
//! cosine similarity normalises at comparison time. Storing normalised vectors would silently
//! discard magnitude that a future scorer might want.

use crate::error::{Error, Result};
use crate::raw::{all_zero, bytes_at, u8_at, u16_at};

pub const MAGIC: [u8; 4] = *b"SEMB";
pub const HEADER_LEN: usize = 8;
/// f32 little-endian.
pub const KIND_F32: u8 = 0;

#[derive(Debug, Clone)]
pub struct Embeddings {
    pub dims: usize,
    pub count: usize,
    pub stored_len: usize,
    vectors: Vec<f32>,
}

impl Embeddings {
    /// The vector for routing entry `i`.
    #[must_use]
    pub fn get(&self, i: usize) -> Option<&[f32]> {
        let start = i.checked_mul(self.dims)?;
        self.vectors.get(start..start + self.dims)
    }
}

/// Reads an embedding block at `off`, if one is there.
///
/// Returns `Ok(None)` when the magic is absent, which is how a file without embeddings reads.
///
/// # Errors
/// Rejects a block whose declared shape does not fit the bytes, or an unknown storage kind.
pub fn read(bytes: &[u8], off: usize, count: usize) -> Result<Option<Embeddings>> {
    let Ok(magic) = bytes_at(bytes, off, 4) else {
        return Ok(None);
    };
    if magic != MAGIC {
        return Ok(None);
    }
    let dims = u16_at(bytes, off + 4)? as usize;
    let kind = u8_at(bytes, off + 6)?;
    all_zero(bytes, off + 7, 1, "embedding reserved 0x07")?;
    if kind != KIND_F32 {
        return Err(Error::EmbeddingKindUnknown(kind));
    }
    if dims == 0 {
        return Err(Error::EmbeddingShape);
    }

    let n = count.checked_mul(dims).ok_or(Error::EmbeddingShape)?;
    let raw = bytes_at(
        bytes,
        off + HEADER_LEN,
        n.checked_mul(4).ok_or(Error::EmbeddingShape)?,
    )?;
    let mut vectors = Vec::with_capacity(n);
    for c in raw.chunks_exact(4) {
        vectors.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
    }
    Ok(Some(Embeddings {
        dims,
        count,
        stored_len: (HEADER_LEN + n * 4).next_multiple_of(8),
        vectors,
    }))
}

/// The exact bytes a writer emits for a set of vectors.
///
/// # Errors
/// Rejects an empty set, ragged vectors, or more dimensions than `u16::MAX`.
pub fn encode(vectors: &[Vec<f32>]) -> Result<Vec<u8>> {
    let dims = vectors.first().map_or(0, Vec::len);
    if dims == 0 || vectors.iter().any(|v| v.len() != dims) {
        return Err(Error::EmbeddingShape);
    }
    let d = u16::try_from(dims).map_err(|_| Error::EmbeddingShape)?;

    let mut out = Vec::with_capacity(HEADER_LEN + vectors.len() * dims * 4 + 8);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&d.to_le_bytes());
    out.push(KIND_F32);
    out.push(0);
    for v in vectors {
        for x in v {
            out.extend_from_slice(&x.to_le_bytes());
        }
    }
    while !out.len().is_multiple_of(8) {
        out.push(0);
    }
    Ok(out)
}

/// Cosine similarity, computed rather than assumed: vectors are stored unnormalised.
#[must_use]
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 { 0.0 } else { dot / denom }
}
