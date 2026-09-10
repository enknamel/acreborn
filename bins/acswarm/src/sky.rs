//! Outdoor environment: sky gradient, distance fog, sun and ambient light.
//!
//! The Region's `SkyDesc` gives, per day group and time of day, the
//! directional and ambient light (colour × brightness), the world fog
//! range and colour, and the sky objects (dome, sun, moons, clouds). The
//! dome itself is a white-textured GfxObj lit by the client, so there is
//! no explicit sky colour in the data; the horizon is the fog colour and
//! the zenith is derived from it.

use ac_formats::region::{Region, SkyTimeOfDay};
use glam::{Vec3, Vec4};

/// Everything the renderer needs to light and fog an outdoor scene.
#[derive(Clone, Debug, PartialEq)]
pub struct Environment {
    /// Sky colour straight up.
    pub sky_zenith: Vec3,
    /// Sky colour at the horizon; terrain fogs to this so it never pops.
    pub sky_horizon: Vec3,
    pub fog_color: Vec3,
    /// Fog starts at this distance (world units) ...
    pub fog_start: f32,
    /// ... and is opaque here. The renderer clamps this to its far plane.
    pub fog_end: f32,
    /// Directional (sun) light colour, brightness folded in.
    pub sun_color: Vec3,
    /// Ambient light colour, brightness folded in.
    pub ambient: Vec3,
    /// Water tint and base opacity.
    pub water_color: Vec4,
}

impl Default for Environment {
    /// The Region's "Sunny" day group at midday (`sky_time` entry with
    /// `begin` 0.27..0.61): fog 150..2400 in colour `0xC3C8DC`, sun
    /// `0xFAD797` × 0.8, ambient `0xE6E6FF` × 0.35.
    fn default() -> Self {
        environment(
            argb(0xFFC3_C8DC),
            150.0,
            2400.0,
            argb(0xFFFA_D797) * 0.8,
            argb(0xFFE6_E6FF) * 0.35,
        )
    }
}

impl Environment {
    /// Environment for the given fraction of the day (0 = midnight, 0.5 =
    /// noon) in the Region's default day group. Values between two
    /// `sky_time` entries are interpolated the way the client's light tick
    /// does; `None` if the Region has no sky description.
    /// Underground: black sky, no fog, dimmer ambient.
    pub fn dungeon() -> Self {
        let day = Self::default();
        Environment {
            sky_zenith: Vec3::ZERO,
            sky_horizon: Vec3::ZERO,
            fog_color: Vec3::ZERO,
            fog_start: 1.0e6,
            fog_end: 2.0e6,
            sun_color: day.sun_color * 0.6,
            ambient: day.ambient,
            water_color: day.water_color,
        }
    }

    pub fn from_region(region: &Region, day_fraction: f32) -> Option<Self> {
        let sky = region.sky.as_ref()?;
        let group = sky.day_groups.first()?;
        let times = &group.sky_time;
        if times.is_empty() {
            return None;
        }
        let t = day_fraction.rem_euclid(1.0);
        let next = times
            .iter()
            .position(|s| s.begin > t)
            .unwrap_or(times.len());
        let cur = &times[next.saturating_sub(1)];
        let nxt = &times[next % times.len()];
        let span = if nxt.begin > cur.begin {
            nxt.begin - cur.begin
        } else {
            1.0 - cur.begin + nxt.begin
        };
        let f = if span > 0.0 {
            ((t - cur.begin).rem_euclid(1.0) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let lerp = |a: f32, b: f32| a + (b - a) * f;
        let lerp3 = |a: Vec3, b: Vec3| a.lerp(b, f);
        let of = |s: &SkyTimeOfDay| {
            (
                argb(s.world_fog_color),
                s.min_world_fog,
                s.max_world_fog,
                argb(s.dir_color) * s.dir_bright,
                argb(s.amb_color) * s.amb_bright,
            )
        };
        let (fc0, fs0, fe0, sun0, amb0) = of(cur);
        let (fc1, fs1, fe1, sun1, amb1) = of(nxt);
        Some(environment(
            lerp3(fc0, fc1),
            lerp(fs0, fs1),
            lerp(fe0, fe1),
            lerp3(sun0, sun1),
            lerp3(amb0, amb1),
        ))
    }
}

/// Derive the full environment from the Region's fog and light values.
fn environment(fog: Vec3, fog_start: f32, fog_end: f32, sun: Vec3, ambient: Vec3) -> Environment {
    // The dome is lit white, so the sky reads as the haze colour near the
    // horizon deepening to blue overhead: keep the fog's blue, pull the
    // red and green down so the zenith saturates.
    let zenith = fog * Vec3::new(0.42, 0.6, 1.0);
    Environment {
        sky_zenith: zenith,
        sky_horizon: fog,
        fog_color: fog,
        fog_start,
        fog_end: fog_end.max(fog_start + 1.0),
        sun_color: sun,
        ambient,
        water_color: Vec4::new(0.16, 0.36, 0.52, 0.62),
    }
}

/// `0xAARRGGBB` to linear-ish RGB in 0..1 (the renderer works in the
/// swapchain's non-sRGB format, matching how textures are uploaded).
pub fn argb(c: u32) -> Vec3 {
    Vec3::new(
        ((c >> 16) & 0xFF) as f32,
        ((c >> 8) & 0xFF) as f32,
        (c & 0xFF) as f32,
    ) / 255.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argb_unpacks_channels() {
        let c = argb(0xFFC3_C8DC);
        assert!((c.x - 195.0 / 255.0).abs() < 1e-6);
        assert!((c.y - 200.0 / 255.0).abs() < 1e-6);
        assert!((c.z - 220.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn default_matches_region_midday() {
        let Ok(dir) = std::env::var("AC_DATA_DIR") else {
            return;
        };
        let assets = ac_scene::Assets::open(dir).unwrap();
        let region = assets.region().unwrap();
        let env = Environment::from_region(&region, 0.5).unwrap();
        let d = Environment::default();
        assert!(env.fog_color.abs_diff_eq(d.fog_color, 1e-5));
        assert_eq!(env.fog_start, d.fog_start);
        assert_eq!(env.fog_end, d.fog_end);
        // Sun brightness ramps 0.7 -> 0.8 across the midday span.
        assert!(env.sun_color.abs_diff_eq(d.sun_color, 0.05), "{env:?}");
        assert!(env.ambient.abs_diff_eq(d.ambient, 1e-5));
        // Night is darker and foggier than noon.
        let night = Environment::from_region(&region, 0.0).unwrap();
        assert!(night.fog_color.length() < env.fog_color.length());
        assert!(night.fog_end < env.fog_end);
    }
}
