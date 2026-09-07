//! What an enchantment does to whatever it lands on.
//!
//! The client's own SpellTable says a spell's school, its level, and
//! whether it is cast on the caster, on a creature or on an item, but
//! not what it changes. That is server data, so `data/spell_effects.csv`
//! is a copy of it (see `reference/scripts/data/spell_effects.sh`): for
//! each spell, which kind of statistic it touches, which one, and by how
//! much.
//!
//! With that a client can tell Heavy Weapon Mastery from War Magic
//! Mastery without reading either name, which is what lets it buff a
//! character for the skills that character has actually trained, and
//! pick the highest level of each it knows.

use std::sync::OnceLock;

/// The kind of statistic an enchantment changes: the low byte of the
/// server's stat-mod word.
pub mod kind {
    pub const ATTRIBUTE: u32 = 0x01;
    /// A vital: health, stamina or mana, or their regeneration rates.
    pub const VITAL: u32 = 0x02;
    pub const INT: u32 = 0x04;
    pub const FLOAT: u32 = 0x08;
    pub const SKILL: u32 = 0x10;
    /// Armour on the body as a whole (Armor Self).
    pub const BODY_ARMOR: u32 = 0x80;
    pub const MASK: u32 = 0xFF;
}

/// What one enchantment changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effect {
    pub spell: u32,
    /// The server's stat-mod word; see [`kind`] for its low byte.
    pub mod_type: u32,
    /// Which statistic: a skill id, an attribute id (1 Strength .. 6
    /// Self), a vital, or a property id for ints and floats.
    pub key: u32,
    pub value: f32,
}

impl Effect {
    pub fn kind(&self) -> u32 {
        self.mod_type & kind::MASK
    }

    /// The skill this raises or lowers, if it is a skill enchantment.
    pub fn skill(&self) -> Option<u32> {
        (self.kind() == kind::SKILL).then_some(self.key)
    }

    /// The attribute (1 Strength .. 6 Self) it changes, if one.
    pub fn attribute(&self) -> Option<u32> {
        (self.kind() == kind::ATTRIBUTE).then_some(self.key)
    }

    /// A float property of a creature or item it changes, by id: the
    /// resistances 64..70, an armour's ArmorModVs* 13..19, a weapon's
    /// offence or defence.
    pub fn float_property(&self) -> Option<u32> {
        (self.kind() == kind::FLOAT).then_some(self.key)
    }

    pub fn int_property(&self) -> Option<u32> {
        (self.kind() == kind::INT).then_some(self.key)
    }

    /// Changes the armour worn: Armor Self on a body, Impenetrability
    /// (armour level, int 28) or a bane (ArmorModVs*, floats 13..19) on
    /// a piece.
    pub fn is_armor(&self) -> bool {
        self.kind() == kind::BODY_ARMOR
            || self.int_property() == Some(28)
            || self
                .float_property()
                .is_some_and(|p| (13..=19).contains(&p))
    }
}

const DATA: &str = include_str!("../data/spell_effects.csv");

fn parse(text: &str) -> Vec<Effect> {
    let mut out = Vec::with_capacity(4500);
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let mut f = line.split(',');
        let (Some(id), Some(t), Some(k), Some(v)) = (f.next(), f.next(), f.next(), f.next()) else {
            continue;
        };
        let (Ok(spell), Ok(mod_type), Ok(key), Ok(value)) = (
            id.trim().parse::<u32>(),
            t.trim().parse::<u32>(),
            k.trim().parse::<u32>(),
            v.trim().parse::<f32>(),
        ) else {
            continue;
        };
        out.push(Effect {
            spell,
            mod_type,
            key,
            value,
        });
    }
    out.sort_by_key(|e| e.spell);
    out
}

fn all() -> &'static [Effect] {
    static EFFECTS: OnceLock<Vec<Effect>> = OnceLock::new();
    EFFECTS.get_or_init(|| parse(DATA))
}

/// What `spell` changes, if it is an enchantment.
pub fn effect(spell: u32) -> Option<Effect> {
    let all = all();
    all.binary_search_by_key(&spell, |e| e.spell)
        .ok()
        .map(|i| all[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effects_say_what_a_spell_changes_without_its_name() {
        // Heavy Weapon Mastery Self VI raises skill 44 by 35.
        let mastery = effect(423).expect("in the table");
        assert_eq!(mastery.skill(), Some(44));
        assert_eq!(mastery.value, 35.0);
        assert_eq!(mastery.attribute(), None);
        // Strength Self VI raises attribute 1.
        assert_eq!(effect(1332).and_then(|e| e.attribute()), Some(1));
        // Blade Protection Self VI: a resistance float.
        assert_eq!(effect(1114).and_then(|e| e.float_property()), Some(64));
        // Armour: Armor Self, Impenetrability and a bane all count.
        assert!(effect(1312).is_some_and(|e| e.is_armor()));
        assert!(effect(1486).is_some_and(|e| e.is_armor()));
        assert!(effect(1562).is_some_and(|e| e.is_armor()));
        assert!(!mastery.is_armor());
        // A war bolt changes nothing.
        assert_eq!(effect(84), None);
        assert!(all().len() > 4000);
        assert!(all().windows(2).all(|w| w[0].spell < w[1].spell));
    }
}
