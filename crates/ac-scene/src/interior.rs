//! Interior cells (EnvCell + Environment) as meshes.

use ac_formats::environment::CellStruct;
use ac_formats::gfxobj::{CullMode, Polygon};
use ac_formats::landblock::{env_cell_flags, EnvCell};
use ac_formats::surface::SurfaceBase;
use glam::Mat4;

use crate::lighting::{cell_lights, CellLight};
use crate::model::{
    emit_polygon, frame_to_mat, place, PlacedPart, SubMesh, SubMeshes, VertexTable,
};
use crate::{Assets, Result};

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
    /// Lights carried by the cell's static objects, in world space.
    pub lights: Vec<CellLight>,
    /// Full ids of the cells behind this cell's portals.
    pub portal_cells: Vec<u32>,
    /// The cell can be seen from outdoors (`env_cell_flags::SEEN_OUTSIDE`).
    pub seen_outside: bool,
}

/// Build the drawable triangles of a cell structure. Portal polygons (the
/// openings between cells) are skipped; surfaces come from the EnvCell's
/// surface list, indexed by the polygon's surface slot.
fn build_cell_mesh(assets: &Assets, cs: &CellStruct, surfaces: &[u32]) -> Result<Vec<SubMesh>> {
    let verts = VertexTable::new(&cs.vertices);
    let mut by_surface = SubMeshes::new();
    let mut emit = |surface_idx: i16, vids: &[i16], uv_idx: &[u8], flip: bool| -> Result<()> {
        let surface_id = surfaces
            .get(surface_idx.max(0) as usize)
            .copied()
            .unwrap_or(0);
        let sub = by_surface.get_or_insert(surface_id, || {
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
            Ok(SubMesh {
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
        })?;
        emit_polygon(sub, &verts, vids, uv_idx, flip);
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
    Ok(by_surface.finish())
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
        let env = assets.environment(cell.environment_id)?;
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
        // Static object frames are landblock-local, not cell-local: the
        // client places them by the block's frame alone (as does ACViewer
        // with its landblock matrix).
        let mut parts = Vec::new();
        for stab in &cell.static_objects {
            match place(assets, stab.id, origin * frame_to_mat(&stab.frame)) {
                Ok(p) => parts.extend(p),
                Err(e) => tracing::warn!("cell static {:#010x}: {e}", stab.id),
            }
        }
        let lights = cell_lights(assets, &cell.static_objects, origin);
        let portal_cells = cell
            .portals
            .iter()
            .map(|p| (block_id & 0xFFFF_0000) | p.other_cell_id as u32)
            .collect();
        out.push(CellScene {
            cell_id,
            environment_id: cell.environment_id,
            cell_structure: cell.cell_structure,
            transform,
            submeshes,
            parts,
            lights,
            portal_cells,
            seen_outside: cell.flags & env_cell_flags::SEEN_OUTSIDE != 0,
        });
    }
    Ok(out)
}
