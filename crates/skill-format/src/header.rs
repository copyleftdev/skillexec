use crate::error::{Error, Result};
use crate::raw::{hash_at, u16_at, u32_at};

pub const MAGIC: [u8; 8] = [0x8F, b'S', b'K', b'L', 0x0D, 0x0A, 0x1A, 0x0A];
pub const HEADER_LEN: usize = 64;
/// Bumped from 1 for the multi-entry routing block. A v1 file's routing block began with
/// `name_len`, a v2 file's with `count`, and a one-character name is indistinguishable from a
/// one-skill count — so the two layouts must not be allowed to meet. Readers reject an unknown
/// major, which is exactly the case this is.
pub const VERSION_MAJOR: u16 = 2;
pub const VERSION_MINOR: u16 = 0;

/// Every feature bit this reader implements. A file asserting anything outside this mask is
/// rejected rather than partially honoured; skipping what you cannot verify is how signed
/// containers get stripped.
pub const SUPPORTED_FEATURES: u32 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub ver_major: u16,
    pub ver_minor: u16,
    pub feature_flags: u32,
    pub file_len: u32,
    pub manifest_off: u32,
    pub manifest_len: u32,
    pub sig_count: u16,
    pub manifest_root: [u8; 32],
}

impl Header {
    /// `SPEC.md` §5 steps 1–2. Touches exactly the first 64 bytes.
    /// # Errors
    /// Rejects a file whose magic, major version, feature mask, reserved field or
    /// declared length disagrees with its bytes (`SPEC.md` §5 steps 1–2).
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let head = bytes.get(..HEADER_LEN).ok_or(Error::Truncated {
            off: 0,
            need: HEADER_LEN,
        })?;
        if head[..8] != MAGIC {
            return Err(Error::BadMagic);
        }
        let ver_major = u16_at(head, 0x08)?;
        if ver_major != VERSION_MAJOR {
            return Err(Error::UnsupportedMajor(ver_major));
        }
        let feature_flags = u32_at(head, 0x0C)?;
        if feature_flags & !SUPPORTED_FEATURES != 0 {
            return Err(Error::UnknownFeatureFlags(
                feature_flags & !SUPPORTED_FEATURES,
            ));
        }
        let reserved = u16_at(head, 0x1E)?;
        if reserved != 0 {
            return Err(Error::ReservedNonZero {
                what: "header 0x1E",
            });
        }
        let file_len = u32_at(head, 0x10)?;
        if file_len as usize != bytes.len() {
            return Err(Error::LengthMismatch {
                declared: file_len,
                actual: bytes.len(),
            });
        }
        Ok(Self {
            ver_major,
            ver_minor: u16_at(head, 0x0A)?,
            feature_flags,
            file_len,
            manifest_off: u32_at(head, 0x14)?,
            manifest_len: u32_at(head, 0x18)?,
            sig_count: u16_at(head, 0x1C)?,
            manifest_root: hash_at(head, 0x20)?,
        })
    }

    /// Where the routing block starts.
    ///
    /// Derived from the header alone, never stored: it sits immediately after the signature
    /// block. That is the whole point -- a router that has to read the manifest to find the
    /// name has not saved anything, and an offset the manifest owns would put the name behind
    /// the manifest.
    #[must_use]
    pub fn routing_off(&self) -> usize {
        HEADER_LEN + crate::sig::SIG_LEN * self.sig_count as usize
    }

    /// The digest every signature is computed over. Signing 64 bytes signs the file, because
    /// those bytes name the manifest root and the manifest names everything else.
    #[must_use]
    pub fn signing_digest(bytes: &[u8]) -> [u8; 32] {
        *blake3::hash(&bytes[..HEADER_LEN]).as_bytes()
    }
}
