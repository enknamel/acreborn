//! An outdoor landblock assembled for rendering.

use ac_formats::landblock::{CellLandblock, LandblockInfo};
use glam::Mat4;

use crate::model::{frame_to_mat, place, PlacedPart};
use crate::terrain::{self, TerrainMesh};
use crate::{lbid, scenery, Assets, Result};

#[derive(Debug)]
pub struct LandblockScene {
    pub id: u32,
    pub terrain: TerrainMesh,
    /// Static objects and buildings, in world space.
    pub parts: Vec<PlacedPart>,
    pub has_info: bool,
    pub scenery_count: usize,
}

pub fn load(assets: &Assets, block_id: u32) -> Result<LandblockScene> {
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
    if let Some(info) = &info {
        for stab in &info.objects {
            match place(assets, stab.id, origin * frame_to_mat(&stab.frame)) {
                Ok(p) => parts.extend(p),
                Err(e) => tracing::warn!("static {:#010x}: {e}", stab.id),
            }
        }
        for b in &info.buildings {
            match place(assets, b.model_id, origin * frame_to_mat(&b.frame)) {
                Ok(p) => parts.extend(p),
                Err(e) => tracing::warn!("building {:#010x}: {e}", b.model_id),
            }
        }
    }
    let mut scenery_count = 0;
    for inst in scenery::generate(assets, &lb, info.as_ref())? {
        match place(assets, inst.obj_id, origin * inst.local) {
            Ok(p) => {
                scenery_count += 1;
                parts.extend(p)
            }
            Err(e) => tracing::warn!("scenery {:#010x}: {e}", inst.obj_id),
        }
    }
    Ok(LandblockScene {
        id: block_id,
        terrain,
        parts,
        has_info,
        scenery_count,
    })
}
