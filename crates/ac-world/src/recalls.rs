//! Recall spells: the ones that carry the caster to a fixed place
//! rather than to whatever they point at. Two kinds:
//!
//! * The named recalls a character learns from a quest (Aerlinthe
//!   Recall, Ulgrim's Recall, Lyceum Recall...) and the sendings cast by
//!   gems and NPCs. Where each one lands is server data, not in the
//!   client's own files; `data/recalls.csv` is a copy (see
//!   `reference/scripts/data/recalls.sh`). [`fixed`] looks a spell up.
//!
//! * The five whose destination is the character's own: Lifestone
//!   Sending goes to the lifestone last used, Lifestone Recall to the
//!   one tied with Lifestone Tie, Portal Recall to where the last portal
//!   walked through led, and Primary and Secondary Portal Recall to
//!   where the two tied portals lead. The server keeps those positions
//!   (its `PositionType` table) and never sends them, so the client
//!   learns them by watching what the character does; [`dynamic`] says
//!   which saved position each spell uses.

use glam::{Vec2, Vec3};
use std::sync::OnceLock;

/// The server's `PositionType` word for each position it saves per
/// character. Only the ones a recall uses, and the one the server
/// does send (where the last corpse fell).
pub mod position_type {
    /// The lifestone last used ("attuned").
    pub const SANCTUARY: u32 = 4;
    /// Where the portal tied with Primary Portal Tie leads.
    pub const LINKED_PORTAL_ONE: u32 = 8;
    /// Where the last portal walked through leads.
    pub const LAST_PORTAL: u32 = 9;
    /// Where the character last died outdoors (the corpse).
    pub const LAST_OUTSIDE_DEATH: u32 = 14;
    /// The lifestone tied with Lifestone Tie.
    pub const LINKED_LIFESTONE: u32 = 15;
    /// Where the portal tied with Secondary Portal Tie leads.
    pub const LINKED_PORTAL_TWO: u32 = 16;
}

/// Spell ids of the recalls whose destination is the character's own,
/// and of the ties that set those destinations.
pub mod spell {
    pub const PRIMARY_PORTAL_TIE: u32 = 47;
    pub const PRIMARY_PORTAL_RECALL: u32 = 48;
    /// Not a spell at all: the `/lifestone` command, which every
    /// character has whatever its skills. The server takes it as the
    /// `TeleToLifestone` action and asks only that a lifestone has been
    /// attuned -- no magic, no components, half the mana.
    ///
    /// It is given a number here so that a journey can be planned with
    /// it beside the spells, and the number is one no spell uses.
    pub const FREE_LIFESTONE: u32 = 0xF000_0001;
    /// The same for `/marketplace`.
    pub const FREE_MARKETPLACE: u32 = 0xF000_0002;

    /// Whether this is one of the commands rather than a spell, and so
    /// is sent as an action rather than cast.
    pub fn is_free(spell: u32) -> bool {
        spell == FREE_LIFESTONE || spell == FREE_MARKETPLACE
    }

    pub const LIFESTONE_RECALL: u32 = 1635;
    pub const LIFESTONE_SENDING: u32 = 1636;
    pub const LIFESTONE_TIE: u32 = 2644;
    pub const PORTAL_RECALL: u32 = 2645;
    pub const SECONDARY_PORTAL_TIE: u32 = 2646;
    pub const SECONDARY_PORTAL_RECALL: u32 = 2647;
}

/// Which of the character's saved positions a recall spell goes to.
/// `None` for a spell that is not one of the five.
pub fn dynamic(spell_id: u32) -> Option<u32> {
    use position_type::*;
    Some(match spell_id {
        spell::LIFESTONE_SENDING => SANCTUARY,
        spell::LIFESTONE_RECALL => LINKED_LIFESTONE,
        spell::PORTAL_RECALL => LAST_PORTAL,
        spell::PRIMARY_PORTAL_RECALL => LINKED_PORTAL_ONE,
        spell::SECONDARY_PORTAL_RECALL => LINKED_PORTAL_TWO,
        _ => return None,
    })
}

/// The saved position a tie spell sets, so a "successfully linked"
/// after casting it can be filed under the right one.
pub fn tie_sets(spell_id: u32) -> Option<u32> {
    use position_type::*;
    Some(match spell_id {
        spell::LIFESTONE_TIE => LINKED_LIFESTONE,
        spell::PRIMARY_PORTAL_TIE => LINKED_PORTAL_ONE,
        spell::SECONDARY_PORTAL_TIE => LINKED_PORTAL_TWO,
        _ => return None,
    })
}

/// A spell that lands its target at a fixed place.
#[derive(Clone, Debug, PartialEq)]
pub struct Recall {
    pub spell_id: u32,
    pub name: String,
    /// The cell it lands in, and that world position.
    pub cell: u32,
    pub at: Vec3,
}

impl Recall {
    pub fn xy(&self) -> Vec2 {
        self.at.truncate()
    }

    /// Whether it lands outdoors.
    pub fn outdoors(&self) -> bool {
        self.cell & 0xFFFF < 0x100
    }
}

const DATA: &str = include_str!("../data/recalls.csv");

fn parse(text: &str) -> Vec<Recall> {
    let mut out = Vec::with_capacity(600);
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(',').collect();
        if f.len() != 6 {
            continue;
        }
        let (Ok(spell_id), Ok(cell)) = (f[0].parse::<u32>(), u32::from_str_radix(f[2], 16)) else {
            continue;
        };
        let num = |s: &str| s.parse::<f32>().ok();
        let (Some(x), Some(y), Some(z)) = (num(f[3]), num(f[4]), num(f[5])) else {
            continue;
        };
        out.push(Recall {
            spell_id,
            name: f[1].to_string(),
            cell,
            at: crate::landblock_origin(cell) + Vec3::new(x, y, z),
        });
    }
    out
}

/// Every fixed-destination spell, parsed on first use, in spell id order.
pub fn all() -> &'static [Recall] {
    static RECALLS: OnceLock<Vec<Recall>> = OnceLock::new();
    RECALLS.get_or_init(|| parse(DATA))
}

/// Where a spell lands its target, if it is one that goes somewhere
/// fixed.
pub fn fixed(spell_id: u32) -> Option<&'static Recall> {
    let all = all();
    all.binary_search_by_key(&spell_id, |r| r.spell_id)
        .ok()
        .map(|i| &all[i])
}

/// Whether a spell is a recall of either kind: one the trip planner
/// can use, given somewhere for it to go.
pub fn is_recall(spell_id: u32) -> bool {
    dynamic(spell_id).is_some() || fixed(spell_id).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_reads_and_finds_recalls() {
        let all = all();
        assert!(all.len() > 500, "{} recalls", all.len());
        assert!(all.windows(2).all(|w| w[0].spell_id < w[1].spell_id));
        // Aerlinthe Recall lands on Aerlinthe island, outdoors.
        let a = fixed(2041).expect("Aerlinthe Recall");
        assert_eq!(a.name, "Aerlinthe Recall");
        assert_eq!(a.cell, 0xBAE8_001D);
        assert!(a.outdoors());
        assert!((a.at.x - (0xBA as f32 * 192.0 + 84.0)).abs() < 1e-3);
        // Expulsion (cast by a dungeon's NPC) lands inside it.
        let l = fixed(2369).expect("Expulsion");
        assert!(!l.outdoors(), "{:#010x}", l.cell);
        assert!(fixed(1).is_none());
        assert!(is_recall(2041));
        assert!(is_recall(spell::LIFESTONE_SENDING));
        assert!(!is_recall(1));
    }

    #[test]
    fn dynamic_recalls_name_their_positions() {
        assert_eq!(
            dynamic(spell::LIFESTONE_SENDING),
            Some(position_type::SANCTUARY)
        );
        assert_eq!(
            dynamic(spell::PRIMARY_PORTAL_RECALL),
            Some(position_type::LINKED_PORTAL_ONE)
        );
        assert_eq!(
            dynamic(spell::PORTAL_RECALL),
            Some(position_type::LAST_PORTAL)
        );
        assert_eq!(dynamic(2041), None);
        assert_eq!(
            tie_sets(spell::SECONDARY_PORTAL_TIE),
            Some(position_type::LINKED_PORTAL_TWO)
        );
        assert_eq!(tie_sets(spell::PORTAL_RECALL), None);
    }

    #[test]
    fn bad_lines_are_skipped() {
        let v = parse("# c\n\nnope,x\n2041,Aerlinthe Recall,BAE8001D,84,105,26\nx,X,1,1,1,1\n");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].spell_id, 2041);
    }
}
