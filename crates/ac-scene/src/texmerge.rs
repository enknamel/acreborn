//! Outdoor terrain texturing: which textures a landscape cell is painted
//! with, following the client's `TexMerge` (Region 0x13 `land_surf_type`
//! 0). Each 24 m cell has a terrain type and road bits at its four
//! corners. One corner's type is the *base* texture; the others are
//! painted over it through *alpha maps* (corner and side shaped masks,
//! black where the overlay shows), rotated to fit the corners they must
//! cover; roads go on top the same way. The client bakes this into one
//! texture per distinct corner combination; here the recipe is returned so
//! a renderer can blend the layers itself.

use ac_formats::region::{Region, TexMerge};

/// Terrain type index (`TerrainType::RoadType`) of the road texture in
/// `TexMerge::terrain_desc`.
pub const ROAD_TYPE: u32 = 0x20;

/// Rotation of an alpha map, in quarter turns anticlockwise when seen from
/// above; see [`Tables::cell_surface`].
pub type Rotation = u8;

/// One texture painted over the base through an alpha map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overlay {
    /// Texture layer (index into [`Tables::texture_ids`]).
    pub texture: u8,
    /// Alpha map layer (index into [`Tables::alpha_ids`]).
    pub alpha: u8,
    pub rotation: Rotation,
}

/// How to paint one cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellSurface {
    /// Texture layer of the base texture.
    pub base: u8,
    /// Times the base texture repeats across the cell.
    pub tiling: u8,
    /// Terrain overlays, painted in order.
    pub overlays: [Option<Overlay>; 3],
    /// Road overlays, painted last (both use the road texture).
    pub roads: [Option<Overlay>; 2],
}

/// Alpha map lookups derived from the Region's `TexMerge`.
#[derive(Debug, Clone)]
pub struct Tables {
    /// SurfaceTexture ids of the texture layers, in `terrain_desc` order.
    pub texture_ids: Vec<u32>,
    /// SurfaceTexture ids of the alpha layers: corner maps, then side maps,
    /// then road maps.
    pub alpha_ids: Vec<u32>,
    /// Layer + tiling per `terrain_desc` entry, keyed by terrain type.
    types: Vec<(u32, u8, u8)>,
    /// `(tcode, alpha layer)`.
    corner_maps: Vec<(u32, u8)>,
    side_maps: Vec<(u32, u8)>,
    /// `(rcode, alpha layer)`.
    road_maps: Vec<(u32, u8)>,
}

impl Tables {
    /// None when the region does not merge textures (`land_surf_type` 1).
    pub fn from_region(region: &Region) -> Option<Self> {
        region.tex_merge.as_ref().map(Self::from_tex_merge)
    }

    pub fn from_tex_merge(tm: &TexMerge) -> Self {
        let mut alpha_ids = Vec::new();
        let mut maps = |list: &[(u32, u32)]| -> Vec<(u32, u8)> {
            list.iter()
                .map(|&(code, tex)| {
                    let layer = alpha_ids.len() as u8;
                    alpha_ids.push(tex);
                    (code, layer)
                })
                .collect()
        };
        let corner_maps = maps(&tm.corner_terrain_maps);
        let side_maps = maps(&tm.side_terrain_maps);
        let road_maps = maps(&tm.road_maps);
        Tables {
            texture_ids: tm.terrain_desc.iter().map(|(_, t)| t.tex_gid).collect(),
            alpha_ids,
            types: tm
                .terrain_desc
                .iter()
                .enumerate()
                .map(|(i, (ty, t))| (*ty, i as u8, t.tex_tiling.clamp(1, 255) as u8))
                .collect(),
            corner_maps,
            side_maps,
            road_maps,
        }
    }

    /// Texture layer and tiling for a terrain type (the first entry when
    /// the type has none, as the client does).
    fn texture(&self, terrain_type: u32) -> (u8, u8) {
        self.types
            .iter()
            .find(|(t, _, _)| *t == terrain_type)
            .or(self.types.first())
            .map(|&(_, layer, tiling)| (layer, tiling))
            .unwrap_or((0, 1))
    }

    /// The recipe for a cell with the given palette code.
    pub fn cell_surface(&self, pcode: u32) -> CellSurface {
        let (road_layer, road_tiling) = self.texture(ROAD_TYPE);
        let (rcodes, all_road) = road_codes(pcode);
        if all_road {
            return CellSurface {
                base: road_layer,
                tiling: road_tiling,
                ..Default::default()
            };
        }
        let (types, tcodes) = terrain_codes(pcode);
        let (base, tiling) = self.texture(types[0]);
        let mut surface = CellSurface {
            base,
            tiling,
            ..Default::default()
        };
        for (i, &tcode) in tcodes.iter().enumerate() {
            if tcode == 0 {
                break;
            }
            let Some((alpha, rotation)) = self.terrain_alpha(pcode, tcode) else {
                break;
            };
            surface.overlays[i] = Some(Overlay {
                texture: self.texture(types[i + 1]).0,
                alpha,
                rotation,
            });
        }
        for (i, &rcode) in rcodes.iter().enumerate() {
            if rcode == 0 {
                break;
            }
            let Some((alpha, rotation)) = self.road_alpha(pcode, rcode) else {
                break;
            };
            surface.roads[i] = Some(Overlay {
                texture: road_layer,
                alpha,
                rotation,
            });
        }
        surface
    }

    /// The alpha map for a terrain tcode: a pseudo-random pick from the
    /// corner or side maps, rotated until its code matches.
    fn terrain_alpha(&self, pcode: u32, tcode: u32) -> Option<(u8, Rotation)> {
        let maps = if matches!(tcode, 1 | 2 | 4 | 8) {
            &self.corner_maps
        } else {
            &self.side_maps
        };
        let n = maps.len();
        if n == 0 {
            return None;
        }
        let mut pick = (prng(pcode) * n as f32).floor() as usize;
        if pick >= n {
            pick = 0;
        }
        let (code, layer) = maps[pick];
        rotation_to(code, tcode).map(|r| (layer, r))
    }

    /// The alpha map for a road rcode: the first map, from a pseudo-random
    /// start, that matches under some rotation.
    fn road_alpha(&self, pcode: u32, rcode: u32) -> Option<(u8, Rotation)> {
        let n = self.road_maps.len();
        if n == 0 {
            return None;
        }
        let start = (n as f32 * prng(pcode)).floor() as usize;
        (0..n).find_map(|i| {
            let (code, layer) = self.road_maps[(i + start) % n];
            rotation_to(code, rcode).map(|r| (layer, r))
        })
    }
}

/// The client's per-cell hash in [0, 1): `(pcode * 0x523aa99e - 0x51c9e74a)`
/// as an unsigned 32-bit value scaled by 2^-32, in single precision.
fn prng(pcode: u32) -> f32 {
    let v = pcode.wrapping_mul(0x523a_a99e).wrapping_sub(0x51c9_e74a);
    v as f32 * 2.328_306_4e-10
}

/// Quarter turns that carry an alpha map's corner code onto `wanted`: each
/// turn moves a corner bit SW -> SE -> NE -> NW -> SW (bit 3 wraps to 0).
fn rotation_to(mut code: u32, wanted: u32) -> Option<Rotation> {
    for r in 0..4 {
        if code == wanted {
            return Some(r);
        }
        code *= 2;
        if code >= 16 {
            code -= 15;
        }
    }
    None
}

/// Palette code of a cell from its corners in SW, SE, NE, NW order:
/// `(terrain type, road bits)` each.
pub fn pal_code(corners: [(u16, u16); 4]) -> u32 {
    let [(t1, r1), (t2, r2), (t3, r3), (t4, r4)] = corners;
    let terrain = (t1 as u32 & 0x1F) << 15
        | (t2 as u32 & 0x1F) << 10
        | (t3 as u32 & 0x1F) << 5
        | (t4 as u32 & 0x1F);
    let road = (r1 as u32 & 3) << 26
        | (r2 as u32 & 3) << 24
        | (r3 as u32 & 3) << 22
        | (r4 as u32 & 3) << 20;
    // The size bits (1 << 28) are part of the code the client hashes.
    1 << 28 | road | terrain
}

/// Terrain types of the four corners (SW, SE, NE, NW) from a palette code.
fn corner_types(pcode: u32) -> [u32; 4] {
    [
        (pcode >> 15) & 0x1F,
        (pcode >> 10) & 0x1F,
        (pcode >> 5) & 0x1F,
        pcode & 0x1F,
    ]
}

/// Which terrain types a cell paints and through which corner masks:
/// `types[0]` is the base, `types[1 + i]` is painted where the mask
/// `tcodes[i]` (bit per corner: 1 SW, 2 SE, 4 NE, 8 NW; 0 ends the list)
/// is black. Two adjacent corners of the same type share one side mask.
fn terrain_codes(pcode: u32) -> ([u32; 4], [u32; 3]) {
    let corners = corner_types(pcode);
    let mut types = [0u32; 4];
    let mut tcodes = [0u32; 3];
    // The base is the first type that appears at two corners.
    let base = (0..4).find(|&i| (i + 1..4).any(|j| corners[i] == corners[j]));
    let Some(base) = base else {
        // All four differ: the SW type is the base, the rest are corners.
        return (corners, [2, 4, 8]);
    };
    types[0] = corners[base];
    let mut second = None;
    for (k, &t) in corners.iter().enumerate() {
        if t == types[0] {
            continue;
        }
        if tcodes[0] == 0 {
            tcodes[0] = 1 << k;
            types[1] = t;
            second = Some(t);
        } else {
            if second == Some(t) && k > 0 && tcodes[0] == 1 << (k - 1) {
                tcodes[0] += 1 << k;
            } else {
                types[2] = t;
                tcodes[1] = 1 << k;
            }
            break;
        }
    }
    (types, tcodes)
}

/// Road masks of a cell: up to two `(bit per corner)` codes for the road
/// alpha maps, or `all_road` when every corner carries road.
fn road_codes(pcode: u32) -> ([u32; 2], bool) {
    let mut mask = 0;
    if pcode & 0x0C00_0000 != 0 {
        mask |= 1;
    }
    if pcode & 0x0300_0000 != 0 {
        mask |= 2;
    }
    if pcode & 0x00C0_0000 != 0 {
        mask |= 4;
    }
    if pcode & 0x0030_0000 != 0 {
        mask |= 8;
    }
    match mask {
        0xF => ([0, 0], true),
        0xE => ([6, 12], false),
        0xD => ([9, 12], false),
        0xB => ([9, 3], false),
        0x7 => ([3, 6], false),
        m => ([m, 0], false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_formats::region::TerrainTex;

    fn tables() -> Tables {
        let tex = |id| TerrainTex {
            tex_gid: id,
            tex_tiling: 2,
            max_vert_bright: 0,
            min_vert_bright: 0,
            max_vert_saturate: 0,
            min_vert_saturate: 0,
            max_vert_hue: 0,
            min_vert_hue: 0,
            detail_tex_tiling: 1,
            detail_tex_gid: 0,
        };
        Tables::from_tex_merge(&TexMerge {
            base_tex_size: 1024,
            corner_terrain_maps: vec![(8, 0x100), (8, 0x101), (8, 0x102), (8, 0x103)],
            side_terrain_maps: vec![(9, 0x104)],
            road_maps: vec![(9, 0x105), (10, 0x106), (8, 0x107)],
            terrain_desc: vec![
                (0, tex(0x200)),
                (1, tex(0x201)),
                (3, tex(0x203)),
                (ROAD_TYPE, tex(0x220)),
            ],
        })
    }

    #[test]
    fn layers_follow_region_order() {
        let t = tables();
        assert_eq!(t.texture_ids, [0x200, 0x201, 0x203, 0x220]);
        assert_eq!(t.alpha_ids.len(), 8);
        assert_eq!(t.alpha_ids[4], 0x104);
        assert_eq!(t.alpha_ids[7], 0x107);
    }

    #[test]
    fn uniform_cell_has_no_overlays() {
        let s = tables().cell_surface(pal_code([(1, 0); 4]));
        assert_eq!(s.base, 1);
        assert_eq!(s.tiling, 2);
        assert!(s.overlays.iter().all(Option::is_none));
        assert!(s.roads.iter().all(Option::is_none));
    }

    #[test]
    fn single_corner_uses_rotated_corner_map() {
        // NE corner differs: mask 4, the NW-shaped (8) maps need 3 turns.
        let s = tables().cell_surface(pal_code([(1, 0), (1, 0), (3, 0), (1, 0)]));
        assert_eq!(s.base, 1);
        let o = s.overlays[0].unwrap();
        assert_eq!(o.texture, 2);
        assert!(o.alpha < 4);
        assert_eq!(o.rotation, 3);
        assert!(s.overlays[1].is_none());
    }

    #[test]
    fn adjacent_corners_share_a_side_map() {
        // SE + NE differ: side code 6 = the west map (9) turned twice.
        let s = tables().cell_surface(pal_code([(0, 0), (1, 0), (1, 0), (0, 0)]));
        assert_eq!(s.base, 0);
        let o = s.overlays[0].unwrap();
        assert_eq!((o.texture, o.alpha, o.rotation), (1, 4, 2));
        assert!(s.overlays[1].is_none());
        // NE + NW differ: code 12, three turns.
        let s = tables().cell_surface(pal_code([(1, 0), (1, 0), (0, 0), (0, 0)]));
        let o = s.overlays[0].unwrap();
        assert_eq!((o.texture, o.alpha, o.rotation), (0, 4, 3));
    }

    #[test]
    fn opposite_corners_are_two_overlays() {
        let s = tables().cell_surface(pal_code([(1, 0), (0, 0), (1, 0), (0, 0)]));
        let a = s.overlays[0].unwrap();
        let b = s.overlays[1].unwrap();
        // SE (2) is the NW map turned twice; NW (8) is it as drawn.
        assert_eq!((a.texture, a.rotation), (0, 2));
        assert_eq!((b.texture, b.rotation), (0, 0));
    }

    #[test]
    fn four_types_paint_three_corners() {
        let (types, tcodes) = terrain_codes(pal_code([(0, 0), (1, 0), (3, 0), (5, 0)]));
        assert_eq!(types, [0, 1, 3, 5]);
        assert_eq!(tcodes, [2, 4, 8]);
    }

    #[test]
    fn roads() {
        assert_eq!(road_codes(pal_code([(0, 1); 4])), ([0, 0], true));
        assert_eq!(
            road_codes(pal_code([(0, 1), (0, 1), (0, 0), (0, 0)])),
            ([3, 0], false)
        );
        assert_eq!(
            road_codes(pal_code([(0, 1), (0, 1), (0, 1), (0, 0)])),
            ([3, 6], false)
        );
        assert_eq!(
            road_codes(pal_code([(0, 2), (0, 0), (0, 2), (0, 0)])),
            ([5, 0], false)
        );
        let t = tables();
        // Every corner road: the road texture is the base.
        assert_eq!(t.cell_surface(pal_code([(1, 1); 4])).base, 3);
        // West side road: rcode 9, the west map as is.
        let s = t.cell_surface(pal_code([(1, 1), (1, 0), (1, 0), (1, 1)]));
        let r = s.roads[0].unwrap();
        assert_eq!((r.texture, r.alpha, r.rotation), (3, 5, 0));
        assert!(s.roads[1].is_none());
        // Diagonal SW-NE (5): the NW-SE map (10) turned once.
        let s = t.cell_surface(pal_code([(1, 1), (1, 0), (1, 1), (1, 0)]));
        let r = s.roads[0].unwrap();
        assert_eq!((r.alpha, r.rotation), (6, 1));
        // Three corners: two side maps.
        let s = t.cell_surface(pal_code([(1, 1), (1, 1), (1, 1), (1, 0)]));
        assert_eq!(s.roads[0].unwrap().rotation, 1); // 3 = 9 turned once
        assert_eq!(s.roads[1].unwrap().rotation, 2); // 6 = 9 turned twice
    }

    #[test]
    fn hash_matches_client_arithmetic() {
        // pcode * 0x523aa99e - 0x51c9e74a as u32, scaled by 2^-32.
        let p = pal_code([(1, 0), (1, 0), (3, 0), (1, 0)]);
        let v = p.wrapping_mul(0x523a_a99e).wrapping_sub(0x51c9_e74a);
        assert!((prng(p) - v as f32 / 4_294_967_296.0).abs() < 1e-6);
        assert!((0.0..1.0).contains(&prng(p)));
    }

    #[test]
    fn rotations_wrap() {
        assert_eq!(rotation_to(8, 8), Some(0));
        assert_eq!(rotation_to(8, 1), Some(1));
        assert_eq!(rotation_to(8, 2), Some(2));
        assert_eq!(rotation_to(8, 4), Some(3));
        assert_eq!(rotation_to(9, 3), Some(1));
        assert_eq!(rotation_to(9, 12), Some(3));
        assert_eq!(rotation_to(9, 5), None);
    }
}
