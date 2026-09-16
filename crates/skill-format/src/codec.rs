//! zstd, with every attacker-controlled knob bounded before it reaches the decoder.

use std::sync::Arc;

use zstd::dict::{DecoderDictionary, EncoderDictionary};

/// Compression level. 19 is the last level before zstd's "ultra" range, which costs a great
/// deal of time for very little ratio on inputs this size.
pub const LEVEL: i32 = 19;

/// A shared dictionary, digested once.
///
/// zstd has to preprocess a dictionary before it can use it, and that cost is proportional to
/// the dictionary rather than to the input. Building one per call turned a 110 KB dictionary
/// into the dominant cost of compiling a corpus: the work is per *file* in principle and was
/// being paid per *region*.
/// A zstd operation failed. The cause is never actionable at the call site -- a malformed
/// frame and a mismatched dictionary are both just "these bytes are not what was declared".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecError;

#[derive(Clone)]
pub struct Dictionary {
    raw: Arc<Vec<u8>>,
    enc: Arc<EncoderDictionary<'static>>,
    dec: Arc<DecoderDictionary<'static>>,
}

impl core::fmt::Debug for Dictionary {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The digested forms are opaque handles and the raw bytes are not worth printing.
        f.debug_struct("Dictionary")
            .field("bytes", &self.raw.len())
            .finish_non_exhaustive()
    }
}

impl Dictionary {
    #[must_use]
    pub fn new(bytes: &[u8]) -> Self {
        Self {
            raw: Arc::new(bytes.to_vec()),
            enc: Arc::new(EncoderDictionary::copy(bytes, LEVEL)),
            dec: Arc::new(DecoderDictionary::copy(bytes)),
        }
    }

    #[must_use]
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        *blake3::hash(&self.raw).as_bytes()
    }
}

/// Trains a shared dictionary from samples.
///
/// Samples should be the bytes that will actually be compressed -- the payload regions -- not
/// the source they were compiled from. A dictionary trained on Markdown learns Markdown's
/// framing, which the container has already stripped out into tables.
///
/// # Errors
/// Returns [`CodecError`] if zstd cannot train on these samples, which usually means too few
/// of them or too little total data.
pub fn train(samples: &[Vec<u8>], max_bytes: usize) -> Result<Vec<u8>, CodecError> {
    zstd::dict::from_samples(samples, max_bytes).map_err(|_| CodecError)
}

/// # Errors
/// Returns [`CodecError`] if zstd rejects the input or the dictionary.
pub fn compress(src: &[u8], dict: Option<&Dictionary>) -> Result<Vec<u8>, CodecError> {
    match dict {
        None => zstd::bulk::compress(src, LEVEL).map_err(|_| CodecError),
        Some(d) => zstd::bulk::Compressor::with_prepared_dictionary(&d.enc)
            .and_then(|mut c| c.compress(src))
            .map_err(|_| CodecError),
    }
}

/// Decompresses into a buffer capped at `capacity`, which the caller has already bounded. zstd
/// stops at the cap rather than growing, so a declared length cannot become an allocation.
///
/// # Errors
/// Returns [`CodecError`] if the frame is malformed or expands past `capacity`.
pub fn decompress(
    src: &[u8],
    capacity: usize,
    dict: Option<&Dictionary>,
) -> Result<Vec<u8>, CodecError> {
    match dict {
        None => zstd::bulk::decompress(src, capacity).map_err(|_| CodecError),
        Some(d) => zstd::bulk::Decompressor::with_prepared_dictionary(&d.dec)
            .and_then(|mut c| c.decompress(src, capacity))
            .map_err(|_| CodecError),
    }
}
