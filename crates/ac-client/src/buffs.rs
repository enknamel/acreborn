//! Which buffs a character should be wearing, worked out from what it is
//! rather than written down by hand.
//!
//! The game is a closed system: the spells that exist, what each one
//! does, and the skills a character has trained are all known. So the
//! right set of buffs is not a matter of taste but of arithmetic, and a
//! player should not have to type it in. A character keeps up
//!
//! * every Life self-enchantment it knows: the protections, the
//!   regenerations, Armor Self;
//! * every Creature self-enchantment it knows that raises an attribute,
//!   a defence, or a skill it has trained or specialised, so a swordsman
//!   carries Heavy Weapon Mastery and not War Magic Mastery; of the
//!   weapon skills, only the one for the weapon in hand;
//! * the Item auras that suit the way it fights: damage, speed and
//!   accuracy for a weapon in hand, the caster's own for a wand;
//! * and the Item spells that harden what it wears, Impenetrability
//!   and the banes. Cast on the character itself, each of those lands
//!   on every enchantable piece worn and the shield at once, so it is
//!   one cast a spell rather than one a piece.
//!
//! Of each, the highest level it can actually land. Knowing a spell is
//! not the same as being able to cast it: every spell has a power, the
//! cast is rolled against the school's skill as it stands with every
//! buff counted, and a level too far above that skill fizzles more than
//! it lands. So the caller says which spells are castable well enough
//! and the highest of those is chosen. Levels come from the spell's
//! power in the client's own table, never from its name, and spells of
//! one category are one buff at different levels.

use std::collections::BTreeMap;

use ac_formats::spell_table::{school, Spell, SpellTable};
use ac_world::buffs::{effect, kind, Effect};
use ac_world::stats::sac;

use crate::Stance;

/// One buff to keep up: the spell, and what it is cast on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Want {
    pub spell: u32,
    /// The spell's category, which is what an enchantment on the
    /// character reports: the same buff at any level shares it.
    pub category: u32,
    pub power: u32,
    pub target: Target,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// Cast on the character (an untargeted cast).
    Me,
    /// Cast at a thing by guid. For the armour spells that thing is the
    /// character itself: the server spreads a targeted Impenetrability
    /// or bane over everything worn.
    Item(u32),
}

/// What the derivation is told about the character.
pub struct Character<'a> {
    /// Spell ids in the spellbook.
    pub known: &'a [u32],
    /// Skill ids trained or specialised.
    pub trained: &'a [u32],
    /// How it fights, from what is in its hands.
    pub stance: Stance,
    /// The character's own guid, and whether it wears anything the
    /// armour spells could harden.
    pub guid: u32,
    pub wears_armour: bool,
    /// The skill of the weapon in hand, when one is: only that weapon
    /// skill is buffed. `Some(0)` for a caster, which uses none; `None`
    /// for empty hands, when every trained weapon skill is fair.
    pub weapon_skill: Option<u32>,
    /// Whether a spell can be cast well enough to be worth casting at
    /// all, by its id: the school's skill against the spell's power.
    pub usable: &'a dyn Fn(u32) -> bool,
}

/// The skills a weapon is used with: one of these is buffed only when
/// the weapon in hand uses it. A character that has trained three ways
/// of fighting still fights one way at a time.
const WEAPON_SKILLS: [u32; 8] = [41, 44, 45, 46, 47, 48, 49, 50];

/// The float properties of the weapon auras, by what they are for. A
/// caster gains nothing from a faster swing and a swordsman nothing
/// from a mana link, so each stance wants its own.
mod aura {
    /// Damage, speed and attack: for anything swung or shot.
    pub const WEAPON: [u32; 3] = [360, 361, 168];
    /// Elemental damage and mana conversion: for a caster.
    pub const CASTER: [u32; 2] = [170, 171];
}

/// The buffs `me` should be wearing, one per category, the highest level
/// known of each.
/// A spell that runs out sooner than this (seconds) is not a buff worth
/// keeping up; the numbered ones last half an hour or more and the
/// level-eight ones a quarter.
pub const LASTS_AT_LEAST: f64 = 600.0;

pub fn wanted(table: &SpellTable, me: &Character) -> Vec<Want> {
    // Best known spell per (effect, target): what it changes and by how
    // much, then its level. The item cantrips (Minor Flame Bane) sit in
    // categories of their own, so per category they would all be
    // wanted too, for a tenth of what the numbered spell does.
    let mut best: BTreeMap<(u32, u32, Target), (u32, u32, f32)> = BTreeMap::new();
    let mut offer = |spell_id: u32, sp: &Spell, fx: &Effect, target: Target| {
        let entry =
            best.entry((fx.mod_type, fx.key, target))
                .or_insert((spell_id, sp.category, 0.0));
        let better = |a: (f32, u32), b: (f32, u32)| a.0 > b.0 || (a.0 == b.0 && a.1 > b.1);
        let have = table.get(entry.0).map(|s| s.power).unwrap_or(0);
        if entry.2 == 0.0 || better((fx.value.abs(), sp.power), (entry.2, have)) {
            *entry = (spell_id, sp.category, fx.value.abs());
        }
    };
    for &id in me.known {
        let Some(sp) = table.get(id) else { continue };
        if !sp.is_beneficial() {
            continue;
        }
        // A buff is something worn for a good while. The quest and gem
        // spells that outrank the numbered ones in power run out in a
        // minute or less (Licorice Leap, Tusker Sprint), and a character
        // keeping those up did nothing else.
        if !sp.duration().is_some_and(|d| d >= LASTS_AT_LEAST) {
            continue;
        }
        let Some(fx) = effect(id) else { continue };
        if !(me.usable)(id) {
            continue;
        }
        if sp.is_self_targeted() {
            let keep = match sp.school {
                school::LIFE => true,
                school::CREATURE => match fx.skill() {
                    // A skill buff is worth it for a trained skill, and
                    // a weapon skill only for the weapon in hand.
                    Some(skill) => {
                        me.trained.contains(&skill)
                            && (!WEAPON_SKILLS.contains(&skill)
                                || me.weapon_skill.is_none_or(|w| w == skill))
                    }
                    // Attributes, and anything else on the body.
                    None => true,
                },
                school::ITEM => match fx.kind() {
                    // The auras: the ones for the weapon in hand.
                    kind::INT | kind::FLOAT => {
                        let for_weapon = aura::WEAPON.contains(&fx.key);
                        let for_caster = aura::CASTER.contains(&fx.key);
                        match me.stance {
                            Stance::Magic => for_caster || (!for_weapon && !for_caster),
                            _ => for_weapon || (!for_weapon && !for_caster),
                        }
                    }
                    _ => true,
                },
                _ => false,
            };
            if keep {
                offer(id, sp, &fx, Target::Me);
            }
        } else if sp.school == school::ITEM && fx.is_armor() && me.wears_armour {
            // Impenetrability and the banes, cast at ourselves: the
            // server puts them on every piece worn.
            offer(id, sp, &fx, Target::Item(me.guid));
        }
    }
    best.into_iter()
        .map(|((_, _, target), (spell, category, _))| Want {
            spell,
            category,
            power: table.get(spell).map(|s| s.power).unwrap_or(0),
            target,
        })
        .collect()
}

/// Skill ids a character has trained or specialised.
pub fn trained_skills(skills: &[ac_world::stats::Skill]) -> Vec<u32> {
    skills
        .iter()
        .filter(|s| s.advancement >= sac::TRAINED)
        .map(|s| s.id)
        .collect()
}
