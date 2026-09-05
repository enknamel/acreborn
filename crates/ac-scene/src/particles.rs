//! Particle emitters without a GPU: a [`ParticleSystem`] owns the live
//! [`Emitter`]s, each built from a ParticleEmitterInfo (0x32) and a world
//! transform, and every frame produces the camera-facing [`Quad`]s to draw.
//!
//! The rules follow the client's `ParticleEmitter`/`Particle` classes (see
//! ACE's `ACE.Server/Physics/Particles` port for the same logic): a
//! particle is born at a random offset from the emitter with random
//! velocity/acceleration/rotation vectors (`a`, `b`, `c` scaled by the
//! emitter's min/max factors), moves by its `ParticleType` rule, and
//! scales and fades linearly from the start to the final values over its
//! lifespan. Randomness comes from the caller's [`Rng`], so a simulation
//! is reproducible from its seed.

use std::rc::Rc;

use ac_formats::animation::HookData;
use ac_formats::particle_emitter::{EmitterType, ParticleEmitterInfo, ParticleType};
use ac_formats::physics_script::PhysicsScript;
use ac_formats::surface::{flags as surface_flags, SurfaceBase};
use glam::{Mat4, Quat, Vec2, Vec3};

use crate::model::frame_to_mat;
use crate::{Assets, Error, Result};

/// SplitMix64: small, fast, and good enough for spread and jitter.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in `[-1, 1)`.
    pub fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }

    /// Uniform in `[lo, hi)`.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

/// What a particle is textured with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpriteImage {
    /// A Surface (0x08) id: texture plus palette.
    Surface(u32),
    /// A flat ARGB colour.
    Solid(u32),
}

/// The billboard the emitter's `hw_gfxobj_id` describes: a single quad
/// whose surface is the particle image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sprite {
    pub image: SpriteImage,
    /// Width and height of the quad in metres at scale 1.
    pub size: Vec2,
    /// The surface adds light to what is behind it (fire, glows) rather
    /// than covering it (smoke, dust).
    pub additive: bool,
}

impl Sprite {
    /// A white 1 m square for emitters whose sprite cannot be resolved.
    pub const FALLBACK: Sprite = Sprite {
        image: SpriteImage::Solid(0x00FF_FFFF),
        size: Vec2::ONE,
        additive: false,
    };

    /// Resolve a hardware particle GfxObj (0x01) to its sprite: the image
    /// is the first polygon's surface and the size is the extent of the
    /// vertices (a point becomes a 1 m square).
    pub fn from_gfxobj(assets: &Assets, gfxobj_id: u32) -> Result<Sprite> {
        let g = assets.gfxobj(gfxobj_id)?;
        let surface_index = g
            .polygons
            .first()
            .map(|(_, p)| p.pos_surface.max(p.neg_surface).max(0) as usize)
            .unwrap_or(0);
        let surface_id = *g.surfaces.get(surface_index).ok_or_else(|| {
            Error::Other(format!("{gfxobj_id:#010x}: particle GfxObj has no surface"))
        })?;
        let surface = assets.surface(surface_id)?;
        let image = match surface.base {
            SurfaceBase::Solid { color } => SpriteImage::Solid(color),
            SurfaceBase::Image { .. } => SpriteImage::Surface(surface_id),
        };
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for (_, v) in &g.vertices {
            lo = lo.min(v.origin);
            hi = hi.max(v.origin);
        }
        let mut extent = if lo.x.is_finite() {
            (hi - lo).to_array()
        } else {
            [0.0; 3]
        };
        // The quad lies in some axis plane; keep its two long sides.
        extent.sort_by(|a, b| b.total_cmp(a));
        let size = Vec2::new(extent[0], extent[1]);
        let size = if size.x <= 1e-4 {
            Vec2::ONE
        } else if size.y <= 1e-4 {
            Vec2::splat(size.x)
        } else {
            size
        };
        Ok(Sprite {
            image,
            size,
            additive: surface.flags & surface_flags::ADDITIVE != 0,
        })
    }
}

/// One camera-facing quad to draw this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad {
    /// World-space centre.
    pub position: Vec3,
    /// Width and height in metres.
    pub size: Vec2,
    /// RGB tint and opacity (1 minus the particle's translucency).
    pub color: [f32; 4],
    pub image: SpriteImage,
    pub additive: bool,
}

#[derive(Debug, Clone)]
struct Particle {
    birth: f64,
    lifespan: f64,
    /// Emitter origin and rotation at birth.
    start_origin: Vec3,
    /// Birth offset in world space (already rotated by the start frame).
    offset: Vec3,
    a: Vec3,
    b: Vec3,
    c: Vec3,
    start_scale: f32,
    final_scale: f32,
    start_trans: f32,
    final_trans: f32,
    position: Vec3,
    scale: f32,
    trans: f32,
}

/// A live emitter: spawns, moves and retires particles in fixed slots
/// (`max_particles` of them) exactly as the client does.
#[derive(Debug, Clone)]
pub struct Emitter {
    pub info: Rc<ParticleEmitterInfo>,
    pub sprite: Sprite,
    transform: Mat4,
    origin: Vec3,
    rotation: Quat,
    slots: Vec<Option<Particle>>,
    creation_time: f64,
    last_emit_time: f64,
    last_emit_origin: Vec3,
    total_emitted: u32,
    live: u32,
    stopped: bool,
}

impl Emitter {
    /// Create at `time` and emit the initial burst.
    pub fn new(
        info: Rc<ParticleEmitterInfo>,
        sprite: Sprite,
        transform: Mat4,
        time: f64,
        rng: &mut Rng,
    ) -> Self {
        let (_, rotation, origin) = transform.to_scale_rotation_translation();
        let max = info.max_particles.max(0) as usize;
        let mut e = Emitter {
            info,
            sprite,
            transform,
            origin,
            rotation,
            slots: vec![None; max],
            creation_time: time,
            last_emit_time: time,
            last_emit_origin: origin,
            total_emitted: 0,
            live: 0,
            stopped: false,
        };
        for _ in 0..e.info.initial_particles.max(0) {
            e.emit(time, rng);
        }
        e
    }

    pub fn transform(&self) -> Mat4 {
        self.transform
    }

    /// Move the emitter (it follows its parent object or part).
    pub fn set_transform(&mut self, transform: Mat4) {
        self.transform = transform;
        let (_, rotation, origin) = transform.to_scale_rotation_translation();
        self.origin = origin;
        self.rotation = rotation;
    }

    /// Live particles.
    pub fn live(&self) -> usize {
        self.live as usize
    }

    pub fn total_emitted(&self) -> u32 {
        self.total_emitted
    }

    /// No more births; the emitter ends when its last particle dies.
    pub fn stop(&mut self) {
        self.stopped = true;
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Advance to `time`: age and retire particles, then emit if due.
    /// Returns false once the emitter is stopped and empty.
    pub fn update(&mut self, time: f64, rng: &mut Rng) -> bool {
        let parent_local = self.info.is_parent_local();
        let ptype = self.info.particle_type;
        let origin = self.origin;
        let mut live = 0;
        for slot in &mut self.slots {
            if let Some(p) = slot {
                let lifetime = time - p.birth;
                if lifetime >= p.lifespan {
                    *slot = None;
                    continue;
                }
                let base = if parent_local { origin } else { p.start_origin };
                p.advance(ptype, base, lifetime as f32);
                live += 1;
            }
        }
        self.live = live;
        if !self.stopped {
            if self.should_emit(time) {
                self.emit(time, rng);
            }
            self.check_stop(time);
        }
        !self.stopped || self.live > 0
    }

    fn should_emit(&self, time: f64) -> bool {
        let info = &self.info;
        if info.total_particles > 0 && self.total_emitted >= info.total_particles as u32 {
            return false;
        }
        if self.live as usize >= self.slots.len() {
            return false;
        }
        match info.emitter_type {
            EmitterType::BirthratePerSec => time - self.last_emit_time > info.birthrate,
            EmitterType::BirthratePerMeter => {
                self.origin.distance_squared(self.last_emit_origin) as f64
                    >= info.birthrate * info.birthrate
            }
            _ => false,
        }
    }

    fn check_stop(&mut self, time: f64) {
        let info = &self.info;
        if info.total_seconds > 0.0 && self.creation_time + info.total_seconds < time {
            self.stopped = true;
        }
        if info.total_particles > 0 && self.total_emitted >= info.total_particles as u32 {
            self.stopped = true;
        }
    }

    fn emit(&mut self, time: f64, rng: &mut Rng) {
        let Some(idx) = self.slots.iter().position(Option::is_none) else {
            return;
        };
        let info = self.info.clone();
        let rot = self.rotation;
        let origin = self.origin;
        let local = |v: Vec3| rot * v;
        let scaled = |v: Vec3, lo: f32, hi: f32, rng: &mut Rng| v * rng.range(lo, hi);
        let mut offset = local(random_offset(&info, rng));
        let ra = scaled(info.a, info.min_a, info.max_a, rng);
        let rb = scaled(info.b, info.min_b, info.max_b, rng);
        let rc = scaled(info.c, info.min_c, info.max_c, rng);
        use ParticleType::*;
        let (a, b, c) = match info.particle_type {
            Still => (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO),
            LocalVelocity => (local(ra), Vec3::ZERO, Vec3::ZERO),
            GlobalVelocity => (ra, Vec3::ZERO, Vec3::ZERO),
            ParabolicLvga => (local(ra), rb, Vec3::ZERO),
            ParabolicLvgaGr => (local(ra), rb, rc),
            ParabolicLvla => (local(ra), local(rb), Vec3::ZERO),
            ParabolicLvlaLr => (local(ra), local(rb), local(rc)),
            ParabolicGvga => (ra, rb, Vec3::ZERO),
            ParabolicGvgaGr => (ra, rb, rc),
            Swarm => (local(ra), rb, rc),
            Explode => {
                // A random direction whose spread per axis is `c`.
                let yaw = rng.range(-std::f32::consts::PI, std::f32::consts::PI);
                let pitch = rng.range(-std::f32::consts::PI, std::f32::consts::PI);
                let cp = pitch.cos();
                let dir = Vec3::new(
                    yaw.cos() * rc.x * cp,
                    yaw.sin() * rc.y * cp,
                    pitch.sin() * rc.z * cp,
                );
                let dir = if dir.length_squared() < 1e-8 {
                    Vec3::ZERO
                } else {
                    dir.normalize()
                };
                (ra, rb, dir)
            }
            Implode => {
                offset *= rc;
                (ra, rb, offset)
            }
            Unknown | Other(_) => (ra, rb, rc),
        };
        let lifespan = (info.lifespan + rng.signed() as f64 * info.lifespan_rand).max(0.0);
        let jitter = |base: f32, spread: f32, lo: f32, hi: f32, rng: &mut Rng| {
            (base + rng.signed() * spread).clamp(lo, hi)
        };
        let mut p = Particle {
            birth: time,
            lifespan,
            start_origin: origin,
            offset,
            a,
            b,
            c,
            start_scale: jitter(info.start_scale, info.scale_rand, 0.1, 10.0, rng),
            final_scale: jitter(info.final_scale, info.scale_rand, 0.1, 10.0, rng),
            start_trans: jitter(info.start_trans, info.trans_rand, 0.0, 1.0, rng),
            final_trans: jitter(info.final_trans, info.trans_rand, 0.0, 1.0, rng),
            position: origin + offset,
            scale: info.start_scale,
            trans: info.start_trans,
        };
        p.advance(info.particle_type, origin, 0.0);
        self.slots[idx] = Some(p);
        self.live += 1;
        self.total_emitted += 1;
        self.last_emit_time = time;
        self.last_emit_origin = origin;
    }

    /// Append this emitter's particles as quads.
    pub fn quads(&self, out: &mut Vec<Quad>) {
        for p in self.slots.iter().flatten() {
            out.push(Quad {
                position: p.position,
                size: self.sprite.size * p.scale,
                color: [1.0, 1.0, 1.0, (1.0 - p.trans).clamp(0.0, 1.0)],
                image: self.sprite.image,
                additive: self.sprite.additive,
            });
        }
    }

    /// Radius around the emitter origin within which its particles stay.
    pub fn radius(&self) -> f32 {
        self.info
            .sorting_radius()
            .max(self.sprite.size.max_element() * 0.5)
    }
}

/// A birth offset: a random direction in the plane perpendicular to
/// `offset_dir` (any direction when it is zero), `min_offset..max_offset`
/// long.
fn random_offset(info: &ParticleEmitterInfo, rng: &mut Rng) -> Vec3 {
    let r = Vec3::new(rng.signed(), rng.signed(), rng.signed());
    let d = r - info.offset_dir * info.offset_dir.dot(r);
    let dist = rng.range(info.min_offset, info.max_offset);
    if d.length_squared() < 1e-8 {
        return Vec3::ZERO;
    }
    d.normalize() * dist
}

impl Particle {
    /// Position, scale and translucency at `t` seconds after birth, from
    /// the parent origin `base`.
    fn advance(&mut self, ptype: ParticleType, base: Vec3, t: f32) {
        use ParticleType::*;
        let (a, b, c) = (self.a, self.b, self.c);
        let rest = base + self.offset;
        self.position = match ptype {
            Still | Unknown | Other(_) => rest,
            LocalVelocity | GlobalVelocity => rest + a * t,
            ParabolicLvga | ParabolicLvgaGr | ParabolicLvla | ParabolicLvlaLr | ParabolicGvga
            | ParabolicGvgaGr => rest + a * t + b * (t * t * 0.5),
            Swarm => {
                let centre = rest + a * t + c;
                centre + Vec3::new((t * b.x).cos(), (t * b.y).sin(), (t * b.z).cos())
            }
            Explode => rest + (b * t + c * a.x) * t,
            Implode => rest + c * (a.x * t).cos() + b * (t * t),
        };
        let f = if self.lifespan > 0.0 {
            (t as f64 / self.lifespan).min(1.0) as f32
        } else {
            1.0
        };
        self.scale = self.start_scale + (self.final_scale - self.start_scale) * f;
        self.trans = self.start_trans + (self.final_trans - self.start_trans) * f;
    }
}

/// The emitters of one scene, addressed by the ids animation hooks and
/// scripts use (`CreateParticle`/`StopParticle`/`DestroyParticle`).
#[derive(Debug, Default)]
pub struct ParticleSystem {
    emitters: Vec<(u32, Emitter)>,
    next_id: u32,
}

impl ParticleSystem {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an emitter from its info id. A non-zero `emitter_id`
    /// replaces any emitter with that id; 0 allocates one. Returns the id.
    pub fn create(
        &mut self,
        assets: &Assets,
        emitter_info_id: u32,
        transform: Mat4,
        emitter_id: u32,
        time: f64,
        rng: &mut Rng,
    ) -> Result<u32> {
        let info = assets.particle_emitter(emitter_info_id)?;
        let sprite = if info.hw_gfxobj_id != 0 {
            Sprite::from_gfxobj(assets, info.hw_gfxobj_id).unwrap_or_else(|e| {
                tracing::warn!("emitter {emitter_info_id:#010x}: {e}; using a plain sprite");
                Sprite::FALLBACK
            })
        } else {
            Sprite::FALLBACK
        };
        Ok(self.add(Emitter::new(info, sprite, transform, time, rng), emitter_id))
    }

    /// Register an already-built emitter under `emitter_id` (0 allocates).
    pub fn add(&mut self, emitter: Emitter, emitter_id: u32) -> u32 {
        let id = if emitter_id == 0 {
            self.next_id += 1;
            // Script ids are small; keep allocated ones clear of them.
            0x8000_0000 | self.next_id
        } else {
            self.emitters.retain(|(i, _)| *i != emitter_id);
            emitter_id
        };
        self.emitters.push((id, emitter));
        id
    }

    pub fn get(&self, id: u32) -> Option<&Emitter> {
        self.emitters.iter().find(|(i, _)| *i == id).map(|(_, e)| e)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Emitter> {
        self.emitters
            .iter_mut()
            .find(|(i, _)| *i == id)
            .map(|(_, e)| e)
    }

    pub fn stop(&mut self, id: u32) -> bool {
        match self.get_mut(id) {
            Some(e) => {
                e.stop();
                true
            }
            None => false,
        }
    }

    pub fn destroy(&mut self, id: u32) -> bool {
        let n = self.emitters.len();
        self.emitters.retain(|(i, _)| *i != id);
        self.emitters.len() != n
    }

    pub fn len(&self) -> usize {
        self.emitters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.emitters.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &Emitter)> {
        self.emitters.iter().map(|(i, e)| (*i, e))
    }

    /// Advance every emitter to `time`, dropping the finished ones.
    pub fn update(&mut self, time: f64, rng: &mut Rng) {
        self.emitters.retain_mut(|(_, e)| e.update(time, rng));
    }

    /// Every live particle as a quad.
    pub fn quads(&self) -> Vec<Quad> {
        let mut out = Vec::new();
        for (_, e) in &self.emitters {
            e.quads(&mut out);
        }
        out
    }
}

/// Plays a PhysicsScript (0x33) on a [`ParticleSystem`]: the particle
/// hooks fire at their start times relative to `start`; other hook kinds
/// (sounds, fades) are left to the caller.
#[derive(Debug, Clone)]
pub struct ScriptPlayer {
    pub script: Rc<PhysicsScript>,
    start: f64,
    next: usize,
}

impl ScriptPlayer {
    pub fn new(script: Rc<PhysicsScript>, start: f64) -> Self {
        ScriptPlayer {
            script,
            start,
            next: 0,
        }
    }

    /// Fire the hooks due by `time`. `transform` is the object's world
    /// frame (part offsets are applied on top when the hook names one;
    /// `part_transform` supplies a part's frame by index). Returns true
    /// while hooks remain.
    pub fn update(
        &mut self,
        time: f64,
        system: &mut ParticleSystem,
        assets: &Assets,
        transform: Mat4,
        mut part_transform: impl FnMut(u32) -> Option<Mat4>,
        rng: &mut Rng,
    ) -> bool {
        while let Some(h) = self.script.hooks.get(self.next) {
            if self.start + h.start_time > time {
                break;
            }
            self.next += 1;
            match &h.hook.data {
                HookData::CreateParticle {
                    emitter_info_id,
                    part_index,
                    offset,
                    emitter_id,
                }
                | HookData::CreateBlockingParticle {
                    emitter_info_id,
                    part_index,
                    offset,
                    emitter_id,
                } => {
                    let base = part_transform(*part_index).unwrap_or(transform);
                    let m = base * frame_to_mat(offset);
                    if let Err(e) =
                        system.create(assets, *emitter_info_id, m, *emitter_id, time, rng)
                    {
                        tracing::warn!(
                            "script {:#010x}: emitter {emitter_info_id:#010x}: {e}",
                            self.script.id
                        );
                    }
                }
                HookData::StopParticle { emitter_id } => {
                    system.stop(*emitter_id);
                }
                HookData::DestroyParticle { emitter_id } => {
                    system.destroy(*emitter_id);
                }
                _ => {}
            }
        }
        self.next < self.script.hooks.len()
    }

    pub fn is_done(&self) -> bool {
        self.next >= self.script.hooks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(particle_type: ParticleType) -> Rc<ParticleEmitterInfo> {
        Rc::new(ParticleEmitterInfo {
            id: 0x3200_0001,
            unknown: 0,
            emitter_type: EmitterType::BirthratePerSec,
            particle_type,
            gfxobj_id: 0,
            hw_gfxobj_id: 0,
            birthrate: 0.1,
            max_particles: 4,
            initial_particles: 1,
            total_particles: 0,
            total_seconds: 0.0,
            lifespan: 1.0,
            lifespan_rand: 0.0,
            offset_dir: Vec3::Z,
            min_offset: 0.5,
            max_offset: 0.5,
            a: Vec3::Z,
            min_a: 2.0,
            max_a: 2.0,
            b: Vec3::new(0.0, 0.0, -1.0),
            min_b: 1.0,
            max_b: 1.0,
            c: Vec3::ZERO,
            min_c: 1.0,
            max_c: 1.0,
            start_scale: 1.0,
            final_scale: 2.0,
            scale_rand: 0.0,
            start_trans: 0.0,
            final_trans: 1.0,
            trans_rand: 0.0,
            is_parent_local: 0,
        })
    }

    #[test]
    fn rng_is_deterministic_and_in_range() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..1000 {
            let x = a.unit();
            assert_eq!(x, b.unit());
            assert!((0.0..1.0).contains(&x));
            let s = a.signed();
            b.signed();
            assert!((-1.0..1.0).contains(&s));
        }
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
    }

    #[test]
    fn births_respect_rate_and_capacity() {
        let mut rng = Rng::new(1);
        let mut e = Emitter::new(
            info(ParticleType::LocalVelocity),
            Sprite::FALLBACK,
            Mat4::IDENTITY,
            0.0,
            &mut rng,
        );
        assert_eq!(e.live(), 1, "initial burst");
        // 0.1 s between births: after 0.35 s at 60 Hz, three more.
        let mut t = 0.0;
        while t < 0.35 {
            t += 1.0 / 60.0;
            e.update(t, &mut rng);
        }
        assert_eq!(e.live(), 4);
        assert_eq!(e.total_emitted(), 4);
        // Full: no more until one dies at 1 s.
        while t < 0.9 {
            t += 1.0 / 60.0;
            e.update(t, &mut rng);
        }
        assert_eq!(e.total_emitted(), 4);
        while t < 1.05 {
            t += 1.0 / 60.0;
            e.update(t, &mut rng);
        }
        assert_eq!(e.total_emitted(), 5, "slot reused after death");
        assert_eq!(e.live(), 4);
    }

    #[test]
    fn local_velocity_follows_the_emitter_frame() {
        let mut rng = Rng::new(3);
        // The emitter faces +X (yaw -90 degrees), 10 m up.
        let m = Mat4::from_rotation_translation(
            Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2),
            Vec3::new(0.0, 0.0, 10.0),
        );
        let mut e = Emitter::new(
            info(ParticleType::LocalVelocity),
            Sprite::FALLBACK,
            m,
            0.0,
            &mut rng,
        );
        let mut q = Vec::new();
        e.quads(&mut q);
        let born = q[0].position;
        // Offset is perpendicular to +Z, 0.5 m long.
        assert!((born.z - 10.0).abs() < 1e-4, "{born}");
        assert!(((born - Vec3::new(0.0, 0.0, 10.0)).length() - 0.5).abs() < 1e-4);
        e.update(0.5, &mut rng);
        q.clear();
        e.quads(&mut q);
        let p = q.iter().find(|p| p.color[3] < 0.6).unwrap();
        // Velocity is local +Z (2 m/s) which the frame keeps vertical.
        assert!((p.position.z - 11.0).abs() < 1e-4, "{}", p.position);
        assert!((p.position.truncate() - born.truncate()).length() < 1e-4);
        // Half way through life: scale 1.5, opacity 0.5.
        assert!((p.size.x - 1.5).abs() < 1e-4);
        assert!((p.color[3] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn parabolic_particles_fall_back() {
        let mut rng = Rng::new(5);
        let mut i = (*info(ParticleType::ParabolicLvga)).clone();
        i.initial_particles = 1;
        i.max_particles = 1;
        i.total_particles = 1;
        i.min_offset = 0.0;
        i.max_offset = 0.0;
        let mut e = Emitter::new(Rc::new(i), Sprite::FALLBACK, Mat4::IDENTITY, 0.0, &mut rng);
        e.update(1.0 - 1e-6, &mut rng);
        let mut q = Vec::new();
        e.quads(&mut q);
        // z = 2t - t^2/2 at t = 1.
        assert!((q[0].position.z - 1.5).abs() < 1e-3, "{}", q[0].position);
        e.update(1.0, &mut rng);
        assert_eq!(e.live(), 0);
    }

    #[test]
    fn parent_local_particles_move_with_the_emitter() {
        let mut rng = Rng::new(9);
        let mut i = (*info(ParticleType::Still)).clone();
        i.is_parent_local = 1;
        let mut e = Emitter::new(Rc::new(i), Sprite::FALLBACK, Mat4::IDENTITY, 0.0, &mut rng);
        let mut q = Vec::new();
        e.quads(&mut q);
        let p0 = q[0].position;
        e.set_transform(Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0)));
        e.update(0.01, &mut rng);
        q.clear();
        e.quads(&mut q);
        assert!((q[0].position - p0 - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn finite_emitters_stop_and_finish() {
        let mut rng = Rng::new(2);
        let mut i = (*info(ParticleType::Still)).clone();
        i.total_particles = 3;
        i.lifespan = 0.5;
        let mut sys = ParticleSystem::new();
        let id = sys.add(
            Emitter::new(Rc::new(i), Sprite::FALLBACK, Mat4::IDENTITY, 0.0, &mut rng),
            0,
        );
        assert!(id != 0);
        let mut t = 0.0;
        while t < 0.4 {
            t += 1.0 / 60.0;
            sys.update(t, &mut rng);
        }
        let e = sys.get(id).unwrap();
        assert_eq!(e.total_emitted(), 3);
        assert!(e.is_stopped());
        assert_eq!(sys.quads().len(), 3);
        while t < 1.0 {
            t += 1.0 / 60.0;
            sys.update(t, &mut rng);
        }
        assert!(sys.is_empty(), "stopped and empty emitters are dropped");
    }

    #[test]
    fn stop_and_destroy_by_id() {
        let mut rng = Rng::new(4);
        let mut sys = ParticleSystem::new();
        sys.add(
            Emitter::new(
                info(ParticleType::Still),
                Sprite::FALLBACK,
                Mat4::IDENTITY,
                0.0,
                &mut rng,
            ),
            7,
        );
        sys.add(
            Emitter::new(
                info(ParticleType::Still),
                Sprite::FALLBACK,
                Mat4::IDENTITY,
                0.0,
                &mut rng,
            ),
            7,
        );
        assert_eq!(sys.len(), 1, "same id replaces");
        assert!(sys.stop(7));
        assert!(!sys.stop(8));
        sys.update(0.5, &mut rng);
        assert_eq!(sys.quads().len(), 1);
        assert!(sys.destroy(7));
        assert!(sys.is_empty());
    }
}
