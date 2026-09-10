//! What the server calls every property it can send when it identifies
//! an item.
//!
//! An identify is a bag of numbered properties: ints, longs, flags,
//! floats, text and data ids. A client that only understands the dozen
//! it happens to have a field for can only filter loot on those dozen,
//! and players filter on far more than that -- what set a piece of
//! armour belongs to, what a rare's icon sits on, how much of a
//! salvage bag is left, which quest a key belongs to. So the numbers
//! are kept as they arrive and this says what to call each one, which
//! is what turns a rule editor from a list of numbers into a list of
//! names.
//!
//! `data/properties.csv` is a copy of the names the emulator and the
//! community share for them (see `reference/scripts/data/properties.sh`).

use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Which bag of an identify a property lives in. The same number means
/// different things in different bags, so a property is only ever
/// (kind, id).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Int,
    Int64,
    Bool,
    Float,
    Text,
    DataId,
}

impl Kind {
    pub const ALL: [Kind; 6] = [
        Kind::Int,
        Kind::Int64,
        Kind::Bool,
        Kind::Float,
        Kind::Text,
        Kind::DataId,
    ];

    /// The word used in the data file and in a rule.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Int => "int",
            Kind::Int64 => "int64",
            Kind::Bool => "bool",
            Kind::Float => "float",
            Kind::Text => "string",
            Kind::DataId => "dataid",
        }
    }

    /// What a person would call it.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Int => "number",
            Kind::Int64 => "long number",
            Kind::Bool => "flag",
            Kind::Float => "decimal",
            Kind::Text => "text",
            Kind::DataId => "data id",
        }
    }

    pub fn parse(word: &str) -> Option<Kind> {
        Kind::ALL
            .into_iter()
            .find(|k| k.word().eq_ignore_ascii_case(word.trim()))
    }
}

const TABLE: &str = include_str!("../data/properties.csv");

type Names = BTreeMap<(Kind, u32), &'static str>;

fn table() -> &'static Names {
    static NAMES: OnceLock<Names> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut m = Names::new();
        for line in TABLE.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let mut f = line.splitn(3, ',');
            let (Some(kind), Some(id), Some(name)) = (f.next(), f.next(), f.next()) else {
                continue;
            };
            let (Some(kind), Ok(id)) = (Kind::parse(kind), id.trim().parse::<u32>()) else {
                continue;
            };
            m.insert((kind, id), name.trim());
        }
        m
    })
}

/// What this property is called, or `None` when the server has sent
/// one nobody has a name for. An unnamed property is still usable in a
/// rule -- it has a number -- it just reads as a number.
pub fn name_of(kind: Kind, id: u32) -> Option<&'static str> {
    table().get(&(kind, id)).copied()
}

/// The property with this name, if there is one. Names are unique
/// within a kind but not across kinds, so `Workmanship` is both an int
/// and a float and the caller says which it wants.
pub fn by_name(kind: Kind, name: &str) -> Option<u32> {
    let name = name.trim();
    table()
        .iter()
        .find(|((k, _), n)| *k == kind && n.eq_ignore_ascii_case(name))
        .map(|((_, id), _)| *id)
}

/// Every property of a kind, by number, for a list to choose from.
pub fn of_kind(kind: Kind) -> Vec<(u32, &'static str)> {
    table()
        .iter()
        .filter(|((k, _), _)| *k == kind)
        .map(|((_, id), n)| (*id, *n))
        .collect()
}

/// How many are known, for a sanity check.
pub fn count() -> usize {
    table().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_properties_an_identify_can_carry_are_all_named() {
        assert!(count() > 800, "properties: {}", count());
        // The ones a loot rule reaches for most.
        assert_eq!(name_of(Kind::Int, 19), Some("Value"));
        assert_eq!(name_of(Kind::Int, 105), Some("ItemWorkmanship"));
        assert_eq!(name_of(Kind::Int, 28), Some("ArmorLevel"));
        assert_eq!(name_of(Kind::Text, 1), Some("Name"));
        // Names go back the other way, for a rule that was written
        // down rather than clicked.
        assert_eq!(by_name(Kind::Int, "Value"), Some(19));
        assert_eq!(by_name(Kind::Int, "value"), Some(19));
        assert_eq!(by_name(Kind::Int, "NotAProperty"), None);
        // A number nobody has a name for is still a property.
        assert_eq!(name_of(Kind::Int, 99_999), None);
        // Every kind has some.
        for kind in Kind::ALL {
            assert!(!of_kind(kind).is_empty(), "{}", kind.word());
        }
        assert_eq!(Kind::parse("dataid"), Some(Kind::DataId));
        assert_eq!(Kind::parse("nonsense"), None);
    }
}
