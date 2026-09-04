//! Build GPU batches from ac-scene output.

use std::collections::HashMap;

use ac_scene::{landblock, model, Assets, CELL_SIZE};
use anyhow::Result;
use glam::{Mat4, Vec3};

use crate::gpu::{Batch, MaterialKey, Rgba, Vertex};

pub struct Built {
    pub batches: HashMap<MaterialKey, Batch>,
    pub center: Vec3,
    pub radius: f32,
}

fn push_mesh(batches: &mut HashMap<MaterialKey, Batch>, mesh: &model::Mesh, transform: Mat4) {
    let normal_mat = transform.inverse().transpose();
    for sub in &mesh.submeshes {
        let key = match sub.solid_color {
            Some(c) => MaterialKey::Solid(c),
            None => MaterialKey::Texture(sub.surface_id),
        };
        let alpha = 1.0 - sub.translucency.clamp(0.0, 1.0);
        let verts: Vec<Vertex> = sub
            .vertices
            .iter()
            .map(|v| Vertex {
                position: transform.transform_point3(v.position).to_array(),
                normal: normal_mat
                    .transform_vector3(v.normal)
                    .normalize_or(Vec3::Z)
                    .to_array(),
                uv: v.uv.to_array(),
                color: [1.0, 1.0, 1.0, alpha],
            })
            .collect();
        batches.entry(key).or_default().push(&verts, &sub.indices);
    }
}

/// Terrain material key: the SurfaceTexture id of the terrain type's base
/// texture from the Region tex-merge table, or a color fallback.
fn terrain_key(assets: &Assets, terrain_type: u16) -> Result<MaterialKey> {
    let region = assets.region()?;
    if let Some(tm) = &region.tex_merge {
        if let Some((_, tex)) = tm
            .terrain_desc
            .iter()
            .find(|(t, _)| *t == terrain_type as u32)
        {
            return Ok(MaterialKey::Texture(tex.tex_gid));
        }
    }
    let color = region
        .terrain_types
        .get(terrain_type as usize)
        .map(|t| t.color)
        .unwrap_or(0xFF80_8080);
    Ok(MaterialKey::Solid(color))
}

pub fn build_landblocks(assets: &Assets, center_id: u32, radius: u32) -> Result<Built> {
    let cx = ac_scene::lbid::block_x(center_id);
    let cy = ac_scene::lbid::block_y(center_id);
    let mut batches: HashMap<MaterialKey, Batch> = HashMap::new();
    let mut mesh_cache: HashMap<u32, model::Mesh> = HashMap::new();
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for bx in cx.saturating_sub(radius)..=(cx + radius).min(255) {
        for by in cy.saturating_sub(radius)..=(cy + radius).min(255) {
            let id = ac_scene::lbid::from_xy(bx, by);
            let scene = match landblock::load(assets, id) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("landblock {id:#010x}: {e}");
                    continue;
                }
            };
            let origin = ac_scene::lbid::world_origin(id);
            // Terrain: one batch per terrain type, UV tiles once per cell.
            for (cell, tri) in scene.terrain.indices.chunks_exact(6).enumerate() {
                let key = terrain_key(assets, scene.terrain.cell_types[cell])?;
                let verts: Vec<Vertex> = tri
                    .iter()
                    .map(|&i| {
                        let v = &scene.terrain.vertices[i as usize];
                        let p = v.position + origin;
                        min = min.min(p);
                        max = max.max(p);
                        Vertex {
                            position: p.to_array(),
                            normal: v.normal.to_array(),
                            uv: [p.x / CELL_SIZE, p.y / CELL_SIZE],
                            color: [1.0; 4],
                        }
                    })
                    .collect();
                batches
                    .entry(key)
                    .or_default()
                    .push(&verts, &[0, 1, 2, 3, 4, 5]);
            }
            for part in &scene.parts {
                if !mesh_cache.contains_key(&part.gfxobj_id) {
                    let g = match assets.gfxobj(part.gfxobj_id) {
                        Ok(g) => g,
                        Err(e) => {
                            tracing::warn!("gfxobj {:#010x}: {e}", part.gfxobj_id);
                            continue;
                        }
                    };
                    mesh_cache.insert(part.gfxobj_id, model::build_mesh(assets, &g)?);
                }
                push_mesh(&mut batches, &mesh_cache[&part.gfxobj_id], part.transform);
            }
        }
    }
    let center = (min + max) * 0.5;
    Ok(Built {
        batches,
        center,
        radius: (max - min).length() * 0.5,
    })
}

pub fn build_model(assets: &Assets, model_id: u32) -> Result<Built> {
    let mut batches = HashMap::new();
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for part in model::place(assets, model_id, Mat4::IDENTITY)? {
        let g = assets.gfxobj(part.gfxobj_id)?;
        let mesh = model::build_mesh(assets, &g)?;
        for sub in &mesh.submeshes {
            for v in &sub.vertices {
                let p = part.transform.transform_point3(v.position);
                min = min.min(p);
                max = max.max(p);
            }
        }
        push_mesh(&mut batches, &mesh, part.transform);
    }
    let center = (min + max) * 0.5;
    Ok(Built {
        batches,
        center,
        radius: ((max - min).length() * 0.5).max(1.0),
    })
}

pub fn material_image(assets: &Assets, key: MaterialKey) -> Option<Rgba> {
    let MaterialKey::Texture(id) = key else {
        return None;
    };
    let res = if id >> 24 == 0x08 {
        assets.surface_rgba(id).ok().flatten()
    } else {
        assets.texture_rgba(id, None).ok()
    };
    match res {
        Some(img) => Some(Rgba {
            width: img.width,
            height: img.height,
            pixels: img.pixels,
        }),
        None => {
            tracing::warn!("material {id:#010x}: no image");
            None
        }
    }
}
