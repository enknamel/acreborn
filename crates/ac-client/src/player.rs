//! Third-person player controller for the connected viewer: moves the
//! character with WASD, follows floors and terrain, steps up and down
//! ledges, falls under gravity, jumps, tracks the cell id, and reports
//! movement to the server.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use ac_formats::landblock::CellLandblock;
use ac_net::messages::{self, action, motion, RawMotion, WirePosition};
use ac_net::session::Session;
use ac_scene::anim::AnimPlayer;
use ac_scene::collision::{Capsule, CollisionWorld, Vertical, GRAVITY};
use ac_scene::nav::{self, Ground, NavGraph};
use ac_scene::scenery::TerrainSampler;
use ac_scene::Assets;
use glam::{Quat, Vec3};

#[derive(Debug, Clone, Copy, Default)]
pub struct Input {
    pub forward: f32,
    pub strafe: f32,
    pub run: bool,
    /// Edge: true on the frame a jump is requested (full power).
    pub jump: bool,
}

/// The launch of a jump, for the Jump game action (0xF61B).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Jump {
    /// Charge, 0..=1.
    pub power: f32,
    /// Launch velocity in the character's local frame (forward = +Y,
    /// up = +Z), as the JumpPack carries it.
    pub velocity: Vec3,
}

/// `MotionCommand::Falling`, the airborne cycle.
const FALLING: u32 = 0x4000_0015;

struct Block {
    lb: CellLandblock,
    /// Static collision geometry, built on first use.
    collision: Option<CollisionWorld>,
    /// Navigation graph over the collision, built chunk by chunk as
    /// paths are planned.
    nav: Option<NavGraph>,
    /// A dungeon block: no terrain to walk on.
    dungeon: bool,
}

/// Landblock id (`xxyy0000`) containing a world position.
fn block_of(w: Vec3) -> u32 {
    (((w.x / 192.0).floor().clamp(0.0, 255.0) as u32) << 24)
        | (((w.y / 192.0).floor().clamp(0.0, 255.0) as u32) << 16)
}

pub struct Player {
    pub cell: u32,
    /// Landblock-local position.
    pub local: Vec3,
    /// Radians, 0 = north (+Y), increasing counter-clockwise (left).
    pub heading: f32,
    /// MotionStance id used for our animations (non-combat, hand combat...).
    pub stance: u32,
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
    /// One-shots (attacks, emotes) playing over the cycle, in order; the
    /// front one drives the pose until it finishes.
    oneshots: VecDeque<AnimPlayer>,
    current_motion: u32,
    pub n_parts: usize,
    capsule: Capsule,
    /// Vertical speed while airborne (m/s, up positive).
    vz: f32,
    airborne: bool,
    /// World-space horizontal velocity while airborne: the client keeps
    /// the take-off velocity, so nothing steers in the air.
    air_velocity: Vec3,
    /// Horizontal velocity of the last grounded frame.
    ground_velocity: Vec3,
    /// Jump skill used for the launch velocity (see `jump`).
    pub jump_skill: u32,
    /// The most recent take-off, left for the main loop to report; take it
    /// with `Option::take`.
    pub last_jump: Option<Jump>,
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
            stance: motion::STANCE_NON_COMBAT,
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
            oneshots: VecDeque::new(),
            current_motion: 0,
            n_parts: 0,
            capsule: Capsule::default(),
            vz: 0.0,
            airborne: false,
            air_velocity: Vec3::ZERO,
            ground_velocity: Vec3::ZERO,
            jump_skill: 100,
            last_jump: None,
        }
    }

    pub fn is_airborne(&self) -> bool {
        self.airborne
    }

    /// Take off: the launch speed follows the client's jump formula
    /// (`GetJumpHeight`, then `v = sqrt(2 g h)`): height =
    /// `burden_mod * (skill / (skill + 1300) * 22.2 + 0.05) * power`, at
    /// least 0.35 m, with no burden here. The current walking velocity is
    /// carried into the air. Returns false if already airborne.
    pub fn jump(&mut self, power: f32) -> bool {
        if self.airborne {
            return false;
        }
        let power = power.clamp(0.0, 1.0);
        let skill = self.jump_skill as f32;
        let height = ((skill / (skill + 1300.0) * 22.2 + 0.05) * power).max(0.35);
        self.vz = (2.0 * GRAVITY * height).sqrt();
        self.airborne = true;
        self.air_velocity = self.ground_velocity;
        let world = self.air_velocity + Vec3::new(0.0, 0.0, self.vz);
        self.last_jump = Some(Jump {
            power,
            velocity: self.rotation().inverse() * world,
        });
        self.moving = true;
        self.dirty = true;
        true
    }

    /// Attach the character's motion table and start idling. Walk and run
    /// speeds come from the table's cycle velocities.
    pub fn set_motion_table(&mut self, assets: &Assets, setup_id: u32, table_id: u32) {
        self.n_parts = assets.setup(setup_id).map(|s| s.parts.len()).unwrap_or(0);
        if let Ok(setup) = assets.setup(setup_id) {
            // Step heights come from the setup (0.6 / 1.5 m for humans).
            if setup.step_up_height > 0.0 {
                self.capsule.step_up = setup.step_up_height;
            }
            if setup.step_down_height > 0.0 {
                self.capsule.step_down = setup.step_down_height;
            }
        }
        if let Ok(t) = ac_scene::anim::motion_table(assets, table_id) {
            if let Some(w) = t.cycle(self.stance, motion::WALK_FORWARD) {
                if w.velocity.length() > 0.1 {
                    self.walk_speed = w.velocity.length();
                }
            }
            if let Some(r) = t.cycle(self.stance, motion::RUN_FORWARD) {
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
        let stance = self.stance;
        self.anim = self
            .table
            .as_ref()
            .and_then(|t| AnimPlayer::cycle(assets, t, stance, m));
    }

    /// Switch stance (combat mode); the current cycle is re-picked.
    pub fn set_stance(&mut self, assets: &Assets, stance: u32) {
        if self.stance != stance {
            self.stance = stance;
            self.anim = None;
            let m = self.current_motion;
            self.current_motion = 0;
            self.set_motion(assets, m);
        }
    }

    /// Play a one-shot motion command (an attack, an emote) once over the
    /// current stance and motion, then return to the cycle. `cmd` may be a
    /// full MotionCommand id or the low 16 bits a MovementEvent carries;
    /// `speed` scales playback (1.0 = as authored). Commands queue up and
    /// play in order. Returns false if the table has no such animation.
    #[allow(dead_code)] // for the main loop to call on the player's queued commands
    pub fn play_command(&mut self, assets: &Assets, cmd: u32, speed: f32) -> bool {
        let Some(t) = self.table.as_ref() else {
            return false;
        };
        let current = if self.current_motion == 0 {
            motion::READY
        } else {
            self.current_motion
        };
        let idle = t.default_motion(self.stance).unwrap_or(motion::READY);
        let link = AnimPlayer::link(assets, t, self.stance, current, cmd)
            .or_else(|| AnimPlayer::link(assets, t, self.stance, idle, cmd));
        match link {
            Some(mut p) => {
                p.speed = speed.abs().max(0.1);
                self.oneshots.push_back(p);
                self.dirty = true;
                true
            }
            None => {
                tracing::debug!(
                    "no animation for command {cmd:#010x} in stance {:#010x}",
                    self.stance
                );
                false
            }
        }
    }

    /// True while a one-shot is playing.
    #[allow(dead_code)]
    pub fn busy(&self) -> bool {
        !self.oneshots.is_empty()
    }

    /// Advance the animation and pick idle/walk/run from the input. A
    /// queued one-shot overrides the pose until it has played through.
    pub fn animate(&mut self, assets: &Assets, input: &Input, dt: f32) -> Option<Vec<glam::Mat4>> {
        let m = if self.is_airborne() {
            FALLING
        } else if input.forward > 0.0 {
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
        // The cycle keeps time underneath so it resumes in phase.
        if let Some(a) = self.anim.as_mut() {
            a.advance(dt);
        }
        while self.oneshots.front().is_some_and(|p| p.finished()) {
            self.oneshots.pop_front();
        }
        if let Some(front) = self.oneshots.front_mut() {
            front.advance(dt);
            return Some(front.part_transforms(self.n_parts));
        }
        let a = self.anim.as_ref()?;
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
            self.blocks.insert(
                block_id,
                Block {
                    lb,
                    collision: None,
                    nav: None,
                    dungeon: false,
                },
            );
        }
        self.blocks.get(&block_id)
    }

    /// Collision world for a landblock, built from the assembled scene on
    /// first use (this loads the block's models once, ~0.5 s).
    fn collision(&mut self, assets: &Assets, block_id: u32) -> Option<&CollisionWorld> {
        let block_id = block_id & 0xFFFF_0000;
        self.block(assets, block_id)?;
        let b = self.blocks.get_mut(&block_id)?;
        if b.collision.is_none() {
            let scene = ac_scene::landblock::load(assets, block_id).ok()?;
            b.collision = CollisionWorld::from_scene(assets, &scene).ok();
            b.dungeon = scene.is_dungeon;
        }
        b.collision.as_ref()
    }

    /// Whether the straight walk from `from` to `to` is blocked: static
    /// geometry of landblock `block` (or of the blocks under either end)
    /// crosses the chest-height line, or the capsule cannot walk it. The
    /// test that decides between walking straight and planning a route.
    pub fn line_blocked(&mut self, assets: &Assets, block: u32, from: Vec3, to: Vec3) -> bool {
        let mut blocks = vec![block & 0xFFFF_0000];
        for b in [block_of(from), block_of(to)] {
            if !blocks.contains(&b) {
                blocks.push(b);
            }
        }
        if blocks.iter().any(|&blk| {
            self.collision(assets, blk)
                .is_some_and(|c| !nav::line_clear(c, from, to))
        }) {
            return true;
        }
        // The ray misses, but the capsule may still not fit (door jambs,
        // benches, a corridor that bends less than a body width).
        self.walkable(assets, block, from, to) == Some(false)
    }

    /// Whether the capsule can walk the straight line from `from` to `to`
    /// over landblock `block`'s geometry (see `nav::Ground::walkable`);
    /// `None` when the block has no collision.
    fn walkable(&mut self, assets: &Assets, block: u32, from: Vec3, to: Vec3) -> Option<bool> {
        let block = block & 0xFFFF_0000;
        self.collision(assets, block)?;
        let cap = self.capsule;
        let height_table = self.height_table.clone();
        let b = self.blocks.get(&block)?;
        let collision = b.collision.as_ref()?;
        let sampler = TerrainSampler::new(&b.lb, &height_table);
        let origin = ac_world::landblock_origin(block);
        let terrain =
            |x: f32, y: f32| sampler.height_at(Vec3::new(x - origin.x, y - origin.y, 0.0));
        let ground = Ground {
            collision,
            terrain: (!b.dungeon).then_some(&terrain),
        };
        Some(ground.walkable(from, to, &cap).0)
    }

    /// A walkable route from `from` to `to` (world positions, both in
    /// landblock `block`) around the block's static geometry: waypoints
    /// ending with `to`, or `None` when the graph does not connect them.
    /// The graph is built around the search on first use and kept with
    /// the block's collision.
    pub fn find_path(
        &mut self,
        assets: &Assets,
        block: u32,
        from: Vec3,
        to: Vec3,
    ) -> Option<Vec<Vec3>> {
        let block = block & 0xFFFF_0000;
        self.collision(assets, block)?;
        let cap = self.capsule;
        let height_table = self.height_table.clone();
        let b = self.blocks.get_mut(&block)?;
        let same_capsule = |n: &NavGraph| {
            let c = n.capsule;
            c.radius == cap.radius
                && c.height == cap.height
                && c.step_up == cap.step_up
                && c.step_down == cap.step_down
        };
        if b.nav.as_ref().is_some_and(|n| !same_capsule(n)) {
            b.nav = None;
        }
        if b.nav.is_none() {
            let scene = ac_scene::landblock::load(assets, block).ok()?;
            b.nav = Some(NavGraph::for_scene(&scene, b.collision.as_ref()?, &cap));
        }
        let collision = b.collision.as_ref()?;
        let sampler = TerrainSampler::new(&b.lb, &height_table);
        let origin = ac_world::landblock_origin(block);
        let terrain =
            |x: f32, y: f32| sampler.height_at(Vec3::new(x - origin.x, y - origin.y, 0.0));
        let ground = Ground {
            collision,
            terrain: (!b.dungeon).then_some(&terrain),
        };
        let nav = b.nav.as_mut()?;
        let (nodes, chunks) = (nav.len(), nav.chunk_count());
        let started = Instant::now();
        let path = nav.find_path(&ground, from, to);
        if nav.chunk_count() != chunks {
            tracing::debug!(
                "nav {block:#010x}: {} nodes in {} chunks (+{} nodes, {} chunks, {:.0} ms this search)",
                nav.len(),
                nav.chunk_count(),
                nav.len() - nodes,
                nav.chunk_count() - chunks,
                started.elapsed().as_secs_f64() * 1e3
            );
        }
        path
    }

    /// Fraction along `from`..`to` where static geometry first blocks the
    /// segment, if it does.
    pub fn first_wall(&mut self, assets: &Assets, from: Vec3, to: Vec3) -> Option<f32> {
        let mut blocks = vec![block_of(from)];
        if block_of(to) != blocks[0] {
            blocks.push(block_of(to));
        }
        let mut best: Option<f32> = None;
        for blk in blocks {
            if let Some(c) = self.collision(assets, blk) {
                if let Some(f) = c.segment_hit(from, to) {
                    if best.map(|b| f < b).unwrap_or(true) {
                        best = Some(f);
                    }
                }
            }
        }
        best
    }

    /// Pull a third-person camera in front of any wall between the
    /// character's head (`from`) and the wanted camera spot (`to`).
    pub fn clamp_camera(&mut self, assets: &Assets, from: Vec3, to: Vec3) -> Vec3 {
        match self.first_wall(assets, from, to) {
            // Stop a little short of the wall so the near plane stays inside.
            Some(f) => from + (to - from) * (f - 0.7 / (to - from).length()).max(0.0),
            None => to,
        }
    }

    /// Terrain height under a world position, if the landblock loads.
    fn terrain_at(&mut self, assets: &Assets, world: Vec3) -> Option<f32> {
        let blk = block_of(world);
        let local = world - ac_world::landblock_origin(blk);
        let height_table = self.height_table.clone();
        self.block(assets, blk)
            .and_then(|b| TerrainSampler::new(&b.lb, &height_table).height_at(local))
    }

    /// Stand at `world` on the floor/terrain described by `floor`
    /// (cell id 0 = outdoors), updating `cell` and `local`.
    fn place(&mut self, world: Vec3, cell: u32) {
        if cell != 0 {
            self.cell = cell;
        } else {
            let blk = block_of(world);
            self.cell = ac_world::outdoor_cell(blk, world - ac_world::landblock_origin(blk));
        }
        self.local = world - ac_world::landblock_origin(self.cell);
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
        let dir = fwd * input.forward + right * input.strafe;
        let steering = dir.length_squared() >= 1e-6;
        if !self.airborne {
            self.ground_velocity = if steering {
                dir.normalize() * speed
            } else {
                Vec3::ZERO
            };
            if input.jump {
                self.jump(1.0);
            }
        }
        if !self.airborne && !steering {
            self.moving = false;
            return false;
        }
        let old = self.world_position();
        let vel = if self.airborne {
            self.air_velocity
        } else {
            self.ground_velocity
        };
        let target = old + vel * dt;
        // Static geometry of the block we're moving into and the one we're
        // leaving, at a boundary.
        let mut blocks = vec![block_of(target)];
        if self.landblock() != blocks[0] {
            blocks.push(self.landblock());
        }
        let cap = self.capsule;
        let indoors = self.is_indoors();
        let mut world = target;
        if !self.airborne {
            // Walking: walls push, ledges up to step_up are climbed, drops
            // up to step_down are walked down, ceilings block.
            let mut floor: Option<(f32, u32)> = None;
            let mut blocked = false;
            for &blk in &blocks {
                if let Some(c) = self.collision(assets, blk) {
                    let from = Vec3::new(world.x, world.y, old.z);
                    let w = c.walk(from, world, &cap);
                    if w.blocked {
                        blocked = true;
                        break;
                    }
                    world.x = w.pos.x;
                    world.y = w.pos.y;
                    if let Some(f) = w.floor {
                        if floor.map(|(z, _)| f.0 > z).unwrap_or(true) {
                            floor = Some(f);
                        }
                    }
                }
            }
            if blocked {
                world = old;
                floor = Some((old.z, self.cell));
                if !indoors {
                    floor = Some((old.z, 0));
                }
            }
            match floor {
                Some((z, cell)) if cell != 0 => {
                    // Standing on an interior cell's floor: that cell owns us.
                    world.z = z;
                    self.place(world, cell);
                }
                Some((z, _)) if !indoors || z > old.z - 0.5 => {
                    // An outdoor floor (dock, bridge, building step): stand on
                    // it if it is at least as high as the terrain.
                    let terrain = self.terrain_at(assets, world);
                    world.z = terrain.map(|t| t.max(z)).unwrap_or(z);
                    self.place(world, 0);
                }
                _ if indoors => {
                    // Nothing within step range in the interior: fall if
                    // there is a floor somewhere below, else stay put (bad
                    // collision data).
                    let deep = blocks
                        .iter()
                        .filter_map(|&blk| {
                            self.collision(assets, blk)
                                .and_then(|c| c.floor_at(world, cap.step_up, 200.0))
                        })
                        .next();
                    if deep.is_some() {
                        self.airborne = true;
                        self.vz = 0.0;
                        self.air_velocity = self.ground_velocity;
                    } else {
                        world.z = old.z;
                    }
                    self.local = world - ac_world::landblock_origin(self.landblock());
                }
                _ => {
                    // Outdoors on bare terrain: follow it, unless it drops
                    // away by more than a step.
                    match self.terrain_at(assets, world) {
                        Some(t) if t < old.z - cap.step_down => {
                            self.airborne = true;
                            self.vz = 0.0;
                            self.air_velocity = self.ground_velocity;
                        }
                        Some(t) => world.z = t,
                        None => world.z = old.z,
                    }
                    self.place(world, 0);
                }
            }
        }
        if self.airborne {
            // In the air: walls push the whole capsule; gravity integrates
            // the vertical speed; land on the first floor, or bump the
            // ceiling.
            for &blk in &blocks {
                if let Some(c) = self.collision(assets, blk) {
                    world = c.resolve(world, cap.radius, cap.height);
                }
            }
            self.vz -= GRAVITY * dt;
            let dz = self.vz * dt;
            let mut landed: Option<(Vec3, u32)> = None;
            let mut ceiling: Option<Vec3> = None;
            for &blk in &blocks {
                if let Some(c) = self.collision(assets, blk) {
                    match c.vertical(world, dz, &cap) {
                        Vertical::Landed(p, cell) => {
                            if landed.map(|(l, _)| p.z > l.z).unwrap_or(true) {
                                landed = Some((p, cell));
                            }
                        }
                        Vertical::Ceiling(p) => {
                            if ceiling.map(|c| p.z < c.z).unwrap_or(true) {
                                ceiling = Some(p);
                            }
                        }
                        Vertical::Free(_) => {}
                    }
                }
            }
            // Terrain catches us outdoors (or when the floor we would land
            // on is outdoor geometry below the ground).
            if dz < 0.0 && (!indoors || landed.map(|(_, c)| c == 0).unwrap_or(false)) {
                if let Some(t) = self.terrain_at(assets, world) {
                    let above_landing = landed.map(|(l, _)| t > l.z).unwrap_or(true);
                    if t >= world.z + dz && above_landing {
                        landed = Some((Vec3::new(world.x, world.y, t), 0));
                    }
                }
            }
            if let Some((p, cell)) = landed {
                world = p;
                self.airborne = false;
                self.vz = 0.0;
                self.place(world, cell);
            } else {
                if let Some(p) = ceiling {
                    world = p;
                    self.vz = 0.0;
                } else {
                    world.z += dz;
                }
                if indoors {
                    self.local = world - ac_world::landblock_origin(self.landblock());
                } else {
                    self.place(world, 0);
                }
            }
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
    /// Report motion to the server. `quiet` suppresses MoveToState while
    /// the server itself is walking us somewhere: ACE cancels its move-to
    /// chain on any MoveToState it receives.
    pub fn report(&mut self, session: &mut Session, input: &Input, now: Instant, quiet: bool) {
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
        if m != self.last_motion && !quiet {
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
