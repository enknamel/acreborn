//! Navigation for one landblock: walkable points sampled on a grid over
//! the static collision (and the terrain outdoors), joined where a
//! capsule can walk between them, searched with A*.
//!
//! The graph is built lazily and cached by whoever owns the collision
//! world (the client's `Player`). Nodes are feet positions on a floor,
//! snapped away from walls the way the walking code pushes a capsule
//! out; a column of the grid holds one node per level, so stacked
//! dungeon floors do not merge. Edges are directed (a ledge you can walk
//! off is not one you can climb) and validated by sampling the capsule
//! every half metre along the segment: floor continuity within the
//! capsule's step limits, no wall contact, head room, and a clear ray at
//! chest height. `find_path` runs A* between the nodes nearest the two
//! endpoints and then string-pulls the result with the same edge check,
//! so every consecutive pair of waypoints is walkable in a straight line.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::time::{Duration, Instant};

use glam::{Vec2, Vec3};

use crate::collision::{Capsule, CollisionWorld};
use crate::landblock::LandblockScene;
use crate::{lbid, BLOCK_SIZE};

/// Grid spacing inside dungeons and buildings (metres).
pub const INDOOR_SPACING: f32 = 1.5;
/// Grid spacing over open terrain.
pub const OUTDOOR_SPACING: f32 = 4.0;
/// How far apart the capsule is sampled along an edge.
const SUBSTEP: f32 = 0.5;
/// Height above the feet of the ray that must be clear between samples.
const CHEST: f32 = 1.0;
/// Levels closer than this in one column are one floor.
const LEVEL_MERGE: f32 = 0.3;
/// Largest height difference between two neighbouring nodes worth
/// testing (stairs and ramps within one grid step).
const MAX_EDGE_RISE: f32 = 2.0;
/// A snapped node may move at most this fraction of the spacing.
const SNAP_FRACTION: f32 = 0.6;

/// Where the ground is: static collision plus, outdoors, a terrain
/// height function over world `(x, y)`.
pub struct Ground<'a> {
    pub collision: &'a CollisionWorld,
    pub terrain: Option<&'a dyn Fn(f32, f32) -> Option<f32>>,
}

impl Ground<'_> {
    /// The surface a capsule with its feet at `p` stands on: the highest
    /// interior floor within step range, else the higher of an outdoor
    /// floor and the terrain (the rule `Player::update` walks by).
    pub fn surface_at(&self, p: Vec3, cap: &Capsule) -> Option<(f32, u32)> {
        let floor = self.collision.floor_at(p, cap.step_up, cap.step_down);
        match (floor, self.terrain) {
            (Some((z, cell)), _) if cell != 0 => Some((z, cell)),
            (floor, Some(terrain)) => {
                let t = terrain(p.x, p.y)?;
                match floor {
                    Some((z, _)) if z >= t => Some((z, 0)),
                    _ if t <= p.z + cap.step_up && t >= p.z - cap.step_down => Some((t, 0)),
                    _ => None,
                }
            }
            (floor, None) => floor,
        }
    }

    /// The capsule fits at `feet`: touching no wall, head room above.
    fn fits(&self, feet: Vec3, cap: &Capsule) -> bool {
        if self
            .collision
            .wall_contact(feet, cap.radius, cap.height, cap.step_up)
        {
            return false;
        }
        match self.collision.ceiling_at(feet, cap.radius) {
            Some(cz) => cz - feet.z >= cap.height,
            None => true,
        }
    }

    /// Sample the walk from `a` to `b` (feet positions) every
    /// [`SUBSTEP`]: the capsule must fit at every sample, the floor must
    /// continue, and the chest ray between samples must be clear. Returns
    /// whether the walk is possible from `a` to `b` and from `b` to `a`
    /// (they differ by the step-up and step-down limits).
    pub fn walkable(&self, a: Vec3, b: Vec3, cap: &Capsule) -> (bool, bool) {
        let d = b - a;
        let len = flat(d).length();
        let steps = (len / SUBSTEP).ceil().max(1.0) as usize;
        // Track the floor with the wider of the two limits both ways and
        // judge the profile per direction afterwards.
        let range = cap.step_up.max(cap.step_down);
        let probe = Capsule {
            step_up: range,
            step_down: range,
            ..*cap
        };
        let (mut forward, mut backward) = (true, true);
        let mut prev = a;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let at = Vec3::new(a.x + d.x * t, a.y + d.y * t, prev.z);
            let Some((z, _)) = self.surface_at(at, &probe) else {
                return (false, false);
            };
            let here = Vec3::new(at.x, at.y, z);
            if !self.fits(here, cap) || !line_clear(self.collision, prev, here) {
                return (false, false);
            }
            let dz = here.z - prev.z;
            if dz > cap.step_up || dz < -cap.step_down {
                forward = false;
            }
            if -dz > cap.step_up || -dz < -cap.step_down {
                backward = false;
            }
            if !forward && !backward {
                return (false, false);
            }
            prev = here;
        }
        // We must have arrived on b's floor, not one above or below it.
        if (prev.z - b.z).abs() > LEVEL_MERGE {
            return (false, false);
        }
        (forward, backward)
    }
}

/// A straight walk from `a` to `b` has a clear ray at chest height:
/// parallel to the ground, or level with the start when `b` is a drop
/// (walking off a ledge is fine; the ray should not dip into its face).
pub fn line_clear(collision: &CollisionWorld, a: Vec3, b: Vec3) -> bool {
    let up = Vec3::new(0.0, 0.0, CHEST);
    let end = if b.z < a.z - LEVEL_MERGE {
        Vec3::new(b.x, b.y, a.z)
    } else {
        b
    };
    collision.segment_hit(a + up, end + up).is_none()
}

fn flat(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.y)
}

#[derive(Debug, Clone, Copy)]
pub struct NavNode {
    /// Feet position, world space.
    pub pos: Vec3,
    /// Interior cell the floor belongs to, 0 outdoors.
    pub cell: u32,
    /// Lattice column the node was sampled in (it may have been snapped
    /// a little way out of it).
    pub column: (i32, i32),
}

/// Columns per chunk side: chunks are built on demand, so a path only
/// pays for the part of the block it crosses (24 m squares indoors).
const CHUNK: i32 = 16;
/// How far `nearest` looks around a point, in columns.
const NEAREST_REACH: i32 = 2;

pub struct NavGraph {
    pub spacing: f32,
    pub capsule: Capsule,
    /// Lattice range (inclusive) the graph may cover.
    gx: (i32, i32),
    gy: (i32, i32),
    pub nodes: Vec<NavNode>,
    /// Outgoing neighbours per node.
    edges: Vec<Vec<u32>>,
    /// Nodes per grid column `(x / spacing, y / spacing)`.
    columns: HashMap<(i32, i32), Vec<u32>>,
    /// Chunks whose nodes and edges exist.
    built: HashSet<(i32, i32)>,
    /// Time spent building so far.
    pub build_time: Duration,
}

impl NavGraph {
    /// An empty graph over the rectangle `min..=max` (world x, y) on a
    /// lattice of `spacing`, filled in chunk by chunk as paths need it.
    pub fn new(min: Vec2, max: Vec2, spacing: f32, cap: &Capsule) -> Self {
        NavGraph {
            spacing,
            capsule: *cap,
            gx: (
                (min.x / spacing).floor() as i32,
                (max.x / spacing).ceil() as i32,
            ),
            gy: (
                (min.y / spacing).floor() as i32,
                (max.y / spacing).ceil() as i32,
            ),
            nodes: Vec::new(),
            edges: Vec::new(),
            columns: HashMap::new(),
            built: HashSet::new(),
            build_time: Duration::ZERO,
        }
    }

    /// The graph of an assembled landblock: the fine lattice in dungeons
    /// and blocks with buildings, the coarse one over open terrain. The
    /// `Ground` passed to the other methods must describe the same block
    /// (its collision world and, unless it is a dungeon, its terrain).
    pub fn for_scene(scene: &LandblockScene, collision: &CollisionWorld, cap: &Capsule) -> Self {
        let origin = lbid::world_origin(scene.id);
        let square = (
            Vec2::new(origin.x, origin.y),
            Vec2::new(origin.x + BLOCK_SIZE, origin.y + BLOCK_SIZE),
        );
        let hull = collision
            .bounds()
            .map(|(lo, hi)| (flat(lo), flat(hi)))
            .unwrap_or(square);
        if scene.is_dungeon {
            NavGraph::new(hull.0, hull.1, INDOOR_SPACING, cap)
        } else {
            let margin = Vec2::splat(2.0 * OUTDOOR_SPACING);
            let min = square.0.min(hull.0).max(square.0 - margin);
            let max = square.1.max(hull.1).min(square.1 + margin);
            let spacing = if scene.cells.is_empty() {
                OUTDOOR_SPACING
            } else {
                INDOOR_SPACING
            };
            NavGraph::new(min, max, spacing, cap)
        }
    }

    /// Build every chunk now (tests and benchmarks; paths build lazily).
    pub fn build_all(&mut self, ground: &Ground) {
        let (cx0, cy0) = chunk_of(self.gx.0, self.gy.0);
        let (cx1, cy1) = chunk_of(self.gx.1, self.gy.1);
        for cx in cx0..=cx1 {
            for cy in cy0..=cy1 {
                self.ensure_chunk(ground, cx, cy);
            }
        }
    }

    /// Nodes and edges of one chunk, if not there yet. Edges to nodes in
    /// neighbouring chunks are added when the second of the two chunks is
    /// built, so each pair is tested once.
    fn ensure_chunk(&mut self, ground: &Ground, cx: i32, cy: i32) {
        if self.built.contains(&(cx, cy)) {
            return;
        }
        let (cx0, cy0) = chunk_of(self.gx.0, self.gy.0);
        let (cx1, cy1) = chunk_of(self.gx.1, self.gy.1);
        if cx < cx0 || cx > cx1 || cy < cy0 || cy > cy1 {
            return;
        }
        let started = Instant::now();
        self.built.insert((cx, cy));
        let cap = self.capsule;
        let mut fresh: Vec<u32> = Vec::new();
        for gx in (cx * CHUNK..(cx + 1) * CHUNK).filter(|x| (self.gx.0..=self.gx.1).contains(x)) {
            for gy in (cy * CHUNK..(cy + 1) * CHUNK).filter(|y| (self.gy.0..=self.gy.1).contains(y))
            {
                let x = gx as f32 * self.spacing;
                let y = gy as f32 * self.spacing;
                let mut levels = ground.collision.floors_at_xy(x, y);
                if let Some(t) = ground.terrain.and_then(|t| t(x, y)) {
                    levels.push((t, 0));
                }
                levels.sort_by(|a, b| b.0.total_cmp(&a.0));
                let mut placed: Vec<f32> = Vec::new();
                for (z, _) in levels {
                    if placed.iter().any(|&pz| (pz - z).abs() < LEVEL_MERGE) {
                        continue;
                    }
                    let Some(node) = self.place(ground, Vec3::new(x, y, z), (gx, gy), &cap) else {
                        continue;
                    };
                    if placed
                        .iter()
                        .any(|&pz| (pz - node.pos.z).abs() < LEVEL_MERGE)
                    {
                        continue;
                    }
                    placed.push(node.pos.z);
                    let id = self.nodes.len() as u32;
                    self.nodes.push(node);
                    self.edges.push(Vec::new());
                    self.columns.entry((gx, gy)).or_default().push(id);
                    fresh.push(id);
                }
            }
        }
        const DIRS: [(i32, i32); 8] = [
            (1, 0),
            (0, 1),
            (1, 1),
            (1, -1),
            (-1, 0),
            (0, -1),
            (-1, -1),
            (-1, 1),
        ];
        for &i in &fresh {
            let a = self.nodes[i as usize].pos;
            let (gx, gy) = self.nodes[i as usize].column;
            for (k, (dx, dy)) in DIRS.iter().enumerate() {
                let col = (gx + dx, gy + dy);
                let other = chunk_of(col.0, col.1);
                // Inside the chunk, the four forward directions cover
                // every pair; toward an older chunk, all eight.
                if other == (cx, cy) {
                    if k >= 4 {
                        continue;
                    }
                } else if !self.built.contains(&other) {
                    continue;
                }
                let Some(others) = self.columns.get(&col) else {
                    continue;
                };
                for j in others.clone() {
                    let b = self.nodes[j as usize].pos;
                    if (a.z - b.z).abs() > MAX_EDGE_RISE {
                        continue;
                    }
                    let (fwd, back) = ground.walkable(a, b, &cap);
                    if fwd {
                        self.edges[i as usize].push(j);
                    }
                    if back {
                        self.edges[j as usize].push(i);
                    }
                }
            }
        }
        self.build_time += started.elapsed();
    }

    /// Build the chunks covering the columns `gx0..=gx1` x `gy0..=gy1`.
    fn ensure_columns(&mut self, ground: &Ground, gx0: i32, gx1: i32, gy0: i32, gy1: i32) {
        let (cx0, cy0) = chunk_of(gx0, gy0);
        let (cx1, cy1) = chunk_of(gx1, gy1);
        for cx in cx0..=cx1 {
            for cy in cy0..=cy1 {
                self.ensure_chunk(ground, cx, cy);
            }
        }
    }

    /// Make sure every neighbour column of `node` lies in a built chunk,
    /// so its edge list is complete.
    fn ensure_neighbours(&mut self, ground: &Ground, node: u32) {
        let (gx, gy) = self.nodes[node as usize].column;
        let here = chunk_of(gx, gy);
        if chunk_of(gx - 1, gy - 1) == here && chunk_of(gx + 1, gy + 1) == here {
            return;
        }
        self.ensure_columns(ground, gx - 1, gx + 1, gy - 1, gy + 1);
    }

    /// Snap a sample point out of walls and onto its floor; `None` when
    /// no capsule stands there.
    fn place(
        &self,
        ground: &Ground,
        p: Vec3,
        column: (i32, i32),
        cap: &Capsule,
    ) -> Option<NavNode> {
        let q = ground
            .collision
            .resolve_above(p, cap.radius, cap.height, cap.step_up);
        if flat(q - p).length() > SNAP_FRACTION * self.spacing {
            return None;
        }
        let (z, cell) = ground.surface_at(Vec3::new(q.x, q.y, p.z), cap)?;
        let feet = Vec3::new(q.x, q.y, z);
        ground.fits(feet, cap).then_some(NavNode {
            pos: feet,
            cell,
            column,
        })
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.iter().map(Vec::len).sum()
    }

    /// Chunks built so far.
    pub fn chunk_count(&self) -> usize {
        self.built.len()
    }

    pub fn neighbours(&self, node: u32) -> &[u32] {
        &self.edges[node as usize]
    }

    /// The node nearest `p` within a few grid steps, preferring the same
    /// level (height differences count triple). Builds what it looks at.
    pub fn nearest(&mut self, ground: &Ground, p: Vec3) -> Option<u32> {
        let gx = (p.x / self.spacing).round() as i32;
        let gy = (p.y / self.spacing).round() as i32;
        let r = NEAREST_REACH;
        self.ensure_columns(ground, gx - r, gx + r, gy - r, gy + r);
        let mut best: Option<(f32, u32)> = None;
        for dx in -r..=r {
            for dy in -r..=r {
                let Some(ids) = self.columns.get(&(gx + dx, gy + dy)) else {
                    continue;
                };
                for &id in ids {
                    let d = self.nodes[id as usize].pos - p;
                    if d.z.abs() > MAX_EDGE_RISE {
                        continue;
                    }
                    let score = flat(d).length_squared() + (3.0 * d.z).powi(2);
                    if best.map(|(s, _)| score < s).unwrap_or(true) {
                        best = Some((score, id));
                    }
                }
            }
        }
        best.map(|(_, id)| id)
    }

    /// A* over the nodes from the one nearest `start` to the one nearest
    /// `goal`, building chunks as the search reaches them. The returned
    /// waypoints end with `goal` itself; the start is not included.
    /// `None` when either end has no node nearby or the two are not
    /// connected. A start inside a wall or a post (a spawn point in a
    /// marker) is first pushed out, as the walking code would.
    pub fn find_path(&mut self, ground: &Ground, start: Vec3, goal: Vec3) -> Option<Vec<Vec3>> {
        let cap = self.capsule;
        let start = ground
            .collision
            .resolve_above(start, cap.radius, cap.height, cap.step_up);
        let s = self.nearest(ground, start)?;
        let g = self.nearest(ground, goal)?;
        let nodes = self.astar(ground, s, g)?;
        let mut points: Vec<Vec3> = nodes.iter().map(|&n| self.nodes[n as usize].pos).collect();
        points.push(goal);
        Some(self.smooth(ground, start, points))
    }

    fn astar(&mut self, ground: &Ground, start: u32, goal: u32) -> Option<Vec<u32>> {
        #[derive(PartialEq)]
        struct Open {
            f: f32,
            node: u32,
        }
        impl Eq for Open {}
        impl PartialOrd for Open {
            fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
                Some(self.cmp(o))
            }
        }
        impl Ord for Open {
            fn cmp(&self, o: &Self) -> Ordering {
                o.f.total_cmp(&self.f)
            }
        }
        let goal_pos = self.nodes[goal as usize].pos;
        // Nodes appear as chunks get built; the maps grow with them.
        let mut best: HashMap<u32, f32> = HashMap::new();
        let mut came: HashMap<u32, u32> = HashMap::new();
        let mut closed: HashSet<u32> = HashSet::new();
        let mut open = BinaryHeap::new();
        best.insert(start, 0.0);
        open.push(Open {
            f: self.nodes[start as usize].pos.distance(goal_pos),
            node: start,
        });
        while let Some(Open { node, .. }) = open.pop() {
            if node == goal {
                let mut path = vec![goal];
                let mut cur = goal;
                while cur != start {
                    cur = came[&cur];
                    path.push(cur);
                }
                path.reverse();
                return Some(path);
            }
            if !closed.insert(node) {
                continue;
            }
            self.ensure_neighbours(ground, node);
            let here = self.nodes[node as usize].pos;
            let cost = best[&node];
            for &next in &self.edges[node as usize] {
                if closed.contains(&next) {
                    continue;
                }
                let np = self.nodes[next as usize].pos;
                let g = cost + here.distance(np);
                if best.get(&next).map(|&b| g < b).unwrap_or(true) {
                    best.insert(next, g);
                    came.insert(next, node);
                    open.push(Open {
                        f: g + np.distance(goal_pos),
                        node: next,
                    });
                }
            }
        }
        None
    }

    /// String-pulling: from each kept point, skip ahead to the furthest
    /// point (within a bounded lookahead) still reachable by a straight
    /// walk. Every consecutive pair of the result passes `walkable` and
    /// `line_clear`.
    fn smooth(&self, ground: &Ground, start: Vec3, points: Vec<Vec3>) -> Vec<Vec3> {
        const LOOKAHEAD: usize = 12;
        let cap = &self.capsule;
        let mut out = Vec::with_capacity(points.len());
        let mut from = start;
        let mut i = 0;
        while i < points.len() {
            let mut best = i;
            let last = (i + LOOKAHEAD).min(points.len() - 1);
            for j in (i + 1..=last).rev() {
                let (fwd, _) = ground.walkable(from, points[j], cap);
                if fwd && line_clear(ground.collision, from, points[j]) {
                    best = j;
                    break;
                }
            }
            out.push(points[best]);
            from = points[best];
            i = best + 1;
        }
        out
    }
}

fn chunk_of(gx: i32, gy: i32) -> (i32, i32) {
    (gx.div_euclid(CHUNK), gy.div_euclid(CHUNK))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A quad `a b c d`, counter-clockwise seen from its normal's side.
    fn quad(w: &mut CollisionWorld, a: Vec3, b: Vec3, c: Vec3, d: Vec3, cell: u32) {
        w.add_tri(a, b, c, cell, false);
        w.add_tri(a, c, d, cell, false);
    }

    fn floor(w: &mut CollisionWorld, x0: f32, x1: f32, y0: f32, y1: f32, z: f32, cell: u32) {
        quad(
            w,
            Vec3::new(x0, y0, z),
            Vec3::new(x1, y0, z),
            Vec3::new(x1, y1, z),
            Vec3::new(x0, y1, z),
            cell,
        );
    }

    /// A wall along the y axis at `x` from `y0` to `y1`, 3 m tall, facing -x.
    fn wall_x(w: &mut CollisionWorld, x: f32, y0: f32, y1: f32, cell: u32) {
        w.add_tri(
            Vec3::new(x, y0, 0.0),
            Vec3::new(x, y0, 3.0),
            Vec3::new(x, y1, 3.0),
            cell,
            true,
        );
        w.add_tri(
            Vec3::new(x, y0, 0.0),
            Vec3::new(x, y1, 3.0),
            Vec3::new(x, y1, 0.0),
            cell,
            true,
        );
    }

    /// Two 8 m rooms side by side along x, sharing the wall at x = 8 with
    /// a doorway `door` wide centred on y = 0 (or none).
    fn two_rooms(door: Option<f32>) -> CollisionWorld {
        let mut w = CollisionWorld::default();
        floor(&mut w, 0.0, 8.0, -4.0, 4.0, 0.0, 1);
        floor(&mut w, 8.0, 16.0, -4.0, 4.0, 0.0, 2);
        // Outer walls.
        wall_x(&mut w, 0.0, -4.0, 4.0, 1);
        wall_x(&mut w, 16.0, -4.0, 4.0, 2);
        for y in [-4.0, 4.0] {
            w.add_tri(
                Vec3::new(0.0, y, 0.0),
                Vec3::new(16.0, y, 0.0),
                Vec3::new(16.0, y, 3.0),
                1,
                true,
            );
            w.add_tri(
                Vec3::new(0.0, y, 0.0),
                Vec3::new(16.0, y, 3.0),
                Vec3::new(0.0, y, 3.0),
                1,
                true,
            );
        }
        match door {
            Some(d) => {
                wall_x(&mut w, 8.0, -4.0, -d / 2.0, 1);
                wall_x(&mut w, 8.0, d / 2.0, 4.0, 1);
            }
            None => wall_x(&mut w, 8.0, -4.0, 4.0, 1),
        }
        w
    }

    fn assert_walkable_chain(ground: &Ground, start: Vec3, path: &[Vec3], cap: &Capsule) {
        let mut from = start;
        for &p in path {
            assert!(
                ground.walkable(from, p, cap).0,
                "{from} -> {p} not walkable"
            );
            assert!(
                line_clear(ground.collision, from, p),
                "{from} -> {p} not clear"
            );
            from = p;
        }
    }

    #[test]
    fn two_rooms_joined_by_a_door() {
        let w = two_rooms(Some(1.6));
        let ground = Ground {
            collision: &w,
            terrain: None,
        };
        let cap = Capsule::default();
        let mut g = NavGraph::new(Vec2::new(0.0, -4.0), Vec2::new(16.0, 4.0), 1.0, &cap);
        g.build_all(&ground);
        assert!(g.len() > 50, "{} nodes", g.len());
        assert!(g.edge_count() > g.len(), "{} edges", g.edge_count());
        // Nodes hug the walls no closer than the capsule's radius.
        for n in &g.nodes {
            assert!(
                n.pos.x >= 0.4 - 1e-3 && n.pos.x <= 16.0 - 0.4 + 1e-3,
                "{n:?}"
            );
            assert!(n.pos.y.abs() <= 4.0 - 0.4 + 1e-3, "{n:?}");
        }
        let start = Vec3::new(1.0, 3.0, 0.0);
        let goal = Vec3::new(15.0, 3.0, 0.0);
        // The straight line crosses the wall.
        assert!(!line_clear(&w, start, goal));
        let path = g
            .find_path(&ground, start, goal)
            .expect("a path through the door");
        assert_eq!(*path.last().unwrap(), goal);
        // It passes through the doorway, not the wall.
        let chain: Vec<Vec3> = std::iter::once(start).chain(path.iter().copied()).collect();
        let crossing = chain
            .windows(2)
            .find(|s| (s[0].x - 8.0) * (s[1].x - 8.0) <= 0.0)
            .expect("crosses x = 8");
        let t = (8.0 - crossing[0].x) / (crossing[1].x - crossing[0].x);
        let y = crossing[0].y + (crossing[1].y - crossing[0].y) * t;
        assert!(y.abs() < 0.8, "crossed the wall at y = {y}: {path:?}");
        assert_walkable_chain(&ground, start, &path, &cap);
        // Smoothing cut it down to a handful of turns.
        assert!(path.len() <= 4, "{} waypoints: {path:?}", path.len());
    }

    #[test]
    fn no_path_through_a_solid_wall() {
        let w = two_rooms(None);
        let ground = Ground {
            collision: &w,
            terrain: None,
        };
        let cap = Capsule::default();
        let mut g = NavGraph::new(Vec2::new(0.0, -4.0), Vec2::new(16.0, 4.0), 1.0, &cap);
        g.build_all(&ground);
        let start = Vec3::new(1.0, 3.0, 0.0);
        let goal = Vec3::new(15.0, 3.0, 0.0);
        assert!(g.find_path(&ground, start, goal).is_none());
        // Within one room the path is direct.
        let near = Vec3::new(7.0, -3.0, 0.0);
        let path = g.find_path(&ground, start, near).unwrap();
        assert_eq!(path, vec![near]);
    }

    #[test]
    fn a_door_too_narrow_for_the_capsule_is_closed() {
        let w = two_rooms(Some(0.6));
        let ground = Ground {
            collision: &w,
            terrain: None,
        };
        let cap = Capsule::default();
        let mut g = NavGraph::new(Vec2::new(0.0, -4.0), Vec2::new(16.0, 4.0), 1.0, &cap);
        g.build_all(&ground);
        let start = Vec3::new(1.0, 3.0, 0.0);
        let goal = Vec3::new(15.0, 3.0, 0.0);
        let path = g.find_path(&ground, start, goal);
        assert!(path.is_none(), "{path:?}");
    }

    #[test]
    fn ledges_are_one_way_and_levels_stay_apart() {
        // A low floor and a 1 m higher shelf next to it: walk off the
        // shelf, but not up onto it. A second floor 3 m up is a separate
        // level with its own nodes.
        let mut w = CollisionWorld::default();
        floor(&mut w, 0.0, 6.0, -3.0, 3.0, 0.0, 1);
        floor(&mut w, 6.0, 12.0, -3.0, 3.0, 1.0, 1);
        floor(&mut w, 0.0, 12.0, -3.0, 3.0, 3.5, 2);
        let ground = Ground {
            collision: &w,
            terrain: None,
        };
        let cap = Capsule::default();
        let mut g = NavGraph::new(Vec2::new(0.0, -3.0), Vec2::new(12.0, 3.0), 1.0, &cap);
        g.build_all(&ground);
        let low = Vec3::new(3.0, 0.0, 0.0);
        let high = Vec3::new(9.0, 0.0, 1.0);
        let upstairs = Vec3::new(3.0, 0.0, 3.5);
        assert!(
            g.find_path(&ground, high, low).is_some(),
            "walk off the shelf"
        );
        assert!(g.find_path(&ground, low, high).is_none(), "climb the shelf");
        assert!(
            g.find_path(&ground, low, upstairs).is_none(),
            "levels joined"
        );
        assert!(g
            .find_path(&ground, upstairs, Vec3::new(9.0, 0.0, 3.5))
            .is_some());
        let levels = g
            .nodes
            .iter()
            .filter(|n| (n.pos.x - 3.0).abs() < 0.1 && n.pos.y.abs() < 0.1)
            .count();
        assert_eq!(levels, 2, "one node per level in a column");
    }

    #[test]
    fn terrain_fills_in_outdoors_and_steep_slopes_do_not_connect() {
        let w = CollisionWorld::default();
        // A cliff: flat up to x = 10, then rising 2 m per metre.
        let terrain = |x: f32, _y: f32| Some(if x < 10.0 { 0.0 } else { (x - 10.0) * 2.0 });
        let ground = Ground {
            collision: &w,
            terrain: Some(&terrain),
        };
        let cap = Capsule::default();
        let mut g = NavGraph::new(Vec2::new(0.0, 0.0), Vec2::new(20.0, 8.0), 2.0, &cap);
        g.build_all(&ground);
        assert!(g.len() > 40);
        let flat_a = Vec3::new(1.0, 1.0, 0.0);
        let flat_b = Vec3::new(9.0, 7.0, 0.0);
        assert!(g.find_path(&ground, flat_a, flat_b).is_some());
        let top = Vec3::new(18.0, 4.0, 16.0);
        assert!(
            g.find_path(&ground, flat_a, top).is_none(),
            "climbed the cliff"
        );
    }
}
