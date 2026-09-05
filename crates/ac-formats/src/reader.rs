//! Little-endian cursor over a decoded DAT file with the client's primitive
//! encodings: packed (compressed) u32 counts, DWORD alignment, length-
//! prefixed strings.

use glam::{Quat, Vec3};

use crate::{Error, Result};

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Error unless every byte was consumed. Decoders call this at the end so
    /// a layout mistake surfaces as "trailing bytes" instead of silently
    /// misreading the tail.
    pub fn finish(&self) -> Result<()> {
        if self.is_empty() {
            Ok(())
        } else {
            Err(Error::Trailing {
                at: self.pos,
                len: self.buf.len(),
            })
        }
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(Error::Eof {
            at: self.pos,
            want: n,
        })?;
        if end > self.buf.len() {
            return Err(Error::Eof {
                at: self.pos,
                want: n,
            });
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }

    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.u64()?))
    }

    pub fn vec3(&mut self) -> Result<Vec3> {
        Ok(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }

    /// Quaternion stored as w, x, y, z.
    pub fn quat_wxyz(&mut self) -> Result<Quat> {
        let w = self.f32()?;
        let x = self.f32()?;
        let y = self.f32()?;
        let z = self.f32()?;
        Ok(Quat::from_xyzw(x, y, z, w))
    }

    /// Client "packed DWORD": 1 byte if < 0x80, 2 bytes if the top bit is
    /// set and the next is clear, else 4 bytes.
    pub fn packed_u32(&mut self) -> Result<u32> {
        let b0 = self.u8()? as u32;
        if b0 & 0x80 == 0 {
            return Ok(b0);
        }
        let b1 = self.u8()? as u32;
        if b0 & 0x40 == 0 {
            return Ok(((b0 & 0x7F) << 8) | b1);
        }
        let lo = self.u16()? as u32;
        Ok(((((b0 & 0x3F) << 8) | b1) << 16) | lo)
    }

    /// Data id stored relative to a known type: 2 bytes unless the top bit
    /// is set, in which case 4 bytes with 30 significant bits.
    pub fn data_id_of_type(&mut self, known_type: u32) -> Result<u32> {
        let v = self.u16()? as u32;
        if v & 0x8000 != 0 {
            let lo = self.u16()? as u32;
            Ok(known_type + (((v & 0x3FFF) << 16) | lo))
        } else {
            Ok(known_type + v)
        }
    }

    pub fn align4(&mut self) -> Result<()> {
        let rem = self.pos % 4;
        if rem != 0 {
            self.skip(4 - rem)?;
        }
        Ok(())
    }

    /// u16 length prefix, raw bytes (Windows-1252 in practice), no padding.
    pub fn pstring16(&mut self) -> Result<String> {
        let n = self.u16()? as usize;
        Ok(latin1(self.bytes(n)?))
    }

    /// u8 length prefix.
    pub fn pstring8(&mut self) -> Result<String> {
        let n = self.u8()? as usize;
        Ok(latin1(self.bytes(n)?))
    }

    /// u16 length prefix, then bytes with nibbles swapped (the client's
    /// light "obfuscation" of strings in some tables).
    pub fn obfuscated_string(&mut self) -> Result<String> {
        let n = self.u16()? as usize;
        let b: Vec<u8> = self.bytes(n)?.iter().map(|&c| c.rotate_left(4)).collect();
        Ok(decode_windows_1252(&b))
    }

    /// .NET `BinaryReader.ReadString`: 7-bit varint byte length, then UTF-8.
    /// Used by the character-generation tables.
    pub fn dotnet_string(&mut self) -> Result<String> {
        let mut len = 0usize;
        let mut shift = 0;
        loop {
            let b = self.u8()?;
            len |= ((b & 0x7F) as usize) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 28 {
                return Err(Error::Invalid {
                    what: "string length",
                    detail: "varint too long".into(),
                });
            }
        }
        Ok(decode_windows_1252(self.bytes(len)?))
    }

    /// Packed length prefix, then UTF-16LE code units.
    pub fn unicode_string(&mut self) -> Result<String> {
        let n = self.packed_u32()? as usize;
        let mut units = Vec::with_capacity(n);
        for _ in 0..n {
            units.push(self.u16()?);
        }
        Ok(String::from_utf16_lossy(&units))
    }

    /// `u32 count` followed by `count` items.
    pub fn list<T>(&mut self, mut item: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        let n = self.u32()? as usize;
        self.fixed(n, &mut item)
    }

    /// Packed count followed by items (the client's "smart array").
    pub fn packed_list<T>(
        &mut self,
        mut item: impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        let n = self.packed_u32()? as usize;
        self.fixed(n, &mut item)
    }

    pub fn fixed<T>(
        &mut self,
        n: usize,
        item: &mut impl FnMut(&mut Self) -> Result<T>,
    ) -> Result<Vec<T>> {
        // Guard against absurd counts from a misparse before allocating.
        if n > self.remaining() {
            return Err(Error::Eof {
                at: self.pos,
                want: n,
            });
        }
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(item(self)?);
        }
        Ok(v)
    }

    /// Packed count, then `(key, value)` pairs.
    pub fn packed_map<K, V>(
        &mut self,
        mut key: impl FnMut(&mut Self) -> Result<K>,
        mut val: impl FnMut(&mut Self) -> Result<V>,
    ) -> Result<Vec<(K, V)>> {
        let n = self.packed_u32()? as usize;
        if n > self.remaining() {
            return Err(Error::Eof {
                at: self.pos,
                want: n,
            });
        }
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let k = key(self)?;
            v.push((k, val(self)?));
        }
        Ok(v)
    }

    /// `u32 count`, then `(key, value)` pairs.
    pub fn map<K, V>(
        &mut self,
        mut key: impl FnMut(&mut Self) -> Result<K>,
        mut val: impl FnMut(&mut Self) -> Result<V>,
    ) -> Result<Vec<(K, V)>> {
        let n = self.u32()? as usize;
        if n > self.remaining() {
            return Err(Error::Eof {
                at: self.pos,
                want: n,
            });
        }
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let k = key(self)?;
            v.push((k, val(self)?));
        }
        Ok(v)
    }

    /// Client "packed hash table": `u16 count, u16 bucket_size`, then pairs.
    pub fn packed_hash_table<K, V>(
        &mut self,
        mut key: impl FnMut(&mut Self) -> Result<K>,
        mut val: impl FnMut(&mut Self) -> Result<V>,
    ) -> Result<Vec<(K, V)>> {
        let n = self.u16()? as usize;
        let _buckets = self.u16()?;
        let mut v = Vec::with_capacity(n.min(self.remaining()));
        for _ in 0..n {
            let k = key(self)?;
            v.push((k, val(self)?));
        }
        Ok(v)
    }
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_u32_widths() {
        assert_eq!(Reader::new(&[0x7F]).packed_u32().unwrap(), 0x7F);
        assert_eq!(Reader::new(&[0x81, 0x02]).packed_u32().unwrap(), 0x0102);
        assert_eq!(
            Reader::new(&[0xC1, 0x02, 0x03, 0x04]).packed_u32().unwrap(),
            0x0102_0403
        );
    }

    #[test]
    fn finish_rejects_trailing() {
        let mut r = Reader::new(&[1, 2, 3, 4, 5]);
        r.u32().unwrap();
        assert!(matches!(r.finish(), Err(Error::Trailing { at: 4, len: 5 })));
    }
}

/// The archives' 8-bit strings are Windows-1252 (the client was a
/// Windows program): plain ASCII passes through, 0x80..0x9F are the
/// typographic characters and the rest is Latin-1. Curly quotes and
/// dashes become their ASCII equivalents, which every UI font has.
pub fn decode_windows_1252(bytes: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '<', '\u{0152}', '\u{FFFD}', '\u{017D}',
        '\u{FFFD}', '\u{FFFD}', '\'', '\'', '"', '"', '\u{2022}', '-', '-', '\u{02DC}', '\u{2122}',
        '\u{0161}', '>', '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => HIGH[(b - 0x80) as usize],
            _ => b as char,
        })
        .collect()
}

#[cfg(test)]
mod cp1252_tests {
    #[test]
    fn typographic_bytes_decode() {
        assert_eq!(
            super::decode_windows_1252(b"Blackmoor\x92s Favor"),
            "Blackmoor's Favor"
        );
        assert_eq!(super::decode_windows_1252(b"caf\xe9"), "caf\u{e9}");
    }
}
