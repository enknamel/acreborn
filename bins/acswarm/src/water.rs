//! Water surfaces over outdoor terrain.
//!
//! The landblock height grid carries a terrain type per vertex; the
//! `Water*` types (running, standing, shallow and deep sea, and the
//! walkable `FauxWater`) mark cells that the client textures as water.
//! Here every cell touching a water vertex gets a flat quad at the water
//! level, drawn translucently over the ground so shores emerge from it
//! where the terrain rises above the surface.

use std::collections::HashMap;

use ac_formats::region::Region;
use ac_scene::terrain::TerrainMesh;
use ac_scene::{CELLS_PER_BLOCK, CELL_SIZE, VERTS_PER_SIDE};
use glam::Vec3;

use crate::gpu::{Batch, MaterialKey, Vertex};

/// Height of the water plane above the water vertices, so flat shores at
/// exactly the water height do not z-fight with the surface.
pub const LIFT: f32 = 0.15;

/// Cells of water tile the texture over this many world units.
const TILE: f32 = CELL_SIZE * 2.0;

/// True for terrain types the Region names as water.
pub fn is_water(region: &Region, terrain_type: u16) -> bool {
    region
        .terrain_types
        .get(terrain_type as usize)
        .map(|t| t.name.contains("Water"))
        .unwrap_or(false)
}

/// The Region's water texture (SurfaceTexture id) for a terrain type, or 0.
fn water_texture(region: &Region, terrain_type: u16) -> u32 {
    region
        .tex_merge
        .as_ref()
        .and_then(|tm| {
            tm.terrain_desc
                .iter()
                .find(|(t, _)| *t == terrain_type as u32)
                .map(|(_, tex)| tex.tex_gid)
        })
        .unwrap_or(0)
}

/// Append water quads for `terrain` (a landblock at world `origin`) to
/// `batches` under [`MaterialKey::Water`] keys.
pub fn push_water(
    batches: &mut HashMap<MaterialKey, Batch>,
    region: &Region,
    terrain: &TerrainMesh,
    origin: Vec3,
) {
    let n = VERTS_PER_SIDE;
    let idx = |x: usize, y: usize| x * n + y;
    for x in 0..CELLS_PER_BLOCK as usize {
        for y in 0..CELLS_PER_BLOCK as usize {
            let corners = [idx(x, y), idx(x + 1, y), idx(x + 1, y + 1), idx(x, y + 1)];
            let wet: Vec<&ac_scene::terrain::TerrainVertex> = corners
                .iter()
                .map(|&i| &terrain.vertices[i])
                .filter(|v| is_water(region, v.terrain_type))
                .collect();
            let Some(level) = wet
                .iter()
                .map(|v| v.position.z)
                .reduce(f32::min)
                .map(|z| z + LIFT)
            else {
                continue;
            };
            let key = MaterialKey::Water(water_texture(region, wet[0].terrain_type));
            let verts: Vec<Vertex> = corners
                .iter()
                .map(|&i| {
                    let p = terrain.vertices[i].position.with_z(level) + origin;
                    Vertex {
                        position: p.to_array(),
                        normal: [0.0, 0.0, 1.0],
                        uv: [p.x / TILE, p.y / TILE],
                        color: [1.0; 4],
                    }
                })
                .collect();
            batches
                .entry(key)
                .or_default()
                .push(&verts, &[0, 1, 2, 0, 2, 3]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holtburg_river_gets_water_at_the_water_height() {
        let Ok(dir) = std::env::var("AC_DATA_DIR") else {
            return;
        };
        let assets = ac_scene::Assets::open(dir).unwrap();
        let region = assets.region().unwrap();
        assert!(is_water(&region, 16));
        assert!(is_water(&region, 22));
        assert!(!is_water(&region, 1));
        // A9B5 has a river; A9B4 (Holtburg) has none.
        let scene = ac_scene::landblock::load(&assets, 0xA9B5_0000).unwrap();
        let mut batches = HashMap::new();
        push_water(&mut batches, &region, &scene.terrain, Vec3::ZERO);
        let quads: usize = batches.values().map(|b| b.indices.len() / 6).sum();
        assert!(quads > 0 && quads < 64, "{quads} water cells");
        for v in batches.values().flat_map(|b| b.vertices.iter()) {
            assert_eq!(v.position[2], 28.0 + LIFT);
        }
        assert!(batches
            .keys()
            .all(|k| matches!(k, MaterialKey::Water(t) if *t != 0)));
        let dry = ac_scene::landblock::load(&assets, 0xA9B4_0000).unwrap();
        let mut none = HashMap::new();
        push_water(&mut none, &region, &dry.terrain, Vec3::ZERO);
        assert!(none.is_empty());
    }
}
