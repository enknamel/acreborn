//! The world map of Dereth as a picture: every pixel takes the colour of
//! the terrain type at the nearest [`WorldGrid`] vertex, water is blue,
//! roads are drawn as tan lines and the height field is hill-shaded with
//! light from the north-west so ranges and valleys read at a glance.
//! Landblocks missing from the archive are deep sea. A render at 8 px per
//! landblock (2040x2040) takes well under a second once the grid is
//! cached, and [`cached`] keeps the pixels in a file beside the grid.

use crate::mapimage::MapImage;
use crate::worldgrid::{WorldGrid, SIDE, SPACING};
use crate::{Assets, Result, BLOCK_SIZE};
use ac_formats::region::Region;
use glam::{Vec2, Vec3};
use std::path::Path;

/// Landblocks per side of the world.
pub const BLOCKS: u32 = 255;
/// Colour of the sea where no landblock exists.
pub const DEEP_SEA: [u8; 4] = [16, 56, 92, 255];
/// Colour of roads.
pub const ROAD: [u8; 4] = [166, 112, 52, 255];
/// The blue every water type is pulled towards.
const WATER_BLUE: [u8; 4] = [36, 104, 172, 255];
/// The map cache file: magic, width, height, origin, scale, pixels.
const MAGIC: &[u8; 8] = b"ACWMAP01";

/// How strongly slopes lighten and darken the terrain colour.
const SHADE_GAIN: f32 = 1.6;
/// Bounds of the shading multiplier.
const SHADE_MIN: f32 = 0.45;
const SHADE_MAX: f32 = 1.45;

/// The renderer's rule: a terrain type is water when the Region names it
/// so (`WaterRunning`, `WaterDeepSea`, `FauxWaterRunning`, ...).
pub fn is_water(region: &Region, terrain_type: u16) -> bool {
    region
        .terrain_types
        .get(terrain_type as usize)
        .map(|t| t.name.contains("Water"))
        .unwrap_or(false)
}

/// Whether a map pixel shows water (blue dominates), including the sea
/// of missing landblocks.
pub fn is_water_color(rgba: [u8; 4]) -> bool {
    rgba[2] > rgba[0] + 20 && rgba[2] > rgba[1]
}

/// Unpack the Region's `0xAARRGGBB` colour.
fn argb(c: u32) -> [u8; 4] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8, 255]
}

/// The map colour of a terrain type: the Region's colour, softened
/// (the DAT values are saturated swatches meant for a debug view) and
/// with water types pushed towards the sea blue.
fn terrain_color(region: &Region, terrain_type: u16) -> [u8; 4] {
    let Some(t) = region.terrain_types.get(terrain_type as usize) else {
        return DEEP_SEA;
    };
    let c = argb(t.color);
    let (r, g, b) = (c[0] as f32, c[1] as f32, c[2] as f32);
    if is_water(region, terrain_type) {
        // Region water colours run from dark deep sea to purple-ish
        // running water; pull them all towards one blue so water reads
        // as water, keeping some of their light/dark difference.
        let (sr, sg, sb) = (
            WATER_BLUE[0] as f32,
            WATER_BLUE[1] as f32,
            WATER_BLUE[2] as f32,
        );
        let k = 0.3;
        return [
            (sr + (r - sr) * k) as u8,
            (sg + (g - sg) * k) as u8,
            (sb + (b - sb) * k) as u8,
            255,
        ];
    }
    let lum = 0.299 * r + 0.587 * g + 0.114 * b;
    let k = 0.42; // keep this much of the saturation
    let m = 0.84; // and darken so highlights have headroom
    [
        ((lum + (r - lum) * k) * m) as u8,
        ((lum + (g - lum) * k) * m) as u8,
        ((lum + (b - lum) * k) * m) as u8,
        255,
    ]
}

/// Lambert shading multiplier of the terrain under a world xy, with the
/// light from the north-west at about 45 degrees.
fn shade(grid: &WorldGrid, world: Vec2) -> f32 {
    let d = SPACING * 0.5;
    let hx = grid.height_at(world + Vec2::new(d, 0.0)) - grid.height_at(world - Vec2::new(d, 0.0));
    let hy = grid.height_at(world + Vec2::new(0.0, d)) - grid.height_at(world - Vec2::new(0.0, d));
    let n = Vec3::new(-hx / (2.0 * d), -hy / (2.0 * d), 1.0).normalize();
    let light = Vec3::new(-1.0, 1.0, 1.4).normalize();
    let flat = light.z;
    let lambert = n.dot(light).max(0.0);
    (1.0 + SHADE_GAIN * (lambert - flat) / flat).clamp(SHADE_MIN, SHADE_MAX)
}

fn scaled(c: [u8; 4], m: f32) -> [u8; 4] {
    let s = |v: u8| (v as f32 * m).round().clamp(0.0, 255.0) as u8;
    [s(c[0]), s(c[1]), s(c[2]), c[3]]
}

/// Map colour and water flag of each of the 32 terrain type slots.
struct Palette {
    colors: [[u8; 4]; 32],
    water: [bool; 32],
}

impl Palette {
    fn from_region(region: &Region) -> Palette {
        let mut p = Palette {
            colors: [DEEP_SEA; 32],
            water: [false; 32],
        };
        for t in 0..32u16 {
            p.colors[t as usize] = terrain_color(region, t);
            p.water[t as usize] = is_water(region, t);
        }
        p
    }
}

/// Render the whole world at `px_per_block` pixels per landblock: origin
/// (0,0), `scale = px_per_block / 192`.
pub fn render(grid: &WorldGrid, region: &Region, px_per_block: u32) -> MapImage {
    render_with(grid, &Palette::from_region(region), px_per_block)
}

fn render_with(grid: &WorldGrid, palette: &Palette, px_per_block: u32) -> MapImage {
    let px_per_block = px_per_block.max(1);
    let scale = px_per_block as f32 / BLOCK_SIZE;
    let side = BLOCKS * px_per_block;
    let mut img = MapImage {
        width: side,
        height: side,
        rgba: vec![0; side as usize * side as usize * 4],
        origin: Vec2::ZERO,
        scale,
    };
    let Palette { colors, water } = palette;

    for py in 0..side {
        let wy = (side - py) as f32 / scale - 0.5 / scale;
        let by = (wy / BLOCK_SIZE).floor().clamp(0.0, 254.0) as usize;
        for px in 0..side {
            let wx = (px as f32 + 0.5) / scale;
            let bx = (wx / BLOCK_SIZE).floor().clamp(0.0, 254.0) as usize;
            let color = if !grid.has_block(bx, by) {
                DEEP_SEA
            } else {
                let (gx, gy) = WorldGrid::nearest_vertex(Vec2::new(wx, wy));
                let t = grid.terrain_type(gx, gy) as usize;
                if water[t] {
                    colors[t]
                } else {
                    scaled(colors[t], shade(grid, Vec2::new(wx, wy)))
                }
            };
            let i = (py as usize * side as usize + px as usize) * 4;
            img.rgba[i..i + 4].copy_from_slice(&color);
        }
    }
    draw_roads(grid, &mut img, px_per_block);
    img
}

/// Roads: every road vertex joined to its road neighbours to the east,
/// north and both diagonals, as lines of [`ROAD`] pixels. The line is
/// one pixel wide up to 4 px per block and thickens slowly above that so
/// it stays visible when the map is shown shrunk.
fn draw_roads(grid: &WorldGrid, img: &mut MapImage, px_per_block: u32) {
    let brush = (px_per_block / 4).clamp(1, 4) as i64;
    let has_road = |gx: isize, gy: isize| {
        gx >= 0
            && gy >= 0
            && (gx as usize) < SIDE
            && (gy as usize) < SIDE
            && grid.road(gx as usize, gy as usize) != 0
    };
    let (scale, height) = (img.scale, img.height as f32);
    let pixel = move |gx: isize, gy: isize| {
        let w = WorldGrid::vertex_world(gx as usize, gy as usize);
        let (px, py) = (w.x * scale, height - w.y * scale);
        // Vertex (gx, gy) sits on the corner between pixels; take the one
        // whose centre the nearest-vertex fill also assigned to it.
        (px.floor() as i64, (py - 1.0).floor().max(0.0) as i64)
    };
    for gx in 0..SIDE as isize {
        for gy in 0..SIDE as isize {
            if !has_road(gx, gy) {
                continue;
            }
            let (x0, y0) = pixel(gx, gy);
            dab(img, x0, y0, brush, ROAD);
            for (dx, dy) in [(1, 0), (0, 1), (1, 1), (-1, 1)] {
                if has_road(gx + dx, gy + dy) {
                    let (x1, y1) = pixel(gx + dx, gy + dy);
                    line(img, x0, y0, x1, y1, brush, ROAD);
                }
            }
        }
    }
}

/// A `brush` x `brush` square of pixels with (x, y) at its top-left.
fn dab(img: &mut MapImage, x: i64, y: i64, brush: i64, rgba: [u8; 4]) {
    for j in 0..brush {
        for i in 0..brush {
            img.put(x + i, y + j, rgba);
        }
    }
}

/// Bresenham line between two pixels, inclusive, `brush` pixels wide.
fn line(img: &mut MapImage, mut x0: i64, mut y0: i64, x1: i64, y1: i64, brush: i64, rgba: [u8; 4]) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        dab(img, x0, y0, brush, rgba);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

/// The cache file name for a resolution.
pub fn cache_name(px_per_block: u32) -> String {
    format!("worldmap_{px_per_block}.rgba")
}

/// The world map from `dir/worldmap_<px>.rgba`, or rendered (from the
/// grid cached in the same directory) and written there, best effort.
pub fn cached(assets: &Assets, dir: &Path, px_per_block: u32) -> Result<MapImage> {
    let path = dir.join(cache_name(px_per_block));
    if let Some(img) = std::fs::read(&path).ok().and_then(|b| from_bytes(&b)) {
        if img.width == BLOCKS * px_per_block.max(1) {
            return Ok(img);
        }
    }
    let grid = WorldGrid::load_cached(assets, dir)?;
    let region = assets.region()?;
    let img = render(&grid, &region, px_per_block);
    if std::fs::create_dir_all(dir).is_ok() {
        if let Err(e) = std::fs::write(&path, to_bytes(&img)) {
            tracing::warn!("could not write {}: {e}", path.display());
        }
    }
    Ok(img)
}

/// Serialise a map for the cache file.
pub fn to_bytes(img: &MapImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + 20 + img.rgba.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&img.width.to_le_bytes());
    out.extend_from_slice(&img.height.to_le_bytes());
    out.extend_from_slice(&img.origin.x.to_le_bytes());
    out.extend_from_slice(&img.origin.y.to_le_bytes());
    out.extend_from_slice(&img.scale.to_le_bytes());
    out.extend_from_slice(&img.rgba);
    out
}

/// Parse a cache file written by [`to_bytes`].
pub fn from_bytes(bytes: &[u8]) -> Option<MapImage> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let u = |at: usize| Some(u32::from_le_bytes(rest.get(at..at + 4)?.try_into().ok()?));
    let f = |at: usize| Some(f32::from_le_bytes(rest.get(at..at + 4)?.try_into().ok()?));
    let width = u(0)?;
    let height = u(4)?;
    let origin = Vec2::new(f(8)?, f(12)?);
    let scale = f(16)?;
    let rgba = rest.get(20..)?;
    if rgba.len() != width as usize * height as usize * 4 || scale <= 0.0 {
        return None;
    }
    Some(MapImage {
        width,
        height,
        rgba: rgba.to_vec(),
        origin,
        scale,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_round_trip() {
        let mut img = MapImage::blank(Vec2::new(1.0, 2.0), Vec2::new(4.0, 3.0), 1.0);
        img.put(1, 1, [9, 8, 7, 255]);
        let back = from_bytes(&to_bytes(&img)).unwrap();
        assert_eq!(back, img);
        assert!(from_bytes(b"ACWMAP01junk").is_none());
        let mut short = to_bytes(&img);
        short.pop();
        assert!(from_bytes(&short).is_none());
    }

    #[test]
    fn water_colours_are_blue() {
        assert!(is_water_color(DEEP_SEA));
        assert!(!is_water_color(ROAD));
        assert!(!is_water_color([60, 120, 60, 255]));
        assert_eq!(argb(0xFF0A_4664), [10, 70, 100, 255]);
    }

    #[test]
    fn lines_are_inclusive_and_connected() {
        let mut img = MapImage::blank(Vec2::ZERO, Vec2::new(8.0, 8.0), 1.0);
        line(&mut img, 0, 0, 5, 2, 1, ROAD);
        assert_eq!(img.get(0, 0), Some(ROAD));
        assert_eq!(img.get(5, 2), Some(ROAD));
        let n = (0..8)
            .flat_map(|y| (0..8).map(move |x| (x, y)))
            .filter(|&(x, y)| img.get(x, y) == Some(ROAD))
            .count();
        assert_eq!(n, 6);
    }

    #[test]
    fn missing_world_is_sea_with_a_shaded_island() {
        // A grid with one landblock: a ridge running north-south so the
        // west face lights up and the east face darkens.
        let mut g = WorldGrid {
            heights: vec![0; SIDE * SIDE],
            terrain: vec![0; SIDE * SIDE],
            height_table: (0..256).map(|i| i as f32 * 2.0).collect(),
            present: vec![false; 65536],
        };
        g.present[256 + 1] = true;
        for x in 8..=16 {
            for y in 8..=16 {
                g.heights[x * SIDE + y] = if x == 12 { 20 } else { 0 };
                g.terrain[x * SIDE + y] = 1 << 2 | if y == 12 { 1 } else { 0 };
            }
        }
        let mut palette = Palette {
            colors: [[60, 120, 60, 255]; 32],
            water: [false; 32],
        };
        palette.colors[0] = DEEP_SEA;
        palette.water[0] = true;
        let img = render_with(&g, &palette, 8);
        assert_eq!((img.width, img.height), (2040, 2040));
        assert_eq!(img.get(0, 0), Some(DEEP_SEA));
        assert_eq!(img.get(2039, 2039), Some(DEEP_SEA));
        // A row off the road (vertex y = 10), west and east of the ridge
        // at vertex x = 12 (world x = 288).
        let p = img.to_pixel(Vec2::new(288.0 - 24.0, 252.0));
        let west = img.get(p.x as i64, p.y as i64).unwrap();
        let p = img.to_pixel(Vec2::new(288.0 + 24.0, 252.0));
        let east = img.get(p.x as i64, p.y as i64).unwrap();
        assert!(!is_water_color(west) && !is_water_color(east));
        assert!(
            west[1] > east[1],
            "west {west:?} lit, east {east:?} in shadow"
        );
        // The road along y = 12 vertices shows as ROAD pixels.
        let p = img.to_pixel(WorldGrid::vertex_world(10, 12));
        assert_eq!(img.get(p.x as i64, p.y as i64 - 1), Some(ROAD));
    }
}
