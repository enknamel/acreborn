//! Interior cells (EnvCell + Environment) as meshes.

use std::collections::HashMap;

use ac_formats::environment::CellStruct;
use ac_formats::gfxobj::{CullMode, Polygon};
use ac_formats::landblock::EnvCell;
use ac_formats::surface::SurfaceBase;
use glam::{Mat4, Vec2};

use crate::model::{frame_to_mat, place, MeshVertex, PlacedPart, SubMesh};
use crate::{Assets, Error, Result};

/// One interior cell ready to draw: its structure mesh (landblock-local
/// transform applied on the caller's side) and its static objects.
#[derive(Debug, Clone)]
pub struct CellScene {
    pub cell_id: u32,
    pub environment_id: u32,
    pub cell_structure: u16,
    /// Landblock-local transform of the cell structure.
    pub transform: Mat4,
    pub submeshes: Vec<SubMesh>,
    pub parts: Vec<PlacedPart>,
}

/// Build the drawable triangles of a cell structure. Portal polygons (the
/// openings between cells) are skipped; surfaces come from the EnvCell's
/// surface list, indexed by the polygon's surface slot.
fn build_cell_mesh(assets: &Assets, cs: &CellStruct, surfaces: &[u32]) -> Result<Vec<SubMesh>> {
    let verts: HashMap<u16, &ac_formats::gfxobj::Vertex> =
        cs.vertices.iter().map(|(k, v)| (*k, v)).collect();
    let mut by_surface: HashMap<u32, SubMesh> = HashMap::new();
    let mut emit = |surface_idx: i16, vids: &[i16], uv_idx: &[u8], flip: bool| -> Result<()> {
        let surface_id = surfaces
            .get(surface_idx.max(0) as usize)
            .copied()
            .unwrap_or(0);
        let sub = match by_surface.entry(surface_id) {
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
                    (Some(0xFF80_8080), 0.0)
                };
                e.insert(SubMesh {
                    surface_id,
                    texture_override: None,
                    palette: None,
                    palette_hash: 0,
                    solid_color,
                    translucency,
                    two_sided: false,
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
    for (id, p) in &cs.polygons {
        if cs.portals.contains(id) {
            continue;
        }
        let p: &Polygon = p;
        emit(p.pos_surface, &p.vertex_ids, &p.pos_uv_indices, false)?;
        if p.cull == CullMode::None {
            emit(p.neg_surface, &p.vertex_ids, &p.neg_uv_indices, true)?;
        }
    }
    let mut v: Vec<SubMesh> = by_surface.into_values().collect();
    v.sort_by_key(|s| s.surface_id);
    Ok(v)
}

/// Load all interior cells of a landblock (`0x100 .. 0x100 + num_cells`).
/// `origin` is the landblock's world transform.
pub fn load_cells(
    assets: &Assets,
    block_id: u32,
    num_cells: u32,
    origin: Mat4,
) -> Result<Vec<CellScene>> {
    let mut out = Vec::with_capacity(num_cells as usize);
    let mut env_cache: HashMap<u32, std::rc::Rc<ac_formats::environment::Environment>> =
        HashMap::new();
    for i in 0..num_cells {
        let cell_id = (block_id & 0xFFFF_0000) | (0x100 + i);
        let Ok(bytes) = assets.cell.read(cell_id) else {
            continue;
        };
        let cell = match EnvCell::parse(cell_id, &bytes) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("envcell {cell_id:#010x}: {e}");
                continue;
            }
        };
        let env = match env_cache.get(&cell.environment_id) {
            Some(e) => e.clone(),
            None => {
                let b = assets.portal.read(cell.environment_id)?;
                let e = std::rc::Rc::new(
                    ac_formats::environment::Environment::parse(cell.environment_id, &b).map_err(
                        |source| Error::Format {
                            id: cell.environment_id,
                            source,
                        },
                    )?,
                );
                env_cache.insert(cell.environment_id, e.clone());
                e
            }
        };
        let Some((_, cs)) = env
            .cells
            .iter()
            .find(|(k, _)| *k == cell.cell_structure as u32)
        else {
            tracing::warn!(
                "envcell {cell_id:#010x}: structure {} not in {:#010x}",
                cell.cell_structure,
                cell.environment_id
            );
            continue;
        };
        let transform = origin * frame_to_mat(&cell.position);
        let submeshes = build_cell_mesh(assets, cs, &cell.surfaces)?;
        let mut parts = Vec::new();
        for stab in &cell.static_objects {
            match place(assets, stab.id, transform * frame_to_mat(&stab.frame)) {
                Ok(p) => parts.extend(p),
                Err(e) => tracing::warn!("cell static {:#010x}: {e}", stab.id),
            }
        }
        out.push(CellScene {
            cell_id,
            environment_id: cell.environment_id,
            cell_structure: cell.cell_structure,
            transform,
            submeshes,
            parts,
        });
    }
    Ok(out)
}
