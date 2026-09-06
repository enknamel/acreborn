//! Overland routing: A* over the [`WorldGrid`] lattice (24 m spacing,
//! 8-connected), for walking from one town to another across Dereth.
//!
//! An edge costs its length, more on a slope and less on a road, and is
//! impassable when it climbs steeper than a character can walk, when
//! either end is sea (the region names the water terrain types; the
//! renderer draws water by the same rule), or when either end lies in a
//! landblock the archive does not have (the sea beyond the map). Fresh
//! water (rivers, lakes) can be crossed at a steep price: the bridges
//! and fords of Dereth are static objects over water terrain, invisible
//! to a terrain grid, and the rivers cut the continent, so a route
//! crosses a river in the shortest span it can find (the road leads to
//! the bridge) but never travels along one. The search is confined to a
//! window around the two endpoints so a route across the continent
//! costs well under a second.
//!
//! Terrain only: buildings, fences and portals are not considered. The
//! client's per-landblock move-to steers around the static geometry on
//! the way from one waypoint to the next.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use ac_formats::region::Region;
use glam::Vec2;

use crate::worldgrid::{WorldGrid, SIDE, SPACING};

/// Steepest rise over run a route will walk. The collision world treats a
/// triangle as a floor when its normal's z exceeds 0.6 (up to a 53 degree
/// slope, grade 1.33), and the local navigation graph refuses a 2 m rise
/// within one 4 m step (grade 0.5) between nodes; the route stays well
/// inside the first and takes the second as a guide, at about 35 degrees.
pub const MAX_GRADE: f32 = 0.7;
/// A road edge (both ends on a road) costs this fraction of its length.
pub const ROAD_FACTOR: f32 = 0.7;
/// An edge costs its length times `1 + SLOPE_PENALTY * grade`.
pub const SLOPE_PENALTY: f32 = 2.0;
/// A fresh-water edge (either end in a river or lake) costs this many
/// times its length: a river is crossed, never followed.
pub const FRESH_WATER_FACTOR: f32 = 6.0;
/// Lattice vertices of margin around the endpoints' bounding box.
pub const WINDOW_MARGIN: usize = 40;
/// How far (vertices) an endpoint in the water is moved to the nearest
/// land vertex before searching.
const SNAP_RADIUS: i32 = 3;

/// What a terrain type is to the route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Water {
    /// Dry land.
    Land,
    /// A river or lake: crossable at [`FRESH_WATER_FACTOR`].
    Fresh,
    /// The sea: impassable.
    Sea,
}

/// Whether a terrain type (index into `region.terrain_types`) is water:
/// the same rule the renderer uses to draw water surfaces.
pub fn is_water(region: &Region, terrain_type: u16) -> bool {
    water_kind(region, terrain_type) != Water::Land
}

/// The route's view of a terrain type: the region names its water types
/// `Water...` (the renderer's rule) and the sea ones `...Sea...`.
pub fn water_kind(region: &Region, terrain_type: u16) -> Water {
    match region.terrain_types.get(terrain_type as usize) {
        Some(t) if t.name.contains("Water") => {
            if t.name.contains("Sea") {
                Water::Sea
            } else {
                Water::Fresh
            }
        }
        _ => Water::Land,
    }
}

/// The route from `from` to `to` (world xy) as waypoints from `from` to
/// `to` inclusive, runs of collinear vertices collapsed; `None` when no
/// walkable path exists inside the search window.
pub fn find(grid: &WorldGrid, region: &Region, from: Vec2, to: Vec2) -> Option<Vec<Vec2>> {
    find_avoiding(grid, region, from, to, &[])
}

/// How dearly a route pays to pass near something in `avoid`.
pub const AVOID_FACTOR: f32 = 25.0;
/// How close counts as near it, metres. Wider than the lattice spacing,
/// so a portal sitting between two vertices still makes both dear.
pub const AVOID_RADIUS: f32 = 34.0;

/// [`find`], keeping away from the spots in `avoid` where it can. A
/// portal swallows whoever walks into it, so a route that is not meant
/// to take one has to give it a wide berth.
pub fn find_avoiding(
    grid: &WorldGrid,
    region: &Region,
    from: Vec2,
    to: Vec2,
    avoid: &[Vec2],
) -> Option<Vec<Vec2>> {
    let water: Vec<Water> = (0..32u16).map(|t| water_kind(region, t)).collect();
    find_with_avoid(
        grid,
        |t| water.get(t as usize).copied().unwrap_or(Water::Sea),
        from,
        to,
        avoid,
    )
}

/// [`find`] with the water rule supplied (terrain type -> [`Water`]).
pub fn find_with(
    grid: &WorldGrid,
    water: impl Fn(u16) -> Water,
    from: Vec2,
    to: Vec2,
) -> Option<Vec<Vec2>> {
    find_with_avoid(grid, water, from, to, &[])
}

/// [`find_with`] that keeps away from the spots in `avoid`.
pub fn find_with_avoid(
    grid: &WorldGrid,
    water: impl Fn(u16) -> Water,
    from: Vec2,
    to: Vec2,
    avoid: &[Vec2],
) -> Option<Vec<Vec2>> {
    let land = Land {
        grid,
        water: &water,
        avoid,
    };
    let start = land.nearest_land(from)?;
    let goal = land.nearest_land(to)?;
    let window = Window::around(start, goal);
    let path = astar(&land, &window, start, goal)?;
    let mut points = Vec::with_capacity(path.len() + 2);
    points.push(from);
    points.extend(path.iter().map(|&(x, y)| WorldGrid::vertex_world(x, y)));
    points.push(to);
    Some(prune_collinear(points))
}

/// The lattice with its walkability rules.
struct Land<'a> {
    grid: &'a WorldGrid,
    water: &'a dyn Fn(u16) -> Water,
    /// Spots to keep away from where the route can (portal mouths it is
    /// not meant to walk into).
    avoid: &'a [Vec2],
}

impl Land<'_> {
    /// What is at a vertex, or `None` off the map (outside the lattice or
    /// in a landblock the archive lacks).
    fn at(&self, gx: usize, gy: usize) -> Option<Water> {
        if gx >= SIDE || gy >= SIDE {
            return None;
        }
        let (bx, by) = ((gx / 8).min(254), (gy / 8).min(254));
        if !self.grid.has_block(bx, by) {
            return None;
        }
        Some((self.water)(self.grid.terrain_type(gx, gy)))
    }

    /// A vertex a character can walk over: on the map and not sea.
    fn passable(&self, gx: usize, gy: usize) -> bool {
        matches!(self.at(gx, gy), Some(Water::Land | Water::Fresh))
    }

    /// A vertex a route may start or end on: dry land.
    fn dry(&self, gx: usize, gy: usize) -> bool {
        self.at(gx, gy) == Some(Water::Land)
    }

    fn height(&self, gx: usize, gy: usize) -> f32 {
        self.grid.height(gx, gy)
    }

    /// Cost of stepping from `a` to its neighbour `b`, or `None` when the
    /// step cannot be walked.
    fn step_cost(&self, a: (usize, usize), b: (usize, usize)) -> Option<f32> {
        if !self.passable(b.0, b.1) {
            return None;
        }
        let diagonal = a.0 != b.0 && a.1 != b.1;
        let len = if diagonal {
            SPACING * std::f32::consts::SQRT_2
        } else {
            SPACING
        };
        let ha = self.height(a.0, a.1);
        let hb = self.height(b.0, b.1);
        let grade = (hb - ha).abs() / len;
        if grade > MAX_GRADE {
            return None;
        }
        if diagonal {
            // The cell is two triangles split along one diagonal: the walk
            // may cross the other two corners' slopes, and a corner in the
            // water puts water in the cell. Be conservative on both.
            let c1 = (b.0, a.1);
            let c2 = (a.0, b.1);
            if !self.passable(c1.0, c1.1) || !self.passable(c2.0, c2.1) {
                return None;
            }
            let limit = MAX_GRADE * SPACING;
            let (h1, h2) = (self.height(c1.0, c1.1), self.height(c2.0, c2.1));
            if (h1 - ha).abs() > limit
                || (h2 - ha).abs() > limit
                || (hb - h1).abs() > limit
                || (hb - h2).abs() > limit
            {
                return None;
            }
        }
        let road = self.grid.road(a.0, a.1) != 0 && self.grid.road(b.0, b.1) != 0;
        let mut cost = len * (1.0 + SLOPE_PENALTY * grade);
        if road {
            cost *= ROAD_FACTOR;
        }
        if !self.dry(a.0, a.1) || !self.dry(b.0, b.1) {
            cost *= FRESH_WATER_FACTOR;
        }
        if !self.avoid.is_empty() {
            let aw = WorldGrid::vertex_world(a.0, a.1);
            let bw = WorldGrid::vertex_world(b.0, b.1);
            let near = |p: &Vec2| p.distance(aw) < AVOID_RADIUS || p.distance(bw) < AVOID_RADIUS;
            if self.avoid.iter().any(near) {
                cost *= AVOID_FACTOR;
            }
        }
        Some(cost)
    }

    /// The dry vertex nearest a world xy within [`SNAP_RADIUS`].
    fn nearest_land(&self, world: Vec2) -> Option<(usize, usize)> {
        let (cx, cy) = WorldGrid::nearest_vertex(world);
        let mut best: Option<((usize, usize), f32)> = None;
        for dx in -SNAP_RADIUS..=SNAP_RADIUS {
            for dy in -SNAP_RADIUS..=SNAP_RADIUS {
                let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                if x < 0 || y < 0 {
                    continue;
                }
                let (x, y) = (x as usize, y as usize);
                if !self.dry(x, y) {
                    continue;
                }
                let d = WorldGrid::vertex_world(x, y).distance_squared(world);
                if best.is_none_or(|(_, bd)| d < bd) {
                    best = Some(((x, y), d));
                }
            }
        }
        best.map(|(v, _)| v)
    }
}

/// The rectangle of vertices the search may visit.
struct Window {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
}

impl Window {
    fn around(a: (usize, usize), b: (usize, usize)) -> Window {
        let x0 = a.0.min(b.0).saturating_sub(WINDOW_MARGIN);
        let y0 = a.1.min(b.1).saturating_sub(WINDOW_MARGIN);
        let x1 = (a.0.max(b.0) + WINDOW_MARGIN).min(SIDE - 1);
        let y1 = (a.1.max(b.1) + WINDOW_MARGIN).min(SIDE - 1);
        Window {
            x0,
            y0,
            w: x1 - x0 + 1,
            h: y1 - y0 + 1,
        }
    }

    fn index(&self, v: (usize, usize)) -> Option<usize> {
        if v.0 < self.x0 || v.1 < self.y0 || v.0 >= self.x0 + self.w || v.1 >= self.y0 + self.h {
            return None;
        }
        Some((v.0 - self.x0) * self.h + (v.1 - self.y0))
    }

    fn vertex(&self, i: usize) -> (usize, usize) {
        (self.x0 + i / self.h, self.y0 + i % self.h)
    }
}

#[derive(PartialEq)]
struct Open {
    f: f32,
    node: u32,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        // Smallest f on top of the max-heap.
        other
            .f
            .total_cmp(&self.f)
            .then_with(|| other.node.cmp(&self.node))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

const DIRS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// The lattice vertices from `start` to `goal` inclusive.
fn astar(
    land: &Land,
    win: &Window,
    start: (usize, usize),
    goal: (usize, usize),
) -> Option<Vec<(usize, usize)>> {
    let n = win.w * win.h;
    let goal_w = WorldGrid::vertex_world(goal.0, goal.1);
    // Admissible: no edge costs less per metre than a flat road.
    let heuristic =
        |v: (usize, usize)| WorldGrid::vertex_world(v.0, v.1).distance(goal_w) * ROAD_FACTOR;
    let mut g = vec![f32::INFINITY; n];
    let mut parent = vec![u32::MAX; n];
    let mut closed = vec![false; n];
    let mut open = BinaryHeap::new();
    let si = win.index(start)?;
    let gi = win.index(goal)?;
    g[si] = 0.0;
    open.push(Open {
        f: heuristic(start),
        node: si as u32,
    });
    while let Some(Open { node, .. }) = open.pop() {
        let i = node as usize;
        if closed[i] {
            continue;
        }
        closed[i] = true;
        if i == gi {
            let mut path = vec![goal];
            let mut at = gi;
            while at != si {
                at = parent[at] as usize;
                path.push(win.vertex(at));
            }
            path.reverse();
            return Some(path);
        }
        let v = win.vertex(i);
        for (dx, dy) in DIRS {
            let (nx, ny) = (v.0 as i32 + dx, v.1 as i32 + dy);
            if nx < 0 || ny < 0 {
                continue;
            }
            let nv = (nx as usize, ny as usize);
            let Some(ni) = win.index(nv) else {
                continue;
            };
            if closed[ni] {
                continue;
            }
            let Some(cost) = land.step_cost(v, nv) else {
                continue;
            };
            let ng = g[i] + cost;
            if ng < g[ni] {
                g[ni] = ng;
                parent[ni] = i as u32;
                open.push(Open {
                    f: ng + heuristic(nv),
                    node: ni as u32,
                });
            }
        }
    }
    None
}

/// Drop every point that lies on the straight line between its
/// neighbours (and repeated points), keeping the first and last.
pub fn prune_collinear(points: Vec<Vec2>) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::with_capacity(points.len());
    for (i, &p) in points.iter().enumerate() {
        if i == 0 || i + 1 == points.len() {
            out.push(p);
            continue;
        }
        let prev = *out.last().unwrap();
        let next = points[i + 1];
        let a = p - prev;
        let b = next - p;
        if a.length_squared() < 1e-6 {
            continue; // repeats the previous point
        }
        if b.length_squared() < 1e-6 {
            continue; // the next point repeats this one; keep that one
        }
        let straight = a.perp_dot(b).abs() <= 1e-3 * a.length() * b.length() && a.dot(b) > 0.0;
        if !straight {
            out.push(p);
        }
    }
    if out.len() >= 2 && out[out.len() - 1] == out[out.len() - 2] {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat world of `blocks` x `blocks` present landblocks; terrain
    /// type 0 everywhere. In these tests type 1 is sea and type 2 fresh
    /// water (see [`kind`]).
    fn flat(blocks: usize) -> WorldGrid {
        let mut g = WorldGrid {
            heights: vec![0; SIDE * SIDE],
            terrain: vec![0; SIDE * SIDE],
            height_table: (0..256).map(|i| i as f32 * 2.0).collect(),
            present: vec![false; 65536],
        };
        for bx in 0..blocks {
            for by in 0..blocks {
                g.present[bx * 256 + by] = true;
            }
        }
        g
    }

    fn kind(t: u16) -> Water {
        match t {
            1 => Water::Sea,
            2 => Water::Fresh,
            _ => Water::Land,
        }
    }

    fn set_type(g: &mut WorldGrid, gx: usize, gy: usize, t: u16) {
        g.terrain[gx * SIDE + gy] = t << 2;
    }

    fn set_road(g: &mut WorldGrid, gx: usize, gy: usize) {
        g.terrain[gx * SIDE + gy] |= 1;
    }

    fn length(path: &[Vec2]) -> f32 {
        path.windows(2).map(|w| w[0].distance(w[1])).sum()
    }

    /// Whether the polyline passes over lattice vertex `v`.
    fn passes(path: &[Vec2], v: (usize, usize)) -> bool {
        let p = WorldGrid::vertex_world(v.0, v.1);
        path.windows(2).any(|w| {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
            (a + ab * t).distance(p) < 1e-2
        })
    }

    #[test]
    fn straight_line_on_flat_ground_collapses_to_two_points() {
        let g = flat(4);
        let from = WorldGrid::vertex_world(2, 2);
        let to = WorldGrid::vertex_world(20, 2);
        let path = find_with(&g, kind, from, to).unwrap();
        assert_eq!(path, vec![from, to]);
        // Off-lattice endpoints are kept as they are.
        let from = Vec2::new(50.0, 50.0);
        let to = Vec2::new(500.0, 60.0);
        let path = find_with(&g, kind, from, to).unwrap();
        assert_eq!(path[0], from);
        assert_eq!(*path.last().unwrap(), to);
    }

    #[test]
    fn rivers_are_crossed_not_followed() {
        let mut g = flat(4);
        // A river two vertices wide along gx = 10..=11, the whole way.
        for gy in 0..SIDE {
            set_type(&mut g, 10, gy, 2);
            set_type(&mut g, 11, gy, 2);
        }
        let from = WorldGrid::vertex_world(2, 2);
        let to = WorldGrid::vertex_world(18, 2);
        let path = find_with(&g, kind, from, to).unwrap();
        // Straight across: no reason to bend.
        assert_eq!(path, vec![from, to]);
        // Endpoints in the river snap to the bank, and the route between
        // two points on the same bank never dips into the water.
        let a = WorldGrid::vertex_world(10, 2);
        let b = WorldGrid::vertex_world(10, 30);
        let path = find_with(&g, kind, a, b).unwrap();
        assert_eq!(path[0], a);
        assert_eq!(*path.last().unwrap(), b);
        for w in path.windows(2) {
            let (p, q) = (
                WorldGrid::nearest_vertex(w[0]),
                WorldGrid::nearest_vertex(w[1]),
            );
            let n = p.0.abs_diff(q.0).max(p.1.abs_diff(q.1)).max(1);
            for i in 1..n {
                let x = (p.0 as f32 + (q.0 as f32 - p.0 as f32) * i as f32 / n as f32).round();
                let y = (p.1 as f32 + (q.1 as f32 - p.1 as f32) * i as f32 / n as f32).round();
                assert_ne!(g.terrain_type(x as usize, y as usize), 2, "{path:?}");
            }
        }
        // A short walk around beats a crossing: from one bank, around
        // the river's end, is cheaper than two crossings at 6x.
        let mut g = flat(4);
        for gy in 0..=6 {
            set_type(&mut g, 10, gy, 2);
        }
        let from = WorldGrid::vertex_world(8, 2);
        let to = WorldGrid::vertex_world(12, 2);
        let path = find_with(&g, kind, from, to).unwrap();
        assert!(passes(&path, (10, 7)), "{path:?}");
    }

    #[test]
    fn water_is_walked_around_and_the_sea_is_unreachable() {
        let mut g = flat(4);
        // An inlet of sea along gx = 10 from gy 0..=20, with a spit at gy 12.
        for gy in 0..=20 {
            if gy != 12 {
                set_type(&mut g, 10, gy, 1);
            }
        }
        let from = WorldGrid::vertex_world(2, 2);
        let to = WorldGrid::vertex_world(18, 2);
        let path = find_with(&g, kind, from, to).unwrap();
        assert!(path.iter().all(|p| {
            let (x, y) = WorldGrid::nearest_vertex(*p);
            g.terrain_type(x, y) != 1
        }));
        // Through the ford: the route goes up to gy 12 and back.
        assert!(passes(&path, (10, 12)), "{path:?}");
        assert!(length(&path) > 2.0 * 10.0 * SPACING);
        // Nothing but sea past the present blocks.
        let t0 = std::time::Instant::now();
        assert!(find_with(&g, kind, from, WorldGrid::vertex_world(200, 200)).is_none());
        assert!(t0.elapsed().as_millis() < 500);
        // A goal standing in the sea is moved to the shore.
        let wet = WorldGrid::vertex_world(10, 4);
        let path = find_with(&g, kind, from, wet).unwrap();
        assert_eq!(*path.last().unwrap(), wet);
        assert!(path.len() >= 2);
        // Far out in the sea there is no shore to move to.
        for gx in 20..32 {
            for gy in 20..32 {
                set_type(&mut g, gx, gy, 1);
            }
        }
        assert!(find_with(&g, kind, from, WorldGrid::vertex_world(26, 26)).is_none());
    }

    #[test]
    fn cliffs_block_and_roads_attract() {
        let mut g = flat(4);
        // A wall: heights jump 30 m along gx = 10 (grade 1.25) except at gy 12.
        for gx in 10..SIDE.min(40) {
            for gy in 0..=20 {
                if gy != 12 {
                    g.heights[gx * SIDE + gy] = 15;
                }
            }
        }
        // Behind the wall the ground stays high, so gy 12 is a ramp only at
        // the wall: make the column gx = 10, gy = 12 a step of 1 (2 m).
        let from = WorldGrid::vertex_world(2, 2);
        let to = WorldGrid::vertex_world(18, 2);
        let path = find_with(&g, kind, from, to);
        // The step of 30 m in 24 m is a wall everywhere but at the ramp,
        // and the ramp climbs 30 m too: no way up.
        assert!(path.is_none());
        // Lower the ramp to 10 m (grade 0.42): passable.
        g.heights[10 * SIDE + 12] = 5;
        // and the vertices beyond it climb gently
        g.heights[11 * SIDE + 12] = 10;
        let path = find_with(&g, kind, from, to).unwrap();
        assert!(passes(&path, (10, 12)), "{path:?}");

        // Roads: on flat ground a road detour of a few vertices beats
        // the straight line when it is at most 1/0.7 times longer.
        let mut g = flat(4);
        for gx in 2..=18 {
            set_road(&mut g, gx, 4);
        }
        for gy in 2..=4 {
            set_road(&mut g, 2, gy);
            set_road(&mut g, 18, gy);
        }
        let path = find_with(&g, kind, from, to).unwrap();
        assert!(passes(&path, (10, 4)), "{path:?}");
        assert!(length(&path) > 1.1 * from.distance(to));
    }

    #[test]
    fn collinear_runs_collapse() {
        let pts: Vec<Vec2> = (0..5).map(|i| Vec2::new(i as f32, 0.0)).collect();
        assert_eq!(
            prune_collinear(pts),
            vec![Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0)]
        );
        let bent = vec![
            Vec2::ZERO,
            Vec2::new(1.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 1.0),
            Vec2::new(2.0, 2.0),
        ];
        assert_eq!(
            prune_collinear(bent),
            vec![Vec2::ZERO, Vec2::new(2.0, 0.0), Vec2::new(2.0, 2.0)]
        );
        // A doubling back is a corner, not collinear.
        let back = vec![Vec2::ZERO, Vec2::new(2.0, 0.0), Vec2::new(1.0, 0.0)];
        assert_eq!(prune_collinear(back.clone()), back);
    }

    /// Needs AC_DATA_DIR: Holtburg to Arwic exists, stays near the
    /// straight line and off the water; a goal in the sea fails fast.
    #[test]
    fn holtburg_to_arwic_over_real_terrain() {
        let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
            return;
        };
        let assets = crate::Assets::open(dir).unwrap();
        let region = assets.region().unwrap();
        let cache = std::env::temp_dir().join("acreborn-test-cache");
        let grid = WorldGrid::load_cached(&assets, &cache).unwrap();
        let map = |ns: f32, ew: f32| Vec2::new((ew + 102.0) * 240.0, (ns + 102.0) * 240.0);
        let holtburg = map(42.1, 33.6);
        let arwic = map(33.6, 56.8);
        let t0 = std::time::Instant::now();
        let path = find(&grid, &region, holtburg, arwic).expect("a route");
        let took = t0.elapsed();
        let len = length(&path);
        let straight = holtburg.distance(arwic);
        eprintln!(
            "Holtburg -> Arwic: {} waypoints, {len:.0} m ({:.2}x straight), {took:?}",
            path.len(),
            len / straight
        );
        assert!(took.as_secs_f32() < 2.0, "{took:?}");
        assert!(len <= 1.6 * straight, "{len} vs {straight}");
        assert_eq!(path[0], holtburg);
        assert_eq!(*path.last().unwrap(), arwic);
        // No waypoint stands in water, and the legs between them cross
        // water (the bridges over the Prosper) for only a small part of
        // the way, never the sea.
        let mut wet = 0.0;
        for w in path.windows(2) {
            let n = (w[0].distance(w[1]) / SPACING).ceil().max(1.0) as usize;
            for i in 0..=n {
                let p = w[0].lerp(w[1], i as f32 / n as f32);
                let (x, y) = WorldGrid::nearest_vertex(p);
                let kind = water_kind(&region, grid.terrain_type(x, y));
                assert_ne!(kind, Water::Sea, "{p:?} is in the sea");
                if kind != Water::Land {
                    wet += w[0].distance(w[1]) / n as f32;
                }
            }
        }
        for p in &path {
            let (x, y) = WorldGrid::nearest_vertex(*p);
            assert!(
                !is_water(&region, grid.terrain_type(x, y)),
                "waypoint {p:?} is on water"
            );
        }
        eprintln!("  in water: {wet:.0} m");
        assert!(wet < 0.05 * len, "{wet} m of {len} in water");
        // Far out at sea, west of everything.
        let t0 = std::time::Instant::now();
        assert!(find(&grid, &region, holtburg, map(42.0, -101.5)).is_none());
        assert!(t0.elapsed().as_secs_f32() < 1.0);
    }
}
