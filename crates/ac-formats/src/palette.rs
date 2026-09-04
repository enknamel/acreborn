//! Palette (0x04): a flat list of ARGB colors.

use serde::Serialize;

use crate::{expect_id, Reader, Result};

#[derive(Debug, Clone, Serialize)]
pub struct Palette {
    pub id: u32,
    /// 0xAARRGGBB
    pub colors: Vec<u32>,
}

impl Palette {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let colors = r.list(|r| r.u32())?;
        r.finish()?;
        Ok(Palette { id, colors })
    }
}
