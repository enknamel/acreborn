//! How ammunition is made.
//!
//! Arrows, quarrels and atlatl darts come from using a bundle of heads
//! on a bundle of shafts. The head decides the element of what comes
//! out and the shaft decides what it fits, and the recipe asks a
//! Fletching skill of the maker. A character that has run out of the
//! ammunition it wants can make more on the spot if it carries the two
//! bundles, which is what an archer does between fights.
//!
//! The recipes are server data, so `data/fletching.csv` is a copy of
//! them (see `reference/scripts/data/fletching.sh`). Nothing is matched
//! by name: the element and the fit of the result are read from the
//! result's own weenie.

use std::sync::OnceLock;

use crate::elements::Element;

/// What ammunition fits: the launcher's and the ammunition's `AmmoType`
/// must agree.
pub mod ammo_type {
    pub const ARROW: u32 = 1;
    pub const BOLT: u32 = 2;
    pub const ATLATL: u32 = 4;

    pub fn name(t: u32) -> &'static str {
        match t {
            ARROW => "arrows",
            BOLT => "quarrels",
            ATLATL => "darts",
            _ => "ammunition",
        }
    }
}

/// The `CombatUse` word: what a thing is for in a fight.
pub mod combat_use {
    pub const MELEE: u32 = 1;
    /// A bow, crossbow or atlatl: shoots ammunition.
    pub const LAUNCHER: u32 = 2;
    pub const AMMO: u32 = 3;
    pub const SHIELD: u32 = 4;
    pub const TWO_HANDED: u32 = 5;
}

/// One recipe: `source` used on `target` makes `amount` of `result`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recipe {
    pub source: u32,
    pub target: u32,
    pub result: u32,
    pub amount: u32,
    /// The Fletching the recipe asks for.
    pub difficulty: u32,
    /// The result's damage type bit; see [`Element::from_bit`].
    pub element_bit: u32,
    /// What the result fits, see [`ammo_type`].
    pub fits: u32,
    pub source_name: String,
    pub target_name: String,
    pub result_name: String,
}

impl Recipe {
    pub fn element(&self) -> Option<Element> {
        Element::from_bit(self.element_bit)
    }
}

const DATA: &str = include_str!("../data/fletching.csv");

fn parse(text: &str) -> Vec<Recipe> {
    let mut out = Vec::with_capacity(320);
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 10 {
            continue;
        }
        let num = |i: usize| f[i].trim().parse::<u32>().unwrap_or(0);
        if num(0) == 0 || num(1) == 0 || num(2) == 0 {
            continue;
        }
        out.push(Recipe {
            source: num(0),
            target: num(1),
            result: num(2),
            amount: num(3),
            difficulty: num(4),
            element_bit: num(5),
            fits: num(6),
            source_name: f[7].replace(';', ","),
            target_name: f[8].replace(';', ","),
            result_name: f[9].replace(';', ","),
        });
    }
    out
}

pub fn all() -> &'static [Recipe] {
    static RECIPES: OnceLock<Vec<Recipe>> = OnceLock::new();
    RECIPES.get_or_init(|| parse(DATA))
}

/// The recipe for using `source` on `target`, if there is one.
pub fn recipe(source: u32, target: u32) -> Option<&'static Recipe> {
    all()
        .iter()
        .find(|r| r.source == source && r.target == target)
}

/// Every recipe whose result fits `fits` (see [`ammo_type`]).
pub fn making(fits: u32) -> impl Iterator<Item = &'static Recipe> {
    all().iter().filter(move |r| r.fits == fits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heads_on_shafts_make_arrows_of_the_heads_element() {
        // Bundle of Arrowheads on Bundle of Arrowshafts: a hundred arrows.
        let plain = recipe(4586, 4585).expect("plain arrows");
        assert_eq!(
            (plain.result, plain.amount, plain.fits),
            (300, 100, ammo_type::ARROW)
        );
        assert_eq!(plain.element(), Some(Element::Pierce));
        assert_eq!(plain.difficulty, 5);
        // Fire arrowheads on the same shafts: fire arrows, harder to make.
        let fire = recipe(5341, 4585).expect("fire arrows");
        assert_eq!(fire.element(), Some(Element::Fire));
        assert!(fire.difficulty > plain.difficulty);
        // The same heads on quarrel shafts fit a crossbow instead.
        assert!(making(ammo_type::BOLT).any(|r| r.source == 5341));
        assert!(all().len() > 300);
        assert!(
            recipe(4585, 4586).is_none(),
            "the other way round is nothing"
        );
    }
}
