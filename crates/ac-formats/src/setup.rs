//! Setup (0x02): a model made of GfxObj parts arranged in a hierarchy, with
//! placement frames, collision shapes, and default animation/motion/sound
//! table references.

use glam::Vec3;
use serde::Serialize;

use crate::animation::AnimationFrame;
use crate::geom::{CylSphere, Frame, Sphere};
use crate::{expect_id, Reader, Result};

pub mod flags {
    pub const HAS_PARENT: u32 = 0x1;
    pub const HAS_DEFAULT_SCALE: u32 = 0x2;
    pub const ALLOW_FREE_HEADING: u32 = 0x4;
    pub const HAS_PHYSICS_BSP: u32 = 0x8;
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Location {
    pub part_id: i32,
    pub frame: Frame,
}

impl Location {
    fn parse(r: &mut Reader) -> Result<Self> {
        Ok(Location {
            part_id: r.i32()?,
            frame: Frame::parse(r)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LightInfo {
    pub viewer_space_location: Frame,
    pub color: u32,
    pub intensity: f32,
    pub falloff: f32,
    pub cone_angle: f32,
}

impl LightInfo {
    fn parse(r: &mut Reader) -> Result<Self> {
        Ok(LightInfo {
            viewer_space_location: Frame::parse(r)?,
            color: r.u32()?,
            intensity: r.f32()?,
            falloff: r.f32()?,
            cone_angle: r.f32()?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Setup {
    pub id: u32,
    pub flags: u32,
    /// GfxObj ids, one per part.
    pub parts: Vec<u32>,
    /// Parent part index per part (`HAS_PARENT`).
    pub parent_index: Vec<u32>,
    pub default_scale: Vec<Vec3>,
    pub holding_locations: Vec<(i32, Location)>,
    pub connection_points: Vec<(i32, Location)>,
    /// Placement id -> one frame per part (+ hooks).
    pub placement_frames: Vec<(i32, AnimationFrame)>,
    pub cyl_spheres: Vec<CylSphere>,
    pub spheres: Vec<Sphere>,
    pub height: f32,
    pub radius: f32,
    pub step_up_height: f32,
    pub step_down_height: f32,
    pub sorting_sphere: Sphere,
    pub selection_sphere: Sphere,
    pub lights: Vec<(i32, LightInfo)>,
    pub default_animation: u32,
    pub default_script: u32,
    pub default_motion_table: u32,
    pub default_sound_table: u32,
    pub default_script_table: u32,
}

impl Setup {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let flags = r.u32()?;
        let n_parts = r.u32()? as usize;
        let parts = r.fixed(n_parts, &mut |r: &mut Reader| r.u32())?;
        let parent_index = if flags & flags::HAS_PARENT != 0 {
            r.fixed(n_parts, &mut |r: &mut Reader| r.u32())?
        } else {
            Vec::new()
        };
        let default_scale = if flags & flags::HAS_DEFAULT_SCALE != 0 {
            r.fixed(n_parts, &mut |r: &mut Reader| r.vec3())?
        } else {
            Vec::new()
        };
        let holding_locations = r.map(|r| r.i32(), Location::parse)?;
        let connection_points = r.map(|r| r.i32(), Location::parse)?;
        let placement_frames = r.map(|r| r.i32(), |r| AnimationFrame::parse(r, n_parts))?;
        let cyl_spheres = r.list(CylSphere::parse)?;
        let spheres = r.list(Sphere::parse)?;
        let height = r.f32()?;
        let radius = r.f32()?;
        let step_up_height = r.f32()?;
        let step_down_height = r.f32()?;
        let sorting_sphere = Sphere::parse(&mut r)?;
        let selection_sphere = Sphere::parse(&mut r)?;
        let lights = r.map(|r| r.i32(), LightInfo::parse)?;
        let default_animation = r.u32()?;
        let default_script = r.u32()?;
        let default_motion_table = r.u32()?;
        let default_sound_table = r.u32()?;
        let default_script_table = r.u32()?;
        r.finish()?;
        Ok(Setup {
            id,
            flags,
            parts,
            parent_index,
            default_scale,
            holding_locations,
            connection_points,
            placement_frames,
            cyl_spheres,
            spheres,
            height,
            radius,
            step_up_height,
            step_down_height,
            sorting_sphere,
            selection_sphere,
            lights,
            default_animation,
            default_script,
            default_motion_table,
            default_sound_table,
            default_script_table,
        })
    }
}
