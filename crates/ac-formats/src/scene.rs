//! Scene (0x12): a list of scenery object descriptions. The Region's
//! scene tables pick a Scene per terrain cell, and the client places its
//! objects procedurally (see `ac-scene::scenery`).

use serde::Serialize;

use crate::geom::Frame;
use crate::{expect_id, Reader, Result};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ObjectDesc {
    /// GfxObj or Setup id.
    pub obj_id: u32,
    pub base_loc: Frame,
    /// Probability of placement in [0, 1].
    pub freq: f32,
    pub displace_x: f32,
    pub displace_y: f32,
    pub min_scale: f32,
    pub max_scale: f32,
    /// Degrees of random heading.
    pub max_rotation: f32,
    /// Allowed range of the terrain normal's z.
    pub min_slope: f32,
    pub max_slope: f32,
    /// Non-zero: align heading to the terrain slope instead of random.
    pub align: u32,
    pub orient: u32,
    /// Non-zero: a server-spawned weenie, not client scenery.
    pub weenie_obj: u32,
}

impl ObjectDesc {
    fn parse(r: &mut Reader) -> Result<Self> {
        Ok(ObjectDesc {
            obj_id: r.u32()?,
            base_loc: Frame::parse(r)?,
            freq: r.f32()?,
            displace_x: r.f32()?,
            displace_y: r.f32()?,
            min_scale: r.f32()?,
            max_scale: r.f32()?,
            max_rotation: r.f32()?,
            min_slope: r.f32()?,
            max_slope: r.f32()?,
            align: r.u32()?,
            orient: r.u32()?,
            weenie_obj: r.u32()?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Scene {
    pub id: u32,
    pub objects: Vec<ObjectDesc>,
}

impl Scene {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let objects = r.list(ObjectDesc::parse)?;
        r.finish()?;
        Ok(Scene { id, objects })
    }
}
