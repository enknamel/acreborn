//! Particle effects attached to the world's static objects (torches,
//! braziers, portals): every static object whose Setup has a default
//! physics script gets a running emitter set while its landblock is loaded.

use std::collections::HashMap;

use ac_formats::landblock::{EnvCell, LandblockInfo};
use ac_scene::model::frame_to_mat;
use ac_scene::particles::Quad;
use ac_scene::{lbid, Assets};
use glam::Mat4;

use crate::particles::Demo;

#[derive(Default)]
pub struct WorldFx {
    blocks: HashMap<u32, Vec<Demo>>,
    /// Simulation runs in fixed 1/60 s steps; leftover time carries over.
    pending: f32,
}

impl WorldFx {
    /// Create emitters for a landblock's static objects (outdoor stabs and
    /// every interior cell's stabs). Objects without a script are skipped.
    pub fn load_block(&mut self, assets: &Assets, block_id: u32) {
        let block_id = block_id & 0xFFFF_0000;
        if self.blocks.contains_key(&block_id) {
            return;
        }
        let origin = Mat4::from_translation(lbid::world_origin(block_id));
        let mut demos = Vec::new();
        let mut try_add = |setup_id: u32, transform: Mat4| {
            let Ok(setup) = assets.setup(setup_id) else {
                return;
            };
            if setup.default_script == 0 {
                return;
            }
            match Demo::new(assets, setup_id, transform) {
                Ok(d) => demos.push(d),
                Err(e) => tracing::debug!("fx for {setup_id:#010x}: {e:#}"),
            }
        };
        let info_id = block_id | 0xFFFE;
        let info = assets
            .cell
            .read(info_id)
            .ok()
            .and_then(|b| LandblockInfo::parse(info_id, &b).ok());
        if let Some(info) = &info {
            for stab in &info.objects {
                try_add(stab.id, origin * frame_to_mat(&stab.frame));
            }
            for n in 0..info.num_cells {
                let cell_id = block_id | (0x100 + n);
                let Some(cell) = assets
                    .cell
                    .read(cell_id)
                    .ok()
                    .and_then(|b| EnvCell::parse(cell_id, &b).ok())
                else {
                    continue;
                };
                let cell_t = origin * frame_to_mat(&cell.position);
                for stab in &cell.static_objects {
                    try_add(stab.id, cell_t * frame_to_mat(&stab.frame));
                }
            }
        }
        if !demos.is_empty() {
            tracing::info!(
                "landblock {block_id:#010x}: {} particle emitters",
                demos.len()
            );
        }
        self.blocks.insert(block_id, demos);
    }

    pub fn unload_block(&mut self, block_id: u32) {
        self.blocks.remove(&(block_id & 0xFFFF_0000));
    }

    /// Advance all emitters by `dt` seconds.
    pub fn update(&mut self, assets: &Assets, dt: f32) {
        self.pending += dt.min(0.25);
        let step = 1.0 / 60.0;
        if self.pending < step {
            return;
        }
        let run = (self.pending / step).floor() * step;
        self.pending -= run;
        for demos in self.blocks.values_mut() {
            for d in demos.iter_mut() {
                d.simulate(assets, run);
            }
        }
    }

    pub fn quads(&self) -> Vec<Quad> {
        let mut out = Vec::new();
        for demos in self.blocks.values() {
            for d in demos {
                out.extend(d.quads());
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.values().all(|v| v.is_empty())
    }
}
