//! Choosing what to fight something with.
//!
//! A character carrying several weapons should not swing whichever it
//! picked up last. Creatures take far more damage from some elements
//! than others, and a weapon that deals the right one, or that has been
//! imbued to rend the target's resistance to it, is worth several times
//! one that has not: a fire weapon against something that takes 1.4
//! times from fire and 0.5 from slashing is nearly three times the
//! weapon before its own damage is counted.
//!
//! None of that matters for a weapon the character cannot pick up. The
//! better a weapon is, the more it asks of whoever wields it --- the top
//! wands want a base War Magic of 275 --- and the server simply refuses
//! to arm anyone who falls short. So a [`Wielder`] is weighed against
//! every requirement first, and what the character is actually skilled
//! at then counts towards the score: a great sword swung at a hundred
//! skill is worth less than a wand cast at four hundred, whatever their
//! elements.
//!
//! [`score`] puts a number on a weapon against a particular creature and
//! [`best`] picks from what is carried. The numbers are a judgement, not
//! the game's own formula: they rank weapons against each other and
//! nothing more, and they only use what an appraisal has told us, so an
//! unappraised weapon is scored on what little its description carries.

use ac_world::elements::{self, imbue, Creature, Element};

use crate::items::ItemStats;
use crate::Stance;

/// Rending strips resistance, so a rended element is never worth less
/// than a neutral one, and is worth this much more on top.
const RENDING_BONUS: f32 = 1.25;
/// Criticals land often enough to matter over a long fight; this is what
/// critical strike is treated as adding against something with health to
/// spare.
const CRIT_STRIKE_BONUS: f32 = 0.5;
/// And crippling blow, which makes them hurt more rather than land more.
const CRIPPLING_BONUS: f32 = 0.35;
/// Ignoring armour is worth about as much as rending it.
const ARMOR_RENDING_BONUS: f32 = 0.25;
/// A creature with at least this much health lives long enough for a
/// better critical rate to pay for itself. Roughly a Drudge Skulker
/// several times over.
pub const LONG_FIGHT_HEALTH: u32 = 200;
/// The skill a weapon is judged as ordinary at. Skill above this counts
/// for the weapon, below it against, which is how a well-trained wand
/// beats a barely-trained sword.
const ORDINARY_SKILL: f32 = 250.0;
/// How far skill may swing the score either way.
const SKILL_FLOOR: f32 = 0.25;
const SKILL_CEILING: f32 = 2.0;

/// What the character can bring to bear: enough to answer whether a
/// weapon may be wielded at all, and how well it would be used.
///
/// Each skill is carried twice: as it stands with every buff counted,
/// which is what a plain skill requirement and a swing are measured
/// against, and as the base without them, which is what a raw-skill
/// requirement wants. The same for attributes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Wielder {
    pub level: u32,
    /// `(skill id, base, current, advancement class)`.
    pub skills: Vec<(u32, u32, u32, u32)>,
    /// Strength, Endurance, Coordination, Quickness, Focus, Self: base
    /// and current.
    pub attributes: [u32; 6],
    pub attributes_current: [u32; 6],
    /// Maximum health, stamina and mana.
    pub vitals: [u32; 3],
}

impl Wielder {
    /// How high this skill stands right now, buffs counted; 0 when the
    /// character does not have it.
    pub fn skill(&self, id: u32) -> u32 {
        self.skills
            .iter()
            .find(|(s, _, _, _)| *s == id)
            .map(|(_, _, current, _)| *current)
            .unwrap_or(0)
    }

    /// The skill without enchantments.
    pub fn skill_base(&self, id: u32) -> u32 {
        self.skills
            .iter()
            .find(|(s, _, _, _)| *s == id)
            .map(|(_, base, _, _)| *base)
            .unwrap_or(0)
    }

    fn advancement(&self, id: u32) -> u32 {
        self.skills
            .iter()
            .find(|(s, _, _, _)| *s == id)
            .map(|(_, _, _, a)| *a)
            .unwrap_or(0)
    }

    /// Whether one `(kind, what, difficulty)` requirement is met. A kind
    /// we do not understand is treated as met: refusing to wield
    /// something over a rule we cannot read would be worse than letting
    /// the server say no.
    pub fn meets(&self, req: (u32, u32, u32)) -> bool {
        use ac_world::wield;
        let (kind, what, difficulty) = req;
        let at = |i: u32, of: &[u32]| of.get(i as usize).copied().unwrap_or(0);
        match kind {
            wield::SKILL => self.skill(what) >= difficulty,
            wield::RAW_SKILL => self.skill_base(what) >= difficulty,
            wield::ATTRIB => at(what.saturating_sub(1), &self.attributes_current) >= difficulty,
            wield::RAW_ATTRIB => at(what.saturating_sub(1), &self.attributes) >= difficulty,
            wield::SECONDARY_ATTRIB | wield::RAW_SECONDARY_ATTRIB => {
                // Health, stamina and mana are 1, 3 and 5.
                at(what.saturating_sub(1) / 2, &self.vitals) >= difficulty
            }
            wield::LEVEL => self.level >= difficulty,
            wield::TRAINING => self.advancement(what) >= difficulty,
            _ => true,
        }
    }

    /// Whether every requirement on the item is met.
    pub fn can_wield(&self, item: &ItemStats) -> bool {
        item.wield_reqs.iter().all(|r| self.meets(*r))
    }

    /// How much the character's skill with this weapon counts for or
    /// against it. A weapon whose skill is unknown, or a character whose
    /// skills we have not been told, is neither helped nor hurt.
    fn skill_factor(&self, item: &ItemStats) -> f32 {
        if self.skills.is_empty() {
            return 1.0;
        }
        let Some(skill) = skill_used(item) else {
            return 1.0;
        };
        let have = self.skill(skill) as f32;
        (have / ORDINARY_SKILL).clamp(SKILL_FLOOR, SKILL_CEILING)
    }
}

/// Why a weapon was picked, in words a status line can use.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub guid: u32,
    pub name: String,
    pub score: f32,
    /// "fire, rending" or "critical strike".
    pub why: String,
}

/// Which stance a weapon gives, or `None` when it is not a weapon.
pub fn stance_of(item: &ItemStats) -> Option<Stance> {
    use ac_world::item_type;
    if item.item_type & item_type::CASTER != 0 {
        Some(Stance::Magic)
    } else if item.item_type & item_type::MISSILE_WEAPON != 0 {
        Some(Stance::Missile)
    } else if item.item_type & item_type::MELEE_WEAPON != 0 {
        Some(Stance::Melee)
    } else {
        None
    }
}

/// The skill a weapon is used with. The weapon profile says so for a
/// sword or a bow, but a wand's appraisal names no skill: what it does
/// name is the skill it asks of the wielder, which for a caster is the
/// school it is cast with, so that stands in.
fn skill_used(item: &ItemStats) -> Option<u32> {
    if item.weapon_skill_id != 0 {
        return Some(item.weapon_skill_id);
    }
    use ac_world::wield;
    item.wield_reqs
        .iter()
        .find(|(kind, _, _)| *kind == wield::SKILL || *kind == wield::RAW_SKILL)
        .map(|(_, what, _)| *what)
}

/// How good this weapon is against `target`, higher being better.
///
/// The elements it deals are weighed by how much the creature takes from
/// them, rending counted; the result is multiplied by how hard the
/// weapon hits. A long fight is worth more critical damage, so critical
/// strike and crippling blow only count against something with health to
/// spare. Nothing known about the target means every weapon is judged on
/// its damage alone.
pub fn score(item: &ItemStats, target: Option<&Creature>, wielder: &Wielder) -> f32 {
    let elements_dealt = Element::from_bits(item.damage_type_bits);
    let long_fight = target.is_some_and(|c| c.health >= LONG_FIGHT_HEALTH);
    // The best element it deals: a weapon that does two is used for
    // whichever serves better.
    let mut worth = elements_dealt
        .iter()
        .map(|e| {
            let takes = target.map(|c| c.takes_from(*e)).unwrap_or(1.0);
            if item.imbued & e.rending() != 0 {
                takes.max(1.0) * RENDING_BONUS
            } else {
                takes
            }
        })
        .fold(f32::NEG_INFINITY, f32::max);
    if !worth.is_finite() {
        // No damage type recorded: judge it on its damage alone.
        worth = 1.0;
    }
    if item.imbued & imbue::ARMOR_RENDING != 0 || item.imbued & imbue::IGNORE_ALL_ARMOR != 0 {
        worth *= 1.0 + ARMOR_RENDING_BONUS;
    }
    if long_fight {
        if item.imbued & (imbue::CRITICAL_STRIKE | imbue::ALWAYS_CRITICAL) != 0 {
            worth *= 1.0 + CRIT_STRIKE_BONUS;
        }
        if item.imbued & imbue::CRIPPLING_BLOW != 0 {
            worth *= 1.0 + CRIPPLING_BONUS;
        }
    }
    worth * power(item) * wielder.skill_factor(item)
}

/// How hard the weapon hits, on a scale where an ordinary one is about
/// 1.0. A caster is judged by its elemental bonus, everything else by
/// its damage. An unappraised weapon has neither, and is worth 1.0 so it
/// is not ruled out before it has been looked at.
fn power(item: &ItemStats) -> f32 {
    if stance_of(item) == Some(Stance::Magic) {
        // A caster with no bonus reads as 0 until it is appraised.
        return if item.elemental_damage > 0.0 {
            item.elemental_damage
        } else {
            1.0
        };
    }
    let mid = (item.damage_low + item.damage_high) as f32 / 2.0;
    if mid <= 0.0 {
        return 1.0;
    }
    // A middling weapon does about twenty; keep the scale near 1.
    mid / 20.0
}

/// Words for why this weapon suits this target.
fn reason(item: &ItemStats, target: Option<&Creature>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let best = Element::from_bits(item.damage_type_bits)
        .into_iter()
        .max_by(|a, b| {
            let t = |e: &Element| target.map(|c| c.takes_from(*e)).unwrap_or(1.0);
            t(a).total_cmp(&t(b))
        });
    if let Some(e) = best {
        let takes = target.map(|c| c.takes_from(e));
        match takes {
            Some(t) => parts.push(format!("{} x{t:.2}", e.name())),
            None => parts.push(e.name().to_string()),
        }
        if item.imbued & e.rending() != 0 {
            parts.push("rending".into());
        }
    }
    let long_fight = target.is_some_and(|c| c.health >= LONG_FIGHT_HEALTH);
    if long_fight && item.imbued & imbue::CRITICAL_STRIKE != 0 {
        parts.push("critical strike".into());
    }
    if long_fight && item.imbued & imbue::CRIPPLING_BLOW != 0 {
        parts.push("crippling blow".into());
    }
    parts.join(", ")
}

/// The best of `carried` for the stance `want` against `target`.
///
/// Only weapons that give that stance are considered, so asking for a
/// bow never hands back a sword, and only ones the character may
/// actually hold: a wand that wants a War Magic it does not have is not
/// an option however good it would be. `None` when none is carried.
pub fn best(
    carried: &[ItemStats],
    want: Stance,
    target: Option<&Creature>,
    wielder: &Wielder,
) -> Option<Choice> {
    carried
        .iter()
        .filter(|i| stance_of(i) == Some(want))
        .filter(|i| wielder.can_wield(i))
        .map(|i| Choice {
            guid: i.guid,
            name: i.name.clone(),
            score: score(i, target, wielder),
            why: reason(i, target),
        })
        .reduce(|best, next| if next.score > best.score { next } else { best })
}

/// The best weapon carried, whichever way it is used, and the stance it
/// gives. For a character told to fight with whatever suits: a wand it
/// is skilled with beats a sword it is not, and the other way round.
pub fn best_any(
    carried: &[ItemStats],
    target: Option<&Creature>,
    wielder: &Wielder,
) -> Option<(Stance, Choice)> {
    [Stance::Melee, Stance::Missile, Stance::Magic]
        .into_iter()
        .filter_map(|s| best(carried, s, target, wielder).map(|c| (s, c)))
        .reduce(|best, next| {
            if next.1.score > best.1.score {
                next
            } else {
                best
            }
        })
}

/// The vulnerability worth casting on `target`: the element it is
/// weakest to, when it has health enough for the spell to pay for
/// itself. `None` for something that dies before the spell lands, or
/// that nothing is known about.
pub fn vulnerability_for(target: Option<&Creature>, least_health: u32) -> Option<Element> {
    let c = target?;
    if c.health < least_health {
        return None;
    }
    let weakest = c.weakest_to()?;
    // No point telling something it is weak to what it already shrugs
    // off less than everything else: only worth it if it is a real
    // weakness or at least not a resistance.
    (c.takes_from(weakest) > 0.0).then_some(weakest)
}

/// The spells that make a creature take more of `element`, as ids.
pub fn vulnerability_spells(element: Element) -> Vec<u32> {
    elements::vulnerabilities(element)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A character who can hold anything and is ordinary with it.
    fn able() -> Wielder {
        Wielder {
            level: 200,
            skills: Vec::new(),
            attributes: [300; 6],
            attributes_current: [300; 6],
            vitals: [1000; 3],
        }
    }

    fn weapon(kind: u32, low: u32, high: u32, damage_type: u32, imbued: u32) -> ItemStats {
        ItemStats {
            guid: 1,
            name: "Test".into(),
            item_type: kind,
            damage_low: low,
            damage_high: high,
            damage_type_bits: damage_type,
            imbued,
            appraised: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_fire_weapon_beats_a_frost_one_against_something_immune_to_cold() {
        use ac_world::item_type::MELEE_WEAPON;
        let ice = elements::creature_by_id(196).expect("Ice Golem");
        let mut fire = weapon(MELEE_WEAPON, 18, 22, Element::Fire as u32, 0);
        fire.guid = 10;
        fire.name = "Fire Sword".into();
        let mut frost = weapon(MELEE_WEAPON, 18, 22, Element::Cold as u32, 0);
        frost.guid = 11;
        frost.name = "Frost Sword".into();
        assert!(
            score(&frost, Some(ice), &able()) == 0.0,
            "cold does nothing to it"
        );
        assert!(score(&fire, Some(ice), &able()) > 0.0);
        let pick = best(
            &[frost.clone(), fire.clone()],
            Stance::Melee,
            Some(ice),
            &able(),
        )
        .unwrap();
        assert_eq!(pick.guid, 10, "the fire one");
        assert!(pick.why.contains("fire"), "{}", pick.why);
        // Order does not decide it.
        let pick = best(&[fire, frost], Stance::Melee, Some(ice), &able()).unwrap();
        assert_eq!(pick.guid, 10);
    }

    #[test]
    fn rending_the_element_beats_not_rending_it() {
        use ac_world::item_type::MISSILE_WEAPON;
        let ice = elements::creature_by_id(196).expect("Ice Golem");
        let plain = weapon(MISSILE_WEAPON, 18, 22, Element::Fire as u32, 0);
        let rending = weapon(
            MISSILE_WEAPON,
            18,
            22,
            Element::Fire as u32,
            imbue::FIRE_RENDING,
        );
        assert!(score(&rending, Some(ice), &able()) > score(&plain, Some(ice), &able()));
        assert!(reason(&rending, Some(ice)).contains("rending"));
        // Rending the wrong element is worth nothing extra.
        let wrong = weapon(
            MISSILE_WEAPON,
            18,
            22,
            Element::Fire as u32,
            imbue::COLD_RENDING,
        );
        assert_eq!(
            score(&wrong, Some(ice), &able()),
            score(&plain, Some(ice), &able())
        );
    }

    #[test]
    fn critical_strike_only_counts_in_a_long_fight() {
        use ac_world::item_type::CASTER;
        let skulker = elements::creature_by_id(7).expect("Drudge Skulker");
        assert!(skulker.health < LONG_FIGHT_HEALTH);
        let mut crit = weapon(CASTER, 0, 0, Element::Fire as u32, imbue::CRITICAL_STRIKE);
        crit.elemental_damage = 1.2;
        let mut plain = weapon(CASTER, 0, 0, Element::Fire as u32, 0);
        plain.elemental_damage = 1.2;
        assert_eq!(
            score(&crit, Some(skulker), &able()),
            score(&plain, Some(skulker), &able()),
            "a short fight does not pay for a better critical rate"
        );
        // Against something with health to spare it does.
        let tough = Creature {
            wcid: 0,
            name: "Tough".into(),
            health: 5000,
            takes: [Some(1.0); 8],
        };
        assert!(score(&crit, Some(&tough), &able()) > score(&plain, Some(&tough), &able()));
        assert!(reason(&crit, Some(&tough)).contains("critical strike"));
    }

    #[test]
    fn a_stance_is_only_offered_weapons_that_give_it() {
        use ac_world::item_type::{CASTER, MELEE_WEAPON, MISSILE_WEAPON};
        let mut sword = weapon(MELEE_WEAPON, 20, 30, Element::Slash as u32, 0);
        sword.guid = 1;
        let mut bow = weapon(MISSILE_WEAPON, 20, 30, Element::Pierce as u32, 0);
        bow.guid = 2;
        let mut wand = weapon(CASTER, 0, 0, Element::Fire as u32, 0);
        wand.guid = 3;
        let all = [sword, bow, wand];
        assert_eq!(stance_of(&all[0]), Some(Stance::Melee));
        assert_eq!(stance_of(&all[1]), Some(Stance::Missile));
        assert_eq!(stance_of(&all[2]), Some(Stance::Magic));
        for want in [Stance::Melee, Stance::Missile, Stance::Magic] {
            let pick = best(&all, want, None, &able()).expect("one of each is carried");
            let picked = all.iter().find(|i| i.guid == pick.guid);
            assert_eq!(picked.and_then(stance_of), Some(want));
            assert!(pick.score > 0.0, "{want:?} {pick:?}");
        }
        assert!(best(&[], Stance::Melee, None, &able()).is_none());
    }

    #[test]
    fn a_weapon_it_cannot_hold_is_not_an_option() {
        use ac_world::item_type::CASTER;
        use ac_world::wield;
        // The top wands ask for a base War Magic of 275 (skill id 34).
        let mut great = weapon(CASTER, 0, 0, Element::Fire as u32, 0);
        great.guid = 1;
        great.name = "Great Wand".into();
        great.elemental_damage = 2.0;
        great.wield_reqs = vec![(wield::RAW_SKILL, 34, 275)];
        let mut plain = weapon(CASTER, 0, 0, Element::Fire as u32, 0);
        plain.guid = 2;
        plain.name = "Plain Wand".into();
        plain.elemental_damage = 1.1;
        let carried = [great, plain];

        let novice = Wielder {
            level: 20,
            skills: vec![(34, 150, 150, 2)],
            attributes: [100; 6],
            attributes_current: [100; 6],
            vitals: [200; 3],
        };
        let pick = best(&carried, Stance::Magic, None, &novice).expect("the plain one");
        assert_eq!(pick.guid, 2, "the great wand is out of reach");
        assert!(!novice.can_wield(&carried[0]));

        let adept = Wielder {
            skills: vec![(34, 300, 300, 2)],
            ..novice.clone()
        };
        assert!(adept.can_wield(&carried[0]));
        // A buff does not satisfy a raw requirement, but does a plain one.
        let buffed = Wielder {
            skills: vec![(34, 250, 300, 2)],
            ..novice.clone()
        };
        assert!(
            !buffed.can_wield(&carried[0]),
            "275 raw asked, 250 raw held"
        );
        let plain = ItemStats {
            wield_reqs: vec![(wield::SKILL, 34, 275)],
            ..carried[0].clone()
        };
        assert!(
            buffed.can_wield(&plain),
            "275 buffed asked, 300 buffed held"
        );
        assert_eq!(best(&carried, Stance::Magic, None, &adept).unwrap().guid, 1);
        // A requirement kind we do not understand does not stop us.
        let odd = ItemStats {
            wield_reqs: vec![(99, 1, 9999)],
            ..carried[1].clone()
        };
        assert!(novice.can_wield(&odd));
    }

    #[test]
    fn what_the_character_is_good_at_decides_between_kinds() {
        use ac_world::item_type::{CASTER, MELEE_WEAPON};
        // Skill 34 is War Magic, 44 Heavy Weapons.
        let mut wand = weapon(CASTER, 0, 0, Element::Fire as u32, 0);
        wand.guid = 1;
        wand.elemental_damage = 1.0;
        wand.weapon_skill_id = 34;
        let mut sword = weapon(MELEE_WEAPON, 18, 22, Element::Fire as u32, 0);
        sword.guid = 2;
        sword.weapon_skill_id = 44;
        let carried = [wand, sword];

        let mage = Wielder {
            level: 100,
            skills: vec![(34, 400, 400, 3), (44, 60, 60, 1)],
            attributes: [200; 6],
            attributes_current: [200; 6],
            vitals: [500; 3],
        };
        let (stance, pick) = best_any(&carried, None, &mage).expect("something to hold");
        assert_eq!(stance, Stance::Magic, "{pick:?}");
        assert_eq!(pick.guid, 1);

        let swordsman = Wielder {
            skills: vec![(34, 40, 40, 1), (44, 400, 400, 3)],
            ..mage.clone()
        };
        let (stance, pick) = best_any(&carried, None, &swordsman).expect("something to hold");
        assert_eq!(stance, Stance::Melee, "{pick:?}");
        assert_eq!(pick.guid, 2);
        // A wand names no weapon skill, only the skill it asks for; that
        // stands in, so a caster is still judged by its school.
        let mut asking = carried[0].clone();
        asking.weapon_skill_id = 0;
        asking.wield_reqs = vec![(ac_world::wield::RAW_SKILL, 34, 275)];
        assert_eq!(skill_used(&asking), Some(34));
        assert!(mage.skill_factor(&asking) > 1.0);
        assert!(swordsman.skill_factor(&asking) < 1.0);
        // With no skills known at all, neither is favoured for skill.
        let unknown = Wielder::default();
        assert_eq!(unknown.skill_factor(&carried[0]), 1.0);
        assert_eq!(unknown.skill_factor(&carried[1]), 1.0);
    }

    #[test]
    fn a_vulnerability_is_only_worth_it_on_something_that_lasts() {
        let skulker = elements::creature_by_id(7).expect("Drudge Skulker");
        assert_eq!(vulnerability_for(Some(skulker), LONG_FIGHT_HEALTH), None);
        let tough = Creature {
            wcid: 0,
            name: "Tough".into(),
            health: 5000,
            // Weakest to fire.
            takes: [
                Some(0.5),
                Some(0.5),
                Some(0.5),
                Some(0.2),
                Some(1.6),
                Some(0.5),
                Some(0.5),
                None,
            ],
        };
        assert_eq!(
            vulnerability_for(Some(&tough), LONG_FIGHT_HEALTH),
            Some(Element::Fire)
        );
        assert!(!vulnerability_spells(Element::Fire).is_empty());
        assert_eq!(vulnerability_for(None, LONG_FIGHT_HEALTH), None);
    }
}
