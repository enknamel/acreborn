//! The client's 32-bit packet checksum ("Hash32"): `len << 16` plus the sum
//! of little-endian dwords, with a trailing partial dword added big-endian
//! style (first leftover byte in the top byte).

pub fn hash32(data: &[u8]) -> u32 {
    let len = data.len();
    let mut sum = (len as u32) << 16;
    let whole = len / 4 * 4;
    for chunk in data[..whole].chunks_exact(4) {
        sum = sum.wrapping_add(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    let mut shift = 3u32;
    for &b in &data[whole..] {
        sum = sum.wrapping_add((b as u32) << (8 * shift));
        shift = shift.wrapping_sub(1);
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_reference() {
        // Empty: only the length term.
        assert_eq!(hash32(&[]), 0);
        assert_eq!(hash32(&[1, 0, 0, 0]), (4 << 16) + 1);
        // Trailing bytes go into the high bytes first.
        assert_eq!(hash32(&[0xAA]), (1 << 16) + 0xAA00_0000);
        assert_eq!(hash32(&[0xAA, 0xBB]), (2 << 16) + 0xAABB_0000);
        assert_eq!(
            hash32(&[1, 0, 0, 0, 0xAA, 0xBB, 0xCC]),
            (7 << 16) + 1 + 0xAABB_CC00
        );
    }
}
