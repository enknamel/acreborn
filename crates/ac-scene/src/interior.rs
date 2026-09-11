//! Interior cells (EnvCell + Environment) as meshes.

use ac_formats::environment::CellStruct;
use ac_formats::gfxobj::{CullMode, Polygon};
use ac_formats::landblock::{env_cell_flags, EnvCell};
use ac_formats::surface::SurfaceBase;
use glam::{Mat4, Vec3};
use std::rc::Rc;

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
    /// World-space middles of this cell's portal polygons: its doorways
    /// and the openings between its rooms.
    ///
    /// The navigation lattice cannot be relied on to put a node in a
    /// doorway. Holtburg's are about a metre and a half across and a
    /// character is one and a third wide, so a node fits only when a
    /// lattice point falls within a few centimetres of the middle --
    /// which mostly it does not, and the building comes out sealed:
    /// ninety-odd separate islands in one town, every interior cut off
    /// from the street. The data says where the openings are, so the
    /// graph is told rather than left to find them.
    pub doorways: Vec<Vec3>,
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
        // The middle of every opening, in world space. `build_cell_mesh`
        // leaves these polygons out of the drawing because they are holes
        // in the wall; a hole in the wall is exactly where a doorway node
        // belongs.
        let doorways = cs
            .polygons
            .iter()
            .filter(|(id, _)| cs.portals.contains(id))
            .filter_map(|(_, p)| {
                // Across the opening, and at the foot of it. The middle
                // of the polygon is half way up the doorway, and a
                // character standing with its feet at waist height has
                // its head in the wall above the lintel -- which is how
                // two thirds of these were pushed aside as nowhere to
                // stand. The threshold is what is walked over.
                let mut mid = Vec3::ZERO;
                let mut sill = f32::MAX;
                let mut n = 0.0f32;
                for v in &p.vertex_ids {
                    let v = cs.vertices.iter().find(|(k, _)| *k as i32 == *v as i32)?;
                    let w = transform
                        .transform_point3(Vec3::new(v.1.origin[0], v.1.origin[1], v.1.origin[2]));
                    mid += w;
                    sill = sill.min(w.z);
                    n += 1.0;
                }
                (n > 0.0).then(|| {
                    let mid = mid / n;
                    // A hand's breadth above the threshold, so the probe
                    // starts clear of the floor plane itself.
                    Vec3::new(mid.x, mid.y, sill + 0.1)
                })
            })
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
            doorways,
            seen_outside: cell.flags & env_cell_flags::SEEN_OUTSIDE != 0,
        });
    }
    Ok(out)
}

/// Which interior cells a point is inside, the way the client decides
/// it: `EnvCell::point_in_cell` runs the cell structure's own BSP over
/// the point in cell space.
///
/// This is what tells a floor from the outside of one. A dungeon is
/// built out of boxes with walls a metre thick, and the tops and backs
/// of those boxes are ordinary up-facing collision triangles in sealed
/// voids between the rooms. Nothing stands there -- the client would
/// not even call it part of the dungeon -- but a walkable-grid sampler
/// happily lays a lattice over them.
#[derive(Default)]
pub struct CellIndex {
    cells: Vec<IndexedCell>,
    /// Bounds of every cell together, so a point out in the open is
    /// rejected without touching the list.
    lo: Vec3,
    hi: Vec3,
}

#[derive(Clone)]
struct IndexedCell {
    /// World-space bounds of the cell structure, as a first cut.
    lo: Vec3,
    hi: Vec3,
    /// World space to cell space.
    inverse: Mat4,
    environment: Rc<ac_formats::environment::Environment>,
    structure: u16,
}

impl CellIndex {
    /// Index the interior cells of an assembled landblock. Cheap: the
    /// environments are already in the asset cache.
    pub fn build(assets: &Assets, scene: &crate::landblock::LandblockScene) -> CellIndex {
        let mut cells = Vec::with_capacity(scene.cells.len());
        for c in &scene.cells {
            let Ok(env) = assets.environment(c.environment_id) else {
                continue;
            };
            let Some((_, cs)) = env
                .cells
                .iter()
                .find(|(k, _)| *k == c.cell_structure as u32)
            else {
                continue;
            };
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for (_, v) in &cs.vertices {
                let p = c.transform.transform_point3(v.origin);
                lo = lo.min(p);
                hi = hi.max(p);
            }
            if lo.x > hi.x {
                continue;
            }
            cells.push(IndexedCell {
                lo,
                hi,
                inverse: c.transform.inverse(),
                environment: env,
                structure: c.cell_structure,
            });
        }
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for c in &cells {
            lo = lo.min(c.lo);
            hi = hi.max(c.hi);
        }
        CellIndex { cells, lo, hi }
    }

    /// Take another block's cells into this index, so an area spanning
    /// several blocks judges a point against all of them.
    pub fn absorb(&mut self, other: &CellIndex) {
        if other.cells.is_empty() {
            return;
        }
        if self.cells.is_empty() {
            self.lo = other.lo;
            self.hi = other.hi;
        } else {
            self.lo = self.lo.min(other.lo);
            self.hi = self.hi.max(other.hi);
        }
        self.cells.extend(other.cells.iter().cloned());
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// `p` (world space) is inside one of the cells.
    pub fn contains(&self, p: Vec3) -> bool {
        if self.cells.is_empty() || !(p.cmpge(self.lo).all() && p.cmple(self.hi).all()) {
            return false;
        }
        self.cells.iter().any(|c| {
            p.cmpge(c.lo).all()
                && p.cmple(c.hi).all()
                && c.environment
                    .cells
                    .iter()
                    .find(|(k, _)| *k == c.structure as u32)
                    .is_some_and(|(_, s)| s.cell_bsp.contains_point(c.inverse.transform_point3(p)))
        })
    }
}
