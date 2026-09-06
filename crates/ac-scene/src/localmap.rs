//! A landblock's local map: the floor plan of a dungeon, or a top-down
//! view of an outdoor block's terrain with its buildings and objects.
//!
//! Both are drawn from the same geometry the player walks on (the
//! [`CollisionWorld`]) and the terrain mesh, so what the map shows agrees
//! with what blocks the character. A dungeon plan is sized from its
//! geometry (dungeon cells extend well outside the block's 192 m square);
//! an outdoor map is exactly the block square.

use glam::{Vec2, Vec3};

use crate::collision::{CollisionWorld, Tri};
use crate::mapimage::MapImage;
use crate::{landblock, lbid, Assets, Result, BLOCK_SIZE, CELLS_PER_BLOCK, VERTS_PER_SIDE};

/// A rendered local map of one landblock.
#[derive(Clone, Debug)]
pub struct LocalMap {
    pub image: MapImage,
    /// A dungeon floor plan rather than an outdoor block.
    pub dungeon: bool,
    /// z range the floors span (every floor triangle of the block, not
    /// only those inside the `z_range` that was drawn), so a panel knows
    /// which storeys exist. Zero for an outdoor block without floors.
    pub z_min: f32,
    pub z_max: f32,
}

/// Triangles flatter than this (normal z) are floors and roofs; steeper
/// than [`STEEP`] are walls.
const FLOOR: f32 = 0.5;
const STEEP: f32 = 0.5;
/// Metres of clear space around a dungeon's geometry.
const DUNGEON_MARGIN: f32 = 4.0;

/// Render the local map of `block` (a block or cell id; the low 16 bits
/// are ignored) at `px_per_metre`.
///
/// A dungeon block gives a floor plan: floors filled and shaded by height
/// (low = dark, high = light), walls as dark outlines, over a transparent
/// background. With `z_range = Some((lo, hi))` only floors whose mean z is
/// within it (and walls crossing it) are drawn, so overlapping storeys do
/// not smear together; with `None` every floor is drawn, highest last.
///
/// An outdoor block gives the 192 m square: terrain coloured by type with
/// hill shading (light from the north-west), roads in tan, water in blue
/// where the water surface covers the ground, then the footprints of
/// buildings and objects (floors grey, walls dark). `z_range` is ignored
/// outdoors.
pub fn render(
    assets: &Assets,
    block: u32,
    px_per_metre: f32,
    z_range: Option<(f32, f32)>,
) -> Result<LocalMap> {
    let block = block & 0xFFFF_0000;
    let scene = landblock::load(assets, block)?;
    let world = CollisionWorld::from_scene(assets, &scene)?;
    let (z_min, z_max) = floor_span(&world.tris);
    if scene.is_dungeon {
        let image = dungeon_plan(&world.tris, px_per_metre, z_range, z_min, z_max);
        return Ok(LocalMap {
            image,
            dungeon: true,
            z_min,
            z_max,
        });
    }
    let origin = lbid::world_origin(block);
    let mut image = MapImage::blank(
        Vec2::new(origin.x, origin.y),
        Vec2::splat(BLOCK_SIZE),
        px_per_metre,
    );
    let region = assets.region()?;
    draw_terrain(&mut image, &region, &scene.terrain, origin);
    draw_footprints(&mut image, &world.tris);
    Ok(LocalMap {
        image,
        dungeon: false,
        z_min,
        z_max,
    })
}

/// z range of all floor triangles' centres.
fn floor_span(tris: &[Tri]) -> (f32, f32) {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for t in tris.iter().filter(|t| t.normal.z > FLOOR) {
        let z = centre_z(t);
        lo = lo.min(z);
        hi = hi.max(z);
    }
    if lo > hi {
        (0.0, 0.0)
    } else {
        (lo, hi)
    }
}

fn centre_z(t: &Tri) -> f32 {
    (t.a.z + t.b.z + t.c.z) / 3.0
}

fn xy(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.y)
}

/// The colour of a dungeon floor at height fraction `t` (0 = lowest,
/// 1 = highest): a blue-grey ramp, darker low down.
fn floor_colour(t: f32) -> [u8; 4] {
    let t = t.clamp(0.0, 1.0);
    let lo = [70.0, 80.0, 100.0];
    let hi = [190.0, 200.0, 220.0];
    [
        (lo[0] + (hi[0] - lo[0]) * t) as u8,
        (lo[1] + (hi[1] - lo[1]) * t) as u8,
        (lo[2] + (hi[2] - lo[2]) * t) as u8,
        255,
    ]
}

const WALL: [u8; 4] = [24, 24, 32, 255];
const FOOTPRINT: [u8; 4] = [150, 150, 150, 255];
const FOOTPRINT_WALL: [u8; 4] = [40, 40, 40, 255];
const ROAD: [u8; 4] = [196, 168, 120, 255];

fn dungeon_plan(
    tris: &[Tri],
    scale: f32,
    z_range: Option<(f32, f32)>,
    z_min: f32,
    z_max: f32,
) -> MapImage {
    let mut lo = Vec2::splat(f32::INFINITY);
    let mut hi = Vec2::splat(f32::NEG_INFINITY);
    for t in tris {
        for p in [t.a, t.b, t.c] {
            lo = lo.min(xy(p));
            hi = hi.max(xy(p));
        }
    }
    if lo.x > hi.x {
        return MapImage::blank(Vec2::ZERO, Vec2::ONE, scale);
    }
    let margin = Vec2::splat(DUNGEON_MARGIN);
    let mut image = MapImage::blank(lo - margin, hi - lo + margin * 2.0, scale);

    // Floors, lowest first so a higher storey paints over a lower one.
    let in_range = |z: f32| match z_range {
        Some((a, b)) => z >= a && z <= b,
        None => true,
    };
    let mut floors: Vec<&Tri> = tris
        .iter()
        .filter(|t| t.normal.z > FLOOR && in_range(centre_z(t)))
        .collect();
    floors.sort_by(|a, b| centre_z(a).total_cmp(&centre_z(b)));
    let span = (z_max - z_min).max(1.0);
    for t in floors {
        let colour = floor_colour((centre_z(t) - z_min) / span);
        image.fill_world_tri(xy(t.a), xy(t.b), xy(t.c), colour);
    }

    // Walls crossing the z range, as outlines along their projected edges.
    let crosses = |t: &Tri| match z_range {
        Some((a, b)) => {
            let lo = t.a.z.min(t.b.z).min(t.c.z);
            let hi = t.a.z.max(t.b.z).max(t.c.z);
            hi >= a && lo <= b
        }
        None => true,
    };
    for t in tris
        .iter()
        .filter(|t| t.normal.z.abs() < STEEP && crosses(t))
    {
        draw_wall(&mut image, t, WALL);
    }
    image
}

/// A steep triangle seen from above is a sliver: draw its three edges.
fn draw_wall(image: &mut MapImage, t: &Tri, colour: [u8; 4]) {
    line(image, xy(t.a), xy(t.b), colour);
    line(image, xy(t.b), xy(t.c), colour);
    line(image, xy(t.c), xy(t.a), colour);
}

/// A one-pixel line between two world points.
fn line(image: &mut MapImage, a: Vec2, b: Vec2, colour: [u8; 4]) {
    let p = image.to_pixel(a);
    let q = image.to_pixel(b);
    let d = q - p;
    let steps = d.x.abs().max(d.y.abs()).ceil().max(1.0);
    let step = d / steps;
    let mut at = p;
    for _ in 0..=steps as u32 {
        image.put(at.x.floor() as i64, at.y.floor() as i64, colour);
        at += step;
    }
}

/// Fill the pixels within `radius` metres of the segment `a`-`b`.
fn thick_line(image: &mut MapImage, a: Vec2, b: Vec2, radius: f32, colour: [u8; 4]) {
    let p = image.to_pixel(a);
    let q = image.to_pixel(b);
    let r = radius * image.scale;
    let x0 = (p.x.min(q.x) - r).floor() as i64;
    let x1 = (p.x.max(q.x) + r).ceil() as i64;
    let y0 = (p.y.min(q.y) - r).floor() as i64;
    let y1 = (p.y.max(q.y) + r).ceil() as i64;
    let d = q - p;
    let len2 = d.length_squared();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let c = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            let t = if len2 > 0.0 {
                ((c - p).dot(d) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            if (c - (p + d * t)).length() <= r {
                image.put(x, y, colour);
            }
        }
    }
}

/// Fill a world triangle, shading each pixel by barycentric interpolation
/// of the corners' values through `pixel(w0, w1, w2)`, which returns the
/// colour or `None` to leave the pixel alone.
fn fill_tri_with(
    image: &mut MapImage,
    a: Vec2,
    b: Vec2,
    c: Vec2,
    mut pixel: impl FnMut(f32, f32, f32) -> Option<[u8; 4]>,
) {
    let (a, b, c) = (image.to_pixel(a), image.to_pixel(b), image.to_pixel(c));
    let x0 = a.x.min(b.x).min(c.x).floor() as i64;
    let x1 = a.x.max(b.x).max(c.x).ceil() as i64;
    let y0 = a.y.min(b.y).min(c.y).floor() as i64;
    let y1 = a.y.max(b.y).max(c.y).ceil() as i64;
    let edge = |p: Vec2, q: Vec2, r: Vec2| (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
    let area = edge(a, b, c);
    if area.abs() < 1e-9 {
        return;
    }
    for y in y0..=y1 {
        for x in x0..=x1 {
            let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            let w0 = edge(b, c, p) / area;
            let w1 = edge(c, a, p) / area;
            let w2 = edge(a, b, p) / area;
            if w0 >= -1e-4 && w1 >= -1e-4 && w2 >= -1e-4 {
                if let Some(rgba) = pixel(w0, w1, w2) {
                    image.put(x, y, rgba);
                }
            }
        }
    }
}

fn is_water(region: &ac_formats::region::Region, terrain_type: u16) -> bool {
    region
        .terrain_types
        .get(terrain_type as usize)
        .map(|t| t.name.contains("Water"))
        .unwrap_or(false)
}

/// The Region's map colour of a terrain type (`0xAARRGGBB`), softened a
/// little towards grey so the saturated palette does not shout.
fn terrain_colour(region: &ac_formats::region::Region, terrain_type: u16) -> [f32; 3] {
    let c = region
        .terrain_types
        .get(terrain_type as usize)
        .map(|t| t.color)
        .unwrap_or(0x00FF_00FF);
    let soften = |v: u32| v as f32 * 0.8 + 140.0 * 0.2;
    [
        soften((c >> 16) & 0xFF),
        soften((c >> 8) & 0xFF),
        soften(c & 0xFF),
    ]
}

/// Height of the water surface the viewer draws over `terrain` cell
/// `(x, y)`, if any of its corners is water: the same rule as the
/// renderer (the lowest wet corner, lifted a little).
fn water_level(
    region: &ac_formats::region::Region,
    terrain: &crate::terrain::TerrainMesh,
    corners: [usize; 4],
) -> Option<f32> {
    corners
        .iter()
        .map(|&i| &terrain.vertices[i])
        .filter(|v| is_water(region, v.terrain_type))
        .map(|v| v.position.z)
        .reduce(f32::min)
        .map(|z| z + 0.15)
}

fn draw_terrain(
    image: &mut MapImage,
    region: &ac_formats::region::Region,
    terrain: &crate::terrain::TerrainMesh,
    origin: Vec3,
) {
    let n = VERTS_PER_SIDE;
    let idx = |x: usize, y: usize| x * n + y;
    // Light from the north-west, well above the horizon.
    let light = Vec3::new(-1.0, 1.0, 1.6).normalize();
    let flat = light.z;
    let shade_of = |v: &crate::terrain::TerrainVertex| {
        let d = v.normal.normalize_or_zero().dot(light);
        // Flat ground at 1.0; slopes facing the light brighter, away darker.
        (1.0 + (d - flat) * 1.6).clamp(0.45, 1.35)
    };
    let cells = CELLS_PER_BLOCK as usize;
    for cx in 0..cells {
        for cy in 0..cells {
            let cell = cx * cells + cy;
            let corners = [
                idx(cx, cy),
                idx(cx + 1, cy),
                idx(cx + 1, cy + 1),
                idx(cx, cy + 1),
            ];
            let base = terrain_colour(region, terrain.cell_types[cell]);
            let water = water_level(region, terrain, corners);
            let water_colour = corners
                .iter()
                .map(|&i| terrain.vertices[i].terrain_type)
                .find(|&t| is_water(region, t))
                .map(|t| terrain_colour(region, t));
            for tri in 0..2 {
                let i = cell * 6 + tri * 3;
                let v = [
                    &terrain.vertices[terrain.indices[i] as usize],
                    &terrain.vertices[terrain.indices[i + 1] as usize],
                    &terrain.vertices[terrain.indices[i + 2] as usize],
                ];
                let p = [
                    xy(v[0].position + origin),
                    xy(v[1].position + origin),
                    xy(v[2].position + origin),
                ];
                let s = [shade_of(v[0]), shade_of(v[1]), shade_of(v[2])];
                let z = [v[0].position.z, v[1].position.z, v[2].position.z];
                fill_tri_with(image, p[0], p[1], p[2], |w0, w1, w2| {
                    let h = z[0] * w0 + z[1] * w1 + z[2] * w2;
                    let (colour, shade) = match (water, water_colour) {
                        (Some(level), Some(wc)) if h <= level => (wc, 1.0),
                        _ => (base, s[0] * w0 + s[1] * w1 + s[2] * w2),
                    };
                    Some([
                        (colour[0] * shade).clamp(0.0, 255.0) as u8,
                        (colour[1] * shade).clamp(0.0, 255.0) as u8,
                        (colour[2] * shade).clamp(0.0, 255.0) as u8,
                        255,
                    ])
                });
            }
        }
    }
    // Roads run between road-flagged corners of a cell (edges and
    // diagonals alike).
    for cx in 0..cells {
        for cy in 0..cells {
            let corners = [
                idx(cx, cy),
                idx(cx + 1, cy),
                idx(cx + 1, cy + 1),
                idx(cx, cy + 1),
            ];
            let road: Vec<Vec2> = corners
                .iter()
                .map(|&i| &terrain.vertices[i])
                .filter(|v| v.road != 0)
                .map(|v| xy(v.position + origin))
                .collect();
            for (i, &a) in road.iter().enumerate() {
                for &b in &road[i + 1..] {
                    thick_line(image, a, b, 3.0, ROAD);
                }
            }
        }
    }
}

/// Buildings and objects seen from above: their floors and roofs grey,
/// their walls as dark outlines.
fn draw_footprints(image: &mut MapImage, tris: &[Tri]) {
    let mut floors: Vec<&Tri> = tris.iter().filter(|t| t.normal.z > FLOOR).collect();
    floors.sort_by(|a, b| centre_z(a).total_cmp(&centre_z(b)));
    for t in floors {
        image.fill_world_tri(xy(t.a), xy(t.b), xy(t.c), FOOTPRINT);
    }
    for t in tris.iter().filter(|t| t.normal.z.abs() < STEEP) {
        draw_wall(image, t, FOOTPRINT_WALL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(a: Vec3, b: Vec3, c: Vec3, cell: u32) -> Tri {
        let normal = (b - a).cross(c - a).normalize();
        Tri {
            a,
            b,
            c,
            normal,
            cell,
            two_sided: false,
        }
    }

    #[test]
    fn floor_plan_is_sized_from_the_geometry_and_filters_by_z() {
        // A 10 m floor at z = 0 with a negative-x corner, another at z = 10.
        let mut tris = vec![
            tri(
                Vec3::new(-10.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 10.0, 0.0),
                1,
            ),
            tri(
                Vec3::new(20.0, 0.0, 10.0),
                Vec3::new(30.0, 0.0, 10.0),
                Vec3::new(30.0, 10.0, 10.0),
                2,
            ),
        ];
        // A wall from z 0 to 3 along x = 0.
        tris.push(tri(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::new(0.0, 10.0, 3.0),
            1,
        ));
        let (lo, hi) = floor_span(&tris);
        assert_eq!((lo, hi), (0.0, 10.0));
        let plan = dungeon_plan(&tris, 2.0, None, lo, hi);
        // Bounds -10..30 by 0..10 plus a 4 m margin, at 2 px/m.
        assert_eq!(plan.origin, Vec2::new(-14.0, -4.0));
        assert_eq!((plan.width, plan.height), (96, 36));
        let low = plan.to_pixel(Vec2::new(-7.0, 2.0));
        let high = plan.to_pixel(Vec2::new(27.0, 2.0));
        let at = |m: &MapImage, p: Vec2| m.get(p.x as i64, p.y as i64).unwrap();
        assert_eq!(at(&plan, low)[3], 255);
        assert_eq!(at(&plan, high)[3], 255);
        // The higher floor is lighter.
        assert!(at(&plan, high)[0] > at(&plan, low)[0]);
        // Wall outline at x = 0.
        let wall = plan.to_pixel(Vec2::new(0.0, 5.0));
        assert_eq!(at(&plan, wall), WALL);

        let upper = dungeon_plan(&tris, 2.0, Some((8.0, 12.0)), lo, hi);
        assert_eq!(at(&upper, low)[3], 0);
        assert_eq!(at(&upper, high)[3], 255);
        // The wall does not reach z 8, so it is not drawn either.
        assert_eq!(at(&upper, wall)[3], 0);
    }

    #[test]
    fn empty_geometry_gives_a_pixel() {
        let plan = dungeon_plan(&[], 2.0, None, 0.0, 0.0);
        assert_eq!((plan.width, plan.height), (2, 2));
    }

    #[test]
    fn thick_lines_and_shaded_fills_stay_in_bounds() {
        let mut m = MapImage::blank(Vec2::ZERO, Vec2::new(10.0, 10.0), 1.0);
        thick_line(
            &mut m,
            Vec2::new(-5.0, 5.0),
            Vec2::new(15.0, 5.0),
            1.0,
            ROAD,
        );
        assert_eq!(m.get(5, 5), Some(ROAD));
        assert_eq!(m.get(5, 8), Some([0, 0, 0, 0]));
        fill_tri_with(
            &mut m,
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(0.0, 10.0),
            |w0, _, _| (w0 > 0.5).then_some([1, 2, 3, 255]),
        );
        assert_eq!(m.get(1, 9), Some([1, 2, 3, 255]));
        assert_eq!(m.get(9, 1), Some([0, 0, 0, 0]));
    }
}
