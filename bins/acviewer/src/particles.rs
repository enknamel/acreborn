//! Particles in the viewer: turns the quads `ac_scene::particles`
//! produces into GPU draws, and runs an emitter, a script or a Setup's
//! default script offline for the `--emitter` screenshot path.

use std::collections::HashMap;

use ac_scene::particles::{ParticleSystem, Quad, Rng, ScriptPlayer, SpriteImage};
use ac_scene::Assets;
use anyhow::{bail, Context, Result};
use glam::{Mat4, Vec3};

use crate::gpu::{MaterialKey, ParticleDraw, ParticleInstance};

/// The material a sprite image draws with, through the scene's cache.
pub fn material_key(image: SpriteImage) -> MaterialKey {
    match image {
        SpriteImage::Surface(id) => MaterialKey::Texture {
            id,
            tex: 0,
            palette: 0,
        },
        SpriteImage::Solid(argb) => MaterialKey::Solid(argb),
    }
}

/// Group quads into one draw per (image, blend mode). Alpha-blended
/// groups sort back to front from `eye` so nearer smoke covers farther;
/// additive groups are order-independent.
pub fn draws(quads: &[Quad], eye: Vec3) -> Vec<ParticleDraw> {
    let mut groups: HashMap<(SpriteImage, bool), Vec<(f32, ParticleInstance)>> = HashMap::new();
    for q in quads {
        groups.entry((q.image, q.additive)).or_default().push((
            q.position.distance_squared(eye),
            ParticleInstance {
                position: q.position.to_array(),
                size: q.size.to_array(),
                color: q.color,
            },
        ));
    }
    let mut out: Vec<ParticleDraw> = groups
        .into_iter()
        .map(|((image, additive), mut v)| {
            if !additive {
                v.sort_by(|a, b| b.0.total_cmp(&a.0));
            }
            ParticleDraw {
                material: material_key(image),
                additive,
                instances: v.into_iter().map(|(_, i)| i).collect(),
            }
        })
        .collect();
    // Alpha first, then additive light over it.
    out.sort_by_key(|d| d.additive);
    out
}

/// An offline particle simulation: one emitter (0x32), one script (0x33)
/// or a Setup's (0x02) default script, played at `transform`.
pub struct Demo {
    pub system: ParticleSystem,
    rng: Rng,
    player: Option<ScriptPlayer>,
    transform: Mat4,
    time: f64,
}

impl Demo {
    pub fn new(assets: &Assets, id: u32, transform: Mat4) -> Result<Self> {
        let mut rng = Rng::new(0xACE);
        let mut system = ParticleSystem::new();
        let script = match id >> 24 {
            0x32 => {
                system
                    .create(assets, id, transform, 0, 0.0, &mut rng)
                    .with_context(|| format!("emitter {id:#010x}"))?;
                None
            }
            0x33 => Some(id),
            0x02 => {
                let setup = assets.setup(id)?;
                if setup.default_script == 0 {
                    bail!("setup {id:#010x} has no default script");
                }
                Some(setup.default_script)
            }
            _ => bail!("{id:#010x}: not a ParticleEmitterInfo, PhysicsScript or Setup"),
        };
        let player = match script {
            Some(sid) => {
                let s = assets
                    .physics_script(sid)
                    .with_context(|| format!("script {sid:#010x}"))?;
                tracing::info!(
                    "script {sid:#010x}: {} hooks over {:.1} s",
                    s.hooks.len(),
                    s.duration()
                );
                Some(ScriptPlayer::new(s, 0.0))
            }
            None => None,
        };
        Ok(Demo {
            system,
            rng,
            player,
            transform,
            time: 0.0,
        })
    }

    /// Run forward `seconds` at 60 Hz.
    pub fn simulate(&mut self, assets: &Assets, seconds: f32) {
        let dt = 1.0 / 60.0;
        let end = self.time + seconds as f64;
        while self.time < end {
            self.time += dt;
            if let Some(p) = &mut self.player {
                p.update(
                    self.time,
                    &mut self.system,
                    assets,
                    self.transform,
                    |_| None,
                    &mut self.rng,
                );
            }
            self.system.update(self.time, &mut self.rng);
        }
    }

    pub fn quads(&self) -> Vec<Quad> {
        self.system.quads()
    }

    /// Centre and radius of the live particles (the emitter origin and
    /// 1 m when there are none).
    pub fn bounds(&self) -> (Vec3, f32) {
        let quads = self.quads();
        let origin = self.transform.transform_point3(Vec3::ZERO);
        if quads.is_empty() {
            return (origin, 1.0);
        }
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for q in &quads {
            let half = q.size.max_element() * 0.5;
            lo = lo.min(q.position - half);
            hi = hi.max(q.position + half);
        }
        let center = (lo + hi) * 0.5;
        (center, ((hi - lo).length() * 0.5).max(0.5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_group_by_image_and_blend() {
        let q = |x: f32, image, additive| Quad {
            position: Vec3::new(x, 0.0, 0.0),
            size: glam::Vec2::ONE,
            color: [1.0; 4],
            image,
            additive,
        };
        let quads = [
            q(1.0, SpriteImage::Surface(0x0800_0001), false),
            q(5.0, SpriteImage::Surface(0x0800_0001), false),
            q(2.0, SpriteImage::Surface(0x0800_0002), true),
            q(3.0, SpriteImage::Solid(0xFF00_FF00), false),
        ];
        let d = draws(&quads, Vec3::ZERO);
        assert_eq!(d.len(), 3);
        assert!(!d[0].additive && !d[1].additive && d[2].additive);
        let smoke = d
            .iter()
            .find(|d| {
                d.material
                    == MaterialKey::Texture {
                        id: 0x0800_0001,
                        tex: 0,
                        palette: 0,
                    }
            })
            .unwrap();
        // Farthest first.
        assert_eq!(smoke.instances[0].position[0], 5.0);
        assert_eq!(smoke.instances[1].position[0], 1.0);
        assert!(d
            .iter()
            .any(|d| d.material == MaterialKey::Solid(0xFF00_FF00)));
    }

    #[test]
    fn flaming_staff_draws_additive_flames() {
        let Ok(dir) = std::env::var("AC_DATA_DIR") else {
            return;
        };
        let assets = Assets::open(dir).unwrap();
        // A flaming staff: its default script starts five fire emitters
        // along the shaft, among them the torch flame 0x3200026E.
        let mut demo = Demo::new(&assets, 0x0200_03CF, Mat4::IDENTITY).unwrap();
        demo.simulate(&assets, 3.0);
        assert_eq!(demo.system.len(), 5);
        let quads = demo.quads();
        assert!(quads.len() > 10, "{}", quads.len());
        assert!(quads.iter().any(|q| q.additive), "flames are additive");
        let (c, r) = demo.bounds();
        assert!(r > 0.1 && r < 10.0, "{c} {r}");
        let d = draws(&quads, Vec3::NEG_Y);
        assert_eq!(
            d.iter().map(|d| d.instances.len()).sum::<usize>(),
            quads.len()
        );
        // The same seed gives the same frame.
        let mut again = Demo::new(&assets, 0x0200_03CF, Mat4::IDENTITY).unwrap();
        again.simulate(&assets, 3.0);
        assert_eq!(again.quads(), quads);
        // The lone torch flame is alive after three seconds.
        let mut torch = Demo::new(&assets, 0x3200_026E, Mat4::IDENTITY).unwrap();
        torch.simulate(&assets, 3.0);
        assert!(torch.quads().iter().all(|q| q.additive));
        assert!(torch.quads().len() >= 8, "{}", torch.quads().len());
    }
}
