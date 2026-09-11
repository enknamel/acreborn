//! An outdoor landblock assembled for rendering.

use std::rc::Rc;

use ac_formats::landblock::{CellLandblock, LandblockInfo};
use glam::Mat4;

use crate::lighting::{self, LightSampler};
use crate::model::{frame_to_mat, place, PlacedPart};
use crate::terrain::{self, TerrainMesh};
use crate::{lbid, scenery, Assets, Result};

#[derive(Debug)]
pub struct LandblockScene {
    pub id: u32,
    pub terrain: TerrainMesh,
    /// Static objects and buildings, in world space.
    pub parts: Vec<PlacedPart>,
    /// The same placements before they were expanded into parts:
    /// `(model id, world transform)`. Collision needs them whole,
    /// because whether a model collides at all is a property of the
    /// model rather than of any one of its parts.
    pub placements: Vec<(u32, Mat4)>,
    pub has_info: bool,
    pub scenery_count: usize,
    /// A dungeon block: all terrain heights are zero and it only has
    /// interior cells. Its terrain is a placeholder and should not be drawn.
    pub is_dungeon: bool,
    /// Interior cells (buildings' insides, dungeons).
    pub cells: Vec<crate::interior::CellScene>,
    /// Per-cell interior lighting (torches and lamps plus a low ambient).
    pub lights: LightSampler,
}

/// The assembled landblock, from [`Assets::landblock`]'s cache when it
/// was loaded recently (the viewer's renderer and its collision world
/// share one assembly this way).
pub fn load(assets: &Assets, block_id: u32) -> Result<Rc<LandblockScene>> {
    assets.landblock(block_id)
}

/// Assemble a landblock from the archives; [`load`] caches the result.
pub(crate) fn build(assets: &Assets, block_id: u32) -> Result<LandblockScene> {
    let block_id = block_id & 0xFFFF_0000;
    let region = assets.region()?;
    let lb_id = block_id | 0xFFFF;
    let lb = CellLandblock::parse(lb_id, &assets.cell.read(lb_id)?)
        .map_err(|source| crate::Error::Format { id: lb_id, source })?;
    let terrain = terrain::build(&lb, &region.land_defs.land_height_table);

    let origin = Mat4::from_translation(lbid::world_origin(block_id));
    let mut parts = Vec::new();
    let info_id = block_id | 0xFFFE;
    let has_info = assets.cell.entry(info_id).is_some();
    let info = if has_info {
        Some(
            LandblockInfo::parse(info_id, &assets.cell.read(info_id)?).map_err(|source| {
                crate::Error::Format {
                    id: info_id,
                    source,
                }
            })?,
        )
    } else {
        None
    };
    let mut placements: Vec<(u32, Mat4)> = Vec::new();
    if let Some(info) = &info {
        for stab in &info.objects {
            let world = origin * frame_to_mat(&stab.frame);
            match place(assets, stab.id, world) {
                Ok(p) => {
                    placements.push((stab.id, world));
                    parts.extend(p)
                }
                Err(e) => tracing::warn!("static {:#010x}: {e}", stab.id),
            }
        }
        for b in &info.buildings {
            let world = origin * frame_to_mat(&b.frame);
            match place(assets, b.model_id, world) {
                Ok(p) => {
                    placements.push((b.model_id, world));
                    parts.extend(p)
                }
                Err(e) => tracing::warn!("building {:#010x}: {e}", b.model_id),
            }
        }
    }
    let mut scenery_count = 0;
    for inst in scenery::generate(assets, &lb, info.as_ref())? {
        let world = origin * inst.local;
        match place(assets, inst.obj_id, world) {
            Ok(p) => {
                scenery_count += 1;
                placements.push((inst.obj_id, world));
                parts.extend(p)
            }
            Err(e) => tracing::warn!("scenery {:#010x}: {e}", inst.obj_id),
        }
    }
    let cells = match &info {
        Some(info) if info.num_cells > 0 => {
            crate::interior::load_cells(assets, block_id, info.num_cells, origin)?
        }
        _ => Vec::new(),
    };
    let is_dungeon = !cells.is_empty() && lb.height.iter().all(|&h| h == 0);
    let ambient = if is_dungeon {
        lighting::DUNGEON_AMBIENT
    } else {
        lighting::BUILDING_AMBIENT
    };
    let lights = LightSampler::build(&cells, ambient);
    Ok(LandblockScene {
        id: block_id,
        terrain,
        parts,
        placements,
        has_info,
        scenery_count,
        cells,
        is_dungeon,
        lights,
    })
}
