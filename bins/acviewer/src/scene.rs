//! Build GPU batches from ac-scene output.

use std::collections::{HashMap, VecDeque};

use ac_scene::anim::{motion, AnimPlayer};
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
    push_submeshes(batches, &mesh.submeshes, transform);
}

/// Append submeshes transformed into world space to their material
/// batches; returns the bounds of the vertices pushed.
fn push_submeshes(
    batches: &mut HashMap<MaterialKey, Batch>,
    submeshes: &[model::SubMesh],
    transform: Mat4,
) -> (Vec3, Vec3) {
    let normal_mat = transform.inverse().transpose();
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for sub in submeshes {
        let key = match sub.solid_color {
            Some(c) => MaterialKey::Solid(c),
            None => MaterialKey::Texture {
                id: sub.surface_id,
                tex: sub.texture_override.unwrap_or(0),
                palette: sub.palette_hash,
            },
        };
        let alpha = 1.0 - sub.translucency.clamp(0.0, 1.0);
        let batch = batches.entry(key).or_default();
        let base = batch.vertices.len() as u32;
        batch.vertices.reserve(sub.vertices.len());
        batch.indices.reserve(sub.indices.len());
        for v in &sub.vertices {
            let p = transform.transform_point3(v.position);
            min = min.min(p);
            max = max.max(p);
            batch.vertices.push(Vertex {
                position: p.to_array(),
                normal: normal_mat
                    .transform_vector3(v.normal)
                    .normalize_or(Vec3::Z)
                    .to_array(),
                uv: v.uv.to_array(),
                color: [1.0, 1.0, 1.0, alpha],
            });
        }
        batch.indices.extend(sub.indices.iter().map(|i| i + base));
    }
    (min, max)
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
    let t0 = std::time::Instant::now();
    let scene = landblock::load(assets, id)?;
    let t_load = t0.elapsed();
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
    let t_terrain = t0.elapsed() - t_load;
    for cell in &scene.cells {
        let (lo, hi) = push_submeshes(&mut batches, &cell.submeshes, cell.transform);
        min = min.min(lo);
        max = max.max(hi);
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
    let t_cells = t0.elapsed() - t_load - t_terrain;
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
    let t_parts = t0.elapsed() - t_load - t_terrain - t_cells;
    tracing::debug!(
        "block {id:#010x}: load {:.1} ms, terrain {:.1} ms, {} cells {:.1} ms, {} parts {:.1} ms",
        t_load.as_secs_f64() * 1e3,
        t_terrain.as_secs_f64() * 1e3,
        scene.cells.len(),
        t_cells.as_secs_f64() * 1e3,
        scene.parts.len(),
        t_parts.as_secs_f64() * 1e3,
    );
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

/// Animation state for a server object: the looping cycle for its current
/// motion, plus any one-shots (attacks, emotes, a door swinging) playing
/// over it.
pub struct ObjectAnim {
    /// The looping cycle (idle, walk, run, a door held open).
    pub player: Option<AnimPlayer>,
    /// One-shots waiting to play, in order; the front one is playing and
    /// overrides the cycle's pose until it finishes.
    pub oneshots: VecDeque<AnimPlayer>,
    pub n_parts: usize,
    /// Forward motion command the current cycle was chosen for.
    pub motion: u32,
    /// Stance (wire low 16 bits) the cycle was chosen for.
    pub style: u16,
    /// Serial of the newest queued command already turned into a one-shot.
    pub played: u64,
}

/// Most one-shots waiting behind the playing one; older ones are dropped
/// so a burst of motion changes cannot lag the object for seconds.
const MAX_PENDING_ONESHOTS: usize = 4;

impl ObjectAnim {
    fn push_oneshot(&mut self, p: AnimPlayer) {
        self.oneshots.push_back(p);
        while self.oneshots.len() > MAX_PENDING_ONESHOTS + 1 {
            self.oneshots.remove(1);
        }
    }

    /// Advance by `dt` and return the pose to draw, if animated. A
    /// finished one-shot holds its last frame for one more tick before the
    /// next one (or the cycle) takes over.
    pub fn pose(&mut self, dt: f32) -> Option<Vec<Mat4>> {
        // The cycle keeps time underneath so movement resumes in phase.
        if let Some(p) = self.player.as_mut() {
            p.advance(dt);
        }
        while self.oneshots.front().is_some_and(|p| p.finished()) {
            self.oneshots.pop_front();
        }
        if let Some(front) = self.oneshots.front_mut() {
            front.advance(dt);
            return Some(front.part_transforms(self.n_parts));
        }
        let n = self.n_parts;
        self.player.as_ref().map(|p| p.part_transforms(n))
    }
}

/// Cached MotionTables by id (None when the table failed to load).
pub type MotionTables = HashMap<u32, Option<ac_formats::motion_table::MotionTable>>;

/// The stance and looping motion an object's movement state resolves to
/// in its table: Ready (or nothing) means the stance's default motion.
fn resolve_motion(t: &ac_formats::motion_table::MotionTable, m: &ac_world::Motion) -> (u32, u32) {
    let style = if m.style != 0 && t.default_motion(motion::stance_of(m.style)).is_some() {
        motion::stance_of(m.style)
    } else {
        t.default_style
    };
    let idle = t.default_motion(style).unwrap_or(motion::READY);
    // Movement events carry only the low 16 bits of a command.
    let forward = if m.forward & 0xFFFF == motion::READY & 0xFFFF || m.forward == 0 {
        idle
    } else {
        m.forward
    };
    (style, forward)
}

/// Build the animation state for an object's current motion. `prev` is the
/// state it replaces: its one-shots carry over, and the change from its
/// motion to the new one plays the table's transition link first (a door
/// opening, a creature going from standing to running) when there is one.
pub fn object_anim(
    assets: &Assets,
    o: &ac_world::WorldObject,
    tables: &mut MotionTables,
    prev: Option<ObjectAnim>,
) -> ObjectAnim {
    let n_parts = assets.setup(o.setup_id).map(|s| s.parts.len()).unwrap_or(0);
    let wanted = o.motion.forward;
    let mut anim = ObjectAnim {
        player: None,
        oneshots: VecDeque::new(),
        n_parts,
        motion: wanted,
        style: o.motion.style,
        played: prev.as_ref().map(|p| p.played).unwrap_or(0),
    };
    if o.motion_table_id == 0 {
        return anim;
    }
    let table = tables
        .entry(o.motion_table_id)
        .or_insert_with(|| ac_scene::anim::motion_table(assets, o.motion_table_id).ok());
    let Some(t) = table.as_ref() else { return anim };
    let (style, forward) = resolve_motion(t, &o.motion);
    anim.player = AnimPlayer::cycle(assets, t, style, forward).or_else(|| {
        let idle = t.default_motion(style).unwrap_or(motion::READY);
        AnimPlayer::cycle(assets, t, style, idle)
    });
    if let Some(mut prev) = prev {
        anim.oneshots.append(&mut prev.oneshots);
        if prev.style == o.motion.style && prev.motion != wanted {
            let (_, from) = resolve_motion(
                t,
                &ac_world::Motion {
                    forward: prev.motion,
                    ..o.motion
                },
            );
            if let Some(mut link) = AnimPlayer::link(assets, t, style, from, forward) {
                link.speed = o.motion.forward_speed.abs().max(0.1);
                anim.push_oneshot(link);
            }
        }
    }
    anim
}

/// Turn commands the server queued on `o` since `anim` last looked into
/// one-shot players (attacks, emotes) over the current stance and motion.
pub fn queue_commands(
    assets: &Assets,
    o: &ac_world::WorldObject,
    tables: &MotionTables,
    anim: &mut ObjectAnim,
) {
    let table = tables.get(&o.motion_table_id).and_then(|t| t.as_ref());
    for c in o.commands.since(anim.played) {
        anim.played = c.serial;
        let Some(t) = table else { continue };
        let (style, current) = resolve_motion(t, &o.motion);
        let idle = t.default_motion(style).unwrap_or(motion::READY);
        let link = AnimPlayer::link(assets, t, style, current, c.command as u32)
            .or_else(|| AnimPlayer::link(assets, t, style, idle, c.command as u32));
        match link {
            Some(mut p) => {
                p.speed = c.speed.abs().max(0.1);
                anim.push_oneshot(p);
            }
            None => tracing::debug!(
                "{:#010x}: no animation for command {:#06x} in table {:#010x}",
                o.guid,
                c.command,
                o.motion_table_id
            ),
        }
    }
}

/// True if any drawable object has an animation, so batches need refreshing.
pub fn any_animated(anims: &HashMap<u32, ObjectAnim>) -> bool {
    anims
        .values()
        .any(|a| a.player.is_some() || !a.oneshots.is_empty())
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
    tables: &mut MotionTables,
    dt: f32,
) -> (Vec<crate::gpu::Instance>, Vec<Pickable>) {
    let mut out = Vec::new();
    let mut picks = Vec::new();
    for o in world.drawable().filter(|o| !o.is_player) {
        let Some(t) = o.transform() else { continue };
        let (app, key) = appearance_of(assets, o, palettes);
        let stale = anims
            .get(&o.guid)
            .map(|a| a.motion != o.motion.forward || a.style != o.motion.style)
            .unwrap_or(true);
        if stale {
            let prev = anims.remove(&o.guid);
            anims.insert(o.guid, object_anim(assets, o, tables, prev));
        }
        let anim = anims.get_mut(&o.guid).unwrap();
        queue_commands(assets, o, tables, anim);
        let pose = anim.pose(dt);
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
