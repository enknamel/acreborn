//! Static collision geometry for a landblock: world-space triangles from
//! the physics polygons of buildings, statics, scenery and interior cells,
//! bucketed on a 4 m grid.
//!
//! This is a deliberately simple first cut, not the client's BSP/sphere
//! physics: a character is a vertical capsule; walls (steep triangles)
//! push it out horizontally, floors (flat triangles) set its height.

use std::collections::HashMap;

use ac_formats::gfxobj::{GfxObj, Polygon, Vertex};
use glam::{Mat4, Vec3};

use crate::landblock::LandblockScene;
use crate::model::place;
use crate::{Assets, Result};

const GRID: f32 = 4.0;

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

fn cell_of(p: Vec3) -> (i32, i32) {
    ((p.x / GRID).floor() as i32, (p.y / GRID).floor() as i32)
}

impl CollisionWorld {
    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    fn add_tri(&mut self, a: Vec3, b: Vec3, c: Vec3, cell: u32, two_sided: bool) {
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
        let mut pos = p;
        for _ in 0..3 {
            let mut moved = false;
            for t in self.nearby(pos, r + 0.5) {
                if t.normal.z.abs() > 0.6 {
                    continue; // floor/ceiling
                }
                // Test the capsule at three heights against the triangle.
                for frac in [0.25f32, 0.6, 0.95] {
                    let c = pos + Vec3::new(0.0, 0.0, h * frac);
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
