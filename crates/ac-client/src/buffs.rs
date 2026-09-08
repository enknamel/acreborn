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
//! * and, on each piece of armour it wears, the Item spells that
//!   harden it: Impenetrability and the banes.
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
use ac_world::buffs::{effect, kind};
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
    /// Cast on an item the character wears.
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
    /// Guids of the armour it wears.
    pub armour: &'a [u32],
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
pub fn wanted(table: &SpellTable, me: &Character) -> Vec<Want> {
    // Best known spell per (category, target).
    let mut best: BTreeMap<(u32, Target), (u32, u32)> = BTreeMap::new();
    let mut offer = |spell_id: u32, sp: &Spell, target: Target| {
        let entry = best
            .entry((sp.category, target))
            .or_insert((spell_id, sp.power));
        if sp.power > entry.1 {
            *entry = (spell_id, sp.power);
        }
    };
    for &id in me.known {
        let Some(sp) = table.get(id) else { continue };
        if !sp.is_beneficial() {
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
                offer(id, sp, Target::Me);
            }
        } else if sp.school == school::ITEM && fx.is_armor() {
            // Impenetrability and the banes go on each piece worn.
            for &guid in me.armour {
                offer(id, sp, Target::Item(guid));
            }
        }
    }
    best.into_iter()
        .map(|((category, target), (spell, power))| Want {
            spell,
            category,
            power,
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
