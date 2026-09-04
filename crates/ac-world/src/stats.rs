//! The player's own character sheet: name, level, attributes and vitals.
//!
//! Seeded by the PlayerDescription game event (0x13) that arrives right
//! after entering the world, then kept current by the private update
//! messages. Layouts follow ACE's `GameEventPlayerDescription` and the
//! `GameMessagePrivateUpdate*` writers.

use ac_net::messages::{event, opcode, split_game_event};
use ac_net::wire::{Reader, Truncated};

pub const ATTRIBUTE_NAMES: [&str; 6] = [
    "Strength",
    "Endurance",
    "Quickness",
    "Coordination",
    "Focus",
    "Self",
];
pub const VITAL_NAMES: [&str; 3] = ["Health", "Stamina", "Mana"];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Attribute {
    pub ranks: u32,
    pub base: u32,
    pub xp: u32,
}

impl Attribute {
    pub fn value(&self) -> u32 {
        self.base + self.ranks
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Vital {
    pub ranks: u32,
    pub base: u32,
    pub xp: u32,
    pub current: u32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayerStats {
    pub name: String,
    pub level: i32,
    pub total_xp: i64,
    /// Strength, Endurance, Quickness, Coordination, Focus, Self.
    pub attributes: [Attribute; 6],
    /// Health, Stamina, Mana.
    pub vitals: [Vital; 3],
    pub ints: Vec<(u32, i32)>,
    pub int64s: Vec<(u32, i64)>,
    pub strings: Vec<(u32, String)>,
}

pub mod property {
    pub const INT_LEVEL: u32 = 25;
    pub const INT64_TOTAL_EXPERIENCE: u32 = 1;
    pub const STRING_NAME: u32 = 1;
}

impl PlayerStats {
    /// Maximum of a vital, from the retail formulas in the portal's
    /// SecondaryAttributeTable (0x0E000003): health = endurance / 2,
    /// stamina = endurance, mana = self, each plus ranks.
    pub fn vital_max(&self, i: usize) -> u32 {
        let attr = match i {
            0 => (self.attributes[1].value() as f32 / 2.0).round() as u32,
            1 => self.attributes[1].value(),
            _ => self.attributes[5].value(),
        };
        let v = &self.vitals[i];
        v.base + v.ranks + attr
    }

    /// Parse the PlayerDescription body (after the game-event header).
    /// Reads the property tables and the attribute cache; the skill,
    /// spell, enchantment, option and inventory sections that follow are
    /// not needed yet and are left unread.
    pub fn parse_description(body: &[u8]) -> Result<Self, Truncated> {
        let mut r = Reader::new(body);
        let mut st = PlayerStats::default();
        let flags = r.u32()?;
        let _weenie_type = r.u32()?;
        if flags & 0x0001 != 0 {
            let (n, _) = (r.u16()?, r.u16()?);
            for _ in 0..n {
                let k = r.u32()?;
                let v = r.i32()?;
                if k == property::INT_LEVEL {
                    st.level = v;
                }
                st.ints.push((k, v));
            }
        }
        if flags & 0x0080 != 0 {
            let (n, _) = (r.u16()?, r.u16()?);
            for _ in 0..n {
                let k = r.u32()?;
                let v = r.u64()? as i64;
                if k == property::INT64_TOTAL_EXPERIENCE {
                    st.total_xp = v;
                }
                st.int64s.push((k, v));
            }
        }
        if flags & 0x0002 != 0 {
            let (n, _) = (r.u16()?, r.u16()?);
            for _ in 0..n {
                r.u32()?;
                r.u32()?;
            }
        }
        if flags & 0x0004 != 0 {
            let (n, _) = (r.u16()?, r.u16()?);
            for _ in 0..n {
                r.u32()?;
                r.f64()?;
            }
        }
        if flags & 0x0010 != 0 {
            let (n, _) = (r.u16()?, r.u16()?);
            for _ in 0..n {
                let k = r.u32()?;
                let v = r.string16()?;
                if k == property::STRING_NAME {
                    st.name = v.clone();
                }
                st.strings.push((k, v));
            }
        }
        if flags & 0x0008 != 0 {
            let (n, _) = (r.u16()?, r.u16()?);
            for _ in 0..n {
                r.u32()?;
                r.u32()?;
            }
        }
        if flags & 0x0040 != 0 {
            let (n, _) = (r.u16()?, r.u16()?);
            for _ in 0..n {
                r.u32()?;
                r.u32()?;
            }
        }
        if flags & 0x0020 != 0 {
            let (n, _) = (r.u16()?, r.u16()?);
            for _ in 0..n {
                r.u32()?;
                r.bytes(32)?;
            }
        }
        let vector_flags = r.u32()?;
        let _has_health = r.u32()?;
        if vector_flags & 0x0001 != 0 {
            let cache = r.u32()?;
            for (i, a) in st.attributes.iter_mut().enumerate() {
                if cache & (1 << i) != 0 {
                    a.ranks = r.u32()?;
                    a.base = r.u32()?;
                    a.xp = r.u32()?;
                }
            }
            for (i, v) in st.vitals.iter_mut().enumerate() {
                if cache & (0x40 << i) != 0 {
                    v.ranks = r.u32()?;
                    v.base = r.u32()?;
                    v.xp = r.u32()?;
                    v.current = r.u32()?;
                }
            }
        }
        Ok(st)
    }

    /// Apply one server message. Returns true if it was a stats message
    /// (whether or not it decoded).
    pub fn apply(&mut self, op: u32, body: &[u8]) -> bool {
        let r = match op {
            opcode::GAME_EVENT => match split_game_event(body) {
                Some((_, _, event::PLAYER_DESCRIPTION, rest)) => {
                    match Self::parse_description(rest) {
                        Ok(st) => {
                            *self = st;
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
                _ => return false,
            },
            opcode::PRIVATE_UPDATE_ATTRIBUTE => self.update_attribute(body),
            opcode::PRIVATE_UPDATE_VITAL => self.update_vital(body),
            opcode::PRIVATE_UPDATE_ATTRIBUTE_2ND_LEVEL => self.update_vital_current(body),
            opcode::PRIVATE_UPDATE_PROPERTY_INT => self.update_int(body),
            opcode::PRIVATE_UPDATE_PROPERTY_INT64 => self.update_int64(body),
            opcode::PRIVATE_UPDATE_PROPERTY_STRING => self.update_string(body),
            _ => return false,
        };
        if let Err(e) = r {
            tracing::warn!("stats message {op:#06x}: {e}");
        }
        true
    }

    fn update_attribute(&mut self, body: &[u8]) -> Result<(), Truncated> {
        let mut r = Reader::new(body);
        let _seq = r.u8()?;
        let which = r.u32()? as usize;
        let (ranks, base, xp) = (r.u32()?, r.u32()?, r.u32()?);
        if (1..=6).contains(&which) {
            self.attributes[which - 1] = Attribute { ranks, base, xp };
        }
        Ok(())
    }

    fn update_vital(&mut self, body: &[u8]) -> Result<(), Truncated> {
        let mut r = Reader::new(body);
        let _seq = r.u8()?;
        let which = r.u32()?;
        let (ranks, base, xp, current) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
        if let Some(i) = vital_index(which) {
            self.vitals[i] = Vital {
                ranks,
                base,
                xp,
                current,
            };
        }
        Ok(())
    }

    fn update_vital_current(&mut self, body: &[u8]) -> Result<(), Truncated> {
        let mut r = Reader::new(body);
        let _seq = r.u8()?;
        let which = r.u32()?;
        let current = r.u32()?;
        if let Some(i) = vital_index(which) {
            self.vitals[i].current = current;
        }
        Ok(())
    }

    fn update_int(&mut self, body: &[u8]) -> Result<(), Truncated> {
        let mut r = Reader::new(body);
        let _seq = r.u8()?;
        let k = r.u32()?;
        let v = r.i32()?;
        if k == property::INT_LEVEL {
            self.level = v;
        }
        match self.ints.iter_mut().find(|(kk, _)| *kk == k) {
            Some(e) => e.1 = v,
            None => self.ints.push((k, v)),
        }
        Ok(())
    }

    fn update_int64(&mut self, body: &[u8]) -> Result<(), Truncated> {
        let mut r = Reader::new(body);
        let _seq = r.u8()?;
        let k = r.u32()?;
        let v = r.u64()? as i64;
        if k == property::INT64_TOTAL_EXPERIENCE {
            self.total_xp = v;
        }
        match self.int64s.iter_mut().find(|(kk, _)| *kk == k) {
            Some(e) => e.1 = v,
            None => self.int64s.push((k, v)),
        }
        Ok(())
    }

    fn update_string(&mut self, body: &[u8]) -> Result<(), Truncated> {
        let mut r = Reader::new(body);
        let _seq = r.u8()?;
        let k = r.u32()?;
        let v = r.string16()?;
        if k == property::STRING_NAME {
            self.name = v.clone();
        }
        match self.strings.iter_mut().find(|(kk, _)| *kk == k) {
            Some(e) => e.1 = v,
            None => self.strings.push((k, v)),
        }
        Ok(())
    }
}

/// PropertyAttribute2nd: 1 MaxHealth, 2 Health, 3 MaxStamina, 4 Stamina,
/// 5 MaxMana, 6 Mana. Both the max and current ids address the same slot.
fn vital_index(which: u32) -> Option<usize> {
    match which {
        1 | 2 => Some(0),
        3 | 4 => Some(1),
        5 | 6 => Some(2),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_net::wire::Writer;

    #[test]
    fn description_properties_and_attributes() {
        let mut w = Writer::new();
        w.u32(0x0001 | 0x0010 | 0x0080).u32(10);
        // ints: level 12, another
        w.u16(2).u16(64).u32(25).i32(12).u32(21).i32(500);
        // int64: total xp
        w.u16(1).u16(64).u32(1).u64(123_456);
        // strings: name
        w.u16(1).u16(32).u32(1).string16("Reborn");
        w.u32(0x0003).u32(1).u32(0x1FF);
        for (i, base) in [10u32, 100, 30, 40, 50, 60].iter().enumerate() {
            w.u32(i as u32).u32(*base).u32(0);
        }
        // health: ranks 5, current 40; stamina; mana
        w.u32(5).u32(0).u32(0).u32(40);
        w.u32(0).u32(0).u32(0).u32(90);
        w.u32(2).u32(0).u32(0).u32(61);
        let st = PlayerStats::parse_description(&w.finish()).unwrap();
        assert_eq!(st.name, "Reborn");
        assert_eq!(st.level, 12);
        assert_eq!(st.total_xp, 123_456);
        assert_eq!(st.attributes[1].value(), 101);
        assert_eq!(st.vital_max(0), 5 + 51);
        assert_eq!(st.vital_max(1), 101);
        assert_eq!(st.vital_max(2), 2 + 65);
        assert_eq!(st.vitals[0].current, 40);
    }

    #[test]
    fn vital_updates() {
        let mut st = PlayerStats::default();
        let mut w = Writer::new();
        w.u8(1).u32(2).u32(77);
        assert!(st.apply(opcode::PRIVATE_UPDATE_ATTRIBUTE_2ND_LEVEL, &w.finish()));
        assert_eq!(st.vitals[0].current, 77);
        let mut w = Writer::new();
        w.u8(1).u32(6).u32(9).u32(0).u32(0).u32(120);
        assert!(st.apply(opcode::PRIVATE_UPDATE_ATTRIBUTE, &w.finish()));
        assert_eq!(st.attributes[5].value(), 9);
        assert!(!st.apply(opcode::OBJECT_DELETE, &[]));
    }
}
