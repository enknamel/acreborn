//! ParticleEmitterInfo (0x32): how an emitter spawns and moves particles.
//! Every particle is a copy of `hw_gfxobj_id` (a single textured quad the
//! client draws as a point sprite) born at a random offset from the
//! emitter, moved by the `particle_type` rule using the random vectors
//! `a`, `b`, `c`, and scaled/faded from the start to the final values
//! over its lifespan.

use glam::Vec3;
use serde::Serialize;

use crate::{expect_id, Reader, Result};

/// What triggers a birth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EmitterType {
    Unknown,
    /// `birthrate` is the interval in seconds between births.
    BirthratePerSec,
    /// `birthrate` is the distance the emitter travels between births.
    BirthratePerMeter,
    Other(i32),
}

impl From<i32> for EmitterType {
    fn from(v: i32) -> Self {
        match v {
            0 => EmitterType::Unknown,
            1 => EmitterType::BirthratePerSec,
            2 => EmitterType::BirthratePerMeter,
            o => EmitterType::Other(o),
        }
    }
}

/// How a particle moves. The abbreviations name which of `a` (velocity),
/// `b` (acceleration) and `c` (rotation) are in the emitter's Local frame
/// or Global (world) space, and whether the particle also Rotates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ParticleType {
    Unknown,
    Still,
    LocalVelocity,
    ParabolicLvga,
    ParabolicLvgaGr,
    Swarm,
    Explode,
    Implode,
    ParabolicLvla,
    ParabolicLvlaLr,
    ParabolicGvga,
    ParabolicGvgaGr,
    GlobalVelocity,
    Other(i32),
}

impl From<i32> for ParticleType {
    fn from(v: i32) -> Self {
        use ParticleType::*;
        match v {
            0 => Unknown,
            1 => Still,
            2 => LocalVelocity,
            3 => ParabolicLvga,
            4 => ParabolicLvgaGr,
            5 => Swarm,
            6 => Explode,
            7 => Implode,
            8 => ParabolicLvla,
            9 => ParabolicLvlaLr,
            10 => ParabolicGvga,
            11 => ParabolicGvgaGr,
            12 => GlobalVelocity,
            o => Other(o),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParticleEmitterInfo {
    pub id: u32,
    pub unknown: u32,
    pub emitter_type: EmitterType,
    pub particle_type: ParticleType,
    /// GfxObj (0x01) drawn per particle on the software path (usually 0).
    pub gfxobj_id: u32,
    /// GfxObj (0x01) drawn per particle: one quad whose surface is the
    /// particle's texture.
    pub hw_gfxobj_id: u32,
    /// Seconds (or metres) between births.
    pub birthrate: f64,
    /// Particles alive at once.
    pub max_particles: i32,
    /// Born the moment the emitter is created.
    pub initial_particles: i32,
    /// Emit this many then stop; 0 for endless.
    pub total_particles: i32,
    /// Emit for this long then stop; 0 for endless.
    pub total_seconds: f64,
    pub lifespan: f64,
    /// Lifespan varies by up to this much either way.
    pub lifespan_rand: f64,
    /// Births are offset in the plane perpendicular to this (or in any
    /// direction when zero) by a distance in `min_offset..=max_offset`.
    pub offset_dir: Vec3,
    pub min_offset: f32,
    pub max_offset: f32,
    /// Velocity, scaled by a random factor in `min_a..=max_a`.
    pub a: Vec3,
    pub min_a: f32,
    pub max_a: f32,
    /// Acceleration (or swarm frequency), scaled by `min_b..=max_b`.
    pub b: Vec3,
    pub min_b: f32,
    pub max_b: f32,
    /// Rotation rate, explode spread or implode centre, scaled by `min_c..=max_c`.
    pub c: Vec3,
    pub min_c: f32,
    pub max_c: f32,
    pub start_scale: f32,
    pub final_scale: f32,
    pub scale_rand: f32,
    /// Translucency (0 opaque .. 1 invisible) at birth and death.
    pub start_trans: f32,
    pub final_trans: f32,
    pub trans_rand: f32,
    /// Particles follow the emitter (1) or keep the frame they were born in (0).
    pub is_parent_local: i32,
}

impl ParticleEmitterInfo {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let unknown = r.u32()?;
        let emitter_type = EmitterType::from(r.i32()?);
        let particle_type = ParticleType::from(r.i32()?);
        let gfxobj_id = r.u32()?;
        let hw_gfxobj_id = r.u32()?;
        let birthrate = r.f64()?;
        let max_particles = r.i32()?;
        let initial_particles = r.i32()?;
        let total_particles = r.i32()?;
        let total_seconds = r.f64()?;
        let lifespan = r.f64()?;
        let lifespan_rand = r.f64()?;
        let offset_dir = r.vec3()?;
        let min_offset = r.f32()?;
        let max_offset = r.f32()?;
        let a = r.vec3()?;
        let min_a = r.f32()?;
        let max_a = r.f32()?;
        let b = r.vec3()?;
        let min_b = r.f32()?;
        let max_b = r.f32()?;
        let c = r.vec3()?;
        let min_c = r.f32()?;
        let max_c = r.f32()?;
        let start_scale = r.f32()?;
        let final_scale = r.f32()?;
        let scale_rand = r.f32()?;
        let start_trans = r.f32()?;
        let final_trans = r.f32()?;
        let trans_rand = r.f32()?;
        let is_parent_local = r.i32()?;
        r.finish()?;
        Ok(ParticleEmitterInfo {
            id,
            unknown,
            emitter_type,
            particle_type,
            gfxobj_id,
            hw_gfxobj_id,
            birthrate,
            max_particles,
            initial_particles,
            total_particles,
            total_seconds,
            lifespan,
            lifespan_rand,
            offset_dir,
            min_offset,
            max_offset,
            a,
            min_a,
            max_a,
            b,
            min_b,
            max_b,
            c,
            min_c,
            max_c,
            start_scale,
            final_scale,
            scale_rand,
            start_trans,
            final_trans,
            trans_rand,
            is_parent_local,
        })
    }

    pub fn is_parent_local(&self) -> bool {
        self.is_parent_local != 0
    }

    /// Radius of the sphere the emitter's particles stay within: the
    /// birth offset or how far the fastest particle travels in a lifespan.
    pub fn sorting_radius(&self) -> f32 {
        self.max_offset
            .max(self.max_a * self.a.length() * self.lifespan as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The record is a fixed 176 bytes.
    #[test]
    fn fixed_size() {
        let mut b = Vec::new();
        b.extend_from_slice(&0x3200_0001u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&1i32.to_le_bytes());
        b.extend_from_slice(&2i32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0x0100_1234u32.to_le_bytes());
        b.extend_from_slice(&0.05f64.to_le_bytes());
        b.extend_from_slice(&40i32.to_le_bytes());
        b.extend_from_slice(&5i32.to_le_bytes());
        b.extend_from_slice(&0i32.to_le_bytes());
        b.extend_from_slice(&0.0f64.to_le_bytes());
        b.extend_from_slice(&1.5f64.to_le_bytes());
        b.extend_from_slice(&0.5f64.to_le_bytes());
        for v in [0.0f32, 0.0, 1.0, 0.1, 0.3] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for _ in 0..3 {
            for v in [0.0f32, 0.0, 2.0, 0.5, 1.0] {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        for v in [0.5f32, 1.5, 0.1, 0.0, 1.0, 0.0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&1i32.to_le_bytes());
        assert_eq!(b.len(), 176);
        let e = ParticleEmitterInfo::parse(0x3200_0001, &b).unwrap();
        assert_eq!(e.emitter_type, EmitterType::BirthratePerSec);
        assert_eq!(e.particle_type, ParticleType::LocalVelocity);
        assert_eq!(e.hw_gfxobj_id, 0x0100_1234);
        assert_eq!(e.max_particles, 40);
        assert_eq!(e.lifespan, 1.5);
        assert_eq!(e.a, Vec3::new(0.0, 0.0, 2.0));
        assert_eq!(e.max_a, 1.0);
        assert!(e.is_parent_local());
        assert_eq!(e.sorting_radius(), 3.0);
        b.push(0);
        assert!(ParticleEmitterInfo::parse(0x3200_0001, &b).is_err());
    }
}
