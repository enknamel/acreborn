//! What to hit a creature with.
//!
//! Every creature takes more damage from some kinds of attack than
//! others, and the difference is large: an Ice Golem takes nothing at
//! all from cold and full damage from fire, and a Frost Golem is barely
//! scratched by a pierce weapon. The figures are per creature and per
//! damage type, and they are server data, so `data/creatures.csv` is a
//! copy of them (see `reference/scripts/data/creatures.sh`).
//!
//! Creatures are keyed by their weenie class id, which the server puts
//! in every object description, so a creature in view is matched
//! exactly and never by guesswork. Names are kept as well, because the
//! rules a player writes are in names, and because a creature whose
//! weenie is not in this copy of the data can still often be recognised
//! by what it is called.
//!
//! Spells need the same treatment from the other side: the client's own
//! SpellTable does not say what a spell hits with, so
//! `data/spell_elements.csv` maps a spell id to its element, both for
//! the spells that deal it and for the ones that make a target take
//! more of it.
//!
//! Weapons carry an element too, and some are imbued to rend a
//! creature's resistance to it. A fire weapon against something weak to
//! fire is worth several times one that is not, so [`Imbue`] and
//! [`Element::rending`] are here as well, for the code that chooses
//! what to wield.
//!
//! Between the two, [`best_spell`] answers the question that matters
//! while fighting: of the spells I am willing to throw, which one hurts
//! this thing most?

use std::sync::OnceLock;

/// A kind of damage. The values are the game's own `DamageType` bits, so
/// they can be compared with what the server sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Element {
    Slash = 1,
    Pierce = 2,
    Bludgeon = 4,
    Cold = 8,
    Fire = 16,
    Acid = 32,
    Electric = 64,
    Nether = 1024,
}

/// The eight in the order the tables store them.
pub const ALL: [Element; 8] = [
    Element::Slash,
    Element::Pierce,
    Element::Bludgeon,
    Element::Cold,
    Element::Fire,
    Element::Acid,
    Element::Electric,
    Element::Nether,
];

impl Element {
    pub fn name(self) -> &'static str {
        match self {
            Element::Slash => "slash",
            Element::Pierce => "pierce",
            Element::Bludgeon => "bludgeon",
            Element::Cold => "cold",
            Element::Fire => "fire",
            Element::Acid => "acid",
            Element::Electric => "electric",
            Element::Nether => "nether",
        }
    }

    /// From a `DamageType` bit. `None` for the ones that are not a kind
    /// of damage a creature resists (health, stamina and mana drains).
    pub fn from_bit(bit: u32) -> Option<Element> {
        ALL.into_iter().find(|e| *e as u32 == bit)
    }

    /// Where this element sits in a [`Creature`]'s table.
    fn index(self) -> usize {
        ALL.iter().position(|e| *e == self).unwrap_or(0)
    }

    /// The imbued effect that rends this element: a weapon carrying it
    /// strips some of the target's resistance to it.
    pub fn rending(self) -> u32 {
        match self {
            Element::Slash => imbue::SLASH_RENDING,
            Element::Pierce => imbue::PIERCE_RENDING,
            Element::Bludgeon => imbue::BLUDGEON_RENDING,
            Element::Cold => imbue::COLD_RENDING,
            Element::Fire => imbue::FIRE_RENDING,
            Element::Acid => imbue::ACID_RENDING,
            Element::Electric => imbue::ELECTRIC_RENDING,
            Element::Nether => imbue::NETHER_RENDING,
        }
    }

    /// The elements named in a damage-type word, which may name more
    /// than one (a weapon that does both fire and slashing).
    pub fn from_bits(bits: u32) -> Vec<Element> {
        ALL.into_iter().filter(|e| bits & *e as u32 != 0).collect()
    }
}

/// What a weapon has been imbued with: the `ImbuedEffect` word an
/// appraisal reports.
pub mod imbue {
    /// Criticals land far more often. What to bring to something with a
    /// great deal of health, where the extra chance has time to tell.
    pub const CRITICAL_STRIKE: u32 = 0x0001;
    /// Criticals hurt far more.
    pub const CRIPPLING_BLOW: u32 = 0x0002;
    /// Strips some of the target's armour.
    pub const ARMOR_RENDING: u32 = 0x0004;
    pub const SLASH_RENDING: u32 = 0x0008;
    pub const PIERCE_RENDING: u32 = 0x0010;
    pub const BLUDGEON_RENDING: u32 = 0x0020;
    pub const ACID_RENDING: u32 = 0x0040;
    pub const COLD_RENDING: u32 = 0x0080;
    pub const ELECTRIC_RENDING: u32 = 0x0100;
    pub const FIRE_RENDING: u32 = 0x0200;
    pub const NETHER_RENDING: u32 = 0x4000;
    pub const ALWAYS_CRITICAL: u32 = 0x4000_0000;
    pub const IGNORE_ALL_ARMOR: u32 = 0x8000_0000;

    /// Every rending bit, whatever the element.
    pub const ANY_RENDING: u32 = SLASH_RENDING
        | PIERCE_RENDING
        | BLUDGEON_RENDING
        | ACID_RENDING
        | COLD_RENDING
        | ELECTRIC_RENDING
        | FIRE_RENDING
        | NETHER_RENDING;
}

/// How much damage one kind of creature takes from each element.
#[derive(Clone, Debug, PartialEq)]
pub struct Creature {
    /// The weenie class id: what the server calls this kind of thing.
    pub wcid: u32,
    pub name: String,
    /// How much health it has at full, 0 when not recorded. What tells
    /// something worth spending a vulnerability on from something that
    /// dies before the spell lands.
    pub health: u32,
    /// A multiplier per element, in [`ALL`] order. Higher means it is
    /// hurt more; 0 means immune. `None` where the creature has no
    /// figure for that element at all.
    pub takes: [Option<f32>; 8],
}

impl Creature {
    /// How much damage this creature takes from `element`, or 1.0 when
    /// nothing is recorded (the neutral figure the game itself uses).
    pub fn takes_from(&self, element: Element) -> f32 {
        self.takes[element.index()].unwrap_or(1.0)
    }

    /// The element this creature is hurt most by, of those it has a
    /// figure for. `None` when it has none at all.
    pub fn weakest_to(&self) -> Option<Element> {
        ALL.into_iter()
            .filter(|e| self.takes[e.index()].is_some())
            .max_by(|a, b| self.takes_from(*a).total_cmp(&self.takes_from(*b)))
    }
}

const CREATURES: &str = include_str!("../data/creatures.csv");
const SPELLS: &str = include_str!("../data/spell_elements.csv");

fn parse_creatures(text: &str) -> Vec<Creature> {
    let mut out = Vec::with_capacity(6500);
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let mut f = line.split(',');
        let (Some(wcid), Some(name), Some(health)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let (Ok(wcid), false) = (wcid.trim().parse::<u32>(), name.is_empty()) else {
            continue;
        };
        let health = health.trim().parse::<u32>().unwrap_or(0);
        let mut takes = [None; 8];
        for slot in takes.iter_mut() {
            *slot = f.next().and_then(|v| v.trim().parse::<f32>().ok());
        }
        out.push(Creature {
            wcid,
            name: name.replace(';', ","),
            health,
            takes,
        });
    }
    out.sort_by_key(|c| c.wcid);
    out
}

/// What a spell does about an element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Deals damage of that element.
    Attack,
    /// Makes a target take more of that element.
    Vulnerability,
}

fn parse_spells(text: &str) -> Vec<(u32, Role, Element)> {
    let mut out = Vec::with_capacity(900);
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let mut f = line.split(',');
        let (Some(id), Some(role), Some(bit)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let (Ok(id), Ok(bit)) = (id.trim().parse::<u32>(), bit.trim().parse::<u32>()) else {
            continue;
        };
        let role = match role.trim() {
            "attack" => Role::Attack,
            "vuln" => Role::Vulnerability,
            _ => continue,
        };
        // Health, stamina and mana drains are not an element anything
        // is weak to, so they are left out.
        if let Some(e) = Element::from_bit(bit) {
            out.push((id, role, e));
        }
    }
    out.sort_by_key(|(id, _, _)| *id);
    out
}

pub fn creatures() -> &'static [Creature] {
    static ALL_CREATURES: OnceLock<Vec<Creature>> = OnceLock::new();
    ALL_CREATURES.get_or_init(|| parse_creatures(CREATURES))
}

fn spells() -> &'static [(u32, Role, Element)] {
    static ALL_SPELLS: OnceLock<Vec<(u32, Role, Element)>> = OnceLock::new();
    ALL_SPELLS.get_or_init(|| parse_spells(SPELLS))
}

/// What is known about the kind of creature the server calls `wcid`.
/// This is the exact answer; [`creature`] is the guess to fall back on.
pub fn creature_by_id(wcid: u32) -> Option<&'static Creature> {
    let all = creatures();
    all.binary_search_by_key(&wcid, |c| c.wcid)
        .ok()
        .map(|i| &all[i])
}

/// What is known about a creature of this name, for when the weenie is
/// not in this copy of the data.
///
/// Names in the world often carry a title or a rank in front of the
/// kind of thing it is, so an exact match is tried first and then the
/// longest recorded name the given one ends with: "Drudge Skulker" is
/// found at the end of "Weakened Drudge Skulker". Only a whole word
/// counts, or "Nothing In Particular" would be the creature called
/// "Nothing".
pub fn creature(name: &str) -> Option<&'static Creature> {
    let all = creatures();
    let lower = name.trim().to_lowercase();
    let mut exact = None;
    let mut suffix: Option<&'static Creature> = None;
    for c in all {
        let known = c.name.to_lowercase();
        if known == lower {
            exact = Some(c);
            break;
        }
        if lower.len() > known.len()
            && lower.ends_with(&format!(" {known}"))
            && suffix.is_none_or(|s| s.name.len() < c.name.len())
        {
            suffix = Some(c);
        }
    }
    exact.or(suffix)
}

/// What a spell hits with, if it hits with anything.
pub fn spell_element(spell_id: u32) -> Option<Element> {
    match spell_role(spell_id) {
        Some((Role::Attack, e)) => Some(e),
        _ => None,
    }
}

/// What a spell does about an element, and which.
pub fn spell_role(spell_id: u32) -> Option<(Role, Element)> {
    let all = spells();
    all.binary_search_by_key(&spell_id, |(id, _, _)| *id)
        .ok()
        .map(|i| (all[i].1, all[i].2))
}

/// Every spell that makes a target take more of `element`, at every
/// level. The caller picks the strongest it can cast: the level is in
/// the client's own SpellTable, not here.
pub fn vulnerabilities(element: Element) -> Vec<u32> {
    spells()
        .iter()
        .filter(|(_, r, e)| *r == Role::Vulnerability && *e == element)
        .map(|(id, _, _)| *id)
        .collect()
}

/// What is known about a creature the server has told us about: its
/// weenie class id if that is recorded, else its name.
pub fn known(wcid: u32, name: &str) -> Option<&'static Creature> {
    creature_by_id(wcid).or_else(|| creature(name))
}

/// Of `candidates` (spell ids), the one that hurts this creature most,
/// and how much it is multiplied by.
///
/// Spells whose element is unknown are worth the neutral 1.0, so a
/// spell nothing is recorded about is still thrown when it is the only
/// one on offer. Ties keep the order given, which is the order the
/// player put them in.
pub fn best_spell(wcid: u32, name: &str, candidates: &[u32]) -> Option<(u32, f32)> {
    let creature = known(wcid, name);
    let worth = |id: u32| match (creature, spell_element(id)) {
        (Some(c), Some(e)) => c.takes_from(e),
        _ => 1.0,
    };
    candidates
        .iter()
        .map(|&id| (id, worth(id)))
        .reduce(|best, next| if next.1 > best.1 { next } else { best })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_golem_of_ice_is_fought_with_fire() {
        // Weenie 196 is the Ice Golem; the id is the exact key.
        let ice = creature_by_id(196).expect("Ice Golem is in the table");
        assert_eq!(ice.name, "Ice Golem");
        assert_eq!(ice.health, 95);
        assert_eq!(creature("Ice Golem").map(|c| c.wcid), Some(196));
        assert_eq!(ice.takes_from(Element::Cold), 0.0, "immune to cold");
        assert_eq!(ice.takes_from(Element::Fire), 1.0);
        assert_eq!(ice.weakest_to(), Some(Element::Fire));
        // A title in front of the name still finds it.
        assert_eq!(
            creature("Enraged Ice Golem").map(|c| c.name.as_str()),
            Some("Ice Golem")
        );
        // Nothing at all is not an error.
        assert!(creature("Nothing In Particular").is_none());
    }

    #[test]
    fn spells_know_what_they_hit_with() {
        // Flame Bolt V, Frost Bolt V, Lightning Bolt V, Whirling Blade V.
        assert_eq!(spell_element(84), Some(Element::Fire));
        assert_eq!(spell_element(73), Some(Element::Cold));
        assert_eq!(spell_element(79), Some(Element::Electric));
        assert_eq!(spell_element(96), Some(Element::Slash));
        // A heal is not an element.
        assert_eq!(spell_element(1160), None);
        // A vulnerability is about an element but does not deal it.
        assert_eq!(spell_element(1065), None);
        assert_eq!(
            spell_role(1065),
            Some((Role::Vulnerability, Element::Cold)),
            "Cold Vulnerability Other VI"
        );
        let colds = vulnerabilities(Element::Cold);
        assert!(colds.contains(&1065), "{colds:?}");
        assert!(colds.iter().all(|id| spell_element(*id).is_none()));
        assert!(!vulnerabilities(Element::Fire).is_empty());
    }

    #[test]
    fn the_best_spell_is_the_one_it_is_weakest_to() {
        // Frost Bolt and Flame Bolt against something immune to cold.
        let (id, worth) = best_spell(196, "Ice Golem", &[73, 84]).expect("one of the two");
        assert_eq!(id, 84, "fire, not frost");
        assert_eq!(worth, 1.0);
        // Order does not decide it: frost is still refused first.
        assert_eq!(
            best_spell(196, "Ice Golem", &[84, 73]).map(|(i, _)| i),
            Some(84)
        );
        // A creature nothing is known about keeps the order given.
        assert_eq!(
            best_spell(0, "Nothing In Particular", &[73, 84]).map(|(i, _)| i),
            Some(73)
        );
        assert_eq!(best_spell(196, "Ice Golem", &[]), None);
    }

    #[test]
    fn every_creature_in_the_table_reads_back() {
        let all = creatures();
        assert!(all.len() > 6000, "{} creatures", all.len());
        assert!(
            all.windows(2).all(|w| w[0].wcid < w[1].wcid),
            "sorted by id"
        );
        assert!(all.iter().all(|c| c.takes.iter().any(|t| t.is_some())));
        let spells = spells();
        assert!(spells.len() > 800, "{} spells", spells.len());
        assert!(spells.windows(2).all(|w| w[0].0 <= w[1].0), "sorted");
        // Rending bits and elements line up both ways.
        for e in ALL {
            assert_eq!(Element::from_bit(e as u32), Some(e));
            assert!(e.rending() & imbue::ANY_RENDING != 0 || e == Element::Nether);
        }
        assert_eq!(
            Element::from_bits(0x11),
            vec![Element::Slash, Element::Fire]
        );
    }
}
