//! Particle effects attached to the world's objects: every object whose
//! Setup has a default physics script gets a running emitter set.
//!
//! There are two kinds. The landblock's own stabs (torches, braziers,
//! the flames in a dungeon) are in the DATs, so they come and go with
//! their block. Portals, lifestones and the like are sent by the server
//! as ordinary objects, and a portal is nothing *but* its particles --
//! its Setup's single polygon is a fully transparent quad, there only to
//! be clicked -- so without these it is invisible.

use std::collections::HashMap;

use ac_formats::landblock::{EnvCell, LandblockInfo};
use ac_scene::model::frame_to_mat;
use ac_scene::particles::Quad;
use ac_scene::{lbid, Assets};
use ac_world::World;
use glam::Mat4;

use crate::particles::Demo;

/// One server-sent object's effect, and the sweep that last saw the
/// object: an entry no sweep touches belongs to an object that has gone.
struct ObjectFx {
    demo: Demo,
    seen: u64,
}

#[derive(Default)]
pub struct WorldFx {
    blocks: HashMap<u32, Vec<Demo>>,
    objects: HashMap<u32, ObjectFx>,
    /// Which sweep [`WorldFx::sync_objects`] is on, so it can retire
    /// entries without building a set of the live guids every time.
    sweep: u64,
    /// The world generation the last sweep ran on: nothing has appeared,
    /// moved or gone since, so the sweep can be skipped entirely.
    swept_generation: Option<u64>,
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

    /// Match the running effects to the objects the server has sent: a
    /// new object whose Setup has a default script starts one, an object
    /// that moved carries its emitters along, and one that has gone (or
    /// is no longer drawn) takes them with it.
    pub fn sync_objects(&mut self, assets: &Assets, world: &World) {
        if self.swept_generation == Some(world.generation) {
            return;
        }
        self.swept_generation = Some(world.generation);
        self.sweep += 1;
        let sweep = self.sweep;
        for o in world.drawable() {
            let Some(t) = o.transform() else { continue };
            if let Some(fx) = self.objects.get_mut(&o.guid) {
                fx.demo.set_transform(t);
                fx.seen = sweep;
                continue;
            }
            // Most objects have no script at all, so this is the cheap
            // test that keeps the rest of the work off the frame.
            if !assets
                .setup(o.setup_id)
                .is_ok_and(|s| s.default_script != 0)
            {
                continue;
            }
            match Demo::new(assets, o.setup_id, t) {
                Ok(demo) => {
                    self.objects.insert(o.guid, ObjectFx { demo, seen: sweep });
                }
                Err(e) => tracing::debug!("fx for {} ({:#010x}): {e:#}", o.name, o.setup_id),
            }
        }
        self.objects.retain(|_, fx| fx.seen == sweep);
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
        for fx in self.objects.values_mut() {
            fx.demo.simulate(assets, run);
        }
    }

    pub fn quads(&self) -> Vec<Quad> {
        let mut out = Vec::new();
        for demos in self.blocks.values() {
            for d in demos {
                out.extend(d.quads());
            }
        }
        for fx in self.objects.values() {
            out.extend(fx.demo.quads());
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty() && self.blocks.values().all(|v| v.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use ac_scene::particles::SpriteImage;
    use ac_world::object::Position;
    use ac_world::{World, WorldObject};
    use glam::Vec3;

    use super::*;

    /// The Setup every portal in the game shares. Its one polygon is a
    /// fully transparent quad, so the default script's emitters are the
    /// whole of what a portal looks like.
    const PORTAL_SETUP: u32 = 0x0200_01B3;

    fn portal(guid: u32, local: Vec3) -> WorldObject {
        WorldObject {
            guid,
            name: "Portal to Town Network".into(),
            setup_id: PORTAL_SETUP,
            scale: 1.0,
            position: Some(Position::new_flat(0xA9B4_0003, local)),
            ..Default::default()
        }
    }

    #[test]
    fn a_portal_gets_emitters_that_follow_it_and_leave_with_it() {
        let Ok(dir) = std::env::var("AC_DATA_DIR") else {
            return;
        };
        let assets = Assets::open(dir).unwrap();
        let mut world = World::default();
        world
            .objects
            .insert(1, portal(1, Vec3::new(14.4, 55.6, 78.2)));
        // An NPC alongside it: no default script, so no emitters.
        world.objects.insert(
            2,
            WorldObject {
                guid: 2,
                setup_id: 0x0200_0001,
                scale: 1.0,
                position: Some(Position::new_flat(0xA9B4_0003, Vec3::new(20.0, 55.6, 78.2))),
                ..Default::default()
            },
        );

        // `update` takes at most a quarter second at a time, so run it
        // the way the frame loop does.
        let run = |fx: &mut WorldFx, seconds: f32| {
            for _ in 0..(seconds / 0.25).round() as u32 {
                fx.update(&assets, 0.25);
            }
        };

        let mut fx = WorldFx::default();
        fx.sync_objects(&assets, &world);
        assert_eq!(fx.objects.len(), 1, "only the portal has a script");
        run(&mut fx, 1.0);
        let quads = fx.quads();
        assert!(!quads.is_empty(), "the portal draws particles");
        let near = |quads: &[Quad], p: Vec3| quads.iter().all(|q| q.position.distance(p) < 10.0);
        // Both of the portal's emitters name a real image; a white
        // fallback square would mean one of them was not resolved.
        assert!(
            quads
                .iter()
                .all(|q| matches!(q.image, SpriteImage::Surface(_))),
            "{:?}",
            quads.iter().map(|q| q.image).collect::<Vec<_>>()
        );
        let here = lbid::world_origin(0xA9B4_0000) + Vec3::new(14.4, 55.6, 78.2);
        assert!(near(&quads, here), "{:?}", quads[0].position);

        // Unchanged world: the sweep is skipped and the effect stays.
        fx.sync_objects(&assets, &world);
        assert_eq!(fx.objects.len(), 1);

        // Moved: the emitters go along, so the particles born after the
        // move are at the new place (the ones already in the air stay
        // where they were emitted, as they do in the game).
        let moved = Vec3::new(14.4, 55.6, 178.2);
        world.objects.get_mut(&1).unwrap().position = Some(Position::new_flat(0xA9B4_0003, moved));
        world.generation += 1;
        fx.sync_objects(&assets, &world);
        run(&mut fx, 4.0);
        let quads = fx.quads();
        assert!(!quads.is_empty());
        assert!(
            near(&quads, here + Vec3::Z * 100.0),
            "{:?}",
            quads[0].position
        );

        // Gone: so are its emitters.
        world.objects.remove(&1);
        world.generation += 1;
        fx.sync_objects(&assets, &world);
        assert!(fx.objects.is_empty());
        assert!(fx.quads().is_empty());
        assert!(fx.is_empty());
    }
}
