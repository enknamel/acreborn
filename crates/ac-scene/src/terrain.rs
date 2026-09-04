//! Landblock terrain mesh.

use ac_formats::landblock::{terrain as tbits, CellLandblock};
use glam::Vec3;

use crate::{lbid, CELLS_PER_BLOCK, CELL_SIZE, VERTS_PER_SIDE};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainVertex {
    pub position: Vec3,
    pub normal: Vec3,
    /// Terrain type of this vertex (index into Region::terrain_types).
    pub terrain_type: u16,
    pub road: u16,
}

/// Triangles of one landblock, in landblock-local coordinates (add
/// `lbid::world_origin` to place in the world). Each cell contributes two
/// triangles; `cell_types` records the terrain type used for the cell.
#[derive(Debug, Clone)]
pub struct TerrainMesh {
    pub vertices: Vec<TerrainVertex>,
    /// 3 indices per triangle, 2 triangles per cell, cells ordered x-major.
    pub indices: Vec<u32>,
    /// Per cell (x * 8 + y): terrain type of its south-west corner.
    pub cell_types: Vec<u16>,
}

/// The client's cell diagonal rule (credit: AC2D). True = split from the
/// north-west corner to the south-east corner.
pub fn split_dir(block_x: u32, block_y: u32, cell_x: u32, cell_y: u32) -> bool {
    let x = block_x.wrapping_mul(8).wrapping_add(cell_x);
    let y = block_y.wrapping_mul(8).wrapping_add(cell_y);
    let dw = x
        .wrapping_mul(y)
        .wrapping_mul(0x0CCA_C033)
        .wrapping_sub(x.wrapping_mul(0x421B_E3BD))
        .wrapping_add(y.wrapping_mul(0x6C1A_C587))
        .wrapping_sub(0x519B_8F25);
    dw & 0x8000_0000 == 0
}

pub fn build(lb: &CellLandblock, height_table: &[f32]) -> TerrainMesh {
    let bx = lbid::block_x(lb.id);
    let by = lbid::block_y(lb.id);
    let n = VERTS_PER_SIDE;
    let mut vertices = Vec::with_capacity(n * n);
    for x in 0..n {
        for y in 0..n {
            let i = x * n + y;
            let h = height_table
                .get(lb.height[i] as usize)
                .copied()
                .unwrap_or(0.0);
            let t = lb.terrain[i];
            vertices.push(TerrainVertex {
                position: Vec3::new(x as f32 * CELL_SIZE, y as f32 * CELL_SIZE, h),
                normal: Vec3::Z,
                terrain_type: tbits::terrain_type(t),
                road: tbits::road(t),
            });
        }
    }
    let idx = |x: usize, y: usize| (x * n + y) as u32;
    let mut indices = Vec::with_capacity(8 * 8 * 6);
    let mut cell_types = Vec::with_capacity(64);
    for x in 0..CELLS_PER_BLOCK as usize {
        for y in 0..CELLS_PER_BLOCK as usize {
            let ll = idx(x, y);
            let lr = idx(x + 1, y);
            let tl = idx(x, y + 1);
            let tr = idx(x + 1, y + 1);
            // Counter-clockwise when viewed from +Z.
            if split_dir(bx, by, x as u32, y as u32) {
                indices.extend_from_slice(&[ll, lr, tl, lr, tr, tl]);
            } else {
                indices.extend_from_slice(&[ll, lr, tr, ll, tr, tl]);
            }
            cell_types.push(vertices[ll as usize].terrain_type);
        }
    }
    // Smooth normals from face normals.
    let mut acc = vec![Vec3::ZERO; vertices.len()];
    for t in indices.chunks_exact(3) {
        let p0 = vertices[t[0] as usize].position;
        let p1 = vertices[t[1] as usize].position;
        let p2 = vertices[t[2] as usize].position;
        let fnorm = (p1 - p0).cross(p2 - p0);
        for &i in t {
            acc[i as usize] += fnorm;
        }
    }
    for (v, a) in vertices.iter_mut().zip(acc) {
        v.normal = a.normalize_or(Vec3::Z);
    }
    TerrainMesh {
        vertices,
        indices,
        cell_types,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_dir_matches_reference_formula() {
        // Spot values computed from the AC2D formula by hand.
        let f = |x: u32, y: u32| {
            let dw = x
                .wrapping_mul(y)
                .wrapping_mul(0x0CCAC033)
                .wrapping_sub(x.wrapping_mul(0x421BE3BD))
                .wrapping_add(y.wrapping_mul(0x6C1AC587))
                .wrapping_sub(0x519B8F25);
            dw & 0x8000_0000 == 0
        };
        for (bx, by, cx, cy) in [(0, 0, 0, 0), (0xA9, 0xB4, 3, 5), (255, 255, 7, 7)] {
            assert_eq!(split_dir(bx, by, cx, cy), f(bx * 8 + cx, by * 8 + cy));
        }
    }
}
