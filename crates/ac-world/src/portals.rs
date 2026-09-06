//! Every portal in the world: where it stands and where it leads.
//!
//! Portals are server-side objects, so unlike the landscape they are not
//! in the client's own data files. `data/portals.csv` is a copy of the
//! list (see `reference/scripts/data/portals.sh` for how it is made and
//! where it comes from); it is parsed once on first use.
//!
//! A trip planner uses this to travel further than a walk: the Town
//! Network from a town to the hub and out again, a dungeon's "Surface"
//! portal, a town's own portal. [`near`] finds the portals whose mouth
//! is within reach of a spot, and [`to_place`] the ones that come out
//! near somewhere.

use glam::{Vec2, Vec3};
use std::sync::OnceLock;

/// One portal: its mouth (where the character walks in) and its exit.
#[derive(Clone, Debug, PartialEq)]
pub struct Portal {
    pub name: String,
    /// Cell the mouth stands in, and its world position.
    pub from_cell: u32,
    pub from: Vec3,
    /// Cell it comes out in, and that world position.
    pub to_cell: u32,
    pub to: Vec3,
}

impl Portal {
    pub fn from_xy(&self) -> Vec2 {
        self.from.truncate()
    }

    pub fn to_xy(&self) -> Vec2 {
        self.to.truncate()
    }

    /// Whether the mouth is outdoors (a walk can reach it).
    pub fn mouth_outdoors(&self) -> bool {
        self.from_cell & 0xFFFF < 0x100
    }

    /// Whether it comes out outdoors.
    pub fn exit_outdoors(&self) -> bool {
        self.to_cell & 0xFFFF < 0x100
    }

    /// A portal of the Town Network: the ones that link every town to
    /// the hub, and the hub's own portals back out.
    pub fn is_town_network(&self) -> bool {
        self.name.to_lowercase().contains("town network")
    }
}

const DATA: &str = include_str!("../data/portals.csv");

fn parse(text: &str) -> Vec<Portal> {
    let mut out = Vec::with_capacity(4600);
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(',').collect();
        if f.len() != 9 {
            continue;
        }
        let hex = |s: &str| u32::from_str_radix(s, 16).ok();
        let num = |s: &str| s.parse::<f32>().ok();
        let (Some(from_cell), Some(to_cell)) = (hex(f[1]), hex(f[5])) else {
            continue;
        };
        let (Some(sx), Some(sy), Some(sz)) = (num(f[2]), num(f[3]), num(f[4])) else {
            continue;
        };
        let (Some(dx), Some(dy), Some(dz)) = (num(f[6]), num(f[7]), num(f[8])) else {
            continue;
        };
        out.push(Portal {
            name: f[0].to_string(),
            from_cell,
            from: crate::landblock_origin(from_cell) + Vec3::new(sx, sy, sz),
            to_cell,
            to: crate::landblock_origin(to_cell) + Vec3::new(dx, dy, dz),
        });
    }
    out
}

/// Every portal, parsed on first use.
pub fn all() -> &'static [Portal] {
    static PORTALS: OnceLock<Vec<Portal>> = OnceLock::new();
    PORTALS.get_or_init(|| parse(DATA))
}

/// The portals whose mouth is within `radius` metres of `world`, nearest
/// first.
pub fn near(world: Vec2, radius: f32) -> Vec<&'static Portal> {
    let mut v: Vec<&Portal> = all()
        .iter()
        .filter(|p| p.from_xy().distance(world) <= radius)
        .collect();
    v.sort_by(|a, b| {
        a.from_xy()
            .distance(world)
            .total_cmp(&b.from_xy().distance(world))
    });
    v
}

/// The portals that come out within `radius` metres of `world`.
pub fn to_place(world: Vec2, radius: f32) -> Vec<&'static Portal> {
    let mut v: Vec<&Portal> = all()
        .iter()
        .filter(|p| p.to_xy().distance(world) <= radius)
        .collect();
    v.sort_by(|a, b| {
        a.to_xy()
            .distance(world)
            .total_cmp(&b.to_xy().distance(world))
    });
    v
}

/// The portals whose name contains `needle`, case-insensitively.
pub fn named(needle: &str) -> Vec<&'static Portal> {
    let needle = needle.to_lowercase();
    all()
        .iter()
        .filter(|p| p.name.to_lowercase().contains(&needle))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_reads_and_places_portals_in_the_world() {
        let all = all();
        assert!(all.len() > 4000, "{} portals", all.len());
        // Holtburg's Town Network portal stands in Holtburg and comes out
        // in the hub, which is indoors.
        let holtburg = ac_world_holtburg();
        let network: Vec<&Portal> = near(holtburg, 400.0)
            .into_iter()
            .filter(|p| p.is_town_network())
            .collect();
        assert!(
            !network.is_empty(),
            "no Town Network portal within 400 m of Holtburg"
        );
        let p = network[0];
        assert!(p.mouth_outdoors(), "the mouth should be outdoors");
        assert!(!p.exit_outdoors(), "the hub is indoors");
        // And a portal comes out near Arwic.
        let arwic = crate::towns::find("Arwic").unwrap().world_xy();
        assert!(
            !to_place(arwic, 400.0).is_empty(),
            "nothing comes out near Arwic"
        );
        assert!(!named("town network").is_empty());
        assert!(named("no such portal anywhere").is_empty());
    }

    fn ac_world_holtburg() -> Vec2 {
        crate::towns::find("Holtburg").unwrap().world_xy()
    }

    #[test]
    fn bad_lines_are_skipped() {
        let v = parse("# comment\n\nbad,line\nName,A9B40019,1.0,2.0,3.0,70145,4.0,5.0,6.0\n");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "Name");
        assert!(!v[0].exit_outdoors());
        assert!(v[0].mouth_outdoors());
    }
}
