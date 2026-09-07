//! The spells that move health, stamina and mana about.
//!
//! A caster does not wait for mana to come back on its own: it pours
//! stamina into mana with a transfer, and when the stamina runs low
//! refills it with Revitalize, which costs mana but gives back more
//! stamina than that mana made. Round and round; that is how a mage
//! keeps casting. Heal Self is the same kind of thing for health.
//!
//! Which spell does which is server data, so `data/spell_vitals.csv`
//! is a copy of it (see `reference/scripts/data/spell_vitals.sh`):
//! boosts, which restore a vital by an amount, and transfers, which
//! drain a share of one vital into another.

use std::sync::OnceLock;

/// The server's ids for the vitals themselves.
pub mod vital {
    pub const HEALTH: u32 = 2;
    pub const STAMINA: u32 = 4;
    pub const MANA: u32 = 6;

    pub fn name(id: u32) -> &'static str {
        match id {
            HEALTH => "health",
            STAMINA => "stamina",
            MANA => "mana",
            _ => "?",
        }
    }
}

/// Changes a vital by between `low` and `high` points. Negative is a
/// drain: Harm Self is a health boost below zero, and a client that
/// forgets the sign heals itself with it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Boost {
    pub spell: u32,
    /// The vital changed (see [`vital`]).
    pub vital: u32,
    pub low: i32,
    pub high: i32,
}

impl Boost {
    /// Gives the vital rather than taking it.
    pub fn restores(&self) -> bool {
        self.low > 0 && self.high > 0
    }
}

/// Drains `proportion` of `from` into `to`, less `loss` of it. A
/// negative loss is a gain: the top Stamina to Mana gives back half
/// again what it took.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transfer {
    pub spell: u32,
    pub from: u32,
    pub to: u32,
    pub proportion: f32,
    pub loss: f32,
    /// At most this many points moved, 0 for no cap.
    pub cap: u32,
}

impl Transfer {
    /// What `to` gains for `have` points of `from`.
    pub fn gain(&self, have: u32) -> u32 {
        let mut drained = have as f32 * self.proportion;
        if self.cap > 0 {
            drained = drained.min(self.cap as f32);
        }
        (drained * (1.0 - self.loss)).max(0.0).round() as u32
    }
}

const DATA: &str = include_str!("../data/spell_vitals.csv");

fn parse(text: &str) -> (Vec<Boost>, Vec<Transfer>) {
    let mut boosts = Vec::new();
    let mut transfers = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 8 {
            continue;
        }
        let num = |i: usize| f[i].trim().parse::<f32>().unwrap_or(0.0);
        let Ok(spell) = f[0].trim().parse::<u32>() else {
            continue;
        };
        match f[1].trim() {
            "boost" => boosts.push(Boost {
                spell,
                vital: num(2) as u32,
                low: num(3).round() as i32,
                high: num(4).round() as i32,
            }),
            "transfer" => transfers.push(Transfer {
                spell,
                from: num(2) as u32,
                to: num(3) as u32,
                proportion: num(5),
                loss: num(6),
                cap: num(7) as u32,
            }),
            _ => {}
        }
    }
    boosts.sort_by_key(|b| b.spell);
    transfers.sort_by_key(|t| t.spell);
    (boosts, transfers)
}

fn all() -> &'static (Vec<Boost>, Vec<Transfer>) {
    static ALL: OnceLock<(Vec<Boost>, Vec<Transfer>)> = OnceLock::new();
    ALL.get_or_init(|| parse(DATA))
}

pub fn boost(spell: u32) -> Option<Boost> {
    let b = &all().0;
    b.binary_search_by_key(&spell, |x| x.spell)
        .ok()
        .map(|i| b[i])
}

pub fn transfer(spell: u32) -> Option<Transfer> {
    let t = &all().1;
    t.binary_search_by_key(&spell, |x| x.spell)
        .ok()
        .map(|i| t[i])
}

/// Every transfer from one vital into another.
pub fn transfers_between(from: u32, to: u32) -> Vec<Transfer> {
    all()
        .1
        .iter()
        .filter(|t| t.from == from && t.to == to)
        .copied()
        .collect()
}

/// Every spell that restores a vital. The drains, Harm and Enfeeble
/// and their kin, are left out: they change the same vital and are
/// worth nothing to anyone wanting more of it.
pub fn boosts_of(vital: u32) -> Vec<Boost> {
    all()
        .0
        .iter()
        .filter(|b| b.vital == vital && b.restores())
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamina_goes_into_mana_and_revitalize_brings_it_back() {
        // Stamina to Mana Self VI: half the stamina, and half again on top.
        let t = transfer(1681).expect("in the table");
        assert_eq!((t.from, t.to), (vital::STAMINA, vital::MANA));
        assert_eq!(t.gain(100), 75);
        // Level I is capped and loses a tenth.
        let t1 = transfer(1676).expect("in the table");
        assert_eq!(t1.gain(200), 45, "50 drained, less a tenth");
        // Revitalize Self VI restores 80 to 160 stamina; a heal restores health.
        let r = boost(1182).expect("Revitalize Self VI");
        assert_eq!((r.vital, r.low, r.high), (vital::STAMINA, 80, 160));
        let h = boost(1161).expect("Heal Self VI");
        assert_eq!((h.vital, h.low, h.high), (vital::HEALTH, 55, 120));
        // Level seven has a name of its own; the table does not care.
        assert_eq!(
            boost(2073).map(|b| b.vital),
            Some(vital::HEALTH),
            "Adja's Intervention"
        );
        assert!(boosts_of(vital::STAMINA).iter().any(|b| b.spell == 1182));
        // Harm Self and Enfeeble Self are boosts below zero: drains, and
        // never offered to anyone trying to restore something.
        let harm = boost(8).expect("Harm Self I");
        assert!(harm.vital == vital::HEALTH && !harm.restores(), "{harm:?}");
        assert!(!boosts_of(vital::HEALTH).iter().any(|b| b.spell == 8));
        assert!(
            !boosts_of(vital::STAMINA).iter().any(|b| b.spell == 1190),
            "Enfeeble Self II"
        );
        assert!(!transfers_between(vital::STAMINA, vital::MANA).is_empty());
        assert!(
            boost(84).is_none() && transfer(84).is_none(),
            "a war bolt is neither"
        );
    }
}
