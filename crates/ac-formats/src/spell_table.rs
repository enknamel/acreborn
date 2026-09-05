//! SpellTable (0x0E00000E): one record per spell id, plus the equipment
//! spell sets. Layout cross-checked against ACE's `SpellTable`/`SpellBase`
//! and the client (`FUN_00597e20` de-obfuscates the name and description,
//! `FUN_004fe440` hashes them, `FUN_005bdac0` subtracts the key from each
//! non-zero component slot).

use serde::Serialize;

use crate::{expect_id, Reader, Result};

/// Magic schools (`MagicSchool` in the client's skill/spell tables).
pub mod school {
    pub const NONE: u32 = 0;
    pub const WAR: u32 = 1;
    pub const LIFE: u32 = 2;
    pub const ITEM: u32 = 3;
    pub const CREATURE: u32 = 4;
    pub const VOID: u32 = 5;

    pub fn name(school: u32) -> &'static str {
        match school {
            WAR => "War Magic",
            LIFE => "Life Magic",
            ITEM => "Item Enchantment",
            CREATURE => "Creature Enchantment",
            VOID => "Void Magic",
            _ => "Unknown",
        }
    }

    /// Short label for tight UI columns.
    pub fn short_name(school: u32) -> &'static str {
        match school {
            WAR => "War",
            LIFE => "Life",
            ITEM => "Item",
            CREATURE => "Creature",
            VOID => "Void",
            _ => "?",
        }
    }
}

/// Bits of [`Spell::bitfield`].
pub mod flags {
    pub const RESISTABLE: u32 = 0x1;
    pub const PK_SENSITIVE: u32 = 0x2;
    pub const BENEFICIAL: u32 = 0x4;
    /// The spell can only be cast on the caster; the client sends
    /// CastUntargetedSpell for it.
    pub const SELF_TARGETED: u32 = 0x8;
    pub const REVERSED: u32 = 0x10;
    pub const NOT_INDOOR: u32 = 0x20;
    pub const NOT_OUTDOOR: u32 = 0x40;
    pub const NOT_RESEARCHABLE: u32 = 0x80;
    pub const PROJECTILE: u32 = 0x100;
    pub const CREATURE_SPELL: u32 = 0x200;
    pub const EXCLUDED_FROM_ITEM_DESCRIPTIONS: u32 = 0x400;
    pub const IGNORES_MANA_CONVERSION: u32 = 0x800;
    pub const NON_TRACKING_PROJECTILE: u32 = 0x1000;
    pub const FELLOWSHIP_SPELL: u32 = 0x2000;
    pub const FAST_CAST: u32 = 0x4000;
    pub const INDOOR_LONG_RANGE: u32 = 0x8000;
    pub const DAMAGE_OVER_TIME: u32 = 0x10000;
}

/// `meta_spell_type` values (the client's `SpellType`).
pub mod spell_type {
    pub const UNDEF: u32 = 0;
    pub const ENCHANTMENT: u32 = 1;
    pub const PROJECTILE: u32 = 2;
    pub const BOOST: u32 = 3;
    pub const TRANSFER: u32 = 4;
    pub const PORTAL_LINK: u32 = 5;
    pub const PORTAL_RECALL: u32 = 6;
    pub const PORTAL_SUMMON: u32 = 7;
    pub const PORTAL_SENDING: u32 = 8;
    pub const DISPEL: u32 = 9;
    pub const LIFE_PROJECTILE: u32 = 10;
    pub const FELLOW_BOOST: u32 = 11;
    pub const FELLOW_ENCHANTMENT: u32 = 12;
    pub const FELLOW_PORTAL_SENDING: u32 = 13;
    pub const FELLOW_DISPEL: u32 = 14;
    pub const ENCHANTMENT_PROJECTILE: u32 = 15;
}

/// Fields that only some `meta_spell_type`s carry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub enum TypeData {
    #[default]
    None,
    /// Enchantment and FellowEnchantment.
    Enchantment {
        /// Seconds.
        duration: f64,
        degrade_modifier: f32,
        degrade_limit: f32,
    },
    /// PortalSummon.
    PortalSummon {
        /// Seconds.
        portal_lifetime: f64,
    },
}

/// Highest spell level in retail; `level()` never exceeds it.
pub const MAX_LEVEL: u32 = 8;

/// Minimum `power` for each spell level 1..=8 (ACE's `SpellFormula.MinPower`).
const MIN_POWER: [u32; MAX_LEVEL as usize] = [1, 50, 100, 150, 200, 250, 300, 400];

/// Scarab component ids and the spell level the client's spellbook filter
/// assigns to a formula led by each (ACE's `SpellFormula.ScarabLevel`).
const SCARAB_LEVEL: [(u32, u32); 10] = [
    (1, 1),   // Lead
    (2, 2),   // Iron
    (3, 3),   // Copper
    (4, 4),   // Silver
    (5, 5),   // Gold
    (6, 6),   // Pyreal
    (110, 6), // Diamond
    (112, 7), // Platinum
    (192, 7), // Dark
    (193, 8), // Mana
];

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Spell {
    pub name: String,
    pub description: String,
    /// See [`school`].
    pub school: u32,
    /// RenderSurface (0x06) id.
    pub icon_id: u32,
    /// Spells of one category are the same effect at different levels
    /// and do not stack.
    pub category: u32,
    /// See [`flags`].
    pub bitfield: u32,
    pub base_mana: u32,
    pub base_range_constant: f32,
    pub base_range_mod: f32,
    /// Strength within the category; also the skill check difficulty.
    pub power: u32,
    pub economy_mod: f32,
    /// Which per-account taper scramble applies (0 none, 1..3).
    pub formula_version: u32,
    /// Component burn rate.
    pub component_loss: f32,
    /// See [`spell_type`].
    pub meta_spell_type: u32,
    pub meta_spell_id: u32,
    pub type_data: TypeData,
    /// The 8 formula slots, descrambled; 0 = empty. Ids index the
    /// SpellComponentTable.
    pub components: [u32; 8],
    pub caster_effect: u32,
    pub target_effect: u32,
    pub fizzle_effect: u32,
    pub recovery_interval: f64,
    pub recovery_amount: f32,
    /// Sort key for the client's spell list.
    pub display_order: u32,
    pub non_component_target_type: u32,
    /// Extra mana per additional target.
    pub mana_mod: u32,
}

impl Spell {
    fn parse(r: &mut Reader) -> Result<Self> {
        let name = r.obfuscated_string()?;
        r.align4()?;
        let description = r.obfuscated_string()?;
        r.align4()?;
        let school = r.u32()?;
        let icon_id = r.u32()?;
        let category = r.u32()?;
        let bitfield = r.u32()?;
        let base_mana = r.u32()?;
        let base_range_constant = r.f32()?;
        let base_range_mod = r.f32()?;
        let power = r.u32()?;
        let economy_mod = r.f32()?;
        let formula_version = r.u32()?;
        let component_loss = r.f32()?;
        let meta_spell_type = r.u32()?;
        let meta_spell_id = r.u32()?;
        let type_data = match meta_spell_type {
            spell_type::ENCHANTMENT | spell_type::FELLOW_ENCHANTMENT => TypeData::Enchantment {
                duration: r.f64()?,
                degrade_modifier: r.f32()?,
                degrade_limit: r.f32()?,
            },
            spell_type::PORTAL_SUMMON => TypeData::PortalSummon {
                portal_lifetime: r.f64()?,
            },
            _ => TypeData::None,
        };
        let key = formula_key(&name, &description);
        let mut components = [0u32; 8];
        for c in &mut components {
            let raw = r.u32()?;
            if raw != 0 {
                *c = raw.wrapping_sub(key);
            }
        }
        Ok(Spell {
            name,
            description,
            school,
            icon_id,
            category,
            bitfield,
            base_mana,
            base_range_constant,
            base_range_mod,
            power,
            economy_mod,
            formula_version,
            component_loss,
            meta_spell_type,
            meta_spell_id,
            type_data,
            components,
            caster_effect: r.u32()?,
            target_effect: r.u32()?,
            fizzle_effect: r.u32()?,
            recovery_interval: r.f64()?,
            recovery_amount: r.f32()?,
            display_order: r.u32()?,
            non_component_target_type: r.u32()?,
            mana_mod: r.u32()?,
        })
    }

    pub fn has_flag(&self, flag: u32) -> bool {
        self.bitfield & flag != 0
    }

    /// Only castable on the caster (CastUntargetedSpell).
    pub fn is_self_targeted(&self) -> bool {
        self.has_flag(flags::SELF_TARGETED)
    }

    /// Needs a selected target (CastTargetedSpell): everything that is not
    /// self-targeted, including fellowship spells and projectiles.
    pub fn needs_target(&self) -> bool {
        !self.is_self_targeted()
    }

    pub fn is_beneficial(&self) -> bool {
        self.has_flag(flags::BENEFICIAL)
    }

    /// Launches projectiles (war and void bolts, life projectiles,
    /// enchantment-carrying projectiles). Decided by the meta spell type;
    /// the `PROJECTILE` flag bit is not set on the ordinary war bolts.
    pub fn is_projectile(&self) -> bool {
        matches!(
            self.meta_spell_type,
            spell_type::PROJECTILE
                | spell_type::LIFE_PROJECTILE
                | spell_type::ENCHANTMENT_PROJECTILE
        )
    }

    pub fn is_fellowship(&self) -> bool {
        self.has_flag(flags::FELLOWSHIP_SPELL)
    }

    pub fn is_fast_cast(&self) -> bool {
        self.has_flag(flags::FAST_CAST)
    }

    /// The occupied formula slots in order (scarab first, talisman last).
    pub fn formula(&self) -> impl Iterator<Item = u32> + '_ {
        self.components.iter().copied().filter(|&c| c != 0)
    }

    /// Spell level 1..=8 from `power` (the server's rule: the highest
    /// level whose minimum power the spell reaches); 0 for power 0.
    pub fn level(&self) -> u32 {
        MIN_POWER
            .iter()
            .rposition(|&min| self.power >= min)
            .map(|i| i as u32 + 1)
            .unwrap_or(0)
    }

    /// Spell level as the client's spellbook filter derives it: from the
    /// scarab that leads the formula. 0 when the first component is not a
    /// scarab (some quest and creature spells).
    pub fn scarab_level(&self) -> u32 {
        let first = self.components[0];
        SCARAB_LEVEL
            .iter()
            .find(|(id, _)| *id == first)
            .map(|(_, l)| *l)
            .unwrap_or(0)
    }

    /// Enchantment duration in seconds, when the spell is one.
    pub fn duration(&self) -> Option<f64> {
        match self.type_data {
            TypeData::Enchantment { duration, .. } => Some(duration),
            _ => None,
        }
    }
}

/// The client's string hash (`FUN_004fe440`): bytes as signed chars folded
/// into 28 bits. Strings here are Windows-1252 decoded byte-for-byte, so
/// every char maps back to one byte.
pub fn string_hash(s: &str) -> u32 {
    let mut h: u32 = 0;
    for c in s.chars() {
        let byte = c as u32 as u8 as i8;
        h = (h << 4).wrapping_add(byte as i32 as u32);
        if h & 0xF000_0000 != 0 {
            h = (h ^ ((h & 0xF000_0000) >> 24)) & 0x0FFF_FFFF;
        }
    }
    if h == u32::MAX {
        u32::MAX - 1
    } else {
        h
    }
}

/// The per-spell key subtracted from each stored component id.
fn formula_key(name: &str, description: &str) -> u32 {
    (string_hash(description) % 0xBEAD_CF45).wrapping_add(string_hash(name) % 0x1210_7680)
}

/// Spells granted by an equipment set at each combined item level.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SpellSet {
    /// Sorted by piece count; each tier lists the spell ids active at it.
    pub tiers: Vec<(u32, Vec<u32>)>,
}

impl SpellSet {
    fn parse(r: &mut Reader) -> Result<Self> {
        let mut tiers = r.packed_hash_table(|r| r.u32(), |r| r.list(|r| r.u32()))?;
        tiers.sort_by_key(|(k, _)| *k);
        Ok(SpellSet { tiers })
    }

    /// The spells active with `pieces` items of the set equipped: the
    /// highest tier at or below that count.
    pub fn active(&self, pieces: u32) -> &[u32] {
        self.tiers
            .iter()
            .rev()
            .find(|(n, _)| *n <= pieces)
            .map(|(_, s)| s.as_slice())
            .unwrap_or(&[])
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SpellTable {
    pub id: u32,
    /// Sorted by spell id.
    pub spells: Vec<(u32, Spell)>,
    /// Sorted by equipment set id (PropertyInt EquipmentSetId).
    pub spell_sets: Vec<(u32, SpellSet)>,
}

impl SpellTable {
    pub const ID: u32 = 0x0E00_000E;

    pub fn parse(id: u32, bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let id = expect_id(&mut r, id)?;
        let mut spells = r.packed_hash_table(|r| r.u32(), Spell::parse)?;
        let mut spell_sets = r.packed_hash_table(|r| r.u32(), SpellSet::parse)?;
        r.finish()?;
        spells.sort_by_key(|(k, _)| *k);
        spell_sets.sort_by_key(|(k, _)| *k);
        Ok(SpellTable {
            id,
            spells,
            spell_sets,
        })
    }

    pub fn get(&self, spell_id: u32) -> Option<&Spell> {
        self.spells
            .binary_search_by_key(&spell_id, |(k, _)| *k)
            .ok()
            .map(|i| &self.spells[i].1)
    }

    pub fn spell_set(&self, set_id: u32) -> Option<&SpellSet> {
        self.spell_sets
            .binary_search_by_key(&set_id, |(k, _)| *k)
            .ok()
            .map(|i| &self.spell_sets[i].1)
    }

    /// The first spell with exactly this name.
    pub fn find_by_name(&self, name: &str) -> Option<(u32, &Spell)> {
        self.spells
            .iter()
            .find(|(_, s)| s.name == name)
            .map(|(id, s)| (*id, s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_thresholds() {
        let at = |power| Spell {
            power,
            ..Default::default()
        };
        assert_eq!(at(0).level(), 0);
        assert_eq!(at(1).level(), 1);
        assert_eq!(at(49).level(), 1);
        assert_eq!(at(50).level(), 2);
        assert_eq!(at(120).level(), 3);
        assert_eq!(at(300).level(), 7);
        assert_eq!(at(399).level(), 7);
        assert_eq!(at(400).level(), 8);
        assert_eq!(at(9999).level(), 8);
    }

    #[test]
    fn scarab_levels() {
        let led_by = |scarab| Spell {
            components: [scarab, 0, 0, 0, 0, 0, 0, 0],
            ..Default::default()
        };
        assert_eq!(led_by(1).scarab_level(), 1);
        assert_eq!(led_by(6).scarab_level(), 6);
        assert_eq!(led_by(110).scarab_level(), 6);
        assert_eq!(led_by(193).scarab_level(), 8);
        assert_eq!(led_by(0).scarab_level(), 0);
        assert_eq!(led_by(20).scarab_level(), 0);
    }

    #[test]
    fn flags_and_targeting() {
        let s = Spell {
            bitfield: flags::BENEFICIAL | flags::SELF_TARGETED,
            ..Default::default()
        };
        assert!(s.is_self_targeted());
        assert!(!s.needs_target());
        assert!(s.is_beneficial());
        let t = Spell {
            bitfield: flags::RESISTABLE,
            meta_spell_type: spell_type::PROJECTILE,
            ..Default::default()
        };
        assert!(t.needs_target());
        assert!(t.is_projectile());
        assert!(!t.is_beneficial());
    }

    #[test]
    fn hash_folds_to_28_bits_and_signs_bytes() {
        assert_eq!(string_hash(""), 0);
        assert_eq!(string_hash("A"), 0x41);
        assert_eq!(string_hash("AB"), 0x452);
        // Eight chars overflow the top nibble and fold it back in.
        let h = string_hash("Strength Self I");
        assert!(h < 0x1000_0000);
        // Bytes >= 0x80 are signed: "\u{e9}" contributes -0x17, which
        // wraps and is folded back into 28 bits.
        assert_eq!(string_hash("\u{e9}"), 0x0FFF_FF19);
    }

    #[test]
    fn spell_set_tiers() {
        let set = SpellSet {
            tiers: vec![(2, vec![10]), (4, vec![10, 11])],
        };
        assert!(set.active(1).is_empty());
        assert_eq!(set.active(2), &[10]);
        assert_eq!(set.active(3), &[10]);
        assert_eq!(set.active(9), &[10, 11]);
    }
}
