//! The whole of Dereth's terrain as one grid: every landblock's 9x9
//! heights and terrain types from the cell archive stitched into a
//! 2041x2041 vertex lattice at 24 m spacing (landblocks share their edge
//! vertices). Loading reads 65k small files (a few seconds), so a cache
//! file is kept beside the data. The world map and overland routing read
//! from this.

use crate::{Assets, Result};
use ac_formats::landblock::{terrain as tbits, CellLandblock};
use glam::Vec2;
use std::path::{Path, PathBuf};

/// Vertices per side of the world lattice: 255 blocks of 8 cells plus one.
pub const SIDE: usize = 255 * 8 + 1;
/// Metres between vertices.
pub const SPACING: f32 = 24.0;
/// The lattice as a cache file: magic, then heights and terrain words.
const MAGIC: &[u8; 8] = b"ACGRID01";

#[derive(Clone, Debug, PartialEq)]
pub struct WorldGrid {
    /// Height table index per vertex, `x * SIDE + y`.
    pub heights: Vec<u8>,
    /// Raw terrain word per vertex (type, road and scenery bits).
    pub terrain: Vec<u16>,
    /// Region height table: index -> metres.
    pub height_table: Vec<f32>,
    /// Which landblocks exist in the archive (missing ones read as flat
    /// sea at height 0).
    pub present: Vec<bool>,
}

impl WorldGrid {
    /// Read every landblock from the cell archive.
    pub fn load(assets: &Assets) -> Result<WorldGrid> {
        let region = assets.region()?;
        let mut g = WorldGrid {
            heights: vec![0; SIDE * SIDE],
            terrain: vec![0; SIDE * SIDE],
            height_table: region.land_defs.land_height_table.clone(),
            present: vec![false; 256 * 256],
        };
        for bx in 0..255u32 {
            for by in 0..255u32 {
                let id = (bx << 24) | (by << 16) | 0xFFFF;
                let Ok(data) = assets.cell.read(id) else {
                    continue;
                };
                let Ok(lb) = CellLandblock::parse(id, &data) else {
                    continue;
                };
                g.present[(bx * 256 + by) as usize] = true;
                for x in 0..9usize {
                    for y in 0..9usize {
                        let i = x * 9 + y;
                        let gx = bx as usize * 8 + x;
                        let gy = by as usize * 8 + y;
                        g.heights[gx * SIDE + gy] = lb.height[i];
                        g.terrain[gx * SIDE + gy] = lb.terrain[i];
                    }
                }
            }
        }
        Ok(g)
    }

    /// Load from `dir/worldgrid.bin` when it exists, else read the
    /// archive and write the cache (best effort).
    pub fn load_cached(assets: &Assets, dir: &Path) -> Result<WorldGrid> {
        let path = dir.join("worldgrid.bin");
        if let Some(g) = std::fs::read(&path)
            .ok()
            .and_then(|bytes| WorldGrid::from_bytes(&bytes))
        {
            return Ok(g);
        }
        let g = WorldGrid::load(assets)?;
        if std::fs::create_dir_all(dir).is_ok() {
            if let Err(e) = std::fs::write(&path, g.to_bytes()) {
                tracing::warn!("could not write {}: {e}", path.display());
            }
        }
        Ok(g)
    }

    /// The default cache directory: `$ACREBORN_CACHE_DIR`, else
    /// `~/.cache/acreborn`.
    pub fn cache_dir() -> PathBuf {
        if let Some(d) = std::env::var_os("ACREBORN_CACHE_DIR") {
            return PathBuf::from(d);
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        home.join(".cache").join("acreborn")
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(8 + 4 + self.height_table.len() * 4 + SIDE * SIDE * 3 + 65536);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(self.height_table.len() as u32).to_le_bytes());
        for h in &self.height_table {
            out.extend_from_slice(&h.to_le_bytes());
        }
        out.extend_from_slice(&self.heights);
        for t in &self.terrain {
            out.extend_from_slice(&t.to_le_bytes());
        }
        out.extend(self.present.iter().map(|p| *p as u8));
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<WorldGrid> {
        let rest = bytes.strip_prefix(MAGIC)?;
        let n = u32::from_le_bytes(rest.get(0..4)?.try_into().ok()?) as usize;
        let mut at = 4;
        let mut height_table = Vec::with_capacity(n);
        for _ in 0..n {
            height_table.push(f32::from_le_bytes(rest.get(at..at + 4)?.try_into().ok()?));
            at += 4;
        }
        let heights = rest.get(at..at + SIDE * SIDE)?.to_vec();
        at += SIDE * SIDE;
        let terrain: Vec<u16> = rest
            .get(at..at + SIDE * SIDE * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        at += SIDE * SIDE * 2;
        let present = rest.get(at..at + 65536)?.iter().map(|b| *b != 0).collect();
        Some(WorldGrid {
            heights,
            terrain,
            height_table,
            present,
        })
    }

    /// Whether a landblock is in the archive.
    pub fn has_block(&self, bx: usize, by: usize) -> bool {
        self.present.get(bx * 256 + by).copied().unwrap_or(false)
    }

    /// Height in metres at a lattice vertex.
    pub fn height(&self, gx: usize, gy: usize) -> f32 {
        let i = self.heights[gx.min(SIDE - 1) * SIDE + gy.min(SIDE - 1)] as usize;
        self.height_table.get(i).copied().unwrap_or(0.0)
    }

    /// Terrain type (index into the region's `terrain_types`) at a vertex.
    pub fn terrain_type(&self, gx: usize, gy: usize) -> u16 {
        tbits::terrain_type(self.terrain[gx.min(SIDE - 1) * SIDE + gy.min(SIDE - 1)])
    }

    /// Road bits at a vertex (non-zero on a road).
    pub fn road(&self, gx: usize, gy: usize) -> u16 {
        tbits::road(self.terrain[gx.min(SIDE - 1) * SIDE + gy.min(SIDE - 1)])
    }

    /// World xy of a lattice vertex.
    pub fn vertex_world(gx: usize, gy: usize) -> Vec2 {
        Vec2::new(gx as f32 * SPACING, gy as f32 * SPACING)
    }

    /// The lattice vertex nearest a world xy.
    pub fn nearest_vertex(world: Vec2) -> (usize, usize) {
        let gx = (world.x / SPACING).round().clamp(0.0, (SIDE - 1) as f32) as usize;
        let gy = (world.y / SPACING).round().clamp(0.0, (SIDE - 1) as f32) as usize;
        (gx, gy)
    }

    /// Terrain height under a world xy, bilinear between the four
    /// surrounding vertices (close to the drawn mesh, not identical: the
    /// mesh splits each cell into two triangles).
    pub fn height_at(&self, world: Vec2) -> f32 {
        let fx = (world.x / SPACING).clamp(0.0, (SIDE - 1) as f32);
        let fy = (world.y / SPACING).clamp(0.0, (SIDE - 1) as f32);
        let x0 = fx.floor() as usize;
        let y0 = fy.floor() as usize;
        let (x1, y1) = ((x0 + 1).min(SIDE - 1), (y0 + 1).min(SIDE - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let h00 = self.height(x0, y0);
        let h10 = self.height(x1, y0);
        let h01 = self.height(x0, y1);
        let h11 = self.height(x1, y1);
        let a = h00 + (h10 - h00) * tx;
        let b = h01 + (h11 - h01) * tx;
        a + (b - a) * ty
    }

    /// The landblock id (`xxyy0000`) a world xy lies in.
    pub fn block_of(world: Vec2) -> u32 {
        let bx = (world.x / (SPACING * 8.0)).floor().clamp(0.0, 254.0) as u32;
        let by = (world.y / (SPACING * 8.0)).floor().clamp(0.0, 254.0) as u32;
        (bx << 24) | (by << 16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_round_trip_and_lookups() {
        let mut g = WorldGrid {
            heights: vec![0; SIDE * SIDE],
            terrain: vec![0; SIDE * SIDE],
            height_table: vec![0.0, 10.0, 20.0],
            present: vec![false; 65536],
        };
        g.heights[(8 * SIDE) + 8] = 2;
        g.heights[(9 * SIDE) + 8] = 1;
        g.terrain[(8 * SIDE) + 8] = 5 << 2;
        g.present[256 + 1] = true;
        let back = WorldGrid::from_bytes(&g.to_bytes()).unwrap();
        assert_eq!(back, g);
        assert!(back.has_block(1, 1));
        assert!(!back.has_block(0, 0));
        assert_eq!(g.height(8, 8), 20.0);
        assert_eq!(g.terrain_type(8, 8), 5);
        // Halfway between vertex (8,8) at 20 and (9,8) at 10.
        let h = g.height_at(Vec2::new(8.5 * SPACING, 8.0 * SPACING));
        assert!((h - 15.0).abs() < 1e-4, "{h}");
        assert_eq!(WorldGrid::nearest_vertex(Vec2::new(203.0, 190.0)), (8, 8));
        assert_eq!(WorldGrid::vertex_world(8, 8), Vec2::new(192.0, 192.0));
        assert_eq!(WorldGrid::block_of(Vec2::new(192.0, 384.0)), 0x0102_0000);
        assert!(WorldGrid::from_bytes(b"nope").is_none());
    }
}
