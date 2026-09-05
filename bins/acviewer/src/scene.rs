//! Build GPU batches from ac-scene output.

use std::collections::HashMap;

use ac_scene::{landblock, model, texmerge, Assets, CELLS_PER_BLOCK, CELL_SIZE, VERTS_PER_SIDE};
use anyhow::Result;
use glam::{Mat4, Vec3};

use crate::gpu::{Batch, MaterialKey, Rgba, TerrainBlend, Vertex};

pub struct Built {
    pub batches: HashMap<MaterialKey, Batch>,
    pub center: Vec3,
    pub radius: f32,
    /// The centre block is a dungeon (only meaningful for single blocks).
    pub is_dungeon: bool,
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

/// Terrain material key for a region without texture merging: the terrain
/// type's colour.
fn terrain_color_key(assets: &Assets, terrain_type: u16) -> Result<MaterialKey> {
    let region = assets.region()?;
    let color = region
        .terrain_types
        .get(terrain_type as usize)
        .map(|t| t.color)
        .unwrap_or(0xFF80_8080);
    Ok(MaterialKey::Solid(color))
}

/// Blend data for one terrain vertex of a cell painted as `surface`.
fn terrain_blend(surface: &texmerge::CellSurface, cell_uv: [f32; 2]) -> TerrainBlend {
    let word = |o: Option<texmerge::Overlay>| {
        o.map(|o| TerrainBlend::overlay(o.texture, o.alpha, o.rotation))
            .unwrap_or(0)
    };
    TerrainBlend {
        cell_uv,
        layers: [
            surface.base as u32,
            word(surface.overlays[0]),
            word(surface.overlays[1]),
            word(surface.overlays[2]),
        ],
        roads: [word(surface.roads[0]), word(surface.roads[1])],
    }
}

/// Build one landblock's static geometry (terrain, buildings, scenery,
/// interiors) into per-material batches in world space.
pub fn build_landblock(
    assets: &Assets,
    id: u32,
    mesh_cache: &mut HashMap<u32, model::Mesh>,
) -> Result<Built> {
    let mut batches: HashMap<MaterialKey, Batch> = HashMap::new();
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    let scene = landblock::load(assets, id)?;
    let origin = ac_scene::lbid::world_origin(id);
    // Terrain: each cell's six vertices carry the cell's texture recipe
    // (base texture, overlays and roads with their alpha maps); the whole
    // block is one batch. Texture space has u east and v south, with the
    // base texture repeating `tiling` times per cell, like the client's
    // merged cell textures.
    let region = assets.region()?;
    let tables = texmerge::Tables::from_region(&region);
    let terrain_cells = if scene.is_dungeon { 0 } else { usize::MAX };
    for (cell, tri) in scene
        .terrain
        .indices
        .chunks_exact(6)
        .enumerate()
        .take(terrain_cells)
    {
        let (cell_x, cell_y) = (
            (cell / CELLS_PER_BLOCK as usize) as f32,
            (cell % CELLS_PER_BLOCK as usize) as f32,
        );
        let surface = tables
            .as_ref()
            .map(|t| t.cell_surface(scene.terrain.cell_codes[cell]));
        let tiling = surface.map(|s| s.tiling as f32).unwrap_or(1.0);
        let mut blend = Vec::with_capacity(6);
        let verts: Vec<Vertex> = tri
            .iter()
            .map(|&i| {
                let v = &scene.terrain.vertices[i as usize];
                let p = v.position + origin;
                min = min.min(p);
                max = max.max(p);
                let (gx, gy) = (
                    (i as usize / VERTS_PER_SIDE) as f32,
                    (i as usize % VERTS_PER_SIDE) as f32,
                );
                let cell_uv = [gx - cell_x, 1.0 - (gy - cell_y)];
                if let Some(s) = &surface {
                    blend.push(terrain_blend(s, cell_uv));
                }
                Vertex {
                    position: p.to_array(),
                    normal: v.normal.to_array(),
                    uv: [p.x / CELL_SIZE * tiling, -p.y / CELL_SIZE * tiling],
                    color: [1.0; 4],
                }
            })
            .collect();
        match surface {
            Some(_) => batches
                .entry(MaterialKey::Terrain)
                .or_default()
                .push_terrain(&verts, &blend, &[0, 1, 2, 3, 4, 5]),
            None => batches
                .entry(terrain_color_key(assets, scene.terrain.cell_types[cell])?)
                .or_default()
                .push(&verts, &[0, 1, 2, 3, 4, 5]),
        }
    }
    if !scene.is_dungeon {
        let region = assets.region()?;
        crate::water::push_water(&mut batches, &region, &scene.terrain, origin);
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
    let center = (min + max) * 0.5;
    Ok(Built {
        batches,
        center,
        radius: (max - min).length() * 0.5,
        is_dungeon: scene.is_dungeon,
    })
}

pub fn build_landblocks(assets: &Assets, center_id: u32, radius: u32) -> Result<Built> {
    let cx = ac_scene::lbid::block_x(center_id);
    let cy = ac_scene::lbid::block_y(center_id);
    let mut batches: HashMap<MaterialKey, Batch> = HashMap::new();
    let mut mesh_cache: HashMap<u32, model::Mesh> = HashMap::new();
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    // A dungeon block is its own world: its neighbours in id space are
    // unrelated blocks that would overlap it, so never load around one.
    let radius = match landblock::load(assets, center_id) {
        Ok(s) if s.is_dungeon => 0,
        _ => radius,
    };
    for bx in cx.saturating_sub(radius)..=(cx + radius).min(255) {
        for by in cy.saturating_sub(radius)..=(cy + radius).min(255) {
            let id = ac_scene::lbid::from_xy(bx, by);
            let built = match build_landblock(assets, id, &mut mesh_cache) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("landblock {id:#010x}: {e}");
                    continue;
                }
            };
            for (k, b) in built.batches {
                batches.entry(k).or_default().append(&b);
            }
            if built.radius > 0.0 {
                min = min.min(built.center - Vec3::splat(built.radius));
                max = max.max(built.center + Vec3::splat(built.radius));
            }
        }
    }
    let center = (min + max) * 0.5;
    Ok(Built {
        batches,
        center,
        radius: (max - min).length() * 0.5,
        is_dungeon: false,
    })
}

/// One model at the origin, dressed with an appearance (part swaps, texture
/// swaps and a composed palette); the palette must be registered in the
/// `Palettes` passed to `material_image`.
pub fn build_model_with(assets: &Assets, model_id: u32, app: &model::Appearance) -> Result<Built> {
    let mut batches = HashMap::new();
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for part in model::place_with(assets, model_id, Mat4::IDENTITY, app)? {
        let g = assets.gfxobj(part.gfxobj_id)?;
        let mesh = model::build_mesh_with(assets, &g, part.part_index, app)?;
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
        is_dungeon: false,
    })
}

/// Composed palettes referenced by material keys, by hash.
pub type Palettes = HashMap<u64, std::rc::Rc<Vec<u32>>>;

pub fn material_image(assets: &Assets, key: MaterialKey, palettes: &Palettes) -> Option<Rgba> {
    let (id, tex, palette) = match key {
        MaterialKey::Texture { id, tex, palette } => (id, tex, palette),
        MaterialKey::TerrainLayer(n) | MaterialKey::TerrainAlpha(n) => {
            return terrain_layer_image(assets, key, n)
        }
        MaterialKey::Solid(_) | MaterialKey::Terrain | MaterialKey::Water(_) => return None,
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

/// The `n`th terrain texture or alpha map layer of the region's tex-merge
/// tables; None past the end, a magenta placeholder when it fails to decode
/// (so layer numbers stay aligned with `texmerge::Tables`).
fn terrain_layer_image(assets: &Assets, key: MaterialKey, n: u32) -> Option<Rgba> {
    let region = assets.region().ok()?;
    let tables = texmerge::Tables::from_region(&region)?;
    let ids = match key {
        MaterialKey::TerrainAlpha(_) => &tables.alpha_ids,
        _ => &tables.texture_ids,
    };
    let id = *ids.get(n as usize)?;
    match assets.texture_rgba(id, None) {
        Ok(img) => Some(Rgba {
            width: img.width,
            height: img.height,
            pixels: img.pixels,
        }),
        Err(e) => {
            tracing::warn!("terrain layer {id:#010x}: {e}");
            Some(Rgba {
                width: 1,
                height: 1,
                pixels: vec![255, 0, 255, 255],
            })
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
    // Hash in a fixed order: the swap maps iterate differently per instance,
    // and an unstable key would upload a fresh mesh set every frame.
    let mut tex: Vec<(u64, u64, u64)> = app
        .texture_swaps
        .iter()
        .flat_map(|(idx, swaps)| {
            swaps
                .iter()
                .map(move |(old, new)| (*idx as u64, *old as u64, *new as u64))
        })
        .collect();
    tex.sort_unstable();
    let mut parts: Vec<(u64, u64)> = app
        .part_swaps
        .iter()
        .map(|(idx, id)| (*idx as u64, *id as u64))
        .collect();
    parts.sort_unstable();
    let mut key = app.palette_hash;
    for (idx, old, new) in tex {
        key = key.wrapping_mul(0x100_0000_01b3) ^ (idx << 56 | old << 24 ^ new);
    }
    for (idx, id) in parts {
        key = key.wrapping_mul(0x100_0000_01b3) ^ (idx << 48 | id);
    }
    (app, key)
}

/// Batches for the objects a server has placed (players, NPCs, items).
/// Animation state for a server object: the cycle for its current motion.
/// A world-space bounding sphere of one drawn part, for mouse picking.
pub struct Pickable {
    pub guid: u32,
    pub center: Vec3,
    pub radius: f32,
    mesh: std::rc::Rc<crate::gpu::GpuMesh>,
    inv_model: Mat4,
}

impl Pickable {
    pub fn of(guid: u32, inst: &crate::gpu::Instance) -> Self {
        let (c, r) = inst.mesh.bounds;
        let m = inst.model;
        let scale = m
            .x_axis
            .truncate()
            .length()
            .max(m.y_axis.truncate().length())
            .max(m.z_axis.truncate().length());
        Pickable {
            guid,
            center: m.transform_point3(c),
            radius: r * scale,
            mesh: inst.mesh.clone(),
            inv_model: m.inverse(),
        }
    }

    /// Ray parameter of the nearest triangle hit, if any. The bounding
    /// sphere is the broad phase; the triangles are tested in mesh space
    /// with the ray transformed by the inverse model matrix, which keeps
    /// the parameter comparable across objects.
    pub fn hit(&self, origin: Vec3, dir: Vec3) -> Option<f32> {
        let oc = self.center - origin;
        let t = oc.dot(dir);
        let d2 = oc.length_squared() - t * t;
        if d2 > self.radius * self.radius || t + self.radius < 0.0 {
            return None;
        }
        let o = self.inv_model.transform_point3(origin);
        let d = self.inv_model.transform_vector3(dir);
        let p = &self.mesh.pick_positions;
        let mut best: Option<f32> = None;
        for tri in self.mesh.pick_indices.as_chunks::<3>().0 {
            let (a, b, c) = (p[tri[0] as usize], p[tri[1] as usize], p[tri[2] as usize]);
            if let Some(t) = ray_triangle(o, d, a, b, c) {
                if best.map(|bt| t < bt).unwrap_or(true) {
                    best = Some(t);
                }
            }
        }
        best
    }
}

/// Möller–Trumbore, both windings, returns the ray parameter.
fn ray_triangle(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let pv = d.cross(e2);
    let det = e1.dot(pv);
    if det.abs() < 1e-8 {
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

impl Pickable {}

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
) -> (Vec<crate::gpu::Instance>, Vec<Pickable>) {
    let mut out = Vec::new();
    let mut picks = Vec::new();
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
        let inst = instances_for(
            assets,
            gpu,
            meshes,
            palettes,
            o.setup_id,
            t,
            &app,
            key,
            pose.as_deref(),
        );
        picks.extend(inst.iter().map(|i| Pickable::of(o.guid, i)));
        out.extend(inst);
    }
    (out, picks)
}
