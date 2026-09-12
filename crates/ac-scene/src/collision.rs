//! Static collision geometry for a landblock: world-space triangles from
//! the physics polygons of buildings, statics, scenery and interior cells
//! bucketed on a 4 m grid.
//!
//! This is a deliberately simple first cut, not the client's BSP/sphere
//! physics: a character is a vertical capsule; walls (steep triangles)
//! push it out horizontally, floors (flat triangles) set its height,
//! ceilings (down-facing triangles) cap it. Ledges no taller than the
//! capsule's step-up height are walked over rather than pushed back, the
//! way the client's `StepUp` transition retries a blocked move from
//! `step_up_height` higher and then steps down onto the walkable plane.

use std::collections::HashMap;

use ac_formats::gfxobj::{GfxObj, Polygon, Vertex};
use glam::{Mat4, Vec3};

use crate::interior::CellIndex;
use crate::landblock::LandblockScene;
use crate::model::{place, VertexTable};
use crate::{Assets, Result};

const GRID: f32 = 4.0;

/// Gravity in world units (m/s²); the client's `PhysicsGlobals` uses -9.8.
pub const GRAVITY: f32 = 9.8;

/// The vertical capsule that stands in for a character, feet at its
/// position. `step_up`/`step_down` mirror a setup's `step_up_height` and
/// `step_down_height` (0.6 m and 1.5 m for the human setups).
#[derive(Debug, Clone, Copy)]
pub struct Capsule {
    pub radius: f32,
    pub height: f32,
    /// Ledges up to this tall are climbed while walking.
    pub step_up: f32,
    /// Drops up to this deep are walked down without falling.
    pub step_down: f32,
}

impl Capsule {
    /// The same shape: a graph or route built for one serves the other.
    pub fn same(&self, other: &Capsule) -> bool {
        self.radius == other.radius
            && self.height == other.height
            && self.step_up == other.step_up
            && self.step_down == other.step_down
    }
}

impl Default for Capsule {
    fn default() -> Self {
        Capsule {
            radius: 0.48,
            height: 1.835,
            step_up: 0.6,
            step_down: 1.5,
        }
    }
}

/// Result of one walking step through static geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Walk {
    /// Feet position after wall, step and ceiling handling.
    pub pos: Vec3,
    /// The floor we stand on (`z`, cell id) if one was within step range;
    /// `None` means there is nothing under us and we should fall.
    pub floor: Option<(f32, u32)>,
    /// The capsule would not fit under a ceiling at the destination, so
    /// `pos` is the start position.
    pub blocked: bool,
}

/// Result of one vertical (falling or jumping) step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Vertical {
    /// Moved freely to this feet position.
    Free(Vec3),
    /// Landed on a floor: feet position and the floor's cell id.
    Landed(Vec3, u32),
    /// Head hit a ceiling: feet position pushed down to fit under it.
    Ceiling(Vec3),
}

#[derive(Debug, Clone, Copy)]
pub struct Tri {
    pub a: Vec3,
    pub b: Vec3,
    pub c: Vec3,
    pub normal: Vec3,
    /// Interior cell id this triangle belongs to, or 0 for outdoor geometry.
    pub cell: u32,
    /// Two-sided polygons block from either side; one-sided ones only keep
    /// you on their normal's side.
    pub two_sided: bool,
}

#[derive(Default)]
pub struct CollisionWorld {
    pub tris: Vec<Tri>,
    grid: HashMap<(i32, i32), Vec<u32>>,
    /// The interior cells the triangles came from, for
    /// [`inside_cell`](Self::inside_cell).
    cells: CellIndex,
}

/// Möller–Trumbore for either winding; returns the ray parameter (`d` is
/// not normalised, so 1.0 is the far end of the segment).
fn ray_triangle(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let pv = d.cross(e2);
    let det = e1.dot(pv);
    if det.abs() < 1e-10 {
        return None;
    }
    let inv = 1.0 / det;
    let tv = o - a;
    let u = tv.dot(pv) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qv = tv.cross(e1);
    let v = d.dot(qv) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(qv) * inv;
    (t >= 0.0).then_some(t)
}

fn cell_of(p: Vec3) -> (i32, i32) {
    ((p.x / GRID).floor() as i32, (p.y / GRID).floor() as i32)
}

impl CollisionWorld {
    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// Add one triangle (mostly for tests; scenes use `from_scene`).
    pub fn add_tri(&mut self, a: Vec3, b: Vec3, c: Vec3, cell: u32, two_sided: bool) {
        let n = (b - a).cross(c - a);
        if n.length_squared() < 1e-8 {
            return;
        }
        let idx = self.tris.len() as u32;
        self.tris.push(Tri {
            a,
            b,
            c,
            normal: n.normalize(),
            cell,
            two_sided,
        });
        let (x0, y0) = cell_of(a.min(b).min(c));
        let (x1, y1) = cell_of(a.max(b).max(c));
        for x in x0..=x1 {
            for y in y0..=y1 {
                self.grid.entry((x, y)).or_default().push(idx);
            }
        }
    }

    /// Take a copy of every triangle in `other`. Used to merge several
    /// landblocks into one world so a path can cross between them.
    pub fn absorb(&mut self, other: &CollisionWorld) {
        self.tris.reserve(other.tris.len());
        for t in &other.tris {
            self.add_tri(t.a, t.b, t.c, t.cell, t.two_sided);
        }
        self.cells.absorb(&other.cells);
    }

    /// Whether `p` stands inside one of the interior cells this world
    /// was built from, the way `EnvCell::point_in_cell` decides it;
    /// true when there are no interior cells at all.
    ///
    /// Interior geometry has two sides, and the outer one -- the top of
    /// the metre-thick slab a dungeon's floor is made of, the back of a
    /// wall -- is perfectly good up-facing collision sitting in a sealed
    /// void between the rooms. Nothing ever stands there; the client
    /// would not even count such a spot as part of the dungeon. A
    /// walkable grid laid over the raw triangles finds those surfaces
    /// and fills them with nodes that connect to nothing.
    pub fn inside_cell(&self, p: Vec3) -> bool {
        self.cells.is_empty() || self.cells.contains(p)
    }

    /// The same question asked the other way round, for deciding
    /// whether to *ignore* geometry: a world that knows of no interior
    /// cells answers `false`, because not knowing where the cells are
    /// must leave every triangle in place rather than discard them all.
    pub fn in_known_cell(&self, p: Vec3) -> bool {
        !self.cells.is_empty() && self.cells.contains(p)
    }

    /// Add a polygon set (physics polygons if present, else drawing
    /// polygons) transformed by `t`.
    fn add_polys(&mut self, verts: &[(u16, Vertex)], polys: &[(u16, Polygon)], t: Mat4, cell: u32) {
        let table = VertexTable::new(verts);
        let mut pts: Vec<Vec3> = Vec::new();
        for (_, p) in polys {
            pts.clear();
            pts.extend(
                p.vertex_ids
                    .iter()
                    .filter_map(|&id| table.get(id))
                    .map(|v| t.transform_point3(v.origin)),
            );
            let two_sided = p.cull == ac_formats::gfxobj::CullMode::None;
            for i in 1..pts.len().saturating_sub(1) {
                self.add_tri(pts[0], pts[i], pts[i + 1], cell, two_sided);
            }
        }
    }

    /// Add a placed object's geometry; `cell` is the interior cell it
    /// stands in (0 outdoors), so that standing on a door sill, a
    /// staircase or a chest inside a dungeon still counts as being in
    /// that cell.
    fn add_gfxobj(&mut self, g: &GfxObj, t: Mat4, cell: u32) {
        if !g.physics_polygons.is_empty() {
            self.add_polys(&g.vertices, &g.physics_polygons, t, cell);
        } else {
            self.add_polys(&g.vertices, &g.polygons, t, cell);
        }
    }

    /// Build from an assembled landblock: its placed parts (buildings,
    /// statics, scenery) and interior cells.
    pub fn from_scene(assets: &Assets, scene: &LandblockScene) -> Result<Self> {
        let mut w = CollisionWorld {
            cells: CellIndex::build(assets, scene),
            ..Default::default()
        };
        let cells_first = scene.is_dungeon;
        if !cells_first {
            for part in &scene.parts {
                if let Ok(g) = assets.gfxobj(part.gfxobj_id) {
                    w.add_gfxobj(&g, part.transform, 0);
                }
            }
        }
        for cell in &scene.cells {
            // Cell structures: physics polygons in cell space.
            if let Ok(env) = assets.environment(cell.environment_id) {
                if let Some((_, cs)) = env
                    .cells
                    .iter()
                    .find(|(k, _)| *k == cell.cell_structure as u32)
                {
                    // Physics polygons only. A cell structure without any
                    // is an air cell whose every face is a portal (the
                    // space above a hall's balcony): building its drawn
                    // polygons instead put an invisible floor and walls
                    // there. Of 3168 cell structures in the data, the two
                    // without physics polygons are such air cells.
                    w.add_polys(
                        &cs.vertices,
                        &cs.physics_polygons,
                        cell.transform,
                        cell.cell_id,
                    );
                }
            }
            for part in &cell.parts {
                if let Ok(g) = assets.gfxobj(part.gfxobj_id) {
                    w.add_gfxobj(&g, part.transform, cell.cell_id);
                }
            }
        }
        if cells_first {
            // A dungeon's block-level objects (portals, grates, statues)
            // also stand inside cells: tag them with the cell whose floor
            // is under them, else the nearest cell, so nothing in a
            // dungeon reads as outdoor geometry.
            for part in &scene.parts {
                let Ok(g) = assets.gfxobj(part.gfxobj_id) else {
                    continue;
                };
                let origin = part.transform.w_axis.truncate();
                let cell = w
                    .floor_at(origin + Vec3::new(0.0, 0.0, 0.5), 5.0, 50.0)
                    .map(|(_, c)| c)
                    .filter(|&c| c != 0)
                    .or_else(|| {
                        scene
                            .cells
                            .iter()
                            .min_by(|a, b| {
                                let da = (a.transform.w_axis.truncate() - origin).length();
                                let db = (b.transform.w_axis.truncate() - origin).length();
                                da.total_cmp(&db)
                            })
                            .map(|c| c.cell_id)
                    })
                    .unwrap_or(0);
                w.add_gfxobj(&g, part.transform, cell);
            }
        }
        Ok(w)
    }

    pub fn nearby(&self, p: Vec3, r: f32) -> impl Iterator<Item = &Tri> + '_ {
        let (x0, y0) = cell_of(p - Vec3::splat(r));
        let (x1, y1) = cell_of(p + Vec3::splat(r));
        let mut ids: Vec<u32> = Vec::new();
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(v) = self.grid.get(&(x, y)) {
                    ids.extend_from_slice(v);
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter().map(move |i| &self.tris[i as usize])
    }

    /// Nearest triangle along the segment from `from` to `to`, as a
    /// fraction of the segment (0..=1), ignoring facing. Used to keep the
    /// camera out of walls.
    pub fn segment_hit(&self, from: Vec3, to: Vec3) -> Option<f32> {
        let d = to - from;
        let len = d.length();
        if len < 1e-4 {
            return None;
        }
        let mid = (from + to) * 0.5;
        let mut best: Option<f32> = None;
        for t in self.nearby(mid, len * 0.5 + 0.1) {
            if let Some(f) = ray_triangle(from, d, t.a, t.b, t.c) {
                if f <= 1.0 && best.map(|b| f < b).unwrap_or(true) {
                    best = Some(f);
                }
            }
        }
        best
    }

    /// Axis-aligned bounds of every triangle, if there are any.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut it = self.tris.iter();
        let first = it.next()?;
        let (mut lo, mut hi) = (
            first.a.min(first.b).min(first.c),
            first.a.max(first.b).max(first.c),
        );
        for t in it {
            lo = lo.min(t.a).min(t.b).min(t.c);
            hi = hi.max(t.a).max(t.b).max(t.c);
        }
        Some((lo, hi))
    }

    /// Every floor (up-facing triangle) crossing the vertical line at
    /// `(x, y)`: heights with cell ids, highest first. A dungeon's stacked
    /// levels each show up once.
    pub fn floors_at_xy(&self, x: f32, y: f32) -> Vec<(f32, u32)> {
        let p = Vec3::new(x, y, 0.0);
        let mut out: Vec<(f32, u32)> = Vec::new();
        for t in self.nearby(p, 0.5) {
            if t.normal.z < 0.5 || !point_in_tri_xy(p, t) {
                continue;
            }
            let z = t.a.z - ((p.x - t.a.x) * t.normal.x + (p.y - t.a.y) * t.normal.y) / t.normal.z;
            out.push((z, t.cell));
        }
        out.sort_by(|a, b| b.0.total_cmp(&a.0));
        out
    }

    /// Height of the highest floor triangle directly under `p` within
    /// `max_drop` below and `max_rise` above, with its cell id.
    pub fn floor_at(&self, p: Vec3, max_rise: f32, max_drop: f32) -> Option<(f32, u32)> {
        let mut best: Option<(f32, u32)> = None;
        // The best among interior geometry alone. A building's exterior
        // shell carries a solid first-floor slab across the stairwell
        // the interior cells leave open, and it is the higher of the
        // two, so it wins a "highest floor" contest and roofs over the
        // stairs. The client never stands on it: inside a cell it tests
        // the cell's own geometry and the shell is not part of that.
        let mut indoors: Option<(f32, u32)> = None;
        for t in self.nearby(p, 0.5) {
            if t.normal.z < 0.5 {
                continue;
            }
            if !point_in_tri_xy(p, t) {
                continue;
            }
            // z on the plane at (p.x, p.y)
            let z = t.a.z - ((p.x - t.a.x) * t.normal.x + (p.y - t.a.y) * t.normal.y) / t.normal.z;
            if z > p.z + max_rise || z < p.z - max_drop {
                continue;
            }
            if best.map(|(bz, _)| z > bz).unwrap_or(true) {
                best = Some((z, t.cell));
            }
            if t.cell != 0 && indoors.map(|(bz, _)| z > bz).unwrap_or(true) {
                indoors = Some((z, t.cell));
            }
        }
        // Only ever swap one floor for another, never take the last one
        // away: where a cell has no floor of its own we are better off
        // standing on the shell than falling through the world.
        match (best, indoors) {
            (Some((_, 0)), Some(inside)) if self.in_known_cell(p + Vec3::new(0.0, 0.0, 0.1)) => {
                Some(inside)
            }
            _ => best,
        }
    }

    /// Push a capsule (feet at `p`, radius `r`, height `h`) out of steep
    /// triangles. Returns the corrected feet position.
    pub fn resolve(&self, p: Vec3, r: f32, h: f32) -> Vec3 {
        self.resolve_above(p, r, h, 0.0)
    }

    /// Like `resolve`, but the part of the capsule below `p.z + skirt` is
    /// ignored, so ledges no taller than `skirt` do not push back (the
    /// floor snap then climbs them).
    pub fn resolve_above(&self, p: Vec3, r: f32, h: f32, skirt: f32) -> Vec3 {
        self.resolve_from(None, p, r, h, skirt)
    }

    /// [`resolve_above`](Self::resolve_above) for a capsule that came
    /// from `from`: a one-sided triangle only pushes when the capsule
    /// started in front of it. Cell walls carry a separate back face, so
    /// without this the inner face pushes us out and the outer face
    /// pushes us through, and we end up outside the cell.
    pub fn resolve_from(&self, from: Option<Vec3>, p: Vec3, r: f32, h: f32, skirt: f32) -> Vec3 {
        let mut pos = p;
        let hi = h - r;
        let lo = (skirt + r).min(hi);
        let heights = [lo, (lo + hi) * 0.5, hi];
        for _ in 0..3 {
            let mut moved = false;
            for t in self.nearby(pos, r + 0.5) {
                if t.normal.z.abs() > 0.6 {
                    continue; // floor/ceiling
                }
                // Test the capsule at three heights against the triangle.
                for dz in heights {
                    let c = pos + Vec3::new(0.0, 0.0, dz);
                    let q = closest_point_on_tri(c, t);
                    let d = c - q;
                    let dist = d.length();
                    if dist >= r {
                        continue;
                    }
                    let mut push = if t.two_sided {
                        if dist > 1e-5 {
                            d / dist * (r - dist)
                        } else {
                            t.normal * r
                        }
                    } else {
                        // Stay on the normal's side: push out to r in front of the plane.
                        let sd = (c - t.a).dot(t.normal);
                        if sd >= r {
                            continue;
                        }
                        if let Some(f) = from {
                            // Already behind it when the move began: this
                            // face is not the one holding us.
                            let start = f + Vec3::new(0.0, 0.0, dz);
                            if (start - t.a).dot(t.normal) < -1e-3 {
                                continue;
                            }
                        }
                        if sd > 0.0 && dist > sd + 1e-4 {
                            // The nearest point is on the triangle's edge:
                            // the capsule is beside the face, not in front
                            // of it, and the way out is away from the edge.
                            // Pushing along the normal here held a walker
                            // still against the end of a wall they were
                            // walking past (Arwic's gate posts).
                            d / dist * (r - dist)
                        } else {
                            t.normal * (r - sd)
                        }
                    };
                    push.z = 0.0;
                    if push.length_squared() > 1e-10 {
                        pos += push;
                        moved = true;
                    }
                }
            }
            if !moved {
                break;
            }
        }
        if let Some(f) = from {
            // Pushes from opposing faces (a wall and a railing a body
            // width apart) can leave the capsule behind a wall it started
            // in front of. A one-sided face is never crossed from its
            // front: stay where the move began instead.
            for dz in heights {
                let start = f + Vec3::new(0.0, 0.0, dz);
                let end = pos + Vec3::new(0.0, 0.0, dz);
                for t in self.nearby(pos, r + 0.5).chain(self.nearby(f, r + 0.5)) {
                    if t.two_sided || t.normal.z.abs() > 0.6 {
                        continue;
                    }
                    if crosses_front_to_back(start, end, t) {
                        return Vec3::new(f.x, f.y, p.z);
                    }
                }
            }
        }
        pos
    }

    /// Whether a capsule (feet at `p`, radius `r`, height `h`) touches a
    /// steep triangle above `p.z + skirt`: the contact test behind
    /// [`resolve_above`](Self::resolve_above), without the pushing (which
    /// can cancel out between two facing walls).
    pub fn wall_contact(&self, p: Vec3, r: f32, h: f32, skirt: f32) -> bool {
        let hi = h - r;
        let lo = (skirt + r).min(hi);
        let heights = [lo, (lo + hi) * 0.5, hi];
        for t in self.nearby(p, r + 0.5) {
            if t.normal.z.abs() > 0.6 {
                continue;
            }
            for dz in heights {
                let c = p + Vec3::new(0.0, 0.0, dz);
                let q = closest_point_on_tri(c, t);
                if (c - q).length() >= r {
                    continue;
                }
                if t.two_sided || (c - t.a).dot(t.normal) < r {
                    return true;
                }
            }
        }
        false
    }

    /// Lowest ceiling above the feet position `p` within the capsule's
    /// radius `r`: down-facing (or two-sided flat) triangles more than a
    /// hand's breadth above the feet. Returns the ceiling's z.
    pub fn ceiling_at(&self, p: Vec3, r: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        // The same, counting only interior geometry. A building's
        // outside is a separate model from the cells inside it, and the
        // two disagree: the exterior shell carries a solid first-floor
        // slab where the interior cells leave a stairwell. The client
        // never sees the disagreement because `EnvCell::FindCollisions`
        // tests the cell's own structure and statics alone, while the
        // shell belongs to the land cell -- so once you are inside, the
        // shell is not there. Ours is one triangle soup, so we make the
        // same distinction here.
        let mut indoors: Option<f32> = None;
        let min_z = p.z + 0.2;
        for t in self.nearby(p, r) {
            let facing_down = t.normal.z < -0.5 || (t.two_sided && t.normal.z > 0.5);
            if !facing_down {
                continue;
            }
            let z = if point_in_tri_xy(p, t) {
                // Directly overhead: the plane's z at (p.x, p.y).
                t.a.z - ((p.x - t.a.x) * t.normal.x + (p.y - t.a.y) * t.normal.y) / t.normal.z
            } else {
                // Off to the side: the triangle's closest point to the
                // capsule axis, if within the radius.
                let top = t.a.z.max(t.b.z).max(t.c.z).min(p.z + 100.0);
                let q = closest_point_on_tri(Vec3::new(p.x, p.y, top), t);
                if glam::Vec2::new(q.x - p.x, q.y - p.y).length() >= r {
                    continue;
                }
                q.z
            };
            if z < min_z {
                continue;
            }
            if best.map(|b| z < b).unwrap_or(true) {
                best = Some(z);
            }
            if t.cell != 0 && indoors.map(|b| z < b).unwrap_or(true) {
                indoors = Some(z);
            }
        }
        if best != indoors && self.in_known_cell(p + Vec3::new(0.0, 0.0, 0.1)) {
            return indoors;
        }
        best
    }

    /// One walking step of the capsule from `from` to `to` (same z):
    /// walls push it out (ignoring ledges below `step_up`), the highest
    /// floor within `step_up` above or `step_down` below sets the new z,
    /// and a ceiling the capsule does not fit under blocks the move.
    pub fn walk(&self, from: Vec3, to: Vec3, cap: &Capsule) -> Walk {
        let mut target = to;
        // A low overhang beside the path (a brazier, a bracket) pushes
        // the capsule aside like a wall; only one directly overhead
        // blocks the step.
        let mut overhead = false;
        for _ in 0..3 {
            let pos = self.resolve_from(Some(from), target, cap.radius, cap.height, cap.step_up);
            let probe = Vec3::new(pos.x, pos.y, from.z);
            let floor = self.floor_at(probe, cap.step_up, cap.step_down);
            let feet = Vec3::new(pos.x, pos.y, floor.map(|(z, _)| z).unwrap_or(from.z));
            match self.overhang_escape(feet, cap.radius, cap.height) {
                None => {
                    overhead = true;
                    break;
                }
                Some(push) if push.length_squared() < 1e-8 => {
                    return Walk {
                        pos: feet,
                        floor,
                        blocked: false,
                    };
                }
                Some(push) => target = Vec3::new(feet.x + push.x, feet.y + push.y, to.z),
            }
        }
        if !overhead {
            // Three shoves and still somewhere to be pushed away from.
            // That is not a wall, it is a doorway: one jamb pushes the
            // capsule towards the other and the other pushes it back,
            // and the loop runs out mid-argument. Refusing the step
            // here pinned characters in doorways -- able to stand on
            // either side and unable to walk between, while the route
            // they were following was perfectly good and the server
            // was waiting for them to arrive.
            //
            // Nothing is directly overhead, so take the step as first
            // asked and let the wall resolution do what it does for
            // every other step.
            let pos = self.resolve_from(Some(from), to, cap.radius, cap.height, cap.step_up);
            let probe = Vec3::new(pos.x, pos.y, from.z);
            let floor = self.floor_at(probe, cap.step_up, cap.step_down);
            let feet = Vec3::new(pos.x, pos.y, floor.map(|(z, _)| z).unwrap_or(from.z));
            if self.overhang_escape(feet, cap.radius, cap.height).is_some() {
                return Walk {
                    pos: feet,
                    floor,
                    blocked: false,
                };
            }
        }
        Walk {
            pos: from,
            floor: self.floor_at(from, cap.step_up, cap.step_down),
            blocked: true,
        }
    }

    /// How a capsule (feet at `p`, radius `r`, height `h`) gets out from
    /// under a ceiling lower than its height: the horizontal push that
    /// clears every offending triangle (zero when none is too low), or
    /// `None` when one is directly overhead.
    pub fn overhang_escape(&self, p: Vec3, r: f32, h: f32) -> Option<glam::Vec2> {
        let min_z = p.z + 0.2;
        let mut push = glam::Vec2::ZERO;
        // Standing inside a building, the ground outside is not a
        // ceiling. The client chooses the cells it collides against
        // before it collides with anything, and a capsule inside an
        // interior cell never meets the land cell's terrain at all --
        // the same rule `surface_at` walks by.
        //
        // Without this, a shop whose floor lies below the street had
        // the underside of the street hanging over it, and every step
        // inside was refused as having something directly overhead.
        // The character could stand anywhere in the room and walk
        // nowhere: pinned, while the route it was given was sound and
        // the merchant four metres away waited.
        let indoors = self.in_known_cell(p + Vec3::new(0.0, 0.0, 0.1));
        for t in self.nearby(p, r) {
            if indoors && t.cell == 0 {
                continue;
            }
            let facing_down = t.normal.z < -0.5 || (t.two_sided && t.normal.z > 0.5);
            if !facing_down {
                continue;
            }
            if point_in_tri_xy(p, t) {
                let z =
                    t.a.z - ((p.x - t.a.x) * t.normal.x + (p.y - t.a.y) * t.normal.y) / t.normal.z;
                if z >= min_z && z - p.z < h {
                    return None;
                }
                continue;
            }
            let top = t.a.z.max(t.b.z).max(t.c.z).min(p.z + 100.0);
            let q = closest_point_on_tri(Vec3::new(p.x, p.y, top), t);
            let away = glam::Vec2::new(p.x - q.x, p.y - q.y);
            let d = away.length();
            if d >= r || q.z < min_z || q.z - p.z >= h {
                continue;
            }
            let dir = if d > 1e-4 {
                away / d
            } else {
                glam::Vec2::new(-t.normal.x, -t.normal.y).normalize_or(glam::Vec2::X)
            };
            let need = dir * (r - d + 0.02);
            // Keep the largest push per direction rather than summing
            // neighbouring facets of one object.
            if need.length_squared() > push.length_squared() {
                push = need;
            }
        }
        Some(push)
    }

    /// Move the capsule vertically by `dz` (negative = falling): land on
    /// the first floor crossed on the way down, or stop under the first
    /// ceiling the head reaches on the way up.
    pub fn vertical(&self, from: Vec3, dz: f32, cap: &Capsule) -> Vertical {
        let to = from + Vec3::new(0.0, 0.0, dz);
        if dz < 0.0 {
            // A floor up to a step above the feet counts too: a jump that
            // arrives a few centimetres under a ledge's top lands on it
            // instead of passing down through the slab.
            if let Some((z, cell)) = self.floor_at(from, cap.step_up, -dz) {
                return Vertical::Landed(Vec3::new(from.x, from.y, z), cell);
            }
            Vertical::Free(to)
        } else {
            match self.ceiling_at(from, cap.radius) {
                Some(cz) if cz - to.z < cap.height => {
                    Vertical::Ceiling(Vec3::new(from.x, from.y, (cz - cap.height).max(from.z)))
                }
                _ => Vertical::Free(to),
            }
        }
    }
}

/// Whether the segment `a`..`b` passes through triangle `t` from the side
/// its normal points to.
fn crosses_front_to_back(a: Vec3, b: Vec3, t: &Tri) -> bool {
    let sa = (a - t.a).dot(t.normal);
    let sb = (b - t.a).dot(t.normal);
    if sa < -1e-3 || sb >= -1e-3 {
        return false;
    }
    let hit = a + (b - a) * (sa / (sa - sb));
    (closest_point_on_tri(hit, t) - hit).length() < 0.05
}

fn point_in_tri_xy(p: Vec3, t: &Tri) -> bool {
    let (ax, ay) = (t.a.x, t.a.y);
    let (bx, by) = (t.b.x, t.b.y);
    let (cx, cy) = (t.c.x, t.c.y);
    let d1 = (p.x - bx) * (ay - by) - (ax - bx) * (p.y - by);
    let d2 = (p.x - cx) * (by - cy) - (bx - cx) * (p.y - cy);
    let d3 = (p.x - ax) * (cy - ay) - (cx - ax) * (p.y - ay);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

pub fn closest_point_on_tri(p: Vec3, t: &Tri) -> Vec3 {
    // Ericson, Real-Time Collision Detection 5.1.5
    let (a, b, c) = (t.a, t.b, t.c);
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return a + ab * v;
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return a + ac * w;
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return b + (c - b) * w;
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    a + ab * v + ac * w
}

/// Convenience: collision for a single model placed in the world.
pub fn from_model(assets: &Assets, model_id: u32, world: Mat4) -> Result<CollisionWorld> {
    let mut w = CollisionWorld::default();
    for part in place(assets, model_id, world)? {
        if let Ok(g) = assets.gfxobj(part.gfxobj_id) {
            w.add_gfxobj(&g, part.transform, 0);
        }
    }
    Ok(w)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A floor spanning a square, at height `z`, belonging to `cell`.
    fn slab(w: &mut CollisionWorld, x0: f32, x1: f32, y0: f32, y1: f32, z: f32, cell: u32) {
        let p = |x: f32, y: f32| Vec3::new(x, y, z);
        w.add_tri(p(x0, y0), p(x1, y0), p(x1, y1), cell, true);
        w.add_tri(p(x0, y0), p(x1, y1), p(x0, y1), cell, true);
    }

    /// A room whose floor lies below the ground outside it, as Holtburg's
    /// shops do: the town is built down a slope and the street runs over
    /// the top of them.
    fn cellar() -> CollisionWorld {
        let mut w = CollisionWorld::default();
        // The room, in its own cell.
        slab(&mut w, 0.0, 10.0, 0.0, 10.0, 0.0, 0x0111);
        // The street above it, outdoors.
        slab(&mut w, -5.0, 15.0, -5.0, 15.0, 3.0, 0);
        w
    }

    #[test]
    fn the_street_overhead_is_not_a_ceiling_when_you_are_under_it() {
        // `overhang_escape` reads any downward-facing surface above the
        // feet as something to duck out from under, and answers `None`
        // when one is directly overhead -- which the walk takes as "you
        // cannot go that way".
        //
        // Holtburg's shops sit below the road, so the underside of the
        // road hung over the room and every step inside was refused.
        // The character could stand anywhere in it and walk nowhere,
        // which read as running at a wall from the outside.
        let w = cellar();
        let cap = Capsule::default();
        let inside = Vec3::new(5.0, 5.0, 0.0);
        // The street is three metres up and the character is 1.835 tall,
        // so it genuinely is within head height -- which is why the
        // naive reading refuses.
        assert!(
            w.overhang_escape(inside, cap.radius, cap.height).is_some(),
            "the street was taken for a ceiling"
        );
    }

    #[test]
    fn a_real_ceiling_still_stops_you() {
        // The rule is about whose geometry it is, not about ignoring
        // low roofs: something overhead in the same cell still counts.
        let mut w = cellar();
        slab(&mut w, 0.0, 10.0, 0.0, 10.0, 1.0, 0x0111);
        let cap = Capsule::default();
        let inside = Vec3::new(5.0, 5.0, 0.0);
        assert!(
            w.overhang_escape(inside, cap.radius, cap.height).is_none(),
            "walked through a ceiling a metre over its head"
        );
    }

    #[test]
    fn a_step_inside_a_room_under_the_street_is_allowed() {
        // The same thing the walk itself sees: the step that was
        // refused, from one side of the room to the other.
        let w = cellar();
        let cap = Capsule::default();
        let from = Vec3::new(3.0, 5.0, 0.0);
        let to = Vec3::new(6.0, 5.0, 0.0);
        let walk = w.walk(from, to, &cap);
        assert!(!walk.blocked, "a step across the room was refused");
        assert!(walk.pos.distance(from) > 1.0, "it did not move");
    }
}
