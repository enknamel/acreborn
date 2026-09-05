//! Interior lighting.
//!
//! The client lights dungeons and building interiors from the lights that
//! the cells' static objects carry: a torch, brazier or lamp is a Setup
//! whose light list (`Setup::lights`) holds one `LightInfo` per flame in
//! the object's frame. There is no sun underground; a cell is lit by its
//! own lights, by those of the cells behind its portals, and by a low
//! ambient. All shipped lights are point lights (`cone_angle` is never
//! set), `intensity` is a percentage and `falloff` the radius in metres at
//! which the light has faded out.
//!
//! [`accumulate`] sums that light at a point; [`LightSampler`] answers the
//! same question per cell for a whole landblock, for baking cell geometry
//! and tinting the objects standing in it.

use std::collections::HashMap;

use ac_formats::setup::LightInfo;
use glam::{Mat4, Vec3};

use crate::interior::CellScene;
use crate::model::frame_to_mat;

/// Ambient light of a dungeon cell: enough to read the walls, dark enough
/// that torch pools stand out.
pub const DUNGEON_AMBIENT: Vec3 = Vec3::new(0.28, 0.27, 0.31);

/// Ambient light inside a building: daylight leaks in through doors and
/// windows, so interiors above ground are brighter than dungeons.
pub const BUILDING_AMBIENT: Vec3 = Vec3::new(0.46, 0.45, 0.48);

/// Fraction of a light that reaches a surface facing straight away from
/// it (a wrapped Lambert term). Cells are low-poly, so a plain Lambert
/// term leaves hard black seams next to a torch mounted flush on a wall.
const WRAP: f32 = 0.35;

/// Brightest lighting term baked per channel; lights overlapping near a
/// brazier saturate the texture instead of blowing out to white.
const MAX_LIGHT: f32 = 1.6;

/// One point light in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellLight {
    pub position: Vec3,
    /// RGB in 0..1 with the intensity folded in.
    pub color: Vec3,
    /// Distance at which the light has faded to nothing.
    pub radius: f32,
}

impl CellLight {
    /// A Setup's light placed by `world`, the transform of the object that
    /// carries it (the light frame is relative to the object's frame).
    pub fn from_setup_light(l: &LightInfo, world: Mat4) -> Self {
        let origin = (world * frame_to_mat(&l.viewer_space_location)).transform_point3(Vec3::ZERO);
        let c = l.color;
        let rgb = Vec3::new(
            ((c >> 16) & 0xFF) as f32,
            ((c >> 8) & 0xFF) as f32,
            (c & 0xFF) as f32,
        ) / 255.0;
        let intensity = if l.intensity.is_finite() {
            (l.intensity / 100.0).clamp(0.0, 4.0)
        } else {
            1.0
        };
        let radius = if l.falloff.is_finite() {
            l.falloff.clamp(0.0, 200.0)
        } else {
            0.0
        };
        CellLight {
            position: origin,
            color: rgb * intensity,
            radius,
        }
    }

    /// Attenuation at distance `dist`: 1 at the light, fading smoothly to
    /// 0 at the radius (an inverted smoothstep).
    pub fn attenuation(&self, dist: f32) -> f32 {
        if self.radius <= 0.0 || dist >= self.radius {
            return 0.0;
        }
        let x = (dist / self.radius).clamp(0.0, 1.0);
        1.0 - x * x * (3.0 - 2.0 * x)
    }
}

/// Light arriving at `p`: `ambient` plus every light in range. With a
/// surface normal each light is weighted by a wrapped Lambert term; without
/// one (an object rather than a surface) it counts in full.
pub fn accumulate(lights: &[CellLight], ambient: Vec3, p: Vec3, normal: Option<Vec3>) -> Vec3 {
    let mut sum = ambient;
    for l in lights {
        let to_light = l.position - p;
        let dist = to_light.length();
        let att = l.attenuation(dist);
        if att <= 0.0 {
            continue;
        }
        let facing = match normal {
            Some(n) if dist > 1e-4 => {
                let nl = n.dot(to_light / dist).max(0.0);
                WRAP + (1.0 - WRAP) * nl
            }
            _ => 1.0,
        };
        sum += l.color * (att * facing);
    }
    sum.min(Vec3::splat(MAX_LIGHT))
}

/// Everything that lights one cell: its ambient and the lights that reach
/// it (its own, then its portal neighbours').
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CellLighting {
    pub ambient: Vec3,
    pub lights: Vec<CellLight>,
}

impl CellLighting {
    /// Light at `p` (world space) on a surface with `normal`, or at an
    /// object when `None`.
    pub fn at(&self, p: Vec3, normal: Option<Vec3>) -> Vec3 {
        accumulate(&self.lights, self.ambient, p, normal)
    }
}

/// Per-cell lighting of a landblock, keyed by full cell id.
#[derive(Debug, Clone, Default)]
pub struct LightSampler {
    cells: HashMap<u32, CellLighting>,
    /// Lights placed in the block (each counted once).
    light_count: usize,
}

impl LightSampler {
    /// Build from a landblock's cells: each cell gets its own lights plus
    /// those of the cells directly behind its portals.
    pub fn build(cells: &[CellScene], ambient: Vec3) -> Self {
        let own: HashMap<u32, &[CellLight]> = cells
            .iter()
            .map(|c| (c.cell_id, c.lights.as_slice()))
            .collect();
        let mut out = HashMap::with_capacity(cells.len());
        let mut light_count = 0;
        for c in cells {
            let mut lights = c.lights.clone();
            light_count += c.lights.len();
            for n in &c.portal_cells {
                if let Some(l) = own.get(n) {
                    lights.extend_from_slice(l);
                }
            }
            out.insert(c.cell_id, CellLighting { ambient, lights });
        }
        LightSampler {
            cells: out,
            light_count,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Number of lights placed in the block.
    pub fn light_count(&self) -> usize {
        self.light_count
    }

    /// The lighting of a cell, if it is an interior cell of this block.
    pub fn cell(&self, cell_id: u32) -> Option<&CellLighting> {
        self.cells.get(&cell_id)
    }

    /// Summed light colour at world position `p` inside `cell_id`; `None`
    /// for cells this block does not know (outdoors, other blocks).
    pub fn sample(&self, cell_id: u32, p: Vec3) -> Option<Vec3> {
        self.cells.get(&cell_id).map(|c| c.at(p, None))
    }
}

/// The lights carried by the static objects of one cell, in world space.
pub(crate) fn cell_lights(
    assets: &crate::Assets,
    statics: &[ac_formats::landblock::Stab],
    cell_transform: Mat4,
) -> Vec<CellLight> {
    let mut out = Vec::new();
    for stab in statics {
        if stab.id >> 24 != 0x02 {
            continue;
        }
        let Ok(setup) = assets.setup(stab.id) else {
            continue;
        };
        let world = cell_transform * frame_to_mat(&stab.frame);
        out.extend(
            setup
                .lights
                .iter()
                .map(|(_, l)| CellLight::from_setup_light(l, world))
                .filter(|l| l.radius > 0.0),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn torch() -> CellLight {
        CellLight {
            position: Vec3::new(0.0, 0.0, 2.0),
            color: Vec3::new(1.0, 0.5, 0.25),
            radius: 4.0,
        }
    }

    #[test]
    fn accumulate_synthetic_point_light() {
        let ambient = Vec3::splat(0.2);
        let lights = [torch()];
        // Out of range: ambient only.
        let far = accumulate(&lights, ambient, Vec3::new(10.0, 0.0, 2.0), None);
        assert_eq!(far, ambient);
        // At the light, facing it (or an object): ambient plus full colour.
        let at = accumulate(&lights, ambient, Vec3::new(0.0, 0.0, 2.0), None);
        assert!((at - (ambient + torch().color)).length() < 1e-5);
        // Halfway out: the smooth falloff is at one half.
        let mid = accumulate(&lights, ambient, Vec3::new(2.0, 0.0, 2.0), None);
        assert!(
            (mid - (ambient + torch().color * 0.5)).length() < 1e-5,
            "{mid}"
        );
        // A floor two metres under the light, looking up at it.
        let floor = accumulate(&lights, ambient, Vec3::ZERO, Some(Vec3::Z));
        assert!(
            (floor - (ambient + torch().color * 0.5)).length() < 1e-5,
            "{floor}"
        );
        // The same spot facing away still catches the wrapped share.
        let away = accumulate(&lights, ambient, Vec3::ZERO, Some(-Vec3::Z));
        assert!(away.x > ambient.x && away.x < floor.x, "{away}");
        // Never brighter than the clamp.
        let many = vec![torch(); 20];
        let hot = accumulate(&many, ambient, torch().position, None);
        assert!(hot.max_element() <= MAX_LIGHT + 1e-6);
    }

    #[test]
    fn setup_light_is_placed_by_the_object_frame() {
        let l = LightInfo {
            viewer_space_location: ac_formats::geom::Frame {
                origin: Vec3::new(0.0, 0.0, 1.0),
                orientation: glam::Quat::IDENTITY,
            },
            color: 0xFFFF_8040,
            intensity: 50.0,
            falloff: 6.0,
            cone_angle: f32::from_bits(0xCDCD_CDCD),
        };
        let world = Mat4::from_translation(Vec3::new(10.0, 20.0, 0.0));
        let cl = CellLight::from_setup_light(&l, world);
        assert_eq!(cl.position, Vec3::new(10.0, 20.0, 1.0));
        assert!((cl.color - Vec3::new(1.0, 128.0 / 255.0, 64.0 / 255.0) * 0.5).length() < 1e-6);
        assert_eq!(cl.radius, 6.0);
    }
}
