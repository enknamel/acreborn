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

pub fn push_mesh(batches: &mut HashMap<MaterialKey, Batch>, mesh: &model::Mesh, transform: Mat4) {
    let normal_mat = transform.inverse().transpose();
    for sub in &mesh.submeshes {
        let key = match sub.solid_color {
            Some(c) => MaterialKey::Solid(c),
            None => MaterialKey::Texture {
                id: sub.surface_id,
                tex: sub.texture_override.unwrap_or(0),
                palette: sub.palette_hash,
            },
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
            return Ok(MaterialKey::Texture {
                id: tex.tex_gid,
                tex: 0,
                palette: 0,
            });
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
            let terrain_cells = if scene.is_dungeon { 0 } else { usize::MAX };
            for (cell, tri) in scene
                .terrain
                .indices
                .chunks_exact(6)
                .enumerate()
                .take(terrain_cells)
            {
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
            for cell in &scene.cells {
                for sub in &cell.submeshes {
                    for v in &sub.vertices {
                        let p = cell.transform.transform_point3(v.position);
                        min = min.min(p);
                        max = max.max(p);
                    }
                }
                let mesh = model::Mesh {
                    gfxobj_id: cell.cell_id,
                    submeshes: cell.submeshes.clone(),
                };
                push_mesh(&mut batches, &mesh, cell.transform);
                for part in &cell.parts {
                    if !mesh_cache.contains_key(&part.gfxobj_id) {
                        match assets.gfxobj(part.gfxobj_id) {
                            Ok(g) => {
                                mesh_cache.insert(part.gfxobj_id, model::build_mesh(assets, &g)?);
                            }
                            Err(e) => {
                                tracing::warn!("gfxobj {:#010x}: {e}", part.gfxobj_id);
                                continue;
                            }
                        }
                    }
                    push_mesh(&mut batches, &mesh_cache[&part.gfxobj_id], part.transform);
                }
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

/// Composed palettes referenced by material keys, by hash.
pub type Palettes = HashMap<u64, std::rc::Rc<Vec<u32>>>;

pub fn material_image(assets: &Assets, key: MaterialKey, palettes: &Palettes) -> Option<Rgba> {
    let MaterialKey::Texture { id, tex, palette } = key else {
        return None;
    };
    // Which texture: an appearance override, the surface's texture, or the id itself.
    let source = if tex != 0 {
        Some(tex)
    } else if id >> 24 == 0x08 {
        match assets.surface(id).ok().map(|s| s.base) {
            Some(ac_formats::surface::SurfaceBase::Image { texture, .. }) => Some(texture),
            _ => None,
        }
    } else {
        Some(id)
    };
    let res = match (source, palettes.get(&palette)) {
        (Some(t), Some(colors)) if palette != 0 => assets.texture_rgba_with_palette(t, colors).ok(),
        (Some(t), _) if tex != 0 => {
            // Keep the surface's palette for indexed textures if it has one.
            let pal = if id >> 24 == 0x08 {
                match assets.surface(id).ok().map(|s| s.base) {
                    Some(ac_formats::surface::SurfaceBase::Image { palette, .. })
                        if palette != 0 =>
                    {
                        Some(palette)
                    }
                    _ => None,
                }
            } else {
                None
            };
            assets.texture_rgba(t, pal).ok()
        }
        _ if id >> 24 == 0x08 => assets.surface_rgba(id).ok().flatten(),
        (Some(t), _) => assets.texture_rgba(t, None).ok(),
        _ => None,
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

/// Appearance for a world object plus a key that identifies it for caching.
pub fn appearance_of(
    assets: &Assets,
    o: &ac_world::WorldObject,
    palettes: &mut Palettes,
) -> (model::Appearance, u64) {
    let app = model::Appearance::from_obj_desc(
        assets,
        o.palette_id,
        &o.sub_palettes,
        &o.texture_changes,
        &o.anim_part_changes,
    );
    if let Some(p) = &app.palette {
        palettes
            .entry(app.palette_hash)
            .or_insert_with(|| p.clone());
    }
    let mut key = app.palette_hash;
    for (idx, swaps) in &app.texture_swaps {
        for (old, new) in swaps {
            key = key.wrapping_mul(0x100_0000_01b3)
                ^ ((*idx as u64) << 56 | (*old as u64) << 24 ^ *new as u64);
        }
    }
    for (idx, id) in &app.part_swaps {
        key = key.wrapping_mul(0x100_0000_01b3) ^ ((*idx as u64) << 48 | *id as u64);
    }
    (app, key)
}

/// Batches for the objects a server has placed (players, NPCs, items).
/// Animation state for a server object: the cycle for its current motion.
pub struct ObjectAnim {
    pub player: Option<ac_scene::anim::AnimPlayer>,
    pub n_parts: usize,
    /// Forward motion command the current cycle was chosen for.
    pub motion: u32,
}

/// Look up (and cache) the idle cycle for an object's motion table.
pub fn object_anim(
    assets: &Assets,
    o: &ac_world::WorldObject,
    tables: &mut HashMap<u32, Option<ac_formats::motion_table::MotionTable>>,
) -> ObjectAnim {
    use ac_scene::anim::{motion, AnimPlayer};
    let n_parts = assets.setup(o.setup_id).map(|s| s.parts.len()).unwrap_or(0);
    let wanted = o.motion.forward;
    if o.motion_table_id == 0 {
        return ObjectAnim {
            player: None,
            n_parts,
            motion: wanted,
        };
    }
    let table = tables
        .entry(o.motion_table_id)
        .or_insert_with(|| ac_scene::anim::motion_table(assets, o.motion_table_id).ok());
    let player = table.as_ref().and_then(|t| {
        let style = t.default_style;
        // Ready plays the table's default motion for its style (idle).
        // Movement events carry only the low 16 bits of a command.
        let m = if wanted & 0xFFFF == motion::READY & 0xFFFF || wanted == 0 {
            t.default_motion(style).unwrap_or(motion::READY)
        } else {
            wanted
        };
        AnimPlayer::cycle(assets, t, style, m).or_else(|| {
            let idle = t.default_motion(style).unwrap_or(motion::READY);
            AnimPlayer::cycle(assets, t, style, idle)
        })
    });
    ObjectAnim {
        player,
        n_parts,
        motion: wanted,
    }
}

/// True if any drawable object has an animation, so batches need refreshing.
pub fn any_animated(anims: &HashMap<u32, ObjectAnim>) -> bool {
    anims.values().any(|a| a.player.is_some())
}

/// GPU meshes cached by (GfxObj id, appearance key).
pub type GpuMeshCache = HashMap<(u32, u64), std::rc::Rc<crate::gpu::GpuMesh>>;

/// Instances for one model placed at `transform` (with appearance and an
/// optional animated pose); meshes are uploaded once per (GfxObj, look).
#[allow(clippy::too_many_arguments)]
pub fn instances_for(
    assets: &Assets,
    gpu: &crate::gpu::Gpu,
    meshes: &mut GpuMeshCache,
    palettes: &Palettes,
    setup_id: u32,
    transform: Mat4,
    app: &model::Appearance,
    app_key: u64,
    pose: Option<&[Mat4]>,
) -> Vec<crate::gpu::Instance> {
    let parts = match model::place_posed(assets, setup_id, transform, app, pose) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("setup {:#010x}: {e}", setup_id);
            return Vec::new();
        }
    };
    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        let key = (part.gfxobj_id, if app.is_empty() { 0 } else { app_key });
        if !meshes.contains_key(&key) {
            let Ok(g) = assets.gfxobj(part.gfxobj_id) else {
                continue;
            };
            let Ok(m) = model::build_mesh_with(assets, &g, part.part_index, app) else {
                continue;
            };
            meshes.insert(
                key,
                gpu.upload_mesh(&m, |k| material_image(assets, k, palettes)),
            );
        }
        out.push(crate::gpu::Instance {
            mesh: meshes[&key].clone(),
            model: part.transform,
        });
    }
    out
}

/// Instances for every drawable server object (excluding the player),
/// advancing their animations by `dt`.
#[allow(clippy::too_many_arguments)]
pub fn object_instances(
    assets: &Assets,
    gpu: &crate::gpu::Gpu,
    world: &ac_world::World,
    meshes: &mut GpuMeshCache,
    palettes: &mut Palettes,
    anims: &mut HashMap<u32, ObjectAnim>,
    tables: &mut HashMap<u32, Option<ac_formats::motion_table::MotionTable>>,
    dt: f32,
) -> Vec<crate::gpu::Instance> {
    let mut out = Vec::new();
    for o in world.drawable().filter(|o| !o.is_player) {
        let Some(t) = o.transform() else { continue };
        let (app, key) = appearance_of(assets, o, palettes);
        let stale = anims
            .get(&o.guid)
            .map(|a| a.motion != o.motion.forward)
            .unwrap_or(true);
        if stale {
            anims.insert(o.guid, object_anim(assets, o, tables));
        }
        let anim = anims.get_mut(&o.guid).unwrap();
        let pose = anim.player.as_mut().map(|p| {
            p.advance(dt);
            p.part_transforms(anim.n_parts)
        });
        out.extend(instances_for(
            assets,
            gpu,
            meshes,
            palettes,
            o.setup_id,
            t,
            &app,
            key,
            pose.as_deref(),
        ));
    }
    out
}
