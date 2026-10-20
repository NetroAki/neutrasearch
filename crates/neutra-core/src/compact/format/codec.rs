use std::io;

pub(crate) fn put_varint(mut n: u32, out: &mut Vec<u8>) {
    while n >= 0x80 {
        out.push((n as u8) | 0x80);
        n >>= 7;
    }
    out.push(n as u8);
}
pub(in crate::compact) fn get_varint(bytes: &[u8], p: &mut usize) -> io::Result<u32> {
    let (mut n, mut shift) = (0u32, 0);
    loop {
        let b = *bytes
            .get(*p)
            .ok_or_else(|| invalid("truncated posting varint"))?;
        *p += 1;
        if shift >= 32 {
            return Err(invalid("posting varint overflow"));
        }
        n |= ((b & 0x7f) as u32) << shift;
        if b & 0x80 == 0 {
            return Ok(n);
        }
        shift += 7;
    }
}
pub(in crate::compact) fn checked(bytes: &[u8], offset: usize, len: usize) -> io::Result<&[u8]> {
    bytes
        .get(
            offset
                ..offset
                    .checked_add(len)
                    .ok_or_else(|| invalid("index offset overflow"))?,
        )
        .ok_or_else(|| invalid("index section out of bounds"))
}
pub(in crate::compact) fn u16_at(b: &[u8], p: usize) -> io::Result<u16> {
    Ok(u16::from_le_bytes(checked(b, p, 2)?.try_into().unwrap()))
}
pub(in crate::compact) fn u32_at(b: &[u8], p: usize) -> io::Result<u32> {
    Ok(u32::from_le_bytes(checked(b, p, 4)?.try_into().unwrap()))
}
pub(in crate::compact) fn u64_at(b: &[u8], p: usize) -> io::Result<u64> {
    Ok(u64::from_le_bytes(checked(b, p, 8)?.try_into().unwrap()))
}
pub(crate) fn binerr(e: impl std::fmt::Display) -> io::Error {
    invalid(format!("index codec: {e}"))
}
pub(crate) fn invalid(e: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.into())
}
