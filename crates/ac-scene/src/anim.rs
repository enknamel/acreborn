//! Animation playback: a looping cycle from a MotionTable applied to a
//! Setup's parts.

use std::rc::Rc;

use ac_formats::animation::Animation;
use ac_formats::geom::Frame;
use ac_formats::motion_table::{AnimData, MotionTable};
use glam::{Mat4, Quat};

use crate::{Assets, Result};

/// Motion stances and commands used for basic locomotion.
pub mod motion {
    pub const STANCE_NON_COMBAT: u32 = 0x8000_003D;
    pub const READY: u32 = 0x4100_0003;
    pub const WALK_FORWARD: u32 = 0x4500_0005;
    pub const WALK_BACKWARDS: u32 = 0x4500_0006;
    pub const RUN_FORWARD: u32 = 0x4400_0007;
    pub const SIDE_STEP_RIGHT: u32 = 0x6500_000F;
    pub const SIDE_STEP_LEFT: u32 = 0x6500_0010;
}

struct Clip {
    anim: Rc<Animation>,
    low: usize,
    /// Exclusive.
    high: usize,
    fps: f32,
}

impl Clip {
    fn len(&self) -> usize {
        self.high.saturating_sub(self.low)
    }
    fn duration(&self) -> f32 {
        self.len() as f32 / self.fps.abs().max(0.01)
    }
}

/// A looping sequence of animation clips with a playhead.
pub struct AnimPlayer {
    clips: Vec<Clip>,
    pub time: f32,
    total: f32,
    /// Object-space velocity of this motion (from the MotionData).
    pub velocity: glam::Vec3,
}

impl AnimPlayer {
    /// Build a player for the given MotionTable cycle. Returns `None` if
    /// the table has no such motion or its animations fail to load.
    pub fn cycle(assets: &Assets, table: &MotionTable, style: u32, motion: u32) -> Option<Self> {
        let data = table.cycle(style, motion)?;
        let clips = load_clips(assets, &data.anims).ok()?;
        if clips.is_empty() {
            return None;
        }
        let total = clips.iter().map(Clip::duration).sum();
        Some(AnimPlayer {
            clips,
            time: 0.0,
            total,
            velocity: data.velocity,
        })
    }

    pub fn advance(&mut self, dt: f32) {
        if self.total > 0.0 {
            self.time = (self.time + dt).rem_euclid(self.total);
        }
    }

    /// Per-part local transforms at the current time, interpolated
    /// between frames. `n_parts` bounds the result.
    pub fn part_transforms(&self, n_parts: usize) -> Vec<Mat4> {
        let mut t = self.time;
        for c in &self.clips {
            let d = c.duration();
            if t < d || std::ptr::eq(c, self.clips.last().unwrap()) {
                let pos = (t / d).clamp(0.0, 0.9999) * c.len() as f32;
                let (i, frac) = (pos.floor() as usize, pos.fract());
                let (a, b) = if c.fps >= 0.0 {
                    (c.low + i, c.low + (i + 1).min(c.len() - 1))
                } else {
                    (c.high - 1 - i, c.high - 1 - (i + 1).min(c.len() - 1))
                };
                let fa = &c.anim.part_frames[a.min(c.anim.part_frames.len() - 1)];
                let fb = &c.anim.part_frames[b.min(c.anim.part_frames.len() - 1)];
                return (0..n_parts)
                    .map(|p| match (fa.frames.get(p), fb.frames.get(p)) {
                        (Some(x), Some(y)) => lerp_frame(x, y, frac),
                        (Some(x), None) => frame_mat(x),
                        _ => Mat4::IDENTITY,
                    })
                    .collect();
            }
            t -= d;
        }
        vec![Mat4::IDENTITY; n_parts]
    }
}

fn frame_mat(f: &Frame) -> Mat4 {
    Mat4::from_rotation_translation(f.orientation.normalize(), f.origin)
}

fn lerp_frame(a: &Frame, b: &Frame, t: f32) -> Mat4 {
    let q = a
        .orientation
        .normalize()
        .slerp(b.orientation.normalize(), t);
    let o = a.origin.lerp(b.origin, t);
    Mat4::from_rotation_translation(q, o)
}

fn load_clips(assets: &Assets, anims: &[AnimData]) -> Result<Vec<Clip>> {
    let mut out = Vec::with_capacity(anims.len());
    for a in anims {
        let bytes = assets.portal.read(a.anim_id)?;
        let anim = Rc::new(Animation::parse(a.anim_id, &bytes).map_err(|source| {
            crate::Error::Format {
                id: a.anim_id,
                source,
            }
        })?);
        let n = anim.part_frames.len();
        let low = a.low_frame.max(0) as usize;
        let high = if a.high_frame < 0 {
            n
        } else {
            (a.high_frame as usize).min(n)
        };
        if high > low {
            out.push(Clip {
                anim,
                low,
                high,
                fps: if a.framerate == 0.0 {
                    30.0
                } else {
                    a.framerate
                },
            });
        }
    }
    Ok(out)
}

/// Load a MotionTable by id.
pub fn motion_table(assets: &Assets, id: u32) -> Result<MotionTable> {
    let bytes = assets.portal.read(id)?;
    MotionTable::parse(id, &bytes).map_err(|source| crate::Error::Format { id, source })
}

pub fn quat_identity() -> Quat {
    Quat::IDENTITY
}
