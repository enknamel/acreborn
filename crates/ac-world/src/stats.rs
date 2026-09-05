//! The player's own character sheet: name, level, attributes, vitals,
//! skills, spellbook, enchantments, UI options and the carried/wielded
//! object lists.
//!
//! Seeded by the PlayerDescription game event (0x13) that arrives right
//! after entering the world, then kept current by the private update
//! messages. Layouts follow ACE's `GameEventPlayerDescription` and the
//! `GameMessagePrivateUpdate*` writers; the client-side option block
//! (`PlayerModule`) follows aclogview.

use ac_formats::skill_table::SkillBase;
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

/// Skill names by skill id (the client's `STypeSkill`, ACE's `Skill`
/// enum). Retired and unimplemented skills keep their slots.
pub const SKILL_NAMES: [&str; 55] = [
    "None",
    "Axe",
    "Bow",
    "Crossbow",
    "Dagger",
    "Mace",
    "Melee Defense",
    "Missile Defense",
    "Sling",
    "Spear",
    "Staff",
    "Sword",
    "Thrown Weapon",
    "Unarmed Combat",
    "Arcane Lore",
    "Magic Defense",
    "Mana Conversion",
    "Spellcraft",
    "Item Tinkering",
    "Assess Person",
    "Deception",
    "Healing",
    "Jump",
    "Lockpick",
    "Run",
    "Awareness",
    "Arms and Armor Repair",
    "Assess Creature",
    "Weapon Tinkering",
    "Armor Tinkering",
    "Magic Item Tinkering",
    "Creature Enchantment",
    "Item Enchantment",
    "Life Magic",
    "War Magic",
    "Leadership",
    "Loyalty",
    "Fletching",
    "Alchemy",
    "Cooking",
    "Salvaging",
    "Two Handed Combat",
    "Gearcraft",
    "Void Magic",
    "Heavy Weapons",
    "Light Weapons",
    "Finesse Weapons",
    "Missile Weapons",
    "Shield",
    "Dual Wield",
    "Recklessness",
    "Sneak Attack",
    "Dirty Fighting",
    "Challenge",
    "Summoning",
];

pub fn skill_name(id: u32) -> &'static str {
    SKILL_NAMES.get(id as usize).copied().unwrap_or("Unknown")
}

/// Skill advancement classes (`SKILL_ADVANCEMENT_CLASS`).
pub mod sac {
    pub const INACTIVE: u32 = 0;
    pub const UNTRAINED: u32 = 1;
    pub const TRAINED: u32 = 2;
    pub const SPECIALIZED: u32 = 3;
}

pub fn sac_name(advancement: u32) -> &'static str {
    match advancement {
        sac::INACTIVE => "inactive",
        sac::UNTRAINED => "untrained",
        sac::TRAINED => "trained",
        sac::SPECIALIZED => "specialized",
        _ => "unknown",
    }
}

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

/// One entry of the skill table (the client's `Skill` structure).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Skill {
    pub id: u32,
    /// Times raised (`level_from_pp`).
    pub ranks: u16,
    /// Advancement class, see [`sac`].
    pub advancement: u32,
    /// Experience spent on the skill.
    pub xp: u32,
    /// Bonus levels granted at creation (+5 trained, +10 specialized in
    /// ACE), added to the value like ranks.
    pub init_level: u32,
    pub last_check_resistance: u32,
    pub last_used: f64,
}

impl Skill {
    /// The fields after the skill id, shared by PlayerDescription and
    /// PrivateUpdateSkill.
    fn read(id: u32, r: &mut Reader) -> Result<Self, Truncated> {
        let ranks = r.u16()?;
        let _adjust_pp = r.u16()?;
        Ok(Skill {
            id,
            ranks,
            advancement: r.u32()?,
            xp: r.u32()?,
            init_level: r.u32()?,
            last_check_resistance: r.u32()?,
            last_used: r.f64()?,
        })
    }
}

/// One active enchantment (ACE `Network.Structure.Enchantment`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Enchantment {
    pub spell_id: u16,
    pub layer: u16,
    pub category: u16,
    pub power: u32,
    pub start_time: f64,
    pub duration: f64,
    pub caster: u32,
    pub degrade_modifier: f32,
    pub degrade_limit: f32,
    pub last_degraded: f64,
    pub stat_mod_type: u32,
    pub stat_mod_key: u32,
    pub stat_mod_value: f32,
    pub spell_set_id: u32,
}

/// The Vitae penalty is an enchantment with this spell id.
pub const VITAE_SPELL: u16 = 666;

/// `Enchantment::stat_mod_type` bits (ACE `EnchantmentTypeFlags`).
pub mod enchantment_type {
    pub const ATTRIBUTE: u32 = 0x0000_0001;
    pub const SECOND_ATTRIBUTE: u32 = 0x0000_0002;
    pub const INT: u32 = 0x0000_0004;
    pub const FLOAT: u32 = 0x0000_0008;
    pub const SKILL: u32 = 0x0000_0010;
    pub const MULTIPLICATIVE: u32 = 0x0000_4000;
    pub const ADDITIVE: u32 = 0x0000_8000;
    pub const VITAE: u32 = 0x0080_0000;
    pub const COOLDOWN: u32 = 0x0100_0000;
    /// Set by the server on every buff; clear on debuffs.
    pub const BENEFICIAL: u32 = 0x0200_0000;
}

impl Enchantment {
    /// Whether this is a buff (as opposed to a debuff, vitae or a
    /// cooldown), by the server's flag.
    pub fn is_beneficial(&self) -> bool {
        self.stat_mod_type & enchantment_type::BENEFICIAL != 0
    }

    pub fn is_vitae(&self) -> bool {
        self.spell_id == VITAE_SPELL || self.stat_mod_type & enchantment_type::VITAE != 0
    }

    pub fn is_cooldown(&self) -> bool {
        self.stat_mod_type & enchantment_type::COOLDOWN != 0
    }

    /// Seconds left at server time `now` (`start_time` and `now` are the
    /// server's clock; item spells with duration -1 never run out).
    pub fn remaining(&self, now: f64) -> Option<f64> {
        if self.duration < 0.0 {
            return None;
        }
        Some((self.start_time + self.duration - now).max(0.0))
    }

    /// One enchantment record as every enchantment event carries it.
    pub fn parse(r: &mut Reader) -> Result<Self, Truncated> {
        Self::read(r)
    }

    fn read(r: &mut Reader) -> Result<Self, Truncated> {
        let spell_id = r.u16()?;
        let layer = r.u16()?;
        let category = r.u16()?;
        let has_spell_set = r.u16()?;
        let mut e = Enchantment {
            spell_id,
            layer,
            category,
            power: r.u32()?,
            start_time: r.f64()?,
            duration: r.f64()?,
            caster: r.u32()?,
            degrade_modifier: r.f32()?,
            degrade_limit: r.f32()?,
            last_degraded: r.f64()?,
            stat_mod_type: r.u32()?,
            stat_mod_key: r.u32()?,
            stat_mod_value: r.f32()?,
            spell_set_id: 0,
        };
        if has_spell_set != 0 {
            e.spell_set_id = r.u32()?;
        }
        Ok(e)
    }
}

/// Enchantment registry category bits.
pub mod enchantment_mask {
    pub const MULTIPLICATIVE: u32 = 0x1;
    pub const ADDITIVE: u32 = 0x2;
    pub const VITAE: u32 = 0x4;
    pub const COOLDOWN: u32 = 0x8;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Shortcut {
    pub index: u32,
    pub object: u32,
    /// Layered spell id (`u16 id, u16 layer`), 0 for item shortcuts.
    pub spell: u32,
}

/// `PlayerModule` pack-header bits.
pub mod option_flags {
    pub const SHORTCUTS: u32 = 0x0001;
    pub const SQUELCH_LIST: u32 = 0x0002;
    pub const MULTI_SPELL_LISTS: u32 = 0x0004;
    pub const DESIRED_COMPS: u32 = 0x0008;
    pub const EXTENDED_MULTI_SPELL_LISTS: u32 = 0x0010;
    pub const SPELLBOOK_FILTERS: u32 = 0x0020;
    pub const OPTIONS2: u32 = 0x0040;
    pub const TIMESTAMP_FORMAT: u32 = 0x0080;
    pub const GENERIC_QUALITIES: u32 = 0x0100;
    pub const GAMEPLAY_OPTIONS: u32 = 0x0200;
    pub const SPELL_LISTS_8: u32 = 0x0400;
}

/// The client-side settings the server stores for us (`PlayerModule`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CharacterOptions {
    pub flags: u32,
    pub options1: u32,
    pub options2: u32,
    pub spellbook_filters: u32,
    pub shortcuts: Vec<Shortcut>,
    /// Favourite-spell bars, one to eight of them.
    pub spell_bars: Vec<Vec<u32>>,
    /// (spell component id, quantity to rebuy)
    pub desired_comps: Vec<(u32, u32)>,
}

impl CharacterOptions {
    /// Returns false when the block carries a section we cannot decode,
    /// in which case the reader is left somewhere inside it.
    fn read(&mut self, r: &mut Reader) -> Result<bool, Truncated> {
        self.flags = r.u32()?;
        self.options1 = r.u32()?;
        if self.flags & option_flags::SHORTCUTS != 0 {
            let n = read_count(r, 12)?;
            self.shortcuts = (0..n)
                .map(|_| {
                    Ok(Shortcut {
                        index: r.u32()?,
                        object: r.u32()?,
                        spell: r.u32()?,
                    })
                })
                .collect::<Result<_, _>>()?;
        }
        let bars = if self.flags & option_flags::SPELL_LISTS_8 != 0 {
            8
        } else if self.flags & option_flags::EXTENDED_MULTI_SPELL_LISTS != 0 {
            7
        } else if self.flags & option_flags::MULTI_SPELL_LISTS != 0 {
            5
        } else {
            1
        };
        self.spell_bars = (0..bars)
            .map(|_| {
                let n = read_count(r, 4)?;
                (0..n).map(|_| r.u32()).collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<_, _>>()?;
        if self.flags & option_flags::DESIRED_COMPS != 0 {
            let (n, _buckets) = (r.u16()?, r.u16()?);
            self.desired_comps = (0..n)
                .map(|_| Ok((r.u32()?, r.u32()?)))
                .collect::<Result<_, _>>()?;
        }
        if self.flags & option_flags::SPELLBOOK_FILTERS != 0 {
            self.spellbook_filters = r.u32()?;
        }
        if self.flags & option_flags::OPTIONS2 != 0 {
            self.options2 = r.u32()?;
        }
        if self.flags & option_flags::TIMESTAMP_FORMAT != 0 {
            let _format = r.string16()?;
        }
        if self.flags & option_flags::GENERIC_QUALITIES != 0 {
            tracing::debug!("PlayerDescription: generic qualities data present; not decoded");
            return Ok(false);
        }
        if self.flags & option_flags::GAMEPLAY_OPTIONS != 0 {
            let _version = r.u32()?;
            let _buckets = r.u8()?;
            let n = r.u8()?;
            for _ in 0..n {
                if !skip_option_property(r)? {
                    return Ok(false);
                }
            }
            r.align4()?;
        }
        Ok(true)
    }
}

/// Skip one `BaseProperty` of the gameplay-options collection (window
/// placements and opacities). Returns false on an unknown property type.
fn skip_option_property(r: &mut Reader) -> Result<bool, Truncated> {
    let key = r.u32()?;
    match key {
        // opacity: desc, f32
        0x1000_0080 | 0x1000_0081 => {
            r.u32()?;
            r.f32()?;
        }
        // placement x/y/width/height: desc, u32
        0x1000_0086..=0x1000_0089 => {
            r.u32()?;
            r.u32()?;
        }
        // visibility: desc, bool
        0x1000_008A => {
            r.u32()?;
            r.u8()?;
        }
        // title: desc, override flag, literal or (string id, table id), then
        // an empty intrusive hash table header
        0x1000_008D => {
            r.u32()?;
            if r.u8()? == 1 {
                // packed-byte length, then UTF-16 units
                let mut n = r.u8()? as usize;
                if n & 0x80 != 0 {
                    n = ((n & 0x7F) << 8) | r.u8()? as usize;
                }
                r.bytes(n * 2)?;
            } else {
                r.u32()?;
                r.u32()?;
            }
            r.u32()?;
            r.u8()?;
            r.u8()?;
        }
        // chat text type: desc, u64
        0x1000_007F => {
            r.u32()?;
            r.u64()?;
        }
        // placement array: desc, count, nested properties
        0x1000_008C => {
            r.u32()?;
            let n = read_count(r, 4)?;
            for _ in 0..n {
                if !skip_option_property(r)? {
                    return Ok(false);
                }
            }
        }
        // placement: hash table of nested properties
        0x1000_008B => {
            let _buckets = r.u8()?;
            let n = r.u8()?;
            for _ in 0..n {
                if !skip_option_property(r)? {
                    return Ok(false);
                }
            }
        }
        _ => {
            tracing::debug!("PlayerDescription: unknown option property {key:#010x}");
            return Ok(false);
        }
    }
    Ok(true)
}

/// A count that cannot possibly fit in the remaining bytes is a misparse;
/// report it as truncation rather than allocating for it.
fn read_count(r: &mut Reader, item_size: usize) -> Result<usize, Truncated> {
    let n = r.u32()? as usize;
    if n.saturating_mul(item_size) > r.remaining().len() {
        return Err(Truncated(r.pos()));
    }
    Ok(n)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InventoryEntry {
    pub guid: u32,
    /// 0 plain item, 1 container (side pack), 2 foci.
    pub container_type: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WieldedEntry {
    pub guid: u32,
    /// EquipMask bits of the wield location.
    pub location: u32,
    pub priority: u32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlayerStats {
    pub name: String,
    pub level: i32,
    pub total_xp: i64,
    pub available_xp: i64,
    pub skill_credits: i32,
    /// Strength, Endurance, Quickness, Coordination, Focus, Self.
    pub attributes: [Attribute; 6],
    /// Health, Stamina, Mana.
    pub vitals: [Vital; 3],
    pub skills: Vec<Skill>,
    /// Known spell ids.
    pub spells: Vec<u32>,
    pub enchantments: Vec<Enchantment>,
    pub options: CharacterOptions,
    /// Objects in the main pack and side slots, in placement order.
    pub inventory: Vec<InventoryEntry>,
    pub wielded: Vec<WieldedEntry>,
    pub ints: Vec<(u32, i32)>,
    pub int64s: Vec<(u32, i64)>,
    pub strings: Vec<(u32, String)>,
}

pub mod property {
    pub const INT_AVAILABLE_SKILL_CREDITS: u32 = 24;
    pub const INT_LEVEL: u32 = 25;
    pub const INT64_TOTAL_EXPERIENCE: u32 = 1;
    pub const INT64_AVAILABLE_EXPERIENCE: u32 = 2;
    pub const STRING_NAME: u32 = 1;
}

/// PlayerDescription vector-section flags.
mod vector_flags {
    pub const ATTRIBUTE: u32 = 0x0001;
    pub const SKILL: u32 = 0x0002;
    pub const SPELL: u32 = 0x0100;
    pub const ENCHANTMENT: u32 = 0x0200;
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

    /// Current value of a PropertyAttribute id (1 Strength .. 6 Self).
    fn attribute_value(&self, id: u32) -> u32 {
        match id {
            1..=6 => self.attributes[id as usize - 1].value(),
            _ => 0,
        }
    }

    /// A skill's displayed value: the attribute-derived base from the
    /// SkillTable formula (only for skills usable at their advancement
    /// class), plus the creation bonus and ranks. Mirrors ACE's
    /// `CreatureSkill.Current` without enchantments, vitae or
    /// augmentations. Without a table record only the trained part counts.
    pub fn skill_value(&self, skill: &Skill, base: Option<&SkillBase>) -> u32 {
        let from_attributes = base
            .filter(|b| b.usable_at(skill.advancement))
            .map_or(0, |b| b.formula.apply(|id| self.attribute_value(id)));
        from_attributes + skill.init_level + skill.ranks as u32
    }

    pub fn skill(&self, id: u32) -> Option<&Skill> {
        self.skills.iter().find(|s| s.id == id)
    }

    fn skill_mut(&mut self, id: u32) -> &mut Skill {
        match self.skills.iter().position(|s| s.id == id) {
            Some(i) => &mut self.skills[i],
            None => {
                self.skills.push(Skill {
                    id,
                    ..Default::default()
                });
                self.skills.last_mut().unwrap()
            }
        }
    }

    /// Parse the PlayerDescription body (after the game-event header).
    /// The property tables and attribute cache must decode; the sections
    /// after them (skills, spells, enchantments, options, inventory) are
    /// best-effort and a problem there keeps whatever was read so far.
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
                st.set_int(k, v);
            }
        }
        if flags & 0x0080 != 0 {
            let (n, _) = (r.u16()?, r.u16()?);
            for _ in 0..n {
                let k = r.u32()?;
                let v = r.u64()? as i64;
                st.set_int64(k, v);
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
                st.set_string(k, v);
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
        let vectors = r.u32()?;
        let _has_health = r.u32()?;
        if vectors & vector_flags::ATTRIBUTE != 0 {
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
        if let Err(e) = st.parse_tail(&mut r, vectors) {
            tracing::debug!(
                "PlayerDescription tail: {e} of {} bytes (kept {} skills, {} spells, {} items)",
                body.len(),
                st.skills.len(),
                st.spells.len(),
                st.inventory.len()
            );
        }
        Ok(st)
    }

    /// Skills, spellbook, enchantments, options, inventory and wielded
    /// lists. Each section is stored as soon as it decodes.
    fn parse_tail(&mut self, r: &mut Reader, vectors: u32) -> Result<(), Truncated> {
        if vectors & vector_flags::SKILL != 0 {
            let (n, _) = (r.u16()?, r.u16()?);
            self.skills = (0..n)
                .map(|_| {
                    let id = r.u32()?;
                    Skill::read(id, r)
                })
                .collect::<Result<_, _>>()?;
        }
        if vectors & vector_flags::SPELL != 0 {
            let (n, _) = (r.u16()?, r.u16()?);
            self.spells = (0..n)
                .map(|_| {
                    let id = r.u32()?;
                    let _new_config = r.f32()?;
                    Ok(id)
                })
                .collect::<Result<_, _>>()?;
        }
        if vectors & vector_flags::ENCHANTMENT != 0 {
            let mask = r.u32()?;
            let mut list = Vec::new();
            for bit in [
                enchantment_mask::MULTIPLICATIVE,
                enchantment_mask::ADDITIVE,
                enchantment_mask::COOLDOWN,
            ] {
                if mask & bit != 0 {
                    let n = read_count(r, 60)?;
                    for _ in 0..n {
                        list.push(Enchantment::read(r)?);
                    }
                }
            }
            if mask & enchantment_mask::VITAE != 0 {
                list.push(Enchantment::read(r)?);
            }
            self.enchantments = list;
        }
        let mut options = CharacterOptions::default();
        let complete = options.read(r)?;
        self.options = options;
        if !complete {
            tracing::debug!(
                "PlayerDescription: option block not fully decoded; inventory lists skipped"
            );
            return Ok(());
        }
        let n = read_count(r, 8)?;
        self.inventory = (0..n)
            .map(|_| {
                Ok(InventoryEntry {
                    guid: r.u32()?,
                    container_type: r.u32()?,
                })
            })
            .collect::<Result<_, _>>()?;
        let n = read_count(r, 12)?;
        self.wielded = (0..n)
            .map(|_| {
                Ok(WieldedEntry {
                    guid: r.u32()?,
                    location: r.u32()?,
                    priority: r.u32()?,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(())
    }

    // ------------------------------------------------------- spellbook

    /// Add a spell to the book; true if it was new.
    pub fn learn_spell(&mut self, spell: u32) -> bool {
        if self.spells.contains(&spell) {
            return false;
        }
        self.spells.push(spell);
        true
    }

    /// Take a spell out of the book (and off every spell bar); true if it
    /// was there.
    pub fn forget_spell(&mut self, spell: u32) -> bool {
        let before = self.spells.len();
        self.spells.retain(|&s| s != spell);
        for bar in &mut self.options.spell_bars {
            bar.retain(|&s| s != spell);
        }
        self.spells.len() != before
    }

    // ---------------------------------------------------- enchantments

    /// Add an enchantment, replacing the one with the same spell id and
    /// layer if present (a refresh keeps its slot).
    pub fn upsert_enchantment(&mut self, e: Enchantment) {
        match self
            .enchantments
            .iter_mut()
            .find(|x| x.spell_id == e.spell_id && x.layer == e.layer)
        {
            Some(slot) => *slot = e,
            None => self.enchantments.push(e),
        }
    }

    /// Remove the enchantment with this spell id and layer; true if found.
    pub fn remove_enchantment(&mut self, spell_id: u16, layer: u16) -> bool {
        let before = self.enchantments.len();
        self.enchantments
            .retain(|x| !(x.spell_id == spell_id && x.layer == layer));
        self.enchantments.len() != before
    }

    /// Drop every enchantment (MagicPurgeEnchantments).
    pub fn purge_enchantments(&mut self) {
        self.enchantments.clear();
    }

    /// Drop the harmful enchantments; buffs, vitae and cooldowns stay
    /// (MagicPurgeBadEnchantments, sent on death).
    pub fn purge_bad_enchantments(&mut self) {
        self.enchantments
            .retain(|e| e.is_beneficial() || e.is_vitae() || e.is_cooldown());
    }

    /// Apply one server message. `None` if it was not a stats message;
    /// otherwise what changed (whether or not the body decoded).
    pub fn apply(&mut self, op: u32, body: &[u8]) -> Option<StatsApplied> {
        let (r, applied) = match op {
            opcode::GAME_EVENT => match split_game_event(body) {
                Some((_, _, event::PLAYER_DESCRIPTION, rest)) => {
                    match Self::parse_description(rest) {
                        Ok(st) => {
                            *self = st;
                            (Ok(()), StatsApplied::Stats)
                        }
                        Err(e) => (Err(e), StatsApplied::Stats),
                    }
                }
                Some((_, _, event::MAGIC_UPDATE_SPELL, rest)) => {
                    match layered_spell(&mut Reader::new(rest)) {
                        Ok((spell, _layer)) => {
                            self.learn_spell(spell as u32);
                            (
                                Ok(()),
                                StatsApplied::Spellbook {
                                    spell: spell as u32,
                                    known: true,
                                },
                            )
                        }
                        Err(e) => (Err(e), StatsApplied::Stats),
                    }
                }
                Some((_, _, event::MAGIC_REMOVE_SPELL, rest)) => {
                    match layered_spell(&mut Reader::new(rest)) {
                        Ok((spell, _layer)) => {
                            self.forget_spell(spell as u32);
                            (
                                Ok(()),
                                StatsApplied::Spellbook {
                                    spell: spell as u32,
                                    known: false,
                                },
                            )
                        }
                        Err(e) => (Err(e), StatsApplied::Stats),
                    }
                }
                Some((_, _, ev, rest)) if is_enchantment_event(ev) => (
                    self.apply_enchantment_event(ev, rest),
                    StatsApplied::Enchantments,
                ),
                _ => return None,
            },
            opcode::PRIVATE_UPDATE_ATTRIBUTE => (self.update_attribute(body), StatsApplied::Stats),
            opcode::PRIVATE_UPDATE_VITAL => (self.update_vital(body), StatsApplied::Stats),
            opcode::PRIVATE_UPDATE_ATTRIBUTE_2ND_LEVEL => {
                (self.update_vital_current(body), StatsApplied::Stats)
            }
            opcode::PRIVATE_UPDATE_SKILL => (self.update_skill(body), StatsApplied::Stats),
            opcode::PRIVATE_UPDATE_SKILL_LEVEL => {
                (self.update_skill_level(body), StatsApplied::Stats)
            }
            opcode::PRIVATE_UPDATE_SKILL_AC => (self.update_skill_ac(body), StatsApplied::Stats),
            opcode::PRIVATE_UPDATE_PROPERTY_INT => (self.update_int(body), StatsApplied::Stats),
            opcode::PRIVATE_UPDATE_PROPERTY_INT64 => (self.update_int64(body), StatsApplied::Stats),
            opcode::PRIVATE_UPDATE_PROPERTY_STRING => {
                (self.update_string(body), StatsApplied::Stats)
            }
            _ => return None,
        };
        if let Err(e) = r {
            tracing::warn!("stats message {op:#06x}: {e}");
        }
        Some(applied)
    }

    /// The enchantment registry events (`ac_net::messages::event`
    /// 0x02C2..0x02C8 and 0x0312), `rest` being the body after the
    /// game-event header.
    fn apply_enchantment_event(&mut self, ev: u32, rest: &[u8]) -> Result<(), Truncated> {
        let mut r = Reader::new(rest);
        match ev {
            event::MAGIC_UPDATE_ENCHANTMENT => {
                let e = Enchantment::read(&mut r)?;
                self.upsert_enchantment(e);
            }
            event::MAGIC_UPDATE_MULTIPLE_ENCHANTMENTS => {
                let n = read_count(&mut r, 60)?;
                for _ in 0..n {
                    let e = Enchantment::read(&mut r)?;
                    self.upsert_enchantment(e);
                }
            }
            event::MAGIC_REMOVE_ENCHANTMENT | event::MAGIC_DISPEL_ENCHANTMENT => {
                let (spell, layer) = layered_spell(&mut r)?;
                self.remove_enchantment(spell, layer);
            }
            event::MAGIC_REMOVE_MULTIPLE_ENCHANTMENTS
            | event::MAGIC_DISPEL_MULTIPLE_ENCHANTMENTS => {
                let n = read_count(&mut r, 4)?;
                for _ in 0..n {
                    let (spell, layer) = layered_spell(&mut r)?;
                    self.remove_enchantment(spell, layer);
                }
            }
            event::MAGIC_PURGE_ENCHANTMENTS => self.purge_enchantments(),
            event::MAGIC_PURGE_BAD_ENCHANTMENTS => self.purge_bad_enchantments(),
            _ => {}
        }
        Ok(())
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

    /// PrivateUpdateSkill (0x02DD): the whole skill record.
    fn update_skill(&mut self, body: &[u8]) -> Result<(), Truncated> {
        let mut r = Reader::new(body);
        let _seq = r.u8()?;
        let id = r.u32()?;
        let skill = Skill::read(id, &mut r)?;
        *self.skill_mut(id) = skill;
        Ok(())
    }

    /// PrivateUpdateSkillLevel (0x02DF): ranks only.
    fn update_skill_level(&mut self, body: &[u8]) -> Result<(), Truncated> {
        let mut r = Reader::new(body);
        let _seq = r.u8()?;
        let id = r.u32()?;
        let level = r.u32()?;
        self.skill_mut(id).ranks = level.min(u16::MAX as u32) as u16;
        Ok(())
    }

    /// PrivateUpdateSkillAC (0x02E1): advancement class only.
    fn update_skill_ac(&mut self, body: &[u8]) -> Result<(), Truncated> {
        let mut r = Reader::new(body);
        let _seq = r.u8()?;
        let id = r.u32()?;
        let advancement = r.u32()?;
        self.skill_mut(id).advancement = advancement;
        Ok(())
    }

    fn set_int(&mut self, k: u32, v: i32) {
        match k {
            property::INT_LEVEL => self.level = v,
            property::INT_AVAILABLE_SKILL_CREDITS => self.skill_credits = v,
            _ => {}
        }
        match self.ints.iter_mut().find(|(kk, _)| *kk == k) {
            Some(e) => e.1 = v,
            None => self.ints.push((k, v)),
        }
    }

    fn set_int64(&mut self, k: u32, v: i64) {
        match k {
            property::INT64_TOTAL_EXPERIENCE => self.total_xp = v,
            property::INT64_AVAILABLE_EXPERIENCE => self.available_xp = v,
            _ => {}
        }
        match self.int64s.iter_mut().find(|(kk, _)| *kk == k) {
            Some(e) => e.1 = v,
            None => self.int64s.push((k, v)),
        }
    }

    fn set_string(&mut self, k: u32, v: String) {
        if k == property::STRING_NAME {
            self.name = v.clone();
        }
        match self.strings.iter_mut().find(|(kk, _)| *kk == k) {
            Some(e) => e.1 = v,
            None => self.strings.push((k, v)),
        }
    }

    fn update_int(&mut self, body: &[u8]) -> Result<(), Truncated> {
        let mut r = Reader::new(body);
        let _seq = r.u8()?;
        let k = r.u32()?;
        let v = r.i32()?;
        self.set_int(k, v);
        Ok(())
    }

    fn update_int64(&mut self, body: &[u8]) -> Result<(), Truncated> {
        let mut r = Reader::new(body);
        let _seq = r.u8()?;
        let k = r.u32()?;
        let v = r.u64()? as i64;
        self.set_int64(k, v);
        Ok(())
    }

    fn update_string(&mut self, body: &[u8]) -> Result<(), Truncated> {
        let mut r = Reader::new(body);
        let _seq = r.u8()?;
        let k = r.u32()?;
        let v = r.string16()?;
        self.set_string(k, v);
        Ok(())
    }
}

/// What a stats message changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsApplied {
    /// Name, level, attributes, vitals, skills or properties.
    Stats,
    /// A spell entered (`known`) or left the spellbook.
    Spellbook { spell: u32, known: bool },
    /// The enchantment registry changed.
    Enchantments,
}

/// `u16 spell id, u16 layer` (ACE `LayeredSpell`).
fn layered_spell(r: &mut Reader) -> Result<(u16, u16), Truncated> {
    Ok((r.u16()?, r.u16()?))
}

fn is_enchantment_event(ev: u32) -> bool {
    matches!(
        ev,
        event::MAGIC_UPDATE_ENCHANTMENT
            | event::MAGIC_REMOVE_ENCHANTMENT
            | event::MAGIC_UPDATE_MULTIPLE_ENCHANTMENTS
            | event::MAGIC_REMOVE_MULTIPLE_ENCHANTMENTS
            | event::MAGIC_PURGE_ENCHANTMENTS
            | event::MAGIC_DISPEL_ENCHANTMENT
            | event::MAGIC_DISPEL_MULTIPLE_ENCHANTMENTS
            | event::MAGIC_PURGE_BAD_ENCHANTMENTS
    )
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
    use ac_formats::skill_table::{attribute, SkillFormula};
    use ac_net::wire::Writer;

    const MELEE_DEFENSE: u32 = 6;
    const RUN: u32 = 24;
    const LIFE_MAGIC: u32 = 33;
    const HEALING: u32 = 21;

    fn table_entry(attr1: u32, attr2: u32, divisor: u32, min_level: u32) -> SkillBase {
        SkillBase {
            min_level,
            formula: SkillFormula {
                x: 1,
                divisor,
                attr1,
                attr2,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn write_skill(w: &mut Writer, id: u32, ranks: u16, sac: u32, xp: u32, init: u32) {
        w.u32(id)
            .u16(ranks)
            .u16(1)
            .u32(sac)
            .u32(xp)
            .u32(init)
            .u32(0)
            .f64(0.0);
    }

    fn write_enchantment(w: &mut Writer, spell: u16, with_set: bool) {
        write_enchantment_layer(w, spell, 1, with_set, 0x0011);
    }

    fn write_enchantment_layer(
        w: &mut Writer,
        spell: u16,
        layer: u16,
        with_set: bool,
        stat_mod_type: u32,
    ) {
        w.u16(spell)
            .u16(layer)
            .u16(0x9A)
            .u16(with_set as u16)
            .u32(50)
            .f64(1000.0)
            .f64(1800.0)
            .u32(0x5000_0001)
            .f32(1.0)
            .f32(0.0)
            .f64(0.0)
            .u32(stat_mod_type)
            .u32(6)
            .f32(0.15);
        if with_set {
            w.u32(7);
        }
    }

    /// A GameEvent body (after the opcode): our guid, sequence, event
    /// type, then `rest`.
    fn game_event(ev: u32, rest: &[u8]) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32(0x5000_0001).u32(1).u32(ev).bytes(rest);
        w.finish()
    }

    fn ids(st: &PlayerStats) -> Vec<(u16, u16)> {
        st.enchantments
            .iter()
            .map(|e| (e.spell_id, e.layer))
            .collect()
    }

    /// A PlayerDescription body with every section populated, in the
    /// layout ACE sends.
    fn full_description() -> Vec<u8> {
        let mut w = Writer::new();
        w.u32(0x0001 | 0x0010 | 0x0080).u32(10);
        // ints: level 12, skill credits 3, another
        w.u16(3).u16(64);
        w.u32(25).i32(12).u32(24).i32(3).u32(21).i32(500);
        // int64: total xp, available xp
        w.u16(2).u16(64).u32(1).u64(123_456).u32(2).u64(789);
        // strings: name
        w.u16(1).u16(32).u32(1).string16("Reborn");
        // vectors: attributes, skills, spells, enchantments
        w.u32(0x0303).u32(1).u32(0x1FF);
        for (i, base) in [10u32, 100, 30, 40, 50, 60].iter().enumerate() {
            w.u32(i as u32).u32(*base).u32(0);
        }
        w.u32(5).u32(0).u32(0).u32(40);
        w.u32(0).u32(0).u32(0).u32(90);
        w.u32(2).u32(0).u32(0).u32(61);
        // skills
        w.u16(4).u16(32);
        write_skill(&mut w, MELEE_DEFENSE, 10, sac::TRAINED, 4000, 5);
        write_skill(&mut w, RUN, 0, sac::UNTRAINED, 0, 0);
        write_skill(&mut w, LIFE_MAGIC, 20, sac::SPECIALIZED, 9000, 10);
        write_skill(&mut w, HEALING, 3, sac::UNTRAINED, 0, 0);
        // spellbook
        w.u16(2).u16(64).u32(1).f32(2.0).u32(2091).f32(2.0);
        // enchantments: additive list of one (with a set id), and vitae
        w.u32(enchantment_mask::ADDITIVE | enchantment_mask::VITAE);
        w.u32(1);
        write_enchantment(&mut w, 2091, true);
        write_enchantment(&mut w, 666, false);
        // options: shortcuts, 8 spell bars, comps, filters, options2,
        // gameplay options
        w.u32(0x0001 | 0x0008 | 0x0020 | 0x0040 | 0x0200 | 0x0400)
            .u32(0x1234);
        w.u32(1).u32(0).u32(0x8000_0010).u32(0);
        w.u32(2).u32(1).u32(2091);
        for _ in 1..8 {
            w.u32(0);
        }
        w.u16(1).u16(256).u32(1).u32(50);
        w.u32(0x3FFF);
        w.u32(0x0094_8700);
        // PackObjPropertyCollection: version, buckets, 2 properties, one
        // of them a placement array holding a placement with a bool
        w.u32(2).u8(8).u8(2);
        w.u32(0x1000_0086).u32(0x1000_0086).u32(120);
        w.u32(0x1000_008C).u32(0x1000_008C).u32(1);
        w.u32(0x1000_008B).u8(8).u8(1);
        w.u32(0x1000_008A).u32(0x1000_008A).u8(1);
        w.align4();
        // inventory and wielded
        w.u32(2).u32(0x8000_0010).u32(0).u32(0x8000_0011).u32(1);
        w.u32(1).u32(0x8000_0020).u32(0x0000_0001).u32(3);
        w.finish()
    }

    #[test]
    fn description_full_layout() {
        let st = PlayerStats::parse_description(&full_description()).unwrap();
        assert_eq!(st.name, "Reborn");
        assert_eq!(st.level, 12);
        assert_eq!(st.skill_credits, 3);
        assert_eq!(st.total_xp, 123_456);
        assert_eq!(st.available_xp, 789);
        assert_eq!(st.attributes[1].value(), 101);
        assert_eq!(st.vital_max(0), 5 + 51);
        assert_eq!(st.vital_max(1), 101);
        assert_eq!(st.vital_max(2), 2 + 65);
        assert_eq!(st.vitals[0].current, 40);

        assert_eq!(st.skills.len(), 4);
        let md = st.skill(MELEE_DEFENSE).unwrap();
        assert_eq!(
            *md,
            Skill {
                id: MELEE_DEFENSE,
                ranks: 10,
                advancement: sac::TRAINED,
                xp: 4000,
                init_level: 5,
                last_check_resistance: 0,
                last_used: 0.0,
            }
        );
        assert_eq!(st.spells, vec![1, 2091]);
        assert_eq!(st.enchantments.len(), 2);
        assert_eq!(st.enchantments[0].spell_id, 2091);
        assert_eq!(st.enchantments[0].spell_set_id, 7);
        assert_eq!(st.enchantments[0].stat_mod_key, 6);
        assert_eq!(st.enchantments[1].spell_id, 666);
        assert_eq!(st.enchantments[1].spell_set_id, 0);

        assert_eq!(st.options.options1, 0x1234);
        assert_eq!(st.options.options2, 0x0094_8700);
        assert_eq!(st.options.spellbook_filters, 0x3FFF);
        assert_eq!(
            st.options.shortcuts,
            vec![Shortcut {
                index: 0,
                object: 0x8000_0010,
                spell: 0
            }]
        );
        assert_eq!(st.options.spell_bars.len(), 8);
        assert_eq!(st.options.spell_bars[0], vec![1, 2091]);
        assert_eq!(st.options.desired_comps, vec![(1, 50)]);

        assert_eq!(
            st.inventory,
            vec![
                InventoryEntry {
                    guid: 0x8000_0010,
                    container_type: 0
                },
                InventoryEntry {
                    guid: 0x8000_0011,
                    container_type: 1
                },
            ]
        );
        assert_eq!(
            st.wielded,
            vec![WieldedEntry {
                guid: 0x8000_0020,
                location: 1,
                priority: 3
            }]
        );
    }

    #[test]
    fn skill_values_follow_table_formula() {
        let st = PlayerStats::parse_description(&full_description()).unwrap();
        // attribute values (base + ranks): str 10, end 101, quick 32,
        // coord 43, focus 54, self 65
        assert_eq!(st.attributes[2].value(), 32);
        assert_eq!(st.attributes[3].value(), 43);
        let md = table_entry(attribute::QUICKNESS, attribute::COORDINATION, 3, 1);
        // (32 + 43) / 3 = 25, + 5 init + 10 ranks
        assert_eq!(
            st.skill_value(st.skill(MELEE_DEFENSE).unwrap(), Some(&md)),
            40
        );
        let run = table_entry(attribute::QUICKNESS, attribute::NONE, 1, 1);
        assert_eq!(st.skill_value(st.skill(RUN).unwrap(), Some(&run)), 32);
        let lm = table_entry(attribute::FOCUS, attribute::SELF, 4, 2);
        // (54 + 65) / 4 = 29.75 -> 30, + 10 init + 20 ranks
        assert_eq!(st.skill_value(st.skill(LIFE_MAGIC).unwrap(), Some(&lm)), 60);
        // untrained skill that needs training: no attribute base
        let healing = table_entry(attribute::COORDINATION, attribute::FOCUS, 3, 2);
        assert_eq!(
            st.skill_value(st.skill(HEALING).unwrap(), Some(&healing)),
            3
        );
        // no table record at all
        assert_eq!(st.skill_value(st.skill(LIFE_MAGIC).unwrap(), None), 30);
    }

    #[test]
    fn truncated_tail_keeps_head() {
        let body = full_description();
        // Cut inside the skill table: head decodes, tail is dropped.
        let cut = body.len() - 200;
        let st = PlayerStats::parse_description(&body[..cut]).unwrap();
        assert_eq!(st.name, "Reborn");
        assert_eq!(st.level, 12);
        assert!(st.inventory.is_empty());
        // Cut inside the inventory list: everything before it survives.
        let cut = body.len() - 4;
        let st = PlayerStats::parse_description(&body[..cut]).unwrap();
        assert_eq!(st.skills.len(), 4);
        assert_eq!(st.spells.len(), 2);
        assert_eq!(st.options.spell_bars.len(), 8);
        assert_eq!(st.inventory.len(), 2);
        assert!(st.wielded.is_empty());
    }

    #[test]
    fn unknown_option_property_skips_inventory() {
        let mut w = Writer::new();
        w.u32(0).u32(10).u32(0).u32(0);
        w.u32(option_flags::GAMEPLAY_OPTIONS).u32(0);
        w.u32(0);
        w.u32(2).u8(8).u8(1);
        w.u32(0xDEAD_BEEF).u32(1).u32(2);
        w.u32(1).u32(0x8000_0010).u32(0);
        let st = PlayerStats::parse_description(&w.finish()).unwrap();
        assert_eq!(st.options.flags, option_flags::GAMEPLAY_OPTIONS);
        assert!(st.inventory.is_empty());
    }

    #[test]
    fn skill_updates() {
        let mut st = PlayerStats::parse_description(&full_description()).unwrap();
        let mut w = Writer::new();
        w.u8(1);
        write_skill(&mut w, MELEE_DEFENSE, 11, sac::TRAINED, 4500, 5);
        assert!(st
            .apply(opcode::PRIVATE_UPDATE_SKILL, &w.finish())
            .is_some());
        let md = st.skill(MELEE_DEFENSE).unwrap();
        assert_eq!((md.ranks, md.xp), (11, 4500));

        let mut w = Writer::new();
        w.u8(2).u32(RUN).u32(7);
        assert!(st
            .apply(opcode::PRIVATE_UPDATE_SKILL_LEVEL, &w.finish())
            .is_some());
        assert_eq!(st.skill(RUN).unwrap().ranks, 7);

        let mut w = Writer::new();
        w.u8(3).u32(HEALING).u32(sac::TRAINED);
        assert!(st
            .apply(opcode::PRIVATE_UPDATE_SKILL_AC, &w.finish())
            .is_some());
        assert_eq!(st.skill(HEALING).unwrap().advancement, sac::TRAINED);

        // a skill we had not seen yet is created
        let mut w = Writer::new();
        w.u8(4).u32(48).u32(sac::TRAINED);
        assert!(st
            .apply(opcode::PRIVATE_UPDATE_SKILL_AC, &w.finish())
            .is_some());
        assert_eq!(st.skills.len(), 5);
        assert_eq!(st.skill(48).unwrap().advancement, sac::TRAINED);

        let mut w = Writer::new();
        w.u8(5).u32(property::INT64_AVAILABLE_EXPERIENCE).u64(4242);
        assert!(st
            .apply(opcode::PRIVATE_UPDATE_PROPERTY_INT64, &w.finish())
            .is_some());
        assert_eq!(st.available_xp, 4242);
        let mut w = Writer::new();
        w.u8(6).u32(property::INT_LEVEL).i32(13);
        assert!(st
            .apply(opcode::PRIVATE_UPDATE_PROPERTY_INT, &w.finish())
            .is_some());
        assert_eq!(st.level, 13);
    }

    #[test]
    fn vital_updates() {
        let mut st = PlayerStats::default();
        let mut w = Writer::new();
        w.u8(1).u32(2).u32(77);
        assert!(st
            .apply(opcode::PRIVATE_UPDATE_ATTRIBUTE_2ND_LEVEL, &w.finish())
            .is_some());
        assert_eq!(st.vitals[0].current, 77);
        let mut w = Writer::new();
        w.u8(1).u32(6).u32(9).u32(0).u32(0).u32(120);
        assert!(st
            .apply(opcode::PRIVATE_UPDATE_ATTRIBUTE, &w.finish())
            .is_some());
        assert_eq!(st.attributes[5].value(), 9);
        assert!(st.apply(opcode::OBJECT_DELETE, &[]).is_none());
    }

    #[test]
    fn spellbook_events() {
        let mut st = PlayerStats::parse_description(&full_description()).unwrap();
        assert_eq!(st.spells, vec![1, 2091]);
        // MagicUpdateSpell: u16 id, u16 layer.
        let mut w = Writer::new();
        w.u16(2000).u16(0);
        assert_eq!(
            st.apply(
                opcode::GAME_EVENT,
                &game_event(event::MAGIC_UPDATE_SPELL, &w.finish())
            ),
            Some(StatsApplied::Spellbook {
                spell: 2000,
                known: true
            })
        );
        assert_eq!(st.spells, vec![1, 2091, 2000]);
        // Learning it again keeps one copy.
        let mut w = Writer::new();
        w.u16(2000).u16(0);
        st.apply(
            opcode::GAME_EVENT,
            &game_event(event::MAGIC_UPDATE_SPELL, &w.finish()),
        );
        assert_eq!(st.spells.len(), 3);
        // MagicRemoveSpell takes it out of the book and off the bars.
        let mut w = Writer::new();
        w.u16(2091).u16(0);
        assert_eq!(
            st.apply(
                opcode::GAME_EVENT,
                &game_event(event::MAGIC_REMOVE_SPELL, &w.finish())
            ),
            Some(StatsApplied::Spellbook {
                spell: 2091,
                known: false
            })
        );
        assert_eq!(st.spells, vec![1, 2000]);
        assert_eq!(st.options.spell_bars[0], vec![1]);
        // A truncated body is still a stats message.
        assert_eq!(
            st.apply(
                opcode::GAME_EVENT,
                &game_event(event::MAGIC_UPDATE_SPELL, &[1])
            ),
            Some(StatsApplied::Stats)
        );
    }

    #[test]
    fn enchantment_events() {
        use enchantment_type::BENEFICIAL;
        let mut st = PlayerStats::parse_description(&full_description()).unwrap();
        assert_eq!(ids(&st), vec![(2091, 1), (666, 1)]);
        // MagicUpdateEnchantment adds a new spell...
        let mut w = Writer::new();
        write_enchantment_layer(&mut w, 1, 1, false, BENEFICIAL | 0x1);
        assert_eq!(
            st.apply(
                opcode::GAME_EVENT,
                &game_event(event::MAGIC_UPDATE_ENCHANTMENT, &w.finish())
            ),
            Some(StatsApplied::Enchantments)
        );
        assert_eq!(ids(&st), vec![(2091, 1), (666, 1), (1, 1)]);
        // ...replaces the same spell and layer in place...
        let mut w = Writer::new();
        write_enchantment_layer(&mut w, 1, 1, true, BENEFICIAL | 0x1);
        st.apply(
            opcode::GAME_EVENT,
            &game_event(event::MAGIC_UPDATE_ENCHANTMENT, &w.finish()),
        );
        assert_eq!(ids(&st), vec![(2091, 1), (666, 1), (1, 1)]);
        assert_eq!(st.enchantments[2].spell_set_id, 7);
        // ...and a second layer of it is a second entry.
        let mut w = Writer::new();
        write_enchantment_layer(&mut w, 1, 2, false, BENEFICIAL | 0x1);
        st.apply(
            opcode::GAME_EVENT,
            &game_event(event::MAGIC_UPDATE_ENCHANTMENT, &w.finish()),
        );
        assert_eq!(ids(&st), vec![(2091, 1), (666, 1), (1, 1), (1, 2)]);
        // MagicUpdateMultipleEnchantments: count then records; one of
        // them a debuff.
        let mut w = Writer::new();
        w.u32(2);
        write_enchantment_layer(&mut w, 50, 1, false, 0x10);
        write_enchantment_layer(&mut w, 2091, 1, true, BENEFICIAL | 0x10);
        st.apply(
            opcode::GAME_EVENT,
            &game_event(event::MAGIC_UPDATE_MULTIPLE_ENCHANTMENTS, &w.finish()),
        );
        assert_eq!(ids(&st), vec![(2091, 1), (666, 1), (1, 1), (1, 2), (50, 1)]);
        assert!(st.enchantments[0].is_beneficial());
        assert!(!st.enchantments[4].is_beneficial());
        assert!(st.enchantments[1].is_vitae());
        // MagicRemoveEnchantment removes one layer only.
        let mut w = Writer::new();
        w.u16(1).u16(1);
        st.apply(
            opcode::GAME_EVENT,
            &game_event(event::MAGIC_REMOVE_ENCHANTMENT, &w.finish()),
        );
        assert_eq!(ids(&st), vec![(2091, 1), (666, 1), (1, 2), (50, 1)]);
        // An unknown layer is a no-op.
        let mut w = Writer::new();
        w.u16(1).u16(9);
        st.apply(
            opcode::GAME_EVENT,
            &game_event(event::MAGIC_DISPEL_ENCHANTMENT, &w.finish()),
        );
        assert_eq!(st.enchantments.len(), 4);
        // MagicDispelMultipleEnchantments: count then (id, layer) pairs.
        let mut w = Writer::new();
        w.u32(2).u16(1).u16(2).u16(2091).u16(1);
        st.apply(
            opcode::GAME_EVENT,
            &game_event(event::MAGIC_DISPEL_MULTIPLE_ENCHANTMENTS, &w.finish()),
        );
        assert_eq!(ids(&st), vec![(666, 1), (50, 1)]);
        // PurgeBad keeps vitae and buffs, drops the debuff.
        let mut w = Writer::new();
        write_enchantment_layer(&mut w, 3, 1, false, BENEFICIAL | 0x1);
        st.apply(
            opcode::GAME_EVENT,
            &game_event(event::MAGIC_UPDATE_ENCHANTMENT, &w.finish()),
        );
        st.apply(
            opcode::GAME_EVENT,
            &game_event(event::MAGIC_PURGE_BAD_ENCHANTMENTS, &[]),
        );
        assert_eq!(ids(&st), vec![(666, 1), (3, 1)]);
        // Purge drops everything.
        assert_eq!(
            st.apply(
                opcode::GAME_EVENT,
                &game_event(event::MAGIC_PURGE_ENCHANTMENTS, &[])
            ),
            Some(StatsApplied::Enchantments)
        );
        assert!(st.enchantments.is_empty());
        // A short record is reported, not applied.
        assert_eq!(
            st.apply(
                opcode::GAME_EVENT,
                &game_event(event::MAGIC_UPDATE_ENCHANTMENT, &[1, 0, 1, 0])
            ),
            Some(StatsApplied::Enchantments)
        );
        assert!(st.enchantments.is_empty());
    }

    #[test]
    fn enchantment_timing() {
        let e = Enchantment {
            start_time: 100.0,
            duration: 30.0,
            ..Default::default()
        };
        assert_eq!(e.remaining(110.0), Some(20.0));
        assert_eq!(e.remaining(200.0), Some(0.0));
        let item = Enchantment {
            duration: -1.0,
            ..Default::default()
        };
        assert_eq!(item.remaining(0.0), None);
    }

    #[test]
    fn names() {
        assert_eq!(skill_name(MELEE_DEFENSE), "Melee Defense");
        assert_eq!(skill_name(54), "Summoning");
        assert_eq!(skill_name(99), "Unknown");
        assert_eq!(sac_name(sac::SPECIALIZED), "specialized");
    }
}
