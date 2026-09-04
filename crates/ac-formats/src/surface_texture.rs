//! SurfaceTexture (0x05): an indirection from a surface to one or more
//! Texture (0x06) ids (mip/variant list).

use serde::Serialize;

use crate::{expect_id, Reader, Result};

#[derive(Debug, Clone, Serialize)]
pub struct SurfaceTexture {
    pub id: u32,
    pub unknown: u32,
    pub unknown_byte: u8,
    pub textures: Vec<u32>,
}

impl SurfaceTexture {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let unknown = r.u32()?;
        let unknown_byte = r.u8()?;
        let textures = r.list(|r| r.u32())?;
        r.finish()?;
        Ok(SurfaceTexture {
            id,
            unknown,
            unknown_byte,
            textures,
        })
    }
}
