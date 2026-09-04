//! Third-person player controller for the connected viewer: moves the
//! character with WASD, follows terrain outdoors, tracks the cell id, and
//! reports movement to the server.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ac_formats::landblock::{CellLandblock, EnvCell};
use ac_net::messages::{self, action, motion, RawMotion, WirePosition};
use ac_net::session::Session;
use ac_scene::anim::AnimPlayer;
use ac_scene::scenery::TerrainSampler;
use ac_scene::Assets;
use glam::{Quat, Vec3};

pub struct Input {
    pub forward: f32,
    pub strafe: f32,
    pub run: bool,
}

struct Block {
    lb: CellLandblock,
    /// Interior cells: (cell id, landblock-local origin).
    cells: Vec<(u32, Vec3)>,
}

pub struct Player {
    pub cell: u32,
    /// Landblock-local position.
    pub local: Vec3,
    /// Radians, 0 = north (+Y), increasing counter-clockwise (left).
    pub heading: f32,
    walk_speed: f32,
    run_speed: f32,
    blocks: HashMap<u32, Block>,
    height_table: Vec<f32>,
    last_motion: RawMotion,
    last_auto: Instant,
    moving: bool,
    pub dirty: bool,
    table: Option<ac_formats::motion_table::MotionTable>,
    pub anim: Option<AnimPlayer>,
    current_motion: u32,
    pub n_parts: usize,
}

impl Player {
    pub fn new(assets: &Assets, cell: u32, local: Vec3, rotation: Quat) -> Self {
        let fwd = rotation * Vec3::Y;
        let heading = (-fwd.x).atan2(fwd.y);
        let height_table = assets
            .region()
            .map(|r| r.land_defs.land_height_table.clone())
            .unwrap_or_default();
        Player {
            cell,
            local,
            heading,
            walk_speed: 2.5,
            run_speed: 6.0,
            blocks: HashMap::new(),
            height_table,
            last_motion: RawMotion::default(),
            last_auto: Instant::now(),
            moving: false,
            dirty: true,
            table: None,
            anim: None,
            current_motion: 0,
            n_parts: 0,
        }
    }

    /// Attach the character's motion table and start idling. Walk and run
    /// speeds come from the table's cycle velocities.
    pub fn set_motion_table(&mut self, assets: &Assets, setup_id: u32, table_id: u32) {
        self.n_parts = assets.setup(setup_id).map(|s| s.parts.len()).unwrap_or(0);
        if let Ok(t) = ac_scene::anim::motion_table(assets, table_id) {
            if let Some(w) = t.cycle(motion::STANCE_NON_COMBAT, motion::WALK_FORWARD) {
                if w.velocity.length() > 0.1 {
                    self.walk_speed = w.velocity.length();
                }
            }
            if let Some(r) = t.cycle(motion::STANCE_NON_COMBAT, motion::RUN_FORWARD) {
                if r.velocity.length() > 0.1 {
                    self.run_speed = r.velocity.length();
                }
            }
            self.table = Some(t);
        }
        self.set_motion(assets, motion::READY);
    }

    fn set_motion(&mut self, assets: &Assets, m: u32) {
        if m == self.current_motion && self.anim.is_some() {
            return;
        }
        self.current_motion = m;
        self.anim = self
            .table
            .as_ref()
            .and_then(|t| AnimPlayer::cycle(assets, t, motion::STANCE_NON_COMBAT, m));
    }

    /// Advance the animation and pick idle/walk/run from the input.
    pub fn animate(&mut self, assets: &Assets, input: &Input, dt: f32) -> Option<Vec<glam::Mat4>> {
        let m = if input.forward > 0.0 {
            if input.run {
                motion::RUN_FORWARD
            } else {
                motion::WALK_FORWARD
            }
        } else if input.forward < 0.0 {
            motion::WALK_BACKWARDS
        } else if input.strafe > 0.0 {
            motion::SIDE_STEP_RIGHT
        } else if input.strafe < 0.0 {
            motion::SIDE_STEP_LEFT
        } else {
            motion::READY
        };
        self.set_motion(assets, m);
        let a = self.anim.as_mut()?;
        a.advance(dt);
        Some(a.part_transforms(self.n_parts))
    }

    pub fn landblock(&self) -> u32 {
        self.cell & 0xFFFF_0000
    }

    pub fn is_indoors(&self) -> bool {
        (self.cell & 0xFFFF) >= 0x100
    }

    pub fn world_position(&self) -> Vec3 {
        ac_world::landblock_origin(self.cell) + self.local
    }

    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_z(self.heading)
    }

    pub fn forward(&self) -> Vec3 {
        self.rotation() * Vec3::Y
    }

    fn block(&mut self, assets: &Assets, block_id: u32) -> Option<&Block> {
        let block_id = block_id & 0xFFFF_0000;
        if !self.blocks.contains_key(&block_id) {
            let lb_id = block_id | 0xFFFF;
            let lb = CellLandblock::parse(lb_id, &assets.cell.read(lb_id).ok()?).ok()?;
            let mut cells = Vec::new();
            let info_id = block_id | 0xFFFE;
            if let Ok(b) = assets.cell.read(info_id) {
                if let Ok(info) = ac_formats::landblock::LandblockInfo::parse(info_id, &b) {
                    for i in 0..info.num_cells {
                        let cid = block_id | (0x100 + i);
                        if let Ok(cb) = assets.cell.read(cid) {
                            if let Ok(c) = EnvCell::parse(cid, &cb) {
                                cells.push((cid, c.position.origin));
                            }
                        }
                    }
                }
            }
            self.blocks.insert(block_id, Block { lb, cells });
        }
        self.blocks.get(&block_id)
    }

    /// Apply one frame of input. Returns true if the position changed.
    pub fn update(&mut self, assets: &Assets, input: &Input, dt: f32) -> bool {
        let speed = if input.run {
            self.run_speed
        } else {
            self.walk_speed
        };
        let fwd = self.forward();
        let right = fwd.cross(Vec3::Z).normalize_or(Vec3::X);
        let mut delta = fwd * input.forward + right * input.strafe;
        if delta.length_squared() < 1e-6 {
            self.moving = false;
            return false;
        }
        delta = delta.normalize() * speed * dt;
        let mut world = self.world_position() + delta;
        // Crossing a landblock boundary moves us to the neighbour block.
        let bx = (world.x / 192.0).floor().clamp(0.0, 255.0) as u32;
        let by = (world.y / 192.0).floor().clamp(0.0, 255.0) as u32;
        let new_block = (bx << 24) | (by << 16);
        let indoors = self.is_indoors();
        if indoors {
            // Dungeons keep their landblock; pick the nearest cell origin.
            let block_id = self.landblock();
            let local = world - ac_world::landblock_origin(block_id);
            if let Some(b) = self.block(assets, block_id) {
                if let Some((cid, _)) = b.cells.iter().min_by(|a, c| {
                    (a.1 - local)
                        .truncate()
                        .length()
                        .partial_cmp(&(c.1 - local).truncate().length())
                        .unwrap()
                }) {
                    self.cell = *cid;
                }
            }
            self.local = local;
        } else {
            let local = world - ac_world::landblock_origin(new_block);
            let height_table = self.height_table.clone();
            if let Some(b) = self.block(assets, new_block) {
                let sampler = TerrainSampler::new(&b.lb, &height_table);
                if let Some(z) = sampler.height_at(local) {
                    world.z = z;
                }
            }
            self.local = Vec3::new(local.x, local.y, world.z);
            self.cell = ac_world::outdoor_cell(new_block, self.local);
        }
        self.moving = true;
        self.dirty = true;
        true
    }

    pub fn turn(&mut self, d_yaw: f32) {
        self.heading += d_yaw;
        self.dirty = true;
    }

    fn wire(&self) -> WirePosition {
        let q = self.rotation();
        WirePosition {
            cell: self.cell,
            x: self.local.x,
            y: self.local.y,
            z: self.local.z,
            qw: q.w,
            qx: q.x,
            qy: q.y,
            qz: q.z,
        }
    }

    /// Send MoveToState when the input state changes and AutonomousPosition
    /// four times a second while moving.
    pub fn report(&mut self, session: &mut Session, input: &Input, now: Instant) {
        let m = RawMotion {
            running: input.run,
            forward: if input.forward > 0.0 {
                motion::WALK_FORWARD
            } else if input.forward < 0.0 {
                motion::WALK_BACKWARDS
            } else {
                0
            },
            sidestep: if input.strafe > 0.0 {
                motion::SIDE_STEP_RIGHT
            } else if input.strafe < 0.0 {
                motion::SIDE_STEP_LEFT
            } else {
                0
            },
            turn: 0,
        };
        if m != self.last_motion {
            tracing::debug!(
                "-> MoveToState {:?} at {:#010x} {:?}",
                m,
                self.cell,
                self.local
            );
            session.send_action(
                action::MOVE_TO_STATE,
                &messages::move_to_state(&m, &self.wire(), 1, true),
            );
            self.last_motion = m;
            self.last_auto = now;
        } else if self.moving && now - self.last_auto >= Duration::from_millis(250) {
            tracing::debug!("-> AutonomousPosition {:#010x} {:?}", self.cell, self.local);
            session.send_action(
                action::AUTONOMOUS_POSITION,
                &messages::autonomous_position(&self.wire(), 1, true),
            );
            self.last_auto = now;
        }
    }
}
