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
//! `data/spell_elements.csv` maps a spell id to its element.
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
}

/// How much damage one kind of creature takes from each element.
#[derive(Clone, Debug, PartialEq)]
pub struct Creature {
    /// The weenie class id: what the server calls this kind of thing.
    pub wcid: u32,
    pub name: String,
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
        let (Some(wcid), Some(name)) = (f.next(), f.next()) else {
            continue;
        };
        let (Ok(wcid), false) = (wcid.trim().parse::<u32>(), name.is_empty()) else {
            continue;
        };
        let mut takes = [None; 8];
        for slot in takes.iter_mut() {
            *slot = f.next().and_then(|v| v.trim().parse::<f32>().ok());
        }
        out.push(Creature {
            wcid,
            name: name.replace(';', ","),
            takes,
        });
    }
    out.sort_by_key(|c| c.wcid);
    out
}

fn parse_spells(text: &str) -> Vec<(u32, Element)> {
    let mut out = Vec::with_capacity(800);
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let mut f = line.split(',');
        let (Some(id), Some(bit)) = (f.next(), f.next()) else {
            continue;
        };
        let (Ok(id), Ok(bit)) = (id.trim().parse::<u32>(), bit.trim().parse::<u32>()) else {
            continue;
        };
        // Health, stamina and mana drains are not an element anything
        // is weak to, so they are left out.
        if let Some(e) = Element::from_bit(bit) {
            out.push((id, e));
        }
    }
    out.sort_by_key(|(id, _)| *id);
    out
}

pub fn creatures() -> &'static [Creature] {
    static ALL_CREATURES: OnceLock<Vec<Creature>> = OnceLock::new();
    ALL_CREATURES.get_or_init(|| parse_creatures(CREATURES))
}

fn spells() -> &'static [(u32, Element)] {
    static ALL_SPELLS: OnceLock<Vec<(u32, Element)>> = OnceLock::new();
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
    let all = spells();
    all.binary_search_by_key(&spell_id, |(id, _)| *id)
        .ok()
        .map(|i| all[i].1)
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
        assert!(spells.len() > 700, "{} spells", spells.len());
        assert!(spells.windows(2).all(|w| w[0].0 <= w[1].0), "sorted");
    }
}
