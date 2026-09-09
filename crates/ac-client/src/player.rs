//! Third-person player controller for the connected viewer: moves the
//! character with WASD, follows floors and terrain, steps up and down
//! ledges, falls under gravity, jumps, tracks the cell id, and reports
//! movement to the server.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use ac_formats::landblock::CellLandblock;
use ac_net::messages::{self, action, motion, RawMotion, WirePosition};
use ac_net::session::Session;
use ac_scene::anim::AnimPlayer;
use ac_scene::blockcache::BlockCollision;
use ac_scene::collision::{Capsule, Vertical, GRAVITY};
use ac_scene::nav::{self, Ground, NavGraph};
use ac_scene::scenery::TerrainSampler;
use ac_scene::Assets;
use glam::{Quat, Vec3};

#[derive(Debug, Clone, Copy, Default)]
pub struct Input {
    pub forward: f32,
    pub strafe: f32,
    pub run: bool,
    /// Edge: true on the frame a full-power jump is requested (bots,
    /// `--jump`).
    pub jump: bool,
    /// The jump key is down: the jump charges while it is held and
    /// leaps when it is released (see `JUMP_CHARGE_SECS`).
    pub jump_held: bool,
    /// Up (+1) or down (-1) while flying (`Player::noclip`); nothing on
    /// the ground.
    pub climb: f32,
}

/// Holding the jump key this long gives a full-power jump.
pub const JUMP_CHARGE_SECS: f32 = 1.0;

/// The power a jump may have with `stamina` left and no burden (ACE
/// `MovementSystem.GetJumpPower`, the inverse of the stamina cost
/// `ceil((burden + 0.5) * power * 8 + 2)`): below 2 stamina, no jump.
pub fn max_jump_power(stamina: u32, burden: f32) -> f32 {
    ((stamina as f32 - 2.0) / (burden * 8.0 + 4.0)).clamp(0.0, 1.0)
}

/// The stamina a jump of `power` costs (ACE `JumpStaminaCost`).
pub fn jump_stamina_cost(power: f32, burden: f32) -> u32 {
    ((burden + 0.5) * power.clamp(0.0, 1.0) * 8.0 + 2.0).ceil() as u32
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

/// A landblock the character stands in or near. The collision and the
/// graph are the process's (`Assets::block_collision`), shared with
/// every other character there; this only keeps the `Rc`s so that the
/// walk does not look them up every frame.
struct Block {
    lb: CellLandblock,
    /// Static collision geometry, fetched from the shared cache on
    /// first use.
    collision: Option<Rc<BlockCollision>>,
    /// Navigation graph over the collision for our capsule, built chunk
    /// by chunk as paths are planned (by us or anyone else sharing it).
    nav: Option<Rc<RefCell<NavGraph>>>,
    /// A dungeon block: no terrain to walk on.
    dungeon: bool,
}

/// Blocks further than this many landblocks from the character (in
/// either axis) are let go of: they are still in the process cache for
/// whoever is there, and come back at the cost of one lookup.
const KEEP_BLOCKS_WITHIN: u32 = 1;

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
    /// Terrain types that are open sea, read from the region on first
    /// use (see `sea_types`).
    sea_types: Option<Vec<bool>>,
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
    /// One-shot commands (emotes) to send with the next movement state.
    pending_commands: Vec<u32>,
    /// Seconds the jump key has been held on the ground, while charging.
    jump_charge: Option<f32>,
    /// The most power the character can put into a jump right now
    /// (stamina); the main loop keeps it current.
    pub max_jump_power: f32,
    /// How much faster than the run animation's own pace the Run skill
    /// lets this character run (the server's run rate, see [`run_rate`]).
    pub run_rate: f32,
    /// A client-side multiplier on top of that, for whoever wants to get
    /// about faster than the game meant. The server does not hold a
    /// character to its run rate; see [`run_rate`] for what it does hold
    /// them to.
    pub speed_boost: f32,
    /// The height (metres) of a fully charged jump, whatever the Jump
    /// skill says -- the skill's own height still wins when it is more.
    /// Zero leaves it to the skill. Capped at [`MAX_JUMP_HEIGHT`].
    pub jump_height: f32,
    /// The jump charge was set by hand (the wheel) and no longer grows
    /// while the key is held; released when the key is.
    charge_pinned: bool,
    /// Flying: walls, floors and gravity are ignored and `Input::climb`
    /// moves the character up and down. See [`Player::set_noclip`] for
    /// what the server makes of it.
    pub noclip: bool,
    /// What the movement rules allow here (see [`MovementRules`]).
    /// Server-safe until something says otherwise, so a character that
    /// is never told stays inside the rules.
    limits: MovementLimits,
    /// Flying was asked for and the rules refused it; for the Options
    /// panel to say so. Cleared when the rules change or flying is
    /// allowed again.
    noclip_refused: bool,
}

/// How fast a flying character climbs, as a fraction of its run speed.
const CLIMB_RATE: f32 = 0.6;

/// The most a jump may rise (metres). The server calls a character
/// found more than 10 m above the ground it last stood on, a second or
/// more after a jump, a z-position hack (unless its Jump skill is 1000)
/// and puts it back where it was; this stays under that.
pub const MAX_JUMP_HEIGHT: f32 = 9.5;

/// The server's run rate for a Run skill (ACE `MovementSystem.GetRunRate`
/// with no burden): 1 at nothing, about 2.4 at 200, and 4.5 from 800 up.
/// It multiplies the run animation's pace. The server never refuses a
/// move for speed alone -- it drops a position more than 50 m from the
/// last it accepted only when that is also more than a landblock away
/// -- so the practical ceiling is what looks right to everyone else,
/// whose clients run this character at this rate.
pub fn run_rate(run_skill: u32) -> f32 {
    if run_skill >= 800 {
        return 18.0 / 4.0;
    }
    let s = run_skill as f32;
    (s / (s + 200.0) * 11.0 + 4.0) / 4.0
}

/// The most the client will multiply the run speed by, whatever is
/// asked for (the Options slider and `speed_boost` in scripts stop
/// here).
pub const MAX_SPEED_BOOST: f32 = 4.0;

/// Which movement rules to hold the character to.
///
/// The client can run faster than the Run skill, jump higher than the
/// Jump skill and fly through walls. A server that checks what it is
/// told does not follow: it runs its own physics on every position the
/// client reports, holds the character at the wall it was flown into,
/// and force-corrects a character found more than 10 m above the ground
/// it last stood on. The character then looks fine on this screen while
/// the server has it somewhere else, and casting, looting and every
/// other range check goes by the server's idea, not ours.
///
/// Whether to use them is the player's call, wherever they are playing:
/// the run multiplier, the jump height and flying are their own settings
/// and apply on any server. [`MovementRules::ServerSafe`] is there for
/// anyone who would rather be held to the game's own rules; nothing turns
/// it on by itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MovementRules {
    /// The player's own run, jump and flying settings apply, wherever
    /// they are connected. A server that checks the moves it is told
    /// about may refuse them; that is the player's to weigh.
    #[default]
    Unrestricted,
    /// Stay inside what a rule-enforcing server accepts: the game's own
    /// run speed and jump height, and no flying.
    ServerSafe,
}

impl MovementRules {
    /// The settings, in the order the Options panel lists them.
    pub const ALL: [MovementRules; 2] = [MovementRules::Unrestricted, MovementRules::ServerSafe];

    /// Short name for the Options panel.
    pub fn label(self) -> &'static str {
        match self {
            MovementRules::Unrestricted => "My settings",
            MovementRules::ServerSafe => "The game's rules",
        }
    }

    /// One line saying what it does, for the Options panel.
    pub fn help(self) -> &'static str {
        match self {
            MovementRules::Unrestricted => {
                "Your run speed, jump height and flying apply wherever you play"
            }
            MovementRules::ServerSafe => {
                "Hold me to the game's own run speed and jump height, and no flying"
            }
        }
    }

    /// What this setting allows.
    pub fn limits(self) -> MovementLimits {
        match self {
            MovementRules::Unrestricted => MovementLimits::UNRESTRICTED,
            MovementRules::ServerSafe => MovementLimits::SERVER_SAFE,
        }
    }
}

/// What the movement code may do, from the player's [`MovementRules`]
/// setting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovementLimits {
    /// The most the run speed may be multiplied by on top of the Run
    /// skill's own rate.
    pub max_speed_boost: f32,
    /// The most a full jump may be raised to beyond the Jump skill's
    /// own height (0 = the skill's own height, nothing added).
    pub max_jump_height: f32,
    /// Flying (no-clip) is allowed.
    pub noclip: bool,
}

impl MovementLimits {
    /// What a server that checks the moves it is told about accepts:
    /// the Run skill's own pace, the Jump skill's own height, no
    /// flying. Nothing here needs the server to take our word for
    /// anything its own physics would not have done itself.
    pub const SERVER_SAFE: MovementLimits = MovementLimits {
        max_speed_boost: 1.0,
        max_jump_height: 0.0,
        noclip: false,
    };

    /// Everything the client can do.
    pub const UNRESTRICTED: MovementLimits = MovementLimits {
        max_speed_boost: MAX_SPEED_BOOST,
        max_jump_height: MAX_JUMP_HEIGHT,
        noclip: true,
    };

    /// The run multiplier actually used for a wanted one. Slower than
    /// the game allows is always fine; faster is not.
    pub fn clamp_speed_boost(&self, wanted: f32) -> f32 {
        wanted.clamp(0.0, self.max_speed_boost)
    }

    /// The extra jump height actually used for a wanted one (0 leaves
    /// the height to the Jump skill).
    pub fn clamp_jump_height(&self, wanted: f32) -> f32 {
        wanted.clamp(0.0, self.max_jump_height)
    }

    /// These are the server-safe limits.
    pub fn is_server_safe(&self) -> bool {
        *self == MovementLimits::SERVER_SAFE
    }
}

impl Default for MovementLimits {
    /// Safe until told otherwise: a character that never hears which
    /// server it is on stays inside the rules.
    fn default() -> Self {
        MovementLimits::SERVER_SAFE
    }
}

/// A jump worked out before it is made: where the character would fly
/// and where it would come down.
#[derive(Debug, Clone)]
pub struct JumpPreview {
    /// The power previewed (0..=1).
    pub power: f32,
    /// Feet positions along the flight, world space, a frame apart.
    pub path: Vec<Vec3>,
    /// Where the character comes to rest.
    pub landing: Vec3,
    /// False when the flight was cut off still in the air (a very long
    /// fall): `landing` is then the last point followed.
    pub landed: bool,
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
            sea_types: None,
            vz: 0.0,
            airborne: false,
            air_velocity: Vec3::ZERO,
            ground_velocity: Vec3::ZERO,
            jump_skill: 100,
            run_rate: 1.0,
            speed_boost: 1.0,
            jump_height: 0.0,
            charge_pinned: false,
            noclip: false,
            limits: MovementLimits::default(),
            noclip_refused: false,
            last_jump: None,
            pending_commands: Vec::new(),
            jump_charge: None,
            max_jump_power: 1.0,
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
        self.jump_charge = None;
        if self.max_jump_power <= 0.0 {
            return false;
        }
        let power = power.clamp(0.0, self.max_jump_power);
        let skill = self.jump_skill as f32;
        let by_skill = ((skill / (skill + 1300.0) * 22.2 + 0.05) * power).max(0.35);
        let wanted = self.limits.clamp_jump_height(self.jump_height);
        let height = by_skill.max(wanted * power).min(MAX_JUMP_HEIGHT);
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

    /// Send a one-shot command (an emote) to the server with the next
    /// movement state; it relays it to everyone in view.
    pub fn queue_command(&mut self, cmd: u32) {
        self.pending_commands.push(cmd);
    }

    /// Fly, or stop flying. Flying ignores walls, floors and gravity;
    /// the server does not mind, since it takes the position the client
    /// reports (it runs its own collision only to notice what was
    /// touched, portals included) and counts the client as standing on
    /// the ground wherever it says it is. It refuses only a move that is
    /// both 50 m from the last and more than a landblock away, and
    /// crossing between two dungeons or two buildings' interiors in
    /// different landblocks. Stopping mid-air drops the character onto
    /// whatever is below.
    ///
    /// That is a server that takes the client's word for it. One that
    /// runs its own physics keeps the character at the wall it was
    /// flown into, so flying is only for a server that allows it.
    /// Flying is refused when the movement rules do not allow it
    /// ([`MovementLimits::noclip`]): the server would keep the
    /// character where its own physics put it. Returns false then, and
    /// true whenever the character ends up as asked.
    pub fn set_noclip(&mut self, on: bool) -> bool {
        if on && !self.limits.noclip {
            // A follower whose leader is flying asks every tick; say it
            // once until the rules or the answer change.
            if !self.noclip_refused {
                tracing::warn!(
                    "flying refused: server-safe movement is on (Options, \"movement rules\")"
                );
                self.noclip_refused = true;
            }
            return false;
        }
        self.noclip_refused = false;
        if self.noclip == on {
            return true;
        }
        self.noclip = on;
        self.jump_charge = None;
        self.charge_pinned = false;
        if on {
            self.airborne = false;
            self.vz = 0.0;
        } else {
            // Let go: gravity finds the ground (or the floor) below.
            self.airborne = true;
            self.vz = 0.0;
            self.air_velocity = Vec3::ZERO;
        }
        self.moving = true;
        self.dirty = true;
        true
    }

    /// What the movement rules allow this character right now.
    pub fn limits(&self) -> MovementLimits {
        self.limits
    }

    /// Flying was asked for since the rules last changed and refused
    /// (see [`Player::set_noclip`]).
    pub fn noclip_refused(&self) -> bool {
        self.noclip_refused
    }

    /// Hold the character to these limits. Taking flying away while the
    /// character is flying drops it onto whatever is below.
    pub fn set_limits(&mut self, limits: MovementLimits) {
        if self.limits == limits {
            return;
        }
        let was_flying = self.noclip;
        self.limits = limits;
        self.noclip_refused = false;
        if was_flying && !limits.noclip {
            tracing::info!("movement rules: flying off, this server's rules are enforced");
            self.set_noclip(false);
        }
    }

    /// The run multiplier actually used, after the movement rules
    /// ([`Player::speed_boost`] is what was asked for).
    pub fn effective_speed_boost(&self) -> f32 {
        self.limits.clamp_speed_boost(self.speed_boost)
    }

    /// The extra full-jump height actually used, after the movement
    /// rules (0 = the Jump skill's own height).
    pub fn effective_jump_height(&self) -> f32 {
        self.limits.clamp_jump_height(self.jump_height)
    }

    /// One frame of flight: straight to where the input points, then
    /// the cell worked out from what is underfoot (a building's floor a
    /// little way below claims the character; otherwise the open air of
    /// the landblock, or the cell it was already in inside a dungeon).
    fn fly(&mut self, assets: &Assets, input: &Input, dir: Vec3, speed: f32, dt: f32) -> bool {
        let horizontal = if dir.length_squared() >= 1e-6 {
            dir.normalize() * speed
        } else {
            Vec3::ZERO
        };
        let vel = horizontal + Vec3::Z * input.climb.clamp(-1.0, 1.0) * speed * CLIMB_RATE;
        self.ground_velocity = horizontal;
        self.air_velocity = Vec3::ZERO;
        self.vz = 0.0;
        self.airborne = false;
        if vel.length_squared() < 1e-6 {
            self.moving = false;
            return false;
        }
        let old = self.world_position();
        let world = old + vel * dt;
        let blk = block_of(world);
        let dungeon = self.is_indoors() && self.in_dungeon(assets);
        let cap = self.capsule;
        let floor = self
            .collision(assets, blk)
            .and_then(|c| c.floor_at(world + Vec3::Z * 0.2, 0.2, 3.0));
        match floor {
            Some((_, cell)) if cell != 0 => {
                self.cell = cell;
                self.local = world - ac_world::landblock_origin(cell);
            }
            _ if dungeon => {
                // A dungeon has no outside: keep the cell we have.
                let cell = self.cell;
                self.local = world - ac_world::landblock_origin(cell);
            }
            _ => {
                let _ = cap;
                self.place(world, 0);
            }
        }
        self.moving = true;
        self.dirty = true;
        true
    }

    /// The jump charge as power 0..=1 while the key is held, else None.
    pub fn jump_charge(&self) -> Option<f32> {
        self.jump_charge
            .map(|c| (c / JUMP_CHARGE_SECS).min(self.max_jump_power))
    }

    /// Nudge the charge by `delta` (a fraction of full power) while the
    /// key is held, to place the landing spot by hand. From then until
    /// the key is released the charge stays where it is put.
    pub fn adjust_charge(&mut self, delta: f32) {
        let Some(c) = self.jump_charge else {
            return;
        };
        let power = (c / JUMP_CHARGE_SECS + delta).clamp(0.05, 1.0);
        self.jump_charge = Some(power * JUMP_CHARGE_SECS);
        self.charge_pinned = true;
    }

    /// Where a jump at `power` from where the character stands, facing
    /// the way it faces, would go: the flight is run through the same
    /// physics as a real one, on a copy of the character's motion state,
    /// and the character is put back as it was. `None` when a jump is
    /// not possible now (in the air, or out of stamina).
    pub fn preview_jump(&mut self, assets: &Assets, power: f32) -> Option<JumpPreview> {
        if self.airborne || self.max_jump_power <= 0.0 {
            return None;
        }
        let saved = (
            self.cell,
            self.local,
            self.vz,
            self.airborne,
            self.air_velocity,
            self.ground_velocity,
            self.moving,
            self.dirty,
            self.last_jump,
            self.jump_charge,
            self.charge_pinned,
        );
        let power = power.clamp(0.0, 1.0);
        let mut path = Vec::new();
        let mut landed = false;
        if self.jump(power) {
            // Frames of a thirtieth: fine enough for a smooth arc, and a
            // ten-second cap so a fall off the world ends.
            let dt = 1.0 / 30.0;
            let still = Input::default();
            for _ in 0..300 {
                self.update(assets, &still, dt);
                path.push(self.world_position());
                if !self.airborne {
                    landed = true;
                    break;
                }
            }
        }
        (
            self.cell,
            self.local,
            self.vz,
            self.airborne,
            self.air_velocity,
            self.ground_velocity,
            self.moving,
            self.dirty,
            self.last_jump,
            self.jump_charge,
            self.charge_pinned,
        ) = saved;
        let landing = path.last().copied()?;
        Some(JumpPreview {
            power,
            path,
            landing,
            landed,
        })
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
            self.forget_far_blocks();
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

    /// Drop the blocks we have walked away from (see
    /// [`KEEP_BLOCKS_WITHIN`]). The block we stand in and the one under
    /// our world position (a dungeon's cells lie away from its block)
    /// stay, with their neighbours.
    fn forget_far_blocks(&mut self) {
        let here = [self.landblock(), block_of(self.world_position())];
        let near = |a: u32, b: u32| {
            let (ax, ay) = (a >> 24, (a >> 16) & 0xFF);
            let (bx, by) = (b >> 24, (b >> 16) & 0xFF);
            ax.abs_diff(bx) <= KEEP_BLOCKS_WITHIN && ay.abs_diff(by) <= KEEP_BLOCKS_WITHIN
        };
        self.blocks
            .retain(|&blk, _| here.iter().any(|&h| near(h, blk)));
    }

    /// How many landblocks the character holds on to.
    pub fn blocks_held(&self) -> usize {
        self.blocks.len()
    }

    /// Collision world for a landblock, from the process's shared cache
    /// (built from the assembled scene on first use, ~0.5 s once).
    /// Whether the block we stand in is a dungeon: there is no terrain
    /// under it to fall onto, and no cell-0 floor can take us outside.
    fn in_dungeon(&mut self, assets: &Assets) -> bool {
        let blk = self.landblock();
        self.collision(assets, blk);
        self.blocks.get(&blk).map(|b| b.dungeon).unwrap_or(false)
    }

    /// The height of the ground under a world `(x, y)`: the interior
    /// floor or the terrain, whichever a character walking there would
    /// stand on. Used to lay a drawn route along the ground.
    pub fn ground_height(&mut self, assets: &Assets, x: f32, y: f32, near_z: f32) -> Option<f32> {
        let block = block_of(Vec3::new(x, y, 0.0));
        self.collision(assets, block)?;
        let cap = self.capsule;
        let height_table = self.height_table.clone();
        let b = self.blocks.get(&block)?;
        let collision = &b.collision.as_ref()?.world;
        let sampler = TerrainSampler::new(&b.lb, &height_table);
        let origin = ac_world::landblock_origin(block);
        let terrain =
            |x: f32, y: f32| sampler.height_at(Vec3::new(x - origin.x, y - origin.y, 0.0));
        let ground = Ground {
            collision,
            terrain: (!b.dungeon).then_some(&terrain),
            sea: None,
            no_go: None,
            outdoors_only: false,
        };
        // Look from a little above the height we expect, so a floor
        // overhead is not mistaken for the one we are on.
        let from = Vec3::new(x, y, near_z + cap.step_up);
        ground
            .surface_at(from, &cap)
            .or_else(|| collision.floor_at(from, cap.step_up, 30.0))
            .map(|(z, _)| z)
    }

    /// Which of the region's terrain types are open sea, read once. A
    /// character cannot walk into the ocean: the server refuses the
    /// move and it stops dead against nothing, so no route may cross
    /// one.
    fn sea_types(&mut self, assets: &Assets) -> &[bool] {
        if self.sea_types.is_none() {
            self.sea_types = Some(match assets.region() {
                Ok(r) => (0..32u16)
                    .map(|t| {
                        ac_scene::worldroute::water_kind(&r, t) == ac_scene::worldroute::Water::Sea
                    })
                    .collect(),
                Err(_) => Vec::new(),
            });
        }
        self.sea_types.as_deref().unwrap_or(&[])
    }

    /// The shape the character walks as, for planners that work on a
    /// copy of the world.
    pub fn capsule(&self) -> Capsule {
        self.capsule
    }

    fn collision(&mut self, assets: &Assets, block_id: u32) -> Option<Rc<BlockCollision>> {
        let block_id = block_id & 0xFFFF_0000;
        self.block(assets, block_id)?;
        let b = self.blocks.get_mut(&block_id)?;
        if b.collision.is_none() {
            let c = assets.block_collision(block_id).ok()?;
            b.dungeon = c.dungeon;
            b.collision = Some(c);
        }
        b.collision.clone()
    }

    /// Whether a ray from our eyes at `from` (feet) to the chest of
    /// something standing at `to` (feet) crosses static geometry: what a
    /// spell or an arrow would hit before its target. Only the ray, no
    /// capsule: a creature seen over a fence can be shot over it.
    pub fn sees(&mut self, assets: &Assets, from: Vec3, to: Vec3) -> bool {
        let eyes = from + Vec3::new(0.0, 0.0, 1.4);
        let chest = to + Vec3::new(0.0, 0.0, 1.0);
        let mut blocks = vec![self.landblock()];
        for b in [block_of(from), block_of(to)] {
            if !blocks.contains(&b) {
                blocks.push(b);
            }
        }
        !blocks.iter().any(|&blk| {
            self.collision(assets, blk)
                .is_some_and(|c| c.world.segment_hit(eyes, chest).is_some())
        })
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
                .is_some_and(|c| !nav::line_clear(&c.world, from, to))
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
        let sea_types = self.sea_types(assets).to_vec();
        let b = self.blocks.get(&block)?;
        let collision = &b.collision.as_ref()?.world;
        let sampler = TerrainSampler::new(&b.lb, &height_table);
        let origin = ac_world::landblock_origin(block);
        let terrain =
            |x: f32, y: f32| sampler.height_at(Vec3::new(x - origin.x, y - origin.y, 0.0));
        let sea = |x: f32, y: f32| sea_at(&b.lb, &sea_types, x - origin.x, y - origin.y);
        let ground = Ground {
            collision,
            terrain: (!b.dungeon).then_some(&terrain),
            sea: (!b.dungeon).then_some(&sea),
            no_go: None,
            outdoors_only: false,
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
        let sea_types = self.sea_types(assets).to_vec();
        let b = self.blocks.get_mut(&block)?;
        if b.nav
            .as_ref()
            .is_some_and(|n| !n.borrow().capsule.same(&cap))
        {
            b.nav = None;
        }
        if b.nav.is_none() {
            b.nav = Some(b.collision.as_ref()?.nav(assets, &cap).ok()?);
        }
        let collision = &b.collision.as_ref()?.world;
        let sampler = TerrainSampler::new(&b.lb, &height_table);
        let origin = ac_world::landblock_origin(block);
        let terrain =
            |x: f32, y: f32| sampler.height_at(Vec3::new(x - origin.x, y - origin.y, 0.0));
        let sea = |x: f32, y: f32| sea_at(&b.lb, &sea_types, x - origin.x, y - origin.y);
        // A berth from the portals we are not walking to.
        let avoid = crate::pathfinder::portal_mouths_to_avoid(from, to);
        let no_go = |x: f32, y: f32| {
            let here = glam::Vec2::new(x, y);
            avoid
                .iter()
                .any(|a| a.distance(here) < crate::pathfinder::PORTAL_BERTH)
        };
        let ground = Ground {
            collision,
            terrain: (!b.dungeon).then_some(&terrain),
            sea: (!b.dungeon).then_some(&sea),
            no_go: (!avoid.is_empty()).then_some(&no_go),
            outdoors_only: false,
        };
        let mut nav = b.nav.as_ref()?.borrow_mut();
        let (nodes, chunks) = (nav.len(), nav.chunk_count());
        let started = Instant::now();
        // A goal from the overland grid may float a storey off the
        // hillside; the graph only finds nodes near the height asked.
        let to = match (b.dungeon, terrain(to.x, to.y)) {
            (false, Some(z)) if (z - to.z).abs() > 2.0 => Vec3::new(to.x, to.y, z),
            _ => to,
        };
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
            self.run_speed * self.run_rate * self.limits.clamp_speed_boost(self.speed_boost)
        } else {
            self.walk_speed
        };
        let fwd = self.forward();
        let right = fwd.cross(Vec3::Z).normalize_or(Vec3::X);
        let dir = fwd * input.forward + right * input.strafe;
        let steering = dir.length_squared() >= 1e-6;
        if self.noclip {
            return self.fly(assets, input, dir, speed, dt);
        }
        if !self.airborne {
            self.ground_velocity = if steering {
                dir.normalize() * speed
            } else {
                Vec3::ZERO
            };
            if input.jump {
                self.jump(1.0);
            } else if input.jump_held {
                // Charging: the longer the key is held, the higher the
                // leap -- unless the charge was set by hand, which holds.
                if !self.charge_pinned {
                    let c = self.jump_charge.unwrap_or(0.0) + dt;
                    self.jump_charge = Some(c.min(JUMP_CHARGE_SECS));
                }
            } else if let Some(c) = self.jump_charge.take() {
                self.charge_pinned = false;
                self.jump(c / JUMP_CHARGE_SECS);
            }
        } else {
            self.jump_charge = None;
        }
        if !self.airborne && !steering {
            self.moving = false;
            return false;
        }
        let old = self.world_position();
        tracing::trace!(
            "update: airborne {} cell {:#010x} at {:?} heading {:.2} vel {:?}",
            self.airborne,
            self.cell,
            self.local,
            self.heading,
            if self.airborne {
                self.air_velocity
            } else {
                self.ground_velocity
            }
        );
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
        let dungeon = indoors && self.in_dungeon(assets);
        let mut world = target;
        if !self.airborne {
            // Walking: walls push, ledges up to step_up are climbed, drops
            // up to step_down are walked down, ceilings block.
            let mut floor: Option<(f32, u32)> = None;
            let mut blocked = false;
            for &blk in &blocks {
                if let Some(c) = self.collision(assets, blk) {
                    // Where the step began: walls we start in front of
                    // hold us, ledges are measured from our feet.
                    let w = c.walk(old, world, &cap);
                    tracing::trace!(
                        "walk {blk:#010x}: {:?} -> {:?} gives {:?}, floor {:?}, blocked {}",
                        old - ac_world::landblock_origin(blk),
                        world - ac_world::landblock_origin(blk),
                        w.pos - ac_world::landblock_origin(blk),
                        w.floor,
                        w.blocked
                    );
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
            // An interior floor under the ground we walk on -- a cellar,
            // a dungeon beneath a hill -- is not ours to stand on from
            // outside; the hill is. Without this a character walking up a
            // hillside over the Halls of Metos dropped onto the halls'
            // floor and carried on under the mountain.
            if let Some((z, cell)) = floor {
                if cell != 0 && !indoors {
                    if let Some(t) = self.terrain_at(assets, world) {
                        if t > z + 0.5 {
                            floor = Some((t, 0));
                        }
                    }
                }
            }
            match floor {
                Some((z, cell)) if cell != 0 => {
                    // Standing on an interior cell's floor: that cell owns us.
                    world.z = z;
                    self.place(world, cell);
                }
                Some((z, _)) if dungeon => {
                    // Untagged geometry inside a dungeon (a placed object
                    // without a cell): stand on it but stay in our cell; a
                    // dungeon has no outside to walk into.
                    world.z = z;
                    let cell = self.cell;
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
                    // Nothing within step range in the interior. Where
                    // the ground outside is a step away, this is the way
                    // out of a building whose door opens onto bare
                    // terrain (a villa's grounds): go outdoors. Without
                    // this the character kept the cell and walked a
                    // kilometre "inside" it, and the server, sent a cell
                    // and a position that no longer matched, refused
                    // every move from then on.
                    let ground = if dungeon {
                        None
                    } else {
                        self.terrain_at(assets, world)
                            .filter(|&t| t <= old.z + cap.step_up && t >= old.z - cap.step_down)
                    };
                    if let Some(t) = ground {
                        world.z = t;
                        self.place(world, 0);
                    } else {
                        // Fall if there is a floor somewhere below, else
                        // stay put (bad collision data).
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
                    world = c.resolve_from(Some(old), world, cap.radius, cap.height, 0.0);
                }
            }
            // The capsule must fit where it is drifting to: no sliding
            // under a porch slab or a staircase while falling past it.
            let fits = blocks.iter().all(|&blk| {
                self.collision(assets, blk)
                    .and_then(|c| c.ceiling_at(world, cap.radius))
                    .is_none_or(|cz| cz - world.z >= cap.height)
            });
            if !fits {
                world.x = old.x;
                world.y = old.y;
                self.air_velocity = Vec3::ZERO;
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
            // Terrain is the last resort outside a dungeon: it catches us
            // when we are not in an interior cell, when the floor we would
            // land on is outdoor geometry, and when there is no floor at
            // all. That last case is a character who walked out of a
            // building (still carrying its cell id) and jumped: with only
            // the first two rules nothing caught them and they fell
            // through the ground for ever. A cellar's own floor still wins,
            // so being under the terrain indoors is unaffected.
            if dz < 0.0 && !dungeon && (!indoors || landed.is_none_or(|(_, c)| c == 0)) {
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
                // Landing on untagged geometry in a dungeon keeps our cell.
                let cell = if cell == 0 && dungeon {
                    self.cell
                } else {
                    cell
                };
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
            commands: std::mem::take(&mut self.pending_commands),
        };
        if (m != self.last_motion || !m.commands.is_empty()) && !quiet {
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
            self.last_motion = RawMotion {
                commands: Vec::new(),
                ..m
            };
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

/// Whether every corner of the terrain cell holding the landblock-local
/// `(x, y)` is a sea type. The block keeps a terrain code per lattice
/// vertex, nine to a side, so a cell's four corners are the four
/// vertices around it. A cell with a corner ashore is the beach, which
/// can be waded, so only water all the way round counts.
fn sea_at(lb: &CellLandblock, sea_types: &[bool], x: f32, y: f32) -> bool {
    if sea_types.is_empty() {
        return false;
    }
    let last = (ac_scene::CELLS_PER_BLOCK - 1) as f32;
    let cx = (x / ac_scene::CELL_SIZE).floor().clamp(0.0, last) as usize;
    let cy = (y / ac_scene::CELL_SIZE).floor().clamp(0.0, last) as usize;
    let n = ac_scene::VERTS_PER_SIDE;
    [(cx, cy), (cx + 1, cy), (cx, cy + 1), (cx + 1, cy + 1)]
        .into_iter()
        .all(|(vx, vy)| {
            lb.terrain
                .get(vx * n + vy)
                .map(|&t| ac_formats::landblock::terrain::terrain_type(t))
                .and_then(|t| sea_types.get(t as usize))
                .copied()
                .unwrap_or(false)
        })
}

#[cfg(test)]
mod jump_tests {
    use super::*;

    #[test]
    fn stamina_bounds_the_jump() {
        assert_eq!(jump_stamina_cost(1.0, 0.0), 6);
        assert_eq!(jump_stamina_cost(0.0, 0.0), 2);
        assert_eq!(jump_stamina_cost(1.0, 1.0), 14);
        assert!((max_jump_power(6, 0.0) - 1.0).abs() < 1e-6);
        assert!((max_jump_power(4, 0.0) - 0.5).abs() < 1e-6);
        assert_eq!(max_jump_power(1, 0.0), 0.0);
        assert_eq!(max_jump_power(100, 0.0), 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_rate_follows_the_run_skill() {
        assert!((run_rate(0) - 1.0).abs() < 1e-6);
        assert!((run_rate(200) - 2.375).abs() < 1e-6);
        assert!((run_rate(800) - 4.5).abs() < 1e-6);
        assert!((run_rate(2000) - 4.5).abs() < 1e-6);
        assert!(run_rate(100) > run_rate(50));
    }

    #[test]
    fn the_players_settings_apply_wherever_they_play() {
        // Nothing is decided by where we connected: the default lets the
        // player's own run, jump and flying settings through everywhere.
        assert_eq!(MovementRules::default(), MovementRules::Unrestricted);
        assert_eq!(
            MovementRules::Unrestricted.limits(),
            MovementLimits::UNRESTRICTED
        );
        // The opt-in for anyone who would rather be held to the rules.
        assert!(MovementRules::ServerSafe.limits().is_server_safe());
        // Unset limits are the safe ones, so a character nobody has told
        // still starts inside the rules.
        assert!(MovementLimits::default().is_server_safe());
    }

    #[test]
    fn server_safe_limits_hold_the_run_the_jump_and_the_flying() {
        let safe = MovementLimits::SERVER_SAFE;
        // The Run skill's own pace: no boost, but slower is always fine.
        assert_eq!(safe.clamp_speed_boost(2.5), 1.0);
        assert_eq!(safe.clamp_speed_boost(1.0), 1.0);
        assert_eq!(safe.clamp_speed_boost(0.5), 0.5);
        // The Jump skill's own height: nothing added.
        assert_eq!(safe.clamp_jump_height(9.0), 0.0);
        assert_eq!(safe.clamp_jump_height(0.0), 0.0);
        assert!(!safe.noclip);

        let free = MovementLimits::UNRESTRICTED;
        assert_eq!(free.clamp_speed_boost(2.5), 2.5);
        assert_eq!(free.clamp_speed_boost(99.0), MAX_SPEED_BOOST);
        assert_eq!(free.clamp_jump_height(9.0), 9.0);
        assert_eq!(free.clamp_jump_height(99.0), MAX_JUMP_HEIGHT);
        assert!(free.noclip);
        assert!(!free.is_server_safe());
    }
}
