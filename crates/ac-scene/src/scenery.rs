//! Procedural scenery (trees, rocks, bushes...) for an outdoor landblock.
//!
//! The client does not store scenery in the DATs. For every cell it hashes
//! the global cell coordinates to pick a Scene (0x12) from the Region's
//! tables, then hashes again per object to decide presence, displacement,
//! rotation and scale. All hashes are 32-bit wrapping arithmetic whose
//! result is read as an unsigned fraction of 2^32.
//!
//! Ported from the client (`FUN_005311a0`, `FUN_005a6cc0`, `FUN_005a6e60`)
//! and cross-checked with ACE's `Landblock.get_land_scenes`.

use ac_formats::landblock::{terrain as tbits, CellLandblock, LandblockInfo};
use ac_formats::scene::ObjectDesc;
use glam::{Mat4, Quat, Vec3};

use crate::terrain::split_dir;
use crate::{lbid, Assets, Result, CELLS_PER_BLOCK, CELL_SIZE, VERTS_PER_SIDE};

const ROAD_WIDTH: f32 = 5.0;

/// `u32 -> [0, 1)` exactly as the client does it (`(float)v * 2.3283064e-10`,
/// with negative ints bumped by 2^32).
fn frac(v: u32) -> f32 {
    v as f32 * 2.328_306_4e-10
}

/// The shared per-object hash: `y*A - (x*y*B + C)*(k + salt) - x*D`.
fn obj_hash(x: u32, y: u32, k: u32, salt: u32) -> u32 {
    y.wrapping_mul(0x6C1A_C587)
        .wrapping_sub(
            x.wrapping_mul(y)
                .wrapping_mul(0x5111_BFEF)
                .wrapping_add(0x7089_2FB7)
                .wrapping_mul(k.wrapping_add(salt)),
        )
        .wrapping_sub(x.wrapping_mul(0x421B_E3BD))
}

/// Which Scene index (of `count`) a cell uses.
fn scene_index(x: u32, y: u32, count: u32) -> u32 {
    if count <= 1 {
        return 0;
    }
    let v = x
        .wrapping_mul(0x2A7F_2B89)
        .wrapping_add(0x6C1A_C587)
        .wrapping_mul(y)
        .wrapping_sub(x.wrapping_mul(0x421B_E3BD))
        .wrapping_add(0x7F8C_DA01);
    let idx = (count as f32 * frac(v)).floor() as u32;
    if idx >= count {
        0
    } else {
        idx
    }
}

/// Pseudo-random offset within the cell, then a quadrant rotation.
pub fn displace(obj: &ObjectDesc, x: u32, y: u32, k: u32) -> Vec3 {
    let loc = obj.base_loc.origin;
    let dx = if obj.displace_x <= 0.0 {
        loc.x
    } else {
        frac(obj_hash(x, y, k, 0xB2CD)) * obj.displace_x + loc.x
    };
    let dy = if obj.displace_y <= 0.0 {
        loc.y
    } else {
        frac(obj_hash(x, y, k, 0x11C0F)) * obj.displace_y + loc.y
    };
    let q = frac(
        y.wrapping_mul(0x6C1A_C587)
            .wrapping_sub(
                y.wrapping_mul(0x6F7B_D965)
                    .wrapping_add(0x421B_E3BD)
                    .wrapping_mul(x),
            )
            .wrapping_sub(0x17FC_EDFD),
    );
    if q < 0.25 {
        Vec3::new(dx, dy, loc.z)
    } else if q < 0.5 {
        Vec3::new(-dy, dx, loc.z)
    } else if q < 0.75 {
        Vec3::new(-dx, -dy, loc.z)
    } else {
        Vec3::new(dy, -dx, loc.z)
    }
}

pub fn scale(obj: &ObjectDesc, x: u32, y: u32, k: u32) -> f32 {
    if obj.min_scale == obj.max_scale {
        obj.max_scale
    } else {
        (obj.max_scale / obj.min_scale).powf(frac(obj_hash(x, y, k, 0x7F51))) * obj.min_scale
    }
}

/// Random heading in degrees (client convention: 0 = north/+Y, clockwise).
pub fn rotation_deg(obj: &ObjectDesc, x: u32, y: u32, k: u32) -> f32 {
    if obj.max_rotation > 0.0 {
        frac(obj_hash(x, y, k, 0xF697)) * obj.max_rotation
    } else {
        0.0
    }
}

/// Heading (degrees, 0 = +Y, clockwise) to an orientation about Z.
pub fn heading_quat(deg: f32) -> Quat {
    Quat::from_rotation_z(-deg.to_radians())
}

/// Heading of a vector in the client's convention.
fn vec_heading_deg(v: Vec3) -> f32 {
    let n = Vec3::new(v.x, v.y, 0.0);
    if n.length_squared() < 1e-6 {
        return 0.0;
    }
    (450.0 - n.y.atan2(n.x).to_degrees()) % 360.0
}

/// Terrain sampling for one landblock: height and normal at any local point.
pub struct TerrainSampler<'a> {
    lb: &'a CellLandblock,
    heights: [[f32; VERTS_PER_SIDE]; VERTS_PER_SIDE],
}

impl<'a> TerrainSampler<'a> {
    pub fn new(lb: &'a CellLandblock, height_table: &[f32]) -> Self {
        let mut heights = [[0.0; VERTS_PER_SIDE]; VERTS_PER_SIDE];
        for x in 0..VERTS_PER_SIDE {
            for y in 0..VERTS_PER_SIDE {
                heights[x][y] = height_table
                    .get(lb.height[x * VERTS_PER_SIDE + y] as usize)
                    .copied()
                    .unwrap_or(0.0);
            }
        }
        TerrainSampler { lb, heights }
    }

    fn vertex(&self, x: usize, y: usize) -> Vec3 {
        Vec3::new(
            x as f32 * CELL_SIZE,
            y as f32 * CELL_SIZE,
            self.heights[x][y],
        )
    }

    /// Plane (normal, d) of the terrain triangle under `p` (landblock-local).
    pub fn plane_at(&self, p: Vec3) -> Option<(Vec3, f32)> {
        let cx = (p.x / CELL_SIZE).floor();
        let cy = (p.y / CELL_SIZE).floor();
        if cx < 0.0 || cy < 0.0 || cx >= CELLS_PER_BLOCK as f32 || cy >= CELLS_PER_BLOCK as f32 {
            return None;
        }
        let (cx, cy) = (cx as usize, cy as usize);
        let fx = p.x / CELL_SIZE - cx as f32;
        let fy = p.y / CELL_SIZE - cy as f32;
        let ll = self.vertex(cx, cy);
        let lr = self.vertex(cx + 1, cy);
        let tl = self.vertex(cx, cy + 1);
        let tr = self.vertex(cx + 1, cy + 1);
        // The same two triangles the mesh draws (`terrain::build`), which
        // is also how the original splits a cell: `split_dir` true cuts
        // from the lower right to the top left, false from the lower left
        // to the top right. These used to be the other way round, so on
        // any cell whose corners are not level the character walked at a
        // height the ground was not drawn at, and off a hill it ended up
        // metres under it.
        let tri = if split_dir(
            lbid::block_x(self.lb.id),
            lbid::block_y(self.lb.id),
            cx as u32,
            cy as u32,
        ) {
            // Diagonal LR-TL: the lower-left triangle when fx + fy < 1.
            if fx + fy < 1.0 {
                [ll, lr, tl]
            } else {
                [lr, tr, tl]
            }
        } else {
            // Diagonal LL-TR: the lower-right triangle when fx > fy.
            if fx > fy {
                [ll, lr, tr]
            } else {
                [ll, tr, tl]
            }
        };
        let n = (tri[1] - tri[0])
            .cross(tri[2] - tri[0])
            .normalize_or(Vec3::Z);
        let d = -n.dot(tri[0]);
        Some((n, d))
    }

    pub fn height_at(&self, p: Vec3) -> Option<f32> {
        let (n, d) = self.plane_at(p)?;
        if n.z.abs() < 1e-6 {
            return None;
        }
        Some(-((p.y * n.y + p.x * n.x + d) / n.z))
    }

    fn road(&self, x: i32, y: i32) -> u16 {
        if x < 0 || y < 0 || x as usize >= VERTS_PER_SIDE || y as usize >= VERTS_PER_SIDE {
            return 0;
        }
        tbits::road(self.lb.terrain[x as usize * VERTS_PER_SIDE + y as usize])
    }

    /// The client's road proximity test (`FUN_00530d30`).
    pub fn on_road(&self, p: Vec3) -> bool {
        let x = (p.x / CELL_SIZE) as i32;
        let y = (p.y / CELL_SIZE) as i32;
        let r_min = ROAD_WIDTH;
        let r_max = CELL_SIZE - ROAD_WIDTH;
        let r0 = self.road(x, y) > 0;
        let r1 = self.road(x, y + 1) > 0;
        let r2 = self.road(x + 1, y) > 0;
        let r3 = self.road(x + 1, y + 1) > 0;
        if !(r0 || r1 || r2 || r3) {
            return false;
        }
        let dx = p.x - x as f32 * CELL_SIZE;
        let dy = p.y - y as f32 * CELL_SIZE;
        let t = CELL_SIZE;
        match (r0, r1, r2, r3) {
            (true, true, true, true) => true,
            (true, true, true, false) => dx < r_min || dy < r_min,
            (true, true, false, true) => dx < r_min || dy > r_max,
            (true, true, false, false) => dx < r_min,
            (true, false, true, true) => dx > r_max || dy < r_min,
            (true, false, true, false) => dy < r_min,
            (true, false, false, true) => (dx - dy).abs() < r_min,
            (true, false, false, false) => dx + dy < r_min,
            (false, true, true, true) => dx > r_max || dy > r_max,
            (false, true, true, false) => (dx + dy - t).abs() < r_min,
            (false, true, false, true) => dy > r_max,
            (false, true, false, false) => t + dx - dy < r_min,
            (false, false, true, true) => dx > r_max,
            (false, false, true, false) => t - dx + dy < r_min,
            (false, false, false, true) => t * 2.0 - dx - dy < r_min,
            (false, false, false, false) => false,
        }
    }
}

/// A placed scenery instance in landblock-local space.
#[derive(Debug, Clone)]
pub struct SceneryInstance {
    pub obj_id: u32,
    pub local: Mat4,
}

/// Why candidate objects were rejected; for debugging placement.
#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub cells_with_scene: usize,
    pub candidates: usize,
    pub freq: usize,
    pub weenie: usize,
    pub bounds: usize,
    pub road: usize,
    pub building: usize,
    pub slope: usize,
    pub placed: usize,
}

/// Generate scenery for a landblock. `info` is used to skip cells occupied
/// by buildings.
pub fn generate(
    assets: &Assets,
    lb: &CellLandblock,
    info: Option<&LandblockInfo>,
) -> Result<Vec<SceneryInstance>> {
    generate_with_stats(assets, lb, info).map(|(v, _)| v)
}

pub fn generate_with_stats(
    assets: &Assets,
    lb: &CellLandblock,
    info: Option<&LandblockInfo>,
) -> Result<(Vec<SceneryInstance>, Stats)> {
    let mut stats = Stats::default();
    let region = assets.region()?;
    let sampler = TerrainSampler::new(lb, &region.land_defs.land_height_table);
    let bx = lbid::block_x(lb.id) * 8;
    let by = lbid::block_y(lb.id) * 8;

    // Cells covered by buildings: the client flags every land cell a
    // building's parts touch. Approximate with the sorting sphere.
    let mut building_cells = [[false; 8]; 8];
    if let Some(info) = info {
        for b in &info.buildings {
            let r = assets
                .setup(b.model_id)
                .ok()
                .map(|s| s.sorting_sphere.radius)
                .unwrap_or(10.0)
                .max(6.0);
            let o = b.frame.origin;
            for cx in 0..8 {
                for cy in 0..8 {
                    let c = Vec3::new(
                        (cx as f32 + 0.5) * CELL_SIZE,
                        (cy as f32 + 0.5) * CELL_SIZE,
                        o.z,
                    );
                    if (c - o).truncate().length() < r + CELL_SIZE * 0.5 {
                        building_cells[cx][cy] = true;
                    }
                }
            }
        }
    }

    let mut out = Vec::new();
    for cx in 0..CELLS_PER_BLOCK {
        for cy in 0..CELLS_PER_BLOCK {
            let t = lb.terrain[cx as usize * VERTS_PER_SIDE + cy as usize];
            let terrain_type = tbits::terrain_type(t) as usize;
            let scene_type = tbits::scenery(t) as usize;
            let Some(tt) = region.terrain_types.get(terrain_type) else {
                continue;
            };
            let Some(&scene_info) = tt.scene_types.get(scene_type) else {
                continue;
            };
            let Some(st) = region.scene_types.get(scene_info as usize) else {
                continue;
            };
            if st.scenes.is_empty() {
                continue;
            }
            let gx = bx + cx;
            let gy = by + cy;
            let scene_id = st.scenes[scene_index(gx, gy, st.scenes.len() as u32) as usize];
            let scene = match assets.scene(scene_id) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("scene {scene_id:#010x}: {e}");
                    continue;
                }
            };
            stats.cells_with_scene += 1;
            for (k, obj) in scene.objects.iter().enumerate() {
                let k = k as u32;
                stats.candidates += 1;
                if obj.weenie_obj != 0 {
                    stats.weenie += 1;
                    continue;
                }
                if frac(obj_hash(gx, gy, k, 0x5B67)) >= obj.freq {
                    stats.freq += 1;
                    continue;
                }
                let d = displace(obj, gx, gy, k);
                let mut p = Vec3::new(
                    cx as f32 * CELL_SIZE + d.x,
                    cy as f32 * CELL_SIZE + d.y,
                    d.z,
                );
                if p.x < 0.0 || p.y < 0.0 || p.x >= crate::BLOCK_SIZE || p.y >= crate::BLOCK_SIZE {
                    stats.bounds += 1;
                    continue;
                }
                if sampler.on_road(p) {
                    stats.road += 1;
                    continue;
                }
                let (pcx, pcy) = ((p.x / CELL_SIZE) as usize, (p.y / CELL_SIZE) as usize);
                if building_cells[pcx.min(7)][pcy.min(7)] {
                    stats.building += 1;
                    continue;
                }
                let Some((n, _)) = sampler.plane_at(p) else {
                    continue;
                };
                if n.z < obj.min_slope || n.z > obj.max_slope {
                    stats.slope += 1;
                    continue;
                }
                stats.placed += 1;
                let Some(z) = sampler.height_at(p) else {
                    continue;
                };
                p.z = z;
                let heading = if obj.align != 0 {
                    vec_heading_deg(-n)
                } else {
                    rotation_deg(obj, gx, gy, k)
                };
                let orientation = heading_quat(heading) * obj.base_loc.orientation.normalize();
                let s = scale(obj, gx, gy, k);
                let local = Mat4::from_scale_rotation_translation(Vec3::splat(s), orientation, p);
                out.push(SceneryInstance {
                    obj_id: obj.obj_id,
                    local,
                });
            }
        }
    }
    Ok((out, stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frac_is_unsigned() {
        // f32 rounds 0xFFFFFFFF up to 2^32, so the client's fraction can reach 1.0.
        assert!(frac(0xFFFF_FFFF) <= 1.0 && frac(0xFFFF_FFFF) > 0.99);
        assert_eq!(frac(0), 0.0);
        assert!((frac(0x8000_0000) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn scene_index_in_range() {
        for x in 0..2040u32 {
            for y in (0..2040u32).step_by(97) {
                assert!(scene_index(x, y, 5) < 5);
            }
        }
    }
}
