//! Static collision geometry for a landblock: world-space triangles from
//! the physics polygons of buildings, statics, scenery and interior cells,
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

use crate::landblock::LandblockScene;
use crate::model::place;
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

impl Default for Capsule {
    fn default() -> Self {
        Capsule {
            radius: 0.4,
            height: 1.7,
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

    /// Add a polygon set (physics polygons if present, else drawing
    /// polygons) transformed by `t`.
    fn add_polys(&mut self, verts: &[(u16, Vertex)], polys: &[(u16, Polygon)], t: Mat4, cell: u32) {
        let lookup: HashMap<u16, Vec3> = verts
            .iter()
            .map(|(k, v)| (*k, t.transform_point3(v.origin)))
            .collect();
        for (_, p) in polys {
            let pts: Vec<Vec3> = p
                .vertex_ids
                .iter()
                .filter_map(|id| lookup.get(&(*id as u16)).copied())
                .collect();
            let two_sided = p.cull == ac_formats::gfxobj::CullMode::None;
            for i in 1..pts.len().saturating_sub(1) {
                self.add_tri(pts[0], pts[i], pts[i + 1], cell, two_sided);
            }
        }
    }

    fn add_gfxobj(&mut self, g: &GfxObj, t: Mat4) {
        if !g.physics_polygons.is_empty() {
            self.add_polys(&g.vertices, &g.physics_polygons, t, 0);
        } else {
            self.add_polys(&g.vertices, &g.polygons, t, 0);
        }
    }

    /// Build from an assembled landblock: its placed parts (buildings,
    /// statics, scenery) and interior cells.
    pub fn from_scene(assets: &Assets, scene: &LandblockScene) -> Result<Self> {
        let mut w = CollisionWorld::default();
        for part in &scene.parts {
            if let Ok(g) = assets.gfxobj(part.gfxobj_id) {
                w.add_gfxobj(&g, part.transform);
            }
        }
        for cell in &scene.cells {
            // Cell structures: physics polygons in cell space.
            let cell_id = cell.cell_id;
            let env_id = cell.environment_id;
            if let Ok(bytes) = assets.portal.read(env_id) {
                if let Ok(env) = ac_formats::environment::Environment::parse(env_id, &bytes) {
                    if let Some((_, cs)) = env
                        .cells
                        .iter()
                        .find(|(k, _)| *k == cell.cell_structure as u32)
                    {
                        let polys = if cs.physics_polygons.is_empty() {
                            &cs.polygons
                        } else {
                            &cs.physics_polygons
                        };
                        w.add_polys(&cs.vertices, polys, cell.transform, cell_id);
                    }
                }
            }
            for part in &cell.parts {
                if let Ok(g) = assets.gfxobj(part.gfxobj_id) {
                    w.add_gfxobj(&g, part.transform);
                }
            }
        }
        Ok(w)
    }

    fn nearby(&self, p: Vec3, r: f32) -> impl Iterator<Item = &Tri> + '_ {
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

    /// Height of the highest floor triangle directly under `p` within
    /// `max_drop` below and `max_rise` above, with its cell id.
    pub fn floor_at(&self, p: Vec3, max_rise: f32, max_drop: f32) -> Option<(f32, u32)> {
        let mut best: Option<(f32, u32)> = None;
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
        }
        best
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
                        t.normal * (r - sd)
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
        pos
    }

    /// Lowest ceiling above the feet position `p` within the capsule's
    /// radius `r`: down-facing (or two-sided flat) triangles more than a
    /// hand's breadth above the feet. Returns the ceiling's z.
    pub fn ceiling_at(&self, p: Vec3, r: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
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
        }
        best
    }

    /// One walking step of the capsule from `from` to `to` (same z):
    /// walls push it out (ignoring ledges below `step_up`), the highest
    /// floor within `step_up` above or `step_down` below sets the new z,
    /// and a ceiling the capsule does not fit under blocks the move.
    pub fn walk(&self, from: Vec3, to: Vec3, cap: &Capsule) -> Walk {
        let pos = self.resolve_above(to, cap.radius, cap.height, cap.step_up);
        let probe = Vec3::new(pos.x, pos.y, from.z);
        let floor = self.floor_at(probe, cap.step_up, cap.step_down);
        let feet = Vec3::new(pos.x, pos.y, floor.map(|(z, _)| z).unwrap_or(from.z));
        if let Some(cz) = self.ceiling_at(feet, cap.radius) {
            if cz - feet.z < cap.height {
                return Walk {
                    pos: from,
                    floor: self.floor_at(from, cap.step_up, cap.step_down),
                    blocked: true,
                };
            }
        }
        Walk {
            pos: feet,
            floor,
            blocked: false,
        }
    }

    /// Move the capsule vertically by `dz` (negative = falling): land on
    /// the first floor crossed on the way down, or stop under the first
    /// ceiling the head reaches on the way up.
    pub fn vertical(&self, from: Vec3, dz: f32, cap: &Capsule) -> Vertical {
        let to = from + Vec3::new(0.0, 0.0, dz);
        if dz < 0.0 {
            if let Some((z, cell)) = self.floor_at(from, 0.0, -dz) {
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

fn closest_point_on_tri(p: Vec3, t: &Tri) -> Vec3 {
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
            w.add_gfxobj(&g, part.transform);
        }
    }
    Ok(w)
}
