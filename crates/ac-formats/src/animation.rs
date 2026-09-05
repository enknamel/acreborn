//! Animation (0x03): per-frame part transforms plus hooks (events fired at a
//! frame: sounds, particles, attacks, visual tweaks). `AnimationFrame` is
//! shared with Setup placement frames.

use glam::Vec3;
use serde::Serialize;

use crate::geom::Frame;
use crate::{expect_id, Reader, Result};

pub mod flags {
    pub const POS_FRAMES: u32 = 0x1;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum HookDir {
    Backward,
    Both,
    Forward,
    Other(i32),
}

impl From<i32> for HookDir {
    fn from(v: i32) -> Self {
        match v {
            -1 => HookDir::Backward,
            0 => HookDir::Both,
            1 => HookDir::Forward,
            o => HookDir::Other(o),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AttackCone {
    pub part_index: u32,
    pub left_x: f32,
    pub left_y: f32,
    pub right_x: f32,
    pub right_y: f32,
    pub radius: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum HookData {
    NoOp,
    Sound {
        id: u32,
    },
    SoundTable {
        sound_type: u32,
    },
    Attack(AttackCone),
    AnimationDone,
    ReplaceObject {
        part_index: u16,
        gfxobj_id: u32,
    },
    Ethereal {
        ethereal: i32,
    },
    TransparentPart {
        part: u32,
        start: f32,
        end: f32,
        time: f32,
    },
    Luminous {
        start: f32,
        end: f32,
        time: f32,
    },
    LuminousPart {
        part: u32,
        start: f32,
        end: f32,
        time: f32,
    },
    Diffuse {
        start: f32,
        end: f32,
        time: f32,
    },
    DiffusePart {
        part: u32,
        start: f32,
        end: f32,
        time: f32,
    },
    Scale {
        end: f32,
        time: f32,
    },
    CreateParticle {
        emitter_info_id: u32,
        part_index: u32,
        offset: Frame,
        emitter_id: u32,
    },
    DestroyParticle {
        emitter_id: u32,
    },
    StopParticle {
        emitter_id: u32,
    },
    NoDraw {
        no_draw: u32,
    },
    DefaultScript,
    DefaultScriptPart {
        part_index: u32,
    },
    CallPes {
        pes: u32,
        pause: f32,
    },
    Transparent {
        start: f32,
        end: f32,
        time: f32,
    },
    SoundTweaked {
        sound_id: u32,
        priority: f32,
        probability: f32,
        volume: f32,
    },
    SetOmega {
        axis: Vec3,
    },
    TextureVelocity {
        u: f32,
        v: f32,
    },
    TextureVelocityPart {
        part_index: u32,
        u: f32,
        v: f32,
    },
    SetLight {
        light_on: i32,
    },
    /// As `CreateParticle`, but the script waits for the emitter to finish.
    CreateBlockingParticle {
        emitter_info_id: u32,
        part_index: u32,
        offset: Frame,
        emitter_id: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hook {
    pub hook_type: u32,
    pub dir: HookDir,
    pub data: HookData,
}

impl Hook {
    pub(crate) fn parse(r: &mut Reader) -> Result<Self> {
        let hook_type = r.u32()?;
        let dir = HookDir::from(r.i32()?);
        use HookData::*;
        let data = match hook_type {
            0 => NoOp,
            1 => Sound { id: r.u32()? },
            2 => SoundTable {
                sound_type: r.u32()?,
            },
            3 => Attack(AttackCone {
                part_index: r.u32()?,
                left_x: r.f32()?,
                left_y: r.f32()?,
                right_x: r.f32()?,
                right_y: r.f32()?,
                radius: r.f32()?,
                height: r.f32()?,
            }),
            4 => AnimationDone,
            5 => {
                let part_index = r.u16()?;
                let gfxobj_id = r.data_id_of_type(0x0100_0000)?;
                ReplaceObject {
                    part_index,
                    gfxobj_id,
                }
            }
            6 => Ethereal { ethereal: r.i32()? },
            7 => TransparentPart {
                part: r.u32()?,
                start: r.f32()?,
                end: r.f32()?,
                time: r.f32()?,
            },
            8 => Luminous {
                start: r.f32()?,
                end: r.f32()?,
                time: r.f32()?,
            },
            9 => LuminousPart {
                part: r.u32()?,
                start: r.f32()?,
                end: r.f32()?,
                time: r.f32()?,
            },
            10 => Diffuse {
                start: r.f32()?,
                end: r.f32()?,
                time: r.f32()?,
            },
            11 => DiffusePart {
                part: r.u32()?,
                start: r.f32()?,
                end: r.f32()?,
                time: r.f32()?,
            },
            12 => Scale {
                end: r.f32()?,
                time: r.f32()?,
            },
            13 => CreateParticle {
                emitter_info_id: r.u32()?,
                part_index: r.u32()?,
                offset: Frame::parse(r)?,
                emitter_id: r.u32()?,
            },
            14 => DestroyParticle {
                emitter_id: r.u32()?,
            },
            15 => StopParticle {
                emitter_id: r.u32()?,
            },
            16 => NoDraw { no_draw: r.u32()? },
            17 => DefaultScript,
            18 => DefaultScriptPart {
                part_index: r.u32()?,
            },
            19 => CallPes {
                pes: r.u32()?,
                pause: r.f32()?,
            },
            20 => Transparent {
                start: r.f32()?,
                end: r.f32()?,
                time: r.f32()?,
            },
            21 => SoundTweaked {
                sound_id: r.u32()?,
                priority: r.f32()?,
                probability: r.f32()?,
                volume: r.f32()?,
            },
            22 => SetOmega { axis: r.vec3()? },
            23 => TextureVelocity {
                u: r.f32()?,
                v: r.f32()?,
            },
            24 => TextureVelocityPart {
                part_index: r.u32()?,
                u: r.f32()?,
                v: r.f32()?,
            },
            25 => SetLight { light_on: r.i32()? },
            26 => CreateBlockingParticle {
                emitter_info_id: r.u32()?,
                part_index: r.u32()?,
                offset: Frame::parse(r)?,
                emitter_id: r.u32()?,
            },
            other => {
                return Err(crate::Error::Unsupported {
                    what: "animation hook type",
                    value: other,
                })
            }
        };
        Ok(Hook {
            hook_type,
            dir,
            data,
        })
    }
}

/// One frame: a transform per part, plus the hooks that fire on it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnimationFrame {
    pub frames: Vec<Frame>,
    pub hooks: Vec<Hook>,
}

impl AnimationFrame {
    pub fn parse(r: &mut Reader, n_parts: usize) -> Result<Self> {
        let frames = r.fixed(n_parts, &mut Frame::parse)?;
        let hooks = r.list(Hook::parse)?;
        Ok(AnimationFrame { frames, hooks })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Animation {
    pub id: u32,
    pub flags: u32,
    pub num_parts: u32,
    pub num_frames: u32,
    /// Root motion per frame (`POS_FRAMES`).
    pub pos_frames: Vec<Frame>,
    pub part_frames: Vec<AnimationFrame>,
}

impl Animation {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let flags = r.u32()?;
        let num_parts = r.u32()?;
        let num_frames = r.u32()?;
        let pos_frames = if flags & flags::POS_FRAMES != 0 {
            r.fixed(num_frames as usize, &mut Frame::parse)?
        } else {
            Vec::new()
        };
        let part_frames = r.fixed(num_frames as usize, &mut |r: &mut Reader| {
            AnimationFrame::parse(r, num_parts as usize)
        })?;
        r.finish()?;
        Ok(Animation {
            id,
            flags,
            num_parts,
            num_frames,
            pos_frames,
            part_frames,
        })
    }
}
