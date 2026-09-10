//! Portal gems: what a gem in the pack does when it is used.
//!
//! A gem summons a portal to one fixed place. For getting somewhere
//! that makes it a hop straight there, the same shape as a recall
//! spell, with two differences: a gem is spent when used, and a
//! character can only use one it is actually carrying.
//!
//! Like the portals and the recalls these are server-side, so
//! `data/gems.csv` is a copy of the list (see
//! `reference/scripts/data/gems.sh`). The gem's weenie links to a
//! portal and that portal carries the destination; the table records
//! where a character ends up, not the plumbing.

use glam::{Vec2, Vec3};
use std::sync::OnceLock;

/// Summon Portal: the spell nearly every gem casts. It does not move
/// the character, it puts a portal in front of them.
pub const SUMMON_PORTAL: u32 = 157;

/// What using a gem actually does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    /// It summons a portal, which then has to be walked into. Two
    /// actions, and the portal stands for a while.
    Summons,
    /// It moves the character itself, like a recall.
    Teleports,
}

/// A portal gem and where using one puts a character.
#[derive(Clone, Debug, PartialEq)]
pub struct Gem {
    /// The item's weenie class, which is what a carried item says it is.
    pub wcid: u32,
    pub name: String,
    pub cell: u32,
    /// World position of the exit.
    pub at: Vec3,
    /// The spell it casts.
    pub spell: u32,
}

impl Gem {
    /// Whether using it summons a portal or moves the character.
    pub fn how(&self) -> Use {
        if self.spell == SUMMON_PORTAL {
            Use::Summons
        } else {
            Use::Teleports
        }
    }
}

impl Gem {
    pub fn xy(&self) -> Vec2 {
        self.at.truncate()
    }
}

const DATA: &str = include_str!("../data/gems.csv");

fn parse(text: &str) -> Vec<Gem> {
    let mut out = Vec::with_capacity(500);
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(',').collect();
        if f.len() != 7 {
            continue;
        }
        let (Ok(wcid), Ok(cell)) = (f[0].parse::<u32>(), u32::from_str_radix(f[2], 16)) else {
            continue;
        };
        let num = |s: &str| s.parse::<f32>().ok();
        let (Some(x), Some(y), Some(z)) = (num(f[3]), num(f[4]), num(f[5])) else {
            continue;
        };
        out.push(Gem {
            wcid,
            name: f[1].to_string(),
            cell,
            at: crate::landblock_origin(cell) + Vec3::new(x, y, z),
            spell: f[6].trim().parse().unwrap_or(SUMMON_PORTAL),
        });
    }
    out.sort_by_key(|g| g.wcid);
    out
}

/// Every gem, parsed on first use.
pub fn all() -> &'static [Gem] {
    static GEMS: OnceLock<Vec<Gem>> = OnceLock::new();
    GEMS.get_or_init(|| parse(DATA))
}

/// The gem with this weenie class, if it is one.
pub fn of(wcid: u32) -> Option<&'static Gem> {
    let g = all();
    g.binary_search_by_key(&wcid, |x| x.wcid)
        .ok()
        .map(|i| &g[i])
}

/// Gems whose name contains `needle`, nearest destination to `from`
/// first: what a player types to find one worth buying.
pub fn search(needle: &str, from: Vec2) -> Vec<&'static Gem> {
    let needle = needle.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut v: Vec<&Gem> = all()
        .iter()
        .filter(|g| g.name.to_lowercase().contains(&needle))
        .collect();
    v.sort_by(|a, b| a.xy().distance(from).total_cmp(&b.xy().distance(from)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_reads_and_places_the_gems() {
        let all = all();
        assert!(all.len() > 400, "{} gems", all.len());
        assert!(all.iter().all(|g| !g.name.is_empty() && g.cell != 0));
        // Sorted, so the lookup below is sound.
        assert!(all.windows(2).all(|w| w[0].wcid < w[1].wcid));
    }

    #[test]
    fn a_gem_is_found_by_what_it_is() {
        // The Al-Arqas gem links to the Al-Arqas portal.
        let g = search("Al-Arqas Portal Gem", Vec2::ZERO);
        assert!(!g.is_empty(), "no Al-Arqas gem");
        let found = of(g[0].wcid).expect("looked up by wcid");
        assert_eq!(found.name, g[0].name);
        // Something that is not a gem is not one.
        assert!(of(1).is_none());
        assert!(search("", Vec2::ZERO).is_empty());
    }

    #[test]
    fn most_gems_summon_a_portal_rather_than_moving_you() {
        // The difference matters: a summoned portal has to be walked
        // into, which is a second action and more time.
        let summons = all().iter().filter(|g| g.how() == Use::Summons).count();
        assert!(summons > 400, "only {summons} summon");
        // And the handful that do not are told apart by their spell.
        assert!(all().iter().any(|g| g.how() == Use::Teleports));
    }

    #[test]
    fn bad_lines_are_skipped() {
        let v = parse("# c\n\nnope,x\n7,A Gem,A9B40019,1.0,2.0,3.0,157\nx,Bad,ZZ,1,2,3,4\n");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].wcid, 7);
        assert_eq!(v[0].name, "A Gem");
    }
}
