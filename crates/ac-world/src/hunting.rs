//! Where the monsters are: the outdoor landblocks with creature spawns
//! and the levels of what spawns there, so a character can pick a
//! hunting ground that suits its level and go there.
//!
//! Like the portals and landmarks, spawns are server-side and so are
//! not in the client's own data files; `data/hunting.csv` is a copy of
//! the list (see `reference/scripts/data/hunting.sh`). One row per
//! landblock: the average spawn point, how many spawn entries there are,
//! the lowest and highest level among them, and the level and name of
//! the creature that spawns most often, which is what a hunter meets.

use glam::Vec2;
use std::sync::OnceLock;

/// The monsters of one outdoor landblock.
#[derive(Clone, Debug, PartialEq)]
pub struct Ground {
    /// Landblock id (the high half of a cell id).
    pub landblock: u32,
    /// World xy of the middle of the spawns.
    pub at: Vec2,
    /// How many spawn entries the block's generators hold: a rough
    /// measure of how busy it is.
    pub count: u32,
    pub min_level: u32,
    pub max_level: u32,
    /// The level of the creature that spawns most often.
    pub level: u32,
    /// That creature's name.
    pub name: String,
}

/// A ground with fewer spawn entries than this is a stray, not a
/// hunting ground: one chicken behind a house is not worth a journey.
pub const LEAST_SPAWNS: u32 = 2;

impl Ground {
    /// Whether a character of `level` should hunt here, allowing
    /// `margin` levels either way: what spawns most must be within the
    /// margin, something there must be no harder than the margin
    /// allows, and nothing there may be so far above the character
    /// that a stray of it is a death (twice the margin).
    pub fn suits(&self, level: u32, margin: u32) -> bool {
        if self.count < LEAST_SPAWNS {
            return false;
        }
        let low = level.saturating_sub(margin);
        let high = level + margin;
        self.level >= low
            && self.level <= high
            && self.min_level <= high
            && self.max_level <= level + 2 * margin
    }
}

const DATA: &str = include_str!("../data/hunting.csv");

fn parse(text: &str) -> Vec<Ground> {
    let mut out = Vec::with_capacity(1000);
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(',').collect();
        if f.len() != 8 {
            continue;
        }
        let Ok(landblock) = u32::from_str_radix(f[0], 16) else {
            continue;
        };
        let num = |s: &str| s.parse::<f32>().ok();
        let int = |s: &str| s.parse::<u32>().ok();
        let (Some(x), Some(y)) = (num(f[1]), num(f[2])) else {
            continue;
        };
        let (Some(count), Some(min_level), Some(max_level), Some(level)) =
            (int(f[3]), int(f[4]), int(f[5]), int(f[6]))
        else {
            continue;
        };
        let origin = crate::landblock_origin(landblock << 16);
        out.push(Ground {
            landblock,
            at: Vec2::new(origin.x + x, origin.y + y),
            count,
            min_level,
            max_level,
            level,
            name: f[7].to_string(),
        });
    }
    out
}

/// Every ground, parsed on first use.
pub fn all() -> &'static [Ground] {
    static GROUNDS: OnceLock<Vec<Ground>> = OnceLock::new();
    GROUNDS.get_or_init(|| parse(DATA))
}

/// The ground of a landblock, if it has monsters.
pub fn at(landblock: u32) -> Option<&'static Ground> {
    all().iter().find(|g| g.landblock == landblock)
}

/// The nearest ground to `from` that suits a character of `level` (see
/// [`Ground::suits`]), leaving out the landblocks in `skip`: the one
/// the character has just hunted out, and any it could not reach.
pub fn nearest_for(level: u32, margin: u32, from: Vec2, skip: &[u32]) -> Option<&'static Ground> {
    nearest_among(all(), level, margin, from, skip)
}

fn nearest_among<'a>(
    grounds: &'a [Ground],
    level: u32,
    margin: u32,
    from: Vec2,
    skip: &[u32],
) -> Option<&'a Ground> {
    grounds
        .iter()
        .filter(|g| g.suits(level, margin))
        .filter(|g| !skip.contains(&g.landblock))
        .min_by(|a, b| a.at.distance(from).total_cmp(&b.at.distance(from)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ground(landblock: u32, x: f32, count: u32, lo: u32, hi: u32, level: u32) -> Ground {
        Ground {
            landblock,
            at: Vec2::new(x, 0.0),
            count,
            min_level: lo,
            max_level: hi,
            level,
            name: "Thing".into(),
        }
    }

    #[test]
    fn the_table_reads_and_places_the_grounds() {
        let all = all();
        assert!(all.len() > 900, "{} grounds", all.len());
        // Every ground is outdoors and stands where its landblock is.
        for g in all {
            let origin = crate::landblock_origin(g.landblock << 16);
            assert!(g.at.x >= origin.x && g.at.x <= origin.x + 192.0, "{g:?}");
            assert!(g.at.y >= origin.y && g.at.y <= origin.y + 192.0, "{g:?}");
            assert!(g.min_level <= g.level && g.level <= g.max_level, "{g:?}");
        }
        // Drudges near Holtburg for a beginner.
        let holtburg = crate::towns::find("Holtburg").unwrap().world_xy();
        let g = nearest_for(2, 8, holtburg, &[]).expect("a ground for a level 2");
        assert!(g.level <= 10, "{g:?}");
        assert!(
            g.at.distance(holtburg) < 2000.0,
            "{g:?} is far from Holtburg"
        );
        // Something for the strong too.
        assert!(nearest_for(150, 20, holtburg, &[]).is_some());
        assert!(at(g.landblock).is_some());
    }

    #[test]
    fn suitability_is_a_window_on_the_usual_level() {
        // A level 10 with a margin of 5 hunts levels 5..=15.
        assert!(ground(1, 0.0, 5, 8, 12, 10).suits(10, 5));
        assert!(ground(1, 0.0, 5, 5, 5, 5).suits(10, 5));
        assert!(!ground(1, 0.0, 5, 4, 4, 4).suits(10, 5), "too easy");
        assert!(!ground(1, 0.0, 5, 16, 16, 16).suits(10, 5), "too hard");
        // A rare spawn far above the character rules the place out.
        assert!(!ground(1, 0.0, 5, 8, 40, 10).suits(10, 5));
        assert!(
            ground(1, 0.0, 5, 8, 20, 10).suits(10, 5),
            "twice the margin is allowed"
        );
        // A single stray is no hunting ground.
        assert!(!ground(1, 0.0, 1, 10, 10, 10).suits(10, 5));
        // A low level's window does not go below zero.
        assert!(ground(1, 0.0, 5, 1, 3, 2).suits(1, 5));
    }

    #[test]
    fn the_nearest_suitable_ground_is_chosen_and_skips_are_honoured() {
        let grounds = vec![
            ground(0xA, 100.0, 5, 10, 10, 10),
            ground(0xB, 50.0, 5, 10, 10, 10),
            ground(0xC, 10.0, 5, 50, 50, 50),
            ground(0xD, 20.0, 1, 10, 10, 10),
        ];
        let from = Vec2::ZERO;
        assert_eq!(
            nearest_among(&grounds, 10, 5, from, &[]).unwrap().landblock,
            0xB
        );
        assert_eq!(
            nearest_among(&grounds, 10, 5, from, &[0xB])
                .unwrap()
                .landblock,
            0xA
        );
        assert!(nearest_among(&grounds, 10, 5, from, &[0xA, 0xB]).is_none());
        assert_eq!(
            nearest_among(&grounds, 50, 5, from, &[]).unwrap().landblock,
            0xC
        );
    }

    #[test]
    fn bad_lines_are_skipped() {
        let v = parse("# c\n\nnope\nA9B2,74,83,30,7,20,7,Blood Shrethlet\nZZ,1,1,1,1,1,1,x\n");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "Blood Shrethlet");
        assert_eq!(v[0].landblock, 0xA9B2);
        assert_eq!(
            v[0].at,
            Vec2::new(0xA9 as f32 * 192.0 + 74.0, 0xB2 as f32 * 192.0 + 83.0)
        );
    }
}
