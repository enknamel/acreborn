//! GfxObj and Setup to renderable triangle lists.

use ac_formats::gfxobj::{CullMode, GfxObj};
use ac_formats::surface::SurfaceBase;
use glam::{Mat4, Quat, Vec2, Vec3};

use crate::{Assets, Result};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshVertex {
    pub position: Vec3,
    pub normal: Vec3,
    pub uv: Vec2,
}

/// Per-object appearance overrides from an ObjDesc: part model swaps,
/// per-part texture swaps, and a composed palette for indexed textures.
#[derive(Debug, Clone, Default)]
pub struct Appearance {
    /// Part index -> replacement GfxObj id.
    pub part_swaps: std::collections::HashMap<u8, u32>,
    /// Part index -> (old SurfaceTexture id, new SurfaceTexture id).
    pub texture_swaps: std::collections::HashMap<u8, Vec<(u32, u32)>>,
    /// Composed palette colors (0xAARRGGBB) applied to indexed textures,
    /// with a stable hash for material caching.
    pub palette: Option<std::rc::Rc<Vec<u32>>>,
    pub palette_hash: u64,
}

impl Appearance {
    pub fn is_empty(&self) -> bool {
        self.part_swaps.is_empty() && self.texture_swaps.is_empty() && self.palette.is_none()
    }

    /// Build from wire ObjDesc data. Sub-palette offset and length are in
    /// units of 8 colors (0 length = the whole 2048-color palette).
    ///
    /// Later entries replace earlier ones for the same part (part swaps) or
    /// the same part and old texture (texture swaps), as the client's
    /// `ObjDesc::AddAnimPartChange` / `AddTextureMapChange` do: a server
    /// lists the base body first and clothing after it.
    pub fn from_obj_desc(
        assets: &Assets,
        palette_id: u32,
        sub_palettes: &[(u32, u8, u8)],
        texture_changes: &[(u8, u32, u32)],
        part_changes: &[(u8, u32)],
    ) -> Self {
        let mut a = Appearance::default();
        for &(idx, id) in part_changes {
            a.part_swaps.insert(idx, id);
        }
        for &(idx, old, new) in texture_changes {
            let swaps = a.texture_swaps.entry(idx).or_default();
            swaps.retain(|(o, _)| *o != old);
            swaps.push((old, new));
        }
        if palette_id != 0 || !sub_palettes.is_empty() {
            let mut colors: Vec<u32> = if palette_id != 0 {
                assets
                    .palette(palette_id)
                    .map(|p| p.colors.clone())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if colors.len() < 256 {
                colors.resize(256, 0xFF00_0000);
            }
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325 ^ palette_id as u64;
            for &(sub_id, off, len) in sub_palettes {
                let offset = off as usize * 8;
                let count = if len == 0 { 2048 } else { len as usize * 8 };
                if let Ok(sp) = assets.palette(sub_id) {
                    for j in 0..count {
                        if let (Some(dst), Some(src)) =
                            (colors.get_mut(offset + j), sp.colors.get(offset + j))
                        {
                            *dst = *src;
                        }
                    }
                }
                hash = (hash ^ sub_id as u64).wrapping_mul(0x100_0000_01b3)
                    ^ ((off as u64) << 8 | len as u64);
            }
            a.palette = Some(std::rc::Rc::new(colors));
            a.palette_hash = hash;
        }
        a
    }
}

/// Triangles sharing one surface (material).
#[derive(Debug, Clone)]
pub struct SubMesh {
    /// Surface (0x08) id, or 0 if the polygon referenced no surface.
    pub surface_id: u32,
    /// Replacement SurfaceTexture (0x05) id from an appearance texture swap.
    pub texture_override: Option<u32>,
    /// Composed palette for indexed textures, from the appearance.
    pub palette: Option<std::rc::Rc<Vec<u32>>>,
    pub palette_hash: u64,
    /// Solid color (0xAARRGGBB) when the surface has no texture.
    pub solid_color: Option<u32>,
    pub translucency: f32,
    pub two_sided: bool,
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct Mesh {
    pub gfxobj_id: u32,
    pub submeshes: Vec<SubMesh>,
}

/// Build a mesh from a GfxObj's drawing polygons. N-gons are fan
/// triangulated; back faces of two-sided polygons get their own triangles
/// with flipped winding and the negative-side surface/UVs.
pub fn build_mesh(assets: &Assets, g: &GfxObj) -> Result<Mesh> {
    build_mesh_with(assets, g, 0, &Appearance::default())
}

/// Vertices of a GfxObj or cell structure looked up by polygon vertex id.
/// Ids are small and dense, so a table indexed by id beats hashing.
pub(crate) struct VertexTable<'a> {
    by_id: Vec<Option<&'a ac_formats::gfxobj::Vertex>>,
}

impl<'a> VertexTable<'a> {
    pub fn new(verts: &'a [(u16, ac_formats::gfxobj::Vertex)]) -> Self {
        let len = verts
            .iter()
            .map(|(k, _)| *k as usize + 1)
            .max()
            .unwrap_or(0);
        let mut by_id = vec![None; len];
        for (k, v) in verts {
            by_id[*k as usize] = Some(v);
        }
        VertexTable { by_id }
    }

    pub fn get(&self, id: i16) -> Option<&'a ac_formats::gfxobj::Vertex> {
        self.by_id.get(id as u16 as usize).copied().flatten()
    }
}

/// Append one polygon (fan triangulated) to `sub`, with flipped winding
/// and normals for a back face.
pub(crate) fn emit_polygon(
    sub: &mut SubMesh,
    verts: &VertexTable,
    vids: &[i16],
    uv_idx: &[u8],
    flip: bool,
) {
    let base = sub.vertices.len() as u32;
    sub.vertices.reserve(vids.len());
    let mut n = 0u32;
    for (i, &vid) in vids.iter().enumerate() {
        let Some(v) = verts.get(vid) else {
            continue;
        };
        let uv = uv_idx
            .get(i)
            .and_then(|&ui| v.uvs.get(ui as usize))
            .map(|u| Vec2::new(u.u, u.v))
            .unwrap_or(Vec2::ZERO);
        sub.vertices.push(MeshVertex {
            position: v.origin,
            normal: if flip { -v.normal } else { v.normal },
            uv,
        });
        n += 1;
    }
    for i in 1..n.saturating_sub(1) {
        if flip {
            sub.indices
                .extend_from_slice(&[base, base + i + 1, base + i]);
        } else {
            sub.indices
                .extend_from_slice(&[base, base + i, base + i + 1]);
        }
    }
}

/// Submeshes grouped by surface, in first-use order; sorted by surface id
/// when finished. A model uses a handful of surfaces, so a linear scan is
/// cheaper than a map.
pub(crate) struct SubMeshes(Vec<SubMesh>);

impl SubMeshes {
    pub fn new() -> Self {
        SubMeshes(Vec::new())
    }

    /// The submesh for `surface_id`, created by `make` on first use.
    pub fn get_or_insert(
        &mut self,
        surface_id: u32,
        make: impl FnOnce() -> Result<SubMesh>,
    ) -> Result<&mut SubMesh> {
        let idx = match self.0.iter().position(|s| s.surface_id == surface_id) {
            Some(i) => i,
            None => {
                self.0.push(make()?);
                self.0.len() - 1
            }
        };
        Ok(&mut self.0[idx])
    }

    pub fn finish(mut self) -> Vec<SubMesh> {
        self.0.sort_by_key(|s| s.surface_id);
        self.0
    }
}

/// `build_mesh` with appearance overrides for the given part index.
pub fn build_mesh_with(
    assets: &Assets,
    g: &GfxObj,
    part_index: u8,
    app: &Appearance,
) -> Result<Mesh> {
    let swaps = app.texture_swaps.get(&part_index);
    let verts = VertexTable::new(&g.vertices);
    let mut by_surface = SubMeshes::new();

    let mut emit = |surface_idx: i16, vids: &[i16], uv_idx: &[u8], flip: bool| -> Result<()> {
        let surface_id = g
            .surfaces
            .get(surface_idx.max(0) as usize)
            .copied()
            .unwrap_or(0);
        let sub = by_surface.get_or_insert(surface_id, || {
            let (solid_color, translucency, texture) = if surface_id != 0 {
                let s = assets.surface(surface_id)?;
                match s.base {
                    SurfaceBase::Solid { color } => (Some(color), s.translucency, None),
                    SurfaceBase::Image { texture, .. } => (None, s.translucency, Some(texture)),
                }
            } else {
                (Some(0xFFFF_00FF), 0.0, None)
            };
            // A texture swap applies when the surface's texture matches the old id.
            let texture_override = match (swaps, texture) {
                (Some(sw), Some(texture)) => sw
                    .iter()
                    .find(|(old, _)| *old == texture)
                    .map(|(_, new)| *new),
                _ => None,
            };
            Ok(SubMesh {
                surface_id,
                texture_override,
                palette: app.palette.clone(),
                palette_hash: app.palette_hash,
                solid_color,
                translucency,
                two_sided: false,
                vertices: Vec::new(),
                indices: Vec::new(),
            })
        })?;
        emit_polygon(sub, &verts, vids, uv_idx, flip);
        Ok(())
    };

    for (_, p) in &g.polygons {
        emit(p.pos_surface, &p.vertex_ids, &p.pos_uv_indices, false)?;
        if p.cull == CullMode::None {
            emit(p.neg_surface, &p.vertex_ids, &p.neg_uv_indices, true)?;
        }
    }
    Ok(Mesh {
        gfxobj_id: g.id,
        submeshes: by_surface.finish(),
    })
}

/// A model instance: one GfxObj with a world transform.
#[derive(Debug, Clone)]
pub struct PlacedPart {
    pub gfxobj_id: u32,
    pub transform: Mat4,
    /// Index of the part within its Setup (0 for a bare GfxObj).
    pub part_index: u8,
}

pub fn frame_to_mat(f: &ac_formats::geom::Frame) -> Mat4 {
    Mat4::from_rotation_translation(f.orientation.normalize(), f.origin)
}

/// Expand a model id (GfxObj `0x01......` or Setup `0x02......`) placed at
/// `world` into its parts. Setups use placement frame 0 (the default pose).
pub fn place(assets: &Assets, model_id: u32, world: Mat4) -> Result<Vec<PlacedPart>> {
    place_with(assets, model_id, world, &Appearance::default())
}

/// `place` with part swaps from an appearance.
pub fn place_with(
    assets: &Assets,
    model_id: u32,
    world: Mat4,
    app: &Appearance,
) -> Result<Vec<PlacedPart>> {
    place_posed(assets, model_id, world, app, None)
}

/// `place_with` using animated per-part transforms instead of the
/// placement frame when `pose` is given (one transform per Setup part).
pub fn place_posed(
    assets: &Assets,
    model_id: u32,
    world: Mat4,
    app: &Appearance,
    pose: Option<&[Mat4]>,
) -> Result<Vec<PlacedPart>> {
    match model_id >> 24 {
        0x01 => Ok(vec![PlacedPart {
            gfxobj_id: model_id,
            transform: world,
            part_index: 0,
        }]),
        0x02 => {
            let s = assets.setup(model_id)?;
            let placement = s
                .placement_frames
                .iter()
                .find(|(k, _)| *k == 0)
                .or(s.placement_frames.first());
            let mut out = Vec::with_capacity(s.parts.len());
            for (i, &part) in s.parts.iter().enumerate() {
                let part = app.part_swaps.get(&(i as u8)).copied().unwrap_or(part);
                if part == 0 {
                    continue;
                }
                let local = match pose.and_then(|p| p.get(i)) {
                    Some(m) => *m,
                    None => placement
                        .and_then(|(_, af)| af.frames.get(i))
                        .map(frame_to_mat)
                        .unwrap_or(Mat4::IDENTITY),
                };
                let scale = s
                    .default_scale
                    .get(i)
                    .copied()
                    .map(Mat4::from_scale)
                    .unwrap_or(Mat4::IDENTITY);
                out.push(PlacedPart {
                    gfxobj_id: part,
                    transform: world * local * scale,
                    part_index: i as u8,
                });
            }
            Ok(out)
        }
        _ => Ok(Vec::new()),
    }
}

pub fn quat_frame(origin: Vec3, orientation: Quat) -> Mat4 {
    Mat4::from_rotation_translation(orientation, origin)
}
