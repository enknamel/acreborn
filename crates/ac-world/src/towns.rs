//! Gazetteer: the towns and outposts of Dereth with their map coordinates
//! (the game's "42.1N, 33.6E"), and the conversion back to world xy.
//!
//! Source: the Asheron's Call wiki (asheron.fandom.com) through its
//! MediaWiki API, fetched 2026-09-05: every page in `Category:Towns` and
//! the `{{Map Point|ns|N|ew|E}}` template at the top of each. Xarabydun's
//! page has no Map Point, so its entry is the Town Network portal's
//! arrival point from the same page. Coordinates mark the town centre
//! (the wiki's map pin), not a particular door or lifestone.

use glam::Vec2;

/// A named place on the surface of Dereth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    pub name: &'static str,
    /// North positive, south negative.
    pub ns: f32,
    /// East positive, west negative.
    pub ew: f32,
}

impl Place {
    /// World xy of the place (see [`world_xy`]).
    pub fn world_xy(&self) -> Vec2 {
        world_xy(self.ns, self.ew)
    }
}

/// Every known town and outpost, alphabetical.
#[rustfmt::skip]
pub const PLACES: &[Place] = &[
    Place { name: "Ahurenga", ns: 47.0, ew: -90.3 },
    Place { name: "Al-Arqas", ns: -31.2, ew: 13.7 },
    Place { name: "Al-Jalima", ns: 7.4, ew: 4.8 },
    Place { name: "Arwic", ns: 33.6, ew: 56.8 },
    Place { name: "Ayan Baqur", ns: -60.0, ew: -88.0 },
    Place { name: "Baishi", ns: -49.3, ew: 62.9 },
    Place { name: "Bandit Castle", ns: 66.5, ew: 49.9 },
    Place { name: "Beach Fort", ns: 76.0, ew: -49.1 },
    Place { name: "Bluespire", ns: 39.4, ew: -75.4 },
    Place { name: "Candeth Keep", ns: -87.5, ew: -67.1 },
    Place { name: "Cragstone", ns: 25.5, ew: 49.7 },
    Place { name: "Crater Lake Village", ns: 65.0, ew: 13.6 },
    Place { name: "Danby's Outpost", ns: 23.4, ew: -28.7 },
    Place { name: "Dryreach", ns: -8.1, ew: 73.0 },
    Place { name: "Eastham", ns: 17.5, ew: 63.4 },
    Place { name: "Eastwatch", ns: 90.3, ew: -43.1 },
    Place { name: "Fiun Outpost", ns: 95.9, ew: -56.8 },
    Place { name: "Fort Tethana", ns: 1.5, ew: -71.8 },
    Place { name: "Glenden Wood", ns: 29.6, ew: 27.2 },
    Place { name: "Greenspire", ns: 42.9, ew: -66.9 },
    Place { name: "Hebian-To", ns: -39.1, ew: 83.2 },
    Place { name: "Holtburg", ns: 42.3, ew: 33.7 },
    Place { name: "Kara", ns: -83.3, ew: 47.1 },
    Place { name: "Khayyaban", ns: -47.6, ew: 24.7 },
    Place { name: "Kor-Gursha", ns: 67.4, ew: 30.5 },
    Place { name: "Kryst", ns: -74.6, ew: 84.5 },
    Place { name: "Lin", ns: -54.5, ew: 73.1 },
    Place { name: "Linvak Tukal", ns: -77.8, ew: 28.0 },
    Place { name: "Lytelthorpe", ns: 0.9, ew: 51.1 },
    Place { name: "MacNiall's Freehold", ns: -74.2, ew: 92.3 },
    Place { name: "Mar'uun", ns: -10.6, ew: 17.1 },
    Place { name: "Martine's Retreat", ns: 10.6, ew: 58.3 },
    Place { name: "Mayoi", ns: -61.6, ew: 81.9 },
    Place { name: "Merwart Village", ns: 79.9, ew: 59.0 },
    Place { name: "Nanto", ns: -52.5, ew: 82.1 },
    Place { name: "Neydisa Castle", ns: 69.9, ew: 17.6 },
    Place { name: "Oolutanga's Refuge", ns: 2.3, ew: 95.4 },
    Place { name: "Plateau Village", ns: 44.2, ew: -43.4 },
    Place { name: "Qalaba'r", ns: -74.4, ew: 19.3 },
    Place { name: "Redspire", ns: 40.8, ew: -83.1 },
    Place { name: "Rithwic", ns: 10.8, ew: 58.7 },
    Place { name: "Samsur", ns: -2.8, ew: 19.4 },
    Place { name: "Sanamar", ns: 71.8, ew: -60.8 },
    Place { name: "Sawato", ns: -28.7, ew: 59.3 },
    Place { name: "Shoushi", ns: -33.5, ew: 72.8 },
    Place { name: "Silyun", ns: 87.4, ew: -70.5 },
    Place { name: "Stonehold", ns: 68.8, ew: -21.6 },
    Place { name: "Timaru", ns: 44.2, ew: -78.0 },
    Place { name: "Tou-Tou", ns: -28.0, ew: 95.7 },
    Place { name: "Tufa", ns: -13.9, ew: 5.0 },
    Place { name: "Underground City", ns: 21.3, ew: 53.9 },
    Place { name: "Uziz", ns: -25.2, ew: 28.3 },
    Place { name: "Wai Jhou", ns: -62.0, ew: -51.4 },
    Place { name: "Westwatch", ns: 72.8, ew: -61.3 },
    // Town Network arrival point (the page has no map pin).
    Place { name: "Xarabydun", ns: -41.9, ew: 16.1 },
    Place { name: "Yanshi", ns: -12.1, ew: 42.4 },
    Place { name: "Yaraq", ns: -21.6, ew: -1.7 },
    Place { name: "Zaikhal", ns: 13.7, ew: 0.6 },
];

/// The place named `name`, case-insensitive: an exact match first, then
/// the first place whose name starts with it, then the first containing
/// it. Empty names match nothing.
pub fn find(name: &str) -> Option<&'static Place> {
    let want = name.trim().to_lowercase();
    if want.is_empty() {
        return None;
    }
    let lower = |p: &&Place| p.name.to_lowercase();
    PLACES
        .iter()
        .find(|p| lower(p) == want)
        .or_else(|| PLACES.iter().find(|p| lower(p).starts_with(&want)))
        .or_else(|| PLACES.iter().find(|p| lower(p).contains(&want)))
}

/// World xy of a map coordinate: the inverse of `map_coords` (map =
/// world / 240 - 102 per axis; north is +y, east is +x).
pub fn world_xy(ns: f32, ew: f32) -> Vec2 {
    Vec2::new((ew + 102.0) * 240.0, (ns + 102.0) * 240.0)
}

/// Map coordinates of a world xy (north, east), the other way round.
pub fn map_of(world: Vec2) -> (f32, f32) {
    (world.y / 240.0 - 102.0, world.x / 240.0 - 102.0)
}

/// A destination typed by a person: a place name (see [`find`]), map
/// coordinates ("42.1N, 33.6E", "42.1n 33.6e", "42.1N,33.6E"), or a
/// bare world position "x,y" (or "x y") in metres.
pub fn parse_destination(s: &str) -> Option<Vec2> {
    if let Some(p) = find(s) {
        return Some(p.world_xy());
    }
    let parts: Vec<&str> = s
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .collect();
    if parts.len() != 2 {
        return None;
    }
    let signed = |t: &str| -> Option<(f32, Option<char>)> {
        let last = t.chars().last()?.to_ascii_uppercase();
        if "NSEW".contains(last) {
            let n: f32 = t[..t.len() - 1].trim().parse().ok()?;
            Some((n, Some(last)))
        } else {
            Some((t.parse().ok()?, None))
        }
    };
    let (a, sa) = signed(parts[0])?;
    let (b, sb) = signed(parts[1])?;
    match (sa, sb) {
        (None, None) => Some(Vec2::new(a, b)),
        (Some(sa), Some(sb)) => {
            let sign = |c: char| if c == 'S' || c == 'W' { -1.0 } else { 1.0 };
            let (ns, ew) = match (sa, sb) {
                ('N' | 'S', 'E' | 'W') => (a * sign(sa), b * sign(sb)),
                ('E' | 'W', 'N' | 'S') => (b * sign(sb), a * sign(sa)),
                _ => return None,
            };
            Some(world_xy(ns, ew))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_is_case_insensitive_exact_then_prefix_then_substring() {
        assert_eq!(find("holtburg").unwrap().name, "Holtburg");
        assert_eq!(find("ARWIC").unwrap().name, "Arwic");
        // "Lin" exactly, not Linvak Tukal, even though both start with it.
        assert_eq!(find("lin").unwrap().name, "Lin");
        assert_eq!(find("linv").unwrap().name, "Linvak Tukal");
        assert_eq!(find("Plateau").unwrap().name, "Plateau Village");
        assert_eq!(find("freehold").unwrap().name, "MacNiall's Freehold");
        assert!(find("").is_none());
        assert!(find("Atlantis").is_none());
    }

    #[test]
    fn world_xy_inverts_map_coords() {
        let p = find("Holtburg").unwrap();
        let w = p.world_xy();
        // Holtburg is in landblock 0xA9B4: x in [0xA9 * 192, +192).
        assert_eq!((w.x / 192.0) as u32, 0xA9, "{w:?}");
        assert_eq!((w.y / 192.0) as u32, 0xB4, "{w:?}");
        let pos = crate::object::Position::new_flat(
            0xA9B4_0000 | 1,
            glam::Vec3::new(w.x - 0xA9 as f32 * 192.0, w.y - 0xB4 as f32 * 192.0, 0.0),
        );
        let (ns, ew) = crate::map_coords(&pos).unwrap();
        assert!((ns - p.ns).abs() < 1e-3 && (ew - p.ew).abs() < 1e-3);
        let (ns2, ew2) = map_of(w);
        assert!((ns2 - p.ns).abs() < 1e-3 && (ew2 - p.ew).abs() < 1e-3);
        // South and west are negative.
        let s = world_xy(-10.0, -20.0);
        assert!((s.x - 82.0 * 240.0).abs() < 1e-2 && (s.y - 92.0 * 240.0).abs() < 1e-2);
    }

    #[test]
    fn places_are_unique_and_on_the_map() {
        for (i, p) in PLACES.iter().enumerate() {
            assert!(p.ns.abs() <= 102.0 && p.ew.abs() <= 102.0, "{}", p.name);
            assert!(
                !PLACES[..i]
                    .iter()
                    .any(|q| q.name.eq_ignore_ascii_case(p.name)),
                "duplicate {}",
                p.name
            );
        }
        assert!(PLACES.len() >= 50);
    }

    #[test]
    fn destinations_parse_names_map_coords_and_world_xy() {
        let holt = find("Holtburg").unwrap().world_xy();
        assert_eq!(parse_destination("holtburg"), Some(holt));
        let w = world_xy(42.1, 33.6);
        assert_eq!(parse_destination("42.1N, 33.6E"), Some(w));
        assert_eq!(parse_destination("42.1n 33.6e"), Some(w));
        assert_eq!(parse_destination("33.6E,42.1N"), Some(w));
        let sw = world_xy(-10.5, -20.0);
        assert_eq!(parse_destination("10.5S 20W"), Some(sw));
        assert_eq!(
            parse_destination("100.5, 200"),
            Some(Vec2::new(100.5, 200.0))
        );
        assert_eq!(
            parse_destination("100.5 200"),
            Some(Vec2::new(100.5, 200.0))
        );
        assert!(parse_destination("42.1N").is_none());
        assert!(parse_destination("42.1N 33.6N").is_none());
        assert!(parse_destination("nowhere at all").is_none());
    }
}
