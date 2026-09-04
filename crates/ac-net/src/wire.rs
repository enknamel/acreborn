//! Little-endian message body reader/writer with the protocol's string
//! and packed-integer encodings.

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("message truncated at byte {0}")]
pub struct Truncated(pub usize);

pub struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Reader { b, pos: 0 }
    }
    pub fn pos(&self) -> usize {
        self.pos
    }
    pub fn remaining(&self) -> &'a [u8] {
        &self.b[self.pos..]
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], Truncated> {
        if self.pos + n > self.b.len() {
            return Err(Truncated(self.pos));
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8, Truncated> {
        Ok(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, Truncated> {
        let s = self.bytes(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }
    pub fn u32(&mut self) -> Result<u32, Truncated> {
        let s = self.bytes(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    pub fn i32(&mut self) -> Result<i32, Truncated> {
        Ok(self.u32()? as i32)
    }
    pub fn u64(&mut self) -> Result<u64, Truncated> {
        let s = self.bytes(8)?;
        Ok(u64::from_le_bytes(s.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32, Truncated> {
        Ok(f32::from_bits(self.u32()?))
    }
    pub fn f64(&mut self) -> Result<f64, Truncated> {
        Ok(f64::from_bits(self.u64()?))
    }
    pub fn align4(&mut self) -> Result<(), Truncated> {
        let rem = self.pos % 4;
        if rem != 0 {
            self.bytes(4 - rem)?;
        }
        Ok(())
    }
    /// u16 length, bytes, padded so length+bytes is a multiple of 4.
    pub fn string16(&mut self) -> Result<String, Truncated> {
        let n = self.u16()? as usize;
        let s = self.bytes(n)?.iter().map(|&c| c as char).collect();
        let pad = (4 - (2 + n) % 4) % 4;
        self.bytes(pad)?;
        Ok(s)
    }
    /// Client "packed DWORD": u16, or u32 with the top bit of the first
    /// u16 set (`(hi | 0x8000) << 16 | lo` on the wire as two u16s).
    pub fn packed_u32(&mut self) -> Result<u32, Truncated> {
        let hi = self.u16()? as u32;
        if hi & 0x8000 != 0 {
            let lo = self.u16()? as u32;
            Ok(((hi & 0x7FFF) << 16) | lo)
        } else {
            Ok(hi)
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.u32(v as u32)
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.u32(v.to_bits())
    }
    pub fn f64(&mut self, v: f64) -> &mut Self {
        self.u64(v.to_bits())
    }
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }
    pub fn align4(&mut self) -> &mut Self {
        while self.buf.len() % 4 != 0 {
            self.buf.push(0);
        }
        self
    }
    /// u16 length + Windows-1252 bytes + pad to a multiple of 4.
    pub fn string16(&mut self, s: &str) -> &mut Self {
        let bytes: Vec<u8> = s
            .chars()
            .map(|c| if (c as u32) < 256 { c as u8 } else { b'?' })
            .collect();
        self.u16(bytes.len() as u16);
        self.buf.extend_from_slice(&bytes);
        let pad = (4 - (2 + bytes.len()) % 4) % 4;
        self.buf.extend(std::iter::repeat(0).take(pad));
        self
    }
    pub fn packed_u32(&mut self, v: u32) -> &mut Self {
        if v <= 0x7FFF {
            self.u16(v as u16)
        } else {
            self.u16(((v >> 16) | 0x8000) as u16);
            self.u16(v as u16)
        }
    }
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string16_roundtrip_and_padding() {
        let mut w = Writer::new();
        w.string16("1802");
        assert_eq!(w.buf.len(), 8, "2 + 4 + 2 pad");
        w.string16("ab");
        assert_eq!(w.buf.len(), 12);
        let mut r = Reader::new(&w.buf);
        assert_eq!(r.string16().unwrap(), "1802");
        assert_eq!(r.string16().unwrap(), "ab");
        assert!(r.remaining().is_empty());
    }

    #[test]
    fn packed_u32_roundtrip() {
        for v in [0u32, 1, 0x7FFF, 0x8000, 0x1234_5678, 0x7FFF_FFFF] {
            let mut w = Writer::new();
            w.packed_u32(v);
            assert_eq!(Reader::new(&w.buf).packed_u32().unwrap(), v, "{v:#x}");
        }
    }
}
