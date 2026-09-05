//! PaletteSet (0x0F): a list of Palette (0x04) ids ordered by shade, used by
//! character creation (skin and hair colours) and clothing tables to pick
//! one palette from a hue in `0..=1`.

use serde::Serialize;

use crate::{expect_id, Reader, Result};

#[derive(Debug, Clone, Serialize)]
pub struct PaletteSet {
    pub id: u32,
    pub palettes: Vec<u32>,
}

impl PaletteSet {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let palettes = r.list(|r| r.u32())?;
        r.finish()?;
        Ok(PaletteSet { id, palettes })
    }

    /// The palette for a shade in `0..=1` (the client's `PalSet::GetPaletteID`:
    /// `int((count - 1e-6) * shade)`, clamped to the list).
    pub fn palette_for_shade(&self, shade: f32) -> Option<u32> {
        if self.palettes.is_empty() {
            return None;
        }
        let n = self.palettes.len();
        let idx = ((n as f64 - 0.000001) * shade.clamp(0.0, 1.0) as f64) as usize;
        Some(self.palettes[idx.min(n - 1)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shade_picks_palette() {
        let mut data = Vec::new();
        data.extend_from_slice(&0x0F00_0013u32.to_le_bytes());
        data.extend_from_slice(&5u32.to_le_bytes());
        for i in 0..5u32 {
            data.extend_from_slice(&(0x0400_02B6 + i).to_le_bytes());
        }
        let ps = PaletteSet::parse(0x0F00_0013, &data).unwrap();
        assert_eq!(ps.palettes.len(), 5);
        assert_eq!(ps.palette_for_shade(0.0), Some(0x0400_02B6));
        assert_eq!(ps.palette_for_shade(0.5), Some(0x0400_02B8));
        assert_eq!(ps.palette_for_shade(1.0), Some(0x0400_02BA));
        assert_eq!(ps.palette_for_shade(2.0), Some(0x0400_02BA));
    }
}
