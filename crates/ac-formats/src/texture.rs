//! Texture / RenderSurface (0x06): raw pixel data in one of the D3D
//! pixel formats, optionally with a default palette for indexed formats.

use serde::Serialize;

use crate::{expect_id, Reader, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[repr(u32)]
pub enum PixelFormat {
    R8G8B8 = 20,
    A8R8G8B8 = 21,
    X8R8G8B8 = 22,
    R5G6B5 = 23,
    X1R5G5B5 = 24,
    A1R5G5B5 = 25,
    A4R4G4B4 = 26,
    A8 = 28,
    P8 = 41,
    L8 = 50,
    Index16 = 101,
    CustomR8G8B8A8 = 240,
    CustomA8B8G8R8 = 241,
    CustomB8G8R8 = 242,
    CustomLscapeR8G8B8 = 243,
    CustomLscapeAlpha = 244,
    CustomRawJpeg = 500,
    Dxt1 = 0x3154_5844,
    Dxt3 = 0x3354_5844,
    Dxt5 = 0x3554_5844,
    Other(u32),
}

impl From<u32> for PixelFormat {
    fn from(v: u32) -> Self {
        use PixelFormat::*;
        match v {
            20 => R8G8B8,
            21 => A8R8G8B8,
            22 => X8R8G8B8,
            23 => R5G6B5,
            24 => X1R5G5B5,
            25 => A1R5G5B5,
            26 => A4R4G4B4,
            28 => A8,
            41 => P8,
            50 => L8,
            101 => Index16,
            240 => CustomR8G8B8A8,
            241 => CustomA8B8G8R8,
            242 => CustomB8G8R8,
            243 => CustomLscapeR8G8B8,
            244 => CustomLscapeAlpha,
            500 => CustomRawJpeg,
            0x3154_5844 => Dxt1,
            0x3354_5844 => Dxt3,
            0x3554_5844 => Dxt5,
            o => Other(o),
        }
    }
}

impl PixelFormat {
    pub fn is_indexed(self) -> bool {
        matches!(self, PixelFormat::Index16 | PixelFormat::P8)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Texture {
    pub id: u32,
    pub unknown: u32,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    #[serde(skip)]
    pub data: Vec<u8>,
    pub data_len: usize,
    /// Present for `Index16` / `P8`.
    pub default_palette: Option<u32>,
}

impl Texture {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let unknown = r.u32()?;
        let width = r.u32()?;
        let height = r.u32()?;
        let format = PixelFormat::from(r.u32()?);
        let len = r.u32()? as usize;
        let pixels = r.bytes(len)?.to_vec();
        let default_palette = if format.is_indexed() {
            Some(r.u32()?)
        } else {
            None
        };
        r.finish()?;
        Ok(Texture {
            id,
            unknown,
            width,
            height,
            format,
            data_len: pixels.len(),
            data: pixels,
            default_palette,
        })
    }
}
