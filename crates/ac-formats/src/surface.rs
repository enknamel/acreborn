//! Surface (0x08): material description; either a solid color or a texture
//! plus palette, with translucency/luminosity/diffuse factors. The file
//! does not start with its own id.

use serde::Serialize;

use crate::{Reader, Result};

pub mod flags {
    pub const BASE1_SOLID: u32 = 0x1;
    pub const BASE1_IMAGE: u32 = 0x2;
    pub const BASE1_CLIPMAP: u32 = 0x4;
    pub const TRANSLUCENT: u32 = 0x10;
    pub const DIFFUSE: u32 = 0x20;
    pub const LUMINOUS: u32 = 0x40;
    pub const ALPHA: u32 = 0x100;
    pub const INV_ALPHA: u32 = 0x200;
    pub const ADDITIVE: u32 = 0x10000;
    pub const DETAIL: u32 = 0x20000;
    pub const GOURAUD: u32 = 0x1000_0000;
    pub const STIPPLED: u32 = 0x4000_0000;
    pub const PERSPECTIVE: u32 = 0x8000_0000;
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum SurfaceBase {
    Solid { color: u32 },
    Image { texture: u32, palette: u32 },
}

#[derive(Debug, Clone, Serialize)]
pub struct Surface {
    pub id: u32,
    pub flags: u32,
    pub base: SurfaceBase,
    pub translucency: f32,
    pub luminosity: f32,
    pub diffuse: f32,
}

impl Surface {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        let flags = r.u32()?;
        let base = if flags & (flags::BASE1_IMAGE | flags::BASE1_CLIPMAP) != 0 {
            SurfaceBase::Image {
                texture: r.u32()?,
                palette: r.u32()?,
            }
        } else {
            SurfaceBase::Solid { color: r.u32()? }
        };
        let translucency = r.f32()?;
        let luminosity = r.f32()?;
        let diffuse = r.f32()?;
        r.finish()?;
        Ok(Surface {
            id,
            flags,
            base,
            translucency,
            luminosity,
            diffuse,
        })
    }
}
