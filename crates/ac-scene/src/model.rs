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

/// Triangles sharing one surface (material).
#[derive(Debug, Clone)]
pub struct SubMesh {
    /// Surface (0x08) id, or 0 if the polygon referenced no surface.
    pub surface_id: u32,
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
    use std::collections::HashMap;
    let verts: HashMap<u16, &ac_formats::gfxobj::Vertex> =
        g.vertices.iter().map(|(k, v)| (*k, v)).collect();
    let mut by_surface: HashMap<(u32, bool), SubMesh> = HashMap::new();

    let mut emit = |surface_idx: i16, vids: &[i16], uv_idx: &[u8], flip: bool| -> Result<()> {
        let surface_id = g
            .surfaces
            .get(surface_idx.max(0) as usize)
            .copied()
            .unwrap_or(0);
        let two_sided = false;
        let sub = match by_surface.entry((surface_id, two_sided)) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let (solid_color, translucency) = if surface_id != 0 {
                    let s = assets.surface(surface_id)?;
                    let color = match s.base {
                        SurfaceBase::Solid { color } => Some(color),
                        SurfaceBase::Image { .. } => None,
                    };
                    (color, s.translucency)
                } else {
                    (Some(0xFFFF_00FF), 0.0)
                };
                e.insert(SubMesh {
                    surface_id,
                    solid_color,
                    translucency,
                    two_sided,
                    vertices: Vec::new(),
                    indices: Vec::new(),
                })
            }
        };
        let base = sub.vertices.len() as u32;
        let mut n = 0u32;
        for (i, &vid) in vids.iter().enumerate() {
            let Some(v) = verts.get(&(vid as u16)) else {
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
        Ok(())
    };

    for (_, p) in &g.polygons {
        emit(p.pos_surface, &p.vertex_ids, &p.pos_uv_indices, false)?;
        if p.cull == CullMode::None {
            emit(p.neg_surface, &p.vertex_ids, &p.neg_uv_indices, true)?;
        }
    }
    let mut submeshes: Vec<SubMesh> = by_surface.into_values().collect();
    submeshes.sort_by_key(|s| s.surface_id);
    Ok(Mesh {
        gfxobj_id: g.id,
        submeshes,
    })
}

/// A model instance: one GfxObj with a world transform.
#[derive(Debug, Clone)]
pub struct PlacedPart {
    pub gfxobj_id: u32,
    pub transform: Mat4,
}

pub fn frame_to_mat(f: &ac_formats::geom::Frame) -> Mat4 {
    Mat4::from_rotation_translation(f.orientation.normalize(), f.origin)
}

/// Expand a model id (GfxObj `0x01......` or Setup `0x02......`) placed at
/// `world` into its parts. Setups use placement frame 0 (the default pose).
pub fn place(assets: &Assets, model_id: u32, world: Mat4) -> Result<Vec<PlacedPart>> {
    match model_id >> 24 {
        0x01 => Ok(vec![PlacedPart {
            gfxobj_id: model_id,
            transform: world,
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
                if part == 0 {
                    continue;
                }
                let local = placement
                    .and_then(|(_, af)| af.frames.get(i))
                    .map(frame_to_mat)
                    .unwrap_or(Mat4::IDENTITY);
                let scale = s
                    .default_scale
                    .get(i)
                    .copied()
                    .map(Mat4::from_scale)
                    .unwrap_or(Mat4::IDENTITY);
                out.push(PlacedPart {
                    gfxobj_id: part,
                    transform: world * local * scale,
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
