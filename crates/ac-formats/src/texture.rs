//! Texture / RenderSurface (0x06): raw pixel data in one of the D3D
//! pixel formats, optionally with a default palette for indexed formats.

use serde::Serialize;

use crate::{expect_id, Error, Reader, Result};

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

/// A decoded texture: tightly packed RGBA8, top row first.
#[derive(Debug, Clone)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Texture {
    /// Expand to RGBA8. Indexed formats need `palette` (the texture's
    /// `default_palette` unless the caller substitutes one); JPEG payloads
    /// are decoded with the `image` crate.
    pub fn to_rgba8(&self, palette: Option<&[u32]>) -> Result<Rgba> {
        use PixelFormat::*;
        let w = self.width;
        let h = self.height;
        let n = (w * h) as usize;
        let d = &self.data;
        let need = |bpp: usize| -> Result<()> {
            if d.len() < n * bpp {
                Err(Error::Invalid {
                    what: "texture data",
                    detail: format!("{} bytes for {w}x{h}x{bpp}", d.len()),
                })
            } else {
                Ok(())
            }
        };
        let mut px = Vec::with_capacity(n * 4);
        match self.format {
            R8G8B8 | CustomB8G8R8 => {
                need(3)?;
                for p in d[..n * 3].chunks_exact(3) {
                    px.extend_from_slice(&[p[2], p[1], p[0], 255]);
                }
            }
            CustomLscapeR8G8B8 => {
                need(3)?;
                for p in d[..n * 3].chunks_exact(3) {
                    px.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
            }
            A8R8G8B8 | X8R8G8B8 => {
                need(4)?;
                let opaque = self.format == X8R8G8B8;
                for p in d[..n * 4].chunks_exact(4) {
                    px.extend_from_slice(&[p[2], p[1], p[0], if opaque { 255 } else { p[3] }]);
                }
            }
            CustomR8G8B8A8 => {
                need(4)?;
                px.extend_from_slice(&d[..n * 4]);
            }
            CustomA8B8G8R8 => {
                need(4)?;
                for p in d[..n * 4].chunks_exact(4) {
                    px.extend_from_slice(&[p[3], p[2], p[1], p[0]]);
                }
            }
            A8 | L8 | CustomLscapeAlpha => {
                need(1)?;
                for &v in &d[..n] {
                    px.extend_from_slice(&[v, v, v, 255]);
                }
            }
            R5G6B5 => {
                need(2)?;
                for p in d[..n * 2].chunks_exact(2) {
                    let v = u16::from_le_bytes([p[0], p[1]]);
                    let r5 = (v >> 11) & 0x1F;
                    let g6 = (v >> 5) & 0x3F;
                    let b5 = v & 0x1F;
                    px.extend_from_slice(&[
                        ((r5 << 3) | (r5 >> 2)) as u8,
                        ((g6 << 2) | (g6 >> 4)) as u8,
                        ((b5 << 3) | (b5 >> 2)) as u8,
                        255,
                    ]);
                }
            }
            A4R4G4B4 => {
                need(2)?;
                for p in d[..n * 2].chunks_exact(2) {
                    let v = u16::from_le_bytes([p[0], p[1]]);
                    px.extend_from_slice(&[
                        ((v >> 8) & 0xF) as u8 * 17,
                        ((v >> 4) & 0xF) as u8 * 17,
                        (v & 0xF) as u8 * 17,
                        ((v >> 12) & 0xF) as u8 * 17,
                    ]);
                }
            }
            A1R5G5B5 | X1R5G5B5 => {
                need(2)?;
                for p in d[..n * 2].chunks_exact(2) {
                    let v = u16::from_le_bytes([p[0], p[1]]);
                    let c5 = |s: u16| {
                        let c = (v >> s) & 0x1F;
                        ((c << 3) | (c >> 2)) as u8
                    };
                    let a = if self.format == X1R5G5B5 || v & 0x8000 != 0 {
                        255
                    } else {
                        0
                    };
                    px.extend_from_slice(&[c5(10), c5(5), c5(0), a]);
                }
            }
            Index16 | P8 => {
                let pal = palette.ok_or(Error::Invalid {
                    what: "texture",
                    detail: "indexed texture without palette".into(),
                })?;
                let bpp = if self.format == Index16 { 2 } else { 1 };
                need(bpp)?;
                for i in 0..n {
                    let idx = if bpp == 2 {
                        u16::from_le_bytes([d[i * 2], d[i * 2 + 1]]) as usize
                    } else {
                        d[i] as usize
                    };
                    let c = pal.get(idx).copied().unwrap_or(0xFF00_00FF);
                    px.extend_from_slice(&[
                        (c >> 16) as u8,
                        (c >> 8) as u8,
                        c as u8,
                        (c >> 24) as u8,
                    ]);
                }
            }
            Dxt1 | Dxt3 | Dxt5 => {
                let kind = match self.format {
                    Dxt1 => crate::dxt::DxtKind::Dxt1,
                    Dxt3 => crate::dxt::DxtKind::Dxt3,
                    _ => crate::dxt::DxtKind::Dxt5,
                };
                px = crate::dxt::decode(d, w, h, kind).ok_or(Error::Invalid {
                    what: "texture data",
                    detail: "short DXT payload".into(),
                })?;
            }
            CustomRawJpeg => {
                let img = image::load_from_memory_with_format(d, image::ImageFormat::Jpeg)
                    .map_err(|e| Error::Invalid {
                        what: "jpeg",
                        detail: e.to_string(),
                    })?
                    .to_rgba8();
                return Ok(Rgba {
                    width: img.width(),
                    height: img.height(),
                    pixels: img.into_raw(),
                });
            }
            Other(v) => {
                return Err(Error::Unsupported {
                    what: "pixel format",
                    value: v,
                })
            }
        }
        Ok(Rgba {
            width: w,
            height: h,
            pixels: px,
        })
    }

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
