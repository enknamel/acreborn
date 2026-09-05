//! MotionTable (0x09): which animations a Setup plays for each stance
//! ("style") and motion command.
//!
//! Keys combine a style and a motion as `(style << 16) | (motion & 0xFFFFFF)`
//! in 32 bits, so only the low 16 bits of the style survive. `cycles` are
//! looping motions (idle, walk, run), `modifiers` are overlays, `links`
//! are transitions from a current motion to a new one.

use glam::Vec3;
use serde::Serialize;

use crate::{expect_id, Reader, Result};

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AnimData {
    /// Animation (0x03) id.
    pub anim_id: u32,
    pub low_frame: i32,
    /// -1 = to the end.
    pub high_frame: i32,
    /// Frames per second; negative plays backwards.
    pub framerate: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct MotionData {
    pub bitfield: u8,
    pub flags: u8,
    pub anims: Vec<AnimData>,
    /// Movement produced by this motion, in object space per second.
    pub velocity: Vec3,
    pub omega: Vec3,
}

impl MotionData {
    fn parse(r: &mut Reader) -> Result<Self> {
        let n = r.u8()? as usize;
        let bitfield = r.u8()?;
        let flags = r.u8()?;
        r.align4()?;
        let anims = r.fixed(n, &mut |r: &mut Reader| {
            Ok(AnimData {
                anim_id: r.u32()?,
                low_frame: r.i32()?,
                high_frame: r.i32()?,
                framerate: r.f32()?,
            })
        })?;
        let velocity = if flags & 1 != 0 {
            r.vec3()?
        } else {
            Vec3::ZERO
        };
        let omega = if flags & 2 != 0 {
            r.vec3()?
        } else {
            Vec3::ZERO
        };
        Ok(MotionData {
            bitfield,
            flags,
            anims,
            velocity,
            omega,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MotionTable {
    pub id: u32,
    pub default_style: u32,
    /// style -> default motion for that style.
    pub style_defaults: Vec<(u32, u32)>,
    pub cycles: Vec<(u32, MotionData)>,
    pub modifiers: Vec<(u32, MotionData)>,
    /// (style|current motion) -> [(motion, data)]
    pub links: Vec<(u32, Vec<(u32, MotionData)>)>,
}

/// Combine a style and motion into a table key the way the client does.
pub fn motion_key(style: u32, motion: u32) -> u32 {
    (style << 16) | (motion & 0xFF_FFFF)
}

/// Class bits in the high byte of a MotionCommand id. Stances are
/// `STYLE`, looping motions (Ready, walk, run, On/Off) are `SUBSTATE`,
/// one-shots (attacks, emotes) are `ACTION`.
pub mod command_mask {
    pub const STYLE: u32 = 0x8000_0000;
    pub const SUBSTATE: u32 = 0x4000_0000;
    pub const MODIFIER: u32 = 0x2000_0000;
    pub const ACTION: u32 = 0x1000_0000;
}

/// True if two command ids name the same command. The low 16 bits are a
/// command's unique index; the class bits above are dropped on the wire,
/// so a value without them matches any full id with the same index.
pub fn same_command(a: u32, b: u32) -> bool {
    if a >> 16 == 0 || b >> 16 == 0 {
        a & 0xFFFF == b & 0xFFFF
    } else {
        a == b
    }
}

impl MotionTable {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let default_style = r.u32()?;
        let style_defaults = r.list(|r| Ok((r.u32()?, r.u32()?)))?;
        let cycles = r.map(|r| r.u32(), MotionData::parse)?;
        let modifiers = r.map(|r| r.u32(), MotionData::parse)?;
        let links = r.map(|r| r.u32(), |r| r.map(|r| r.u32(), MotionData::parse))?;
        r.finish()?;
        Ok(MotionTable {
            id,
            default_style,
            style_defaults,
            cycles,
            modifiers,
            links,
        })
    }

    pub fn default_motion(&self, style: u32) -> Option<u32> {
        self.style_defaults
            .iter()
            .find(|(s, _)| *s == style)
            .map(|(_, m)| *m)
    }

    /// The looping motion for `style` + `motion`, falling back to the
    /// default style.
    pub fn cycle(&self, style: u32, motion: u32) -> Option<&MotionData> {
        let find = |k: u32| {
            self.cycles
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, d)| d)
        };
        find(motion_key(style, motion)).or_else(|| find(motion_key(self.default_style, motion)))
    }

    /// The transition played when `command` is issued while `current` is
    /// the motion in `style`: a one-shot action (attack, emote) over the
    /// current motion, or the change from one motion to the next (a door
    /// going from Off to On). Falls back to the style's generic links
    /// (`current` = 0). Commands may be full ids or the wire's low 16 bits.
    pub fn link(&self, style: u32, current: u32, command: u32) -> Option<&MotionData> {
        let find = |k: u32| {
            self.links
                .iter()
                .find(|(key, _)| *key == k)
                .and_then(|(_, inner)| inner.iter().find(|(m, _)| same_command(*m, command)))
                .map(|(_, d)| d)
        };
        find(motion_key(style, current)).or_else(|| find(motion_key(style, 0)))
    }

    /// Recover a full command id (with its class bits) from the low 16
    /// bits carried on the wire, from the commands this table links to.
    pub fn full_command(&self, low: u16) -> Option<u32> {
        self.links
            .iter()
            .flat_map(|(_, inner)| inner.iter())
            .map(|(m, _)| *m)
            .find(|m| *m & 0xFFFF == low as u32)
    }
}
