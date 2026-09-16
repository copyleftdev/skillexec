use crate::error::{Error, Result};

fn slice(b: &[u8], off: usize, len: usize) -> Result<&[u8]> {
    let end = off
        .checked_add(len)
        .ok_or(Error::OffsetOverflow { what: "read" })?;
    b.get(off..end).ok_or(Error::Truncated { off, need: len })
}

pub(crate) fn u8_at(b: &[u8], off: usize) -> Result<u8> {
    Ok(slice(b, off, 1)?[0])
}

pub(crate) fn u16_at(b: &[u8], off: usize) -> Result<u16> {
    let a: [u8; 2] = slice(b, off, 2)?
        .try_into()
        .map_err(|_| Error::Truncated { off, need: 2 })?;
    Ok(u16::from_le_bytes(a))
}

pub(crate) fn u32_at(b: &[u8], off: usize) -> Result<u32> {
    let a: [u8; 4] = slice(b, off, 4)?
        .try_into()
        .map_err(|_| Error::Truncated { off, need: 4 })?;
    Ok(u32::from_le_bytes(a))
}

pub(crate) fn hash_at(b: &[u8], off: usize) -> Result<[u8; 32]> {
    slice(b, off, 32)?
        .try_into()
        .map_err(|_| Error::Truncated { off, need: 32 })
}

pub(crate) fn bytes_at(b: &[u8], off: usize, len: usize) -> Result<&[u8]> {
    slice(b, off, len)
}

pub(crate) fn all_zero(b: &[u8], off: usize, len: usize, what: &'static str) -> Result<()> {
    if slice(b, off, len)?.iter().any(|&x| x != 0) {
        return Err(Error::ReservedNonZero { what });
    }
    Ok(())
}
