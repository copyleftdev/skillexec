use ed25519_dalek::{Signature, VerifyingKey};

use crate::error::{Error, Result};
use crate::header::HEADER_LEN;
use crate::raw::{all_zero, bytes_at, hash_at, u16_at};

pub const SIG_LEN: usize = 112;
pub const SIGBLOCK_OFF: usize = HEADER_LEN;
pub const ALG_ED25519: u16 = 1;

pub mod sig_flags {
    pub const PUBLISHER: u16 = 1 << 0;
    pub const ENDORSEMENT: u16 = 1 << 1;
    pub const TRANSPARENCY: u16 = 1 << 2;
}

#[derive(Debug, Clone, Copy)]
pub struct SigEntry {
    pub alg: u16,
    pub flags: u16,
    pub keyid: [u8; 32],
    pub sig: [u8; 64],
}

/// What a caller is willing to accept. Whether an unsigned file is usable is policy, not
/// format: `sig_count = 0` is well-formed and `require_signature` decides if it is allowed.
#[derive(Debug, Clone, Default)]
pub struct TrustPolicy {
    pub require_signature: bool,
    pub keys: Vec<VerifyingKey>,
}

impl TrustPolicy {
    #[must_use]
    pub fn permissive() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn require_any_of(keys: Vec<VerifyingKey>) -> Self {
        Self {
            require_signature: true,
            keys,
        }
    }

    fn key_for(&self, keyid: &[u8; 32]) -> Option<&VerifyingKey> {
        self.keys
            .iter()
            .find(|k| blake3::hash(k.as_bytes()).as_bytes() == keyid)
    }
}

/// `SPEC.md` §5 step 3. Runs before any variable-length parsing.
/// # Errors
/// Rejects a truncated signature block, a duplicated key id, a signature that fails
/// verification, and — when the policy demands one — a file carrying no trusted signature.
pub fn verify(bytes: &[u8], sig_count: u16, policy: &TrustPolicy) -> Result<Vec<SigEntry>> {
    let mut entries = Vec::with_capacity(sig_count as usize);
    let digest = crate::header::Header::signing_digest(bytes);

    for i in 0..sig_count {
        let off = SIGBLOCK_OFF + SIG_LEN * i as usize;
        all_zero(bytes, off + 0x04, 4, "sig reserved 0x04")?;
        all_zero(bytes, off + 0x68, 8, "sig reserved 0x68")?;
        let sig: [u8; 64] =
            bytes_at(bytes, off + 0x28, 64)?
                .try_into()
                .map_err(|_| Error::Truncated {
                    off: off + 0x28,
                    need: 64,
                })?;
        entries.push(SigEntry {
            alg: u16_at(bytes, off)?,
            flags: u16_at(bytes, off + 0x02)?,
            keyid: hash_at(bytes, off + 0x08)?,
            sig,
        });
    }

    for (i, e) in entries.iter().enumerate() {
        if entries[..i].iter().any(|p| p.keyid == e.keyid) {
            return Err(Error::DuplicateKeyId);
        }
    }

    let mut trusted = 0usize;
    for (i, e) in entries.iter().enumerate() {
        let Some(key) = policy.key_for(&e.keyid) else {
            continue;
        };
        if e.alg != ALG_ED25519 {
            continue;
        }
        key.verify_strict(&digest, &Signature::from_bytes(&e.sig))
            .map_err(|_| Error::BadSignature(u16::try_from(i).unwrap_or(u16::MAX)))?;
        trusted += 1;
    }

    if policy.require_signature && trusted == 0 {
        return Err(if entries.is_empty() {
            Error::Unsigned
        } else {
            Error::UntrustedKey(Box::new(entries[0].keyid))
        });
    }
    Ok(entries)
}
