//! EnumMapper (0x22): a portal table mapping enum values to their names,
//! such as the character titles (0x22000041) the client shows for
//! `CharacterTitleId`. Layout: id, base mapper id, numbering type (u8),
//! packed count, then (u32 value, u8-length string) pairs; the title
//! names read `ID_CharacterTitle_Bearer_of_Darkness`.

use crate::reader::Reader;
use crate::Result;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnumMapper {
    pub id: u32,
    pub base: u32,
    pub numbering: u8,
    /// (value, name), in file order.
    pub entries: Vec<(u32, String)>,
}

impl EnumMapper {
    /// The character titles.
    pub const CHARACTER_TITLES: u32 = 0x2200_0041;

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let id = r.u32()?;
        let base = r.u32()?;
        let numbering = r.u8()?;
        let n = r.packed_u32()? as usize;
        let mut entries = Vec::with_capacity(n.min(4096));
        for _ in 0..n {
            let value = r.u32()?;
            let name = r.pstring8()?;
            entries.push((value, name));
        }
        Ok(EnumMapper {
            id,
            base,
            numbering,
            entries,
        })
    }

    pub fn get(&self, value: u32) -> Option<&str> {
        self.entries
            .iter()
            .find(|(v, _)| *v == value)
            .map(|(_, n)| n.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pairs() {
        let mut b = vec![0x41, 0, 0, 0x22, 0, 0, 0, 0, 6, 2];
        b.extend_from_slice(&[
            1, 0, 0, 0, 10, b'A', b'd', b'v', b'e', b'n', b't', b'u', b'r', b'e', b'r',
        ]);
        b.extend_from_slice(&[2, 0, 0, 0, 6, b'A', b'r', b'c', b'h', b'e', b'r']);
        let m = EnumMapper::parse(&b).unwrap();
        assert_eq!(m.id, EnumMapper::CHARACTER_TITLES);
        assert_eq!(m.get(2), Some("Archer"));
        assert_eq!(m.get(1), Some("Adventurer"));
        assert_eq!(m.get(3), None);
    }
}
