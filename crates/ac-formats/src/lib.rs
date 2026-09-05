//! Decoders for the asset types stored in the DAT archives. Each module is
//! one file type; every `parse` takes the raw file bytes and must consume
//! them exactly.
//!
//! Layouts follow the client (see `docs/subsystems/`), cross-checked against
//! ACE's `ACE.DatLoader`.

pub mod dxt;
pub mod reader;

pub mod animation;
pub mod chargen;
pub mod environment;
pub mod gfxobj;
pub mod landblock;
pub mod motion_table;
pub mod palette;
pub mod palette_set;
pub mod region;
pub mod scene;
pub mod setup;
pub mod sound_table;
pub mod surface;
pub mod surface_texture;
pub mod texture;
pub mod wave;

pub use reader::Reader;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unexpected end of data at {at} (wanted {want} more bytes)")]
    Eof { at: usize, want: usize },
    #[error("{at} of {len} bytes consumed; trailing data")]
    Trailing { at: usize, len: usize },
    #[error("file id {found:#010x} does not match requested {expected:#010x}")]
    IdMismatch { expected: u32, found: u32 },
    #[error("unsupported {what} {value:#x}")]
    Unsupported { what: &'static str, value: u32 },
    #[error("invalid {what}: {detail}")]
    Invalid { what: &'static str, detail: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Shared geometry primitives used by several file types.
pub mod geom {
    use glam::{Quat, Vec3};
    use serde::Serialize;

    use crate::{Reader, Result};

    #[derive(Debug, Clone, Copy, PartialEq, Serialize)]
    pub struct Frame {
        pub origin: Vec3,
        pub orientation: Quat,
    }

    impl Frame {
        pub fn parse(r: &mut Reader) -> Result<Self> {
            Ok(Frame {
                origin: r.vec3()?,
                orientation: r.quat_wxyz()?,
            })
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Serialize)]
    pub struct Sphere {
        pub origin: Vec3,
        pub radius: f32,
    }

    impl Sphere {
        pub fn parse(r: &mut Reader) -> Result<Self> {
            Ok(Sphere {
                origin: r.vec3()?,
                radius: r.f32()?,
            })
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Serialize)]
    pub struct CylSphere {
        pub origin: Vec3,
        pub radius: f32,
        pub height: f32,
    }

    impl CylSphere {
        pub fn parse(r: &mut Reader) -> Result<Self> {
            Ok(CylSphere {
                origin: r.vec3()?,
                radius: r.f32()?,
                height: r.f32()?,
            })
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Serialize)]
    pub struct Plane {
        pub normal: Vec3,
        pub d: f32,
    }

    impl Plane {
        pub fn parse(r: &mut Reader) -> Result<Self> {
            Ok(Plane {
                normal: r.vec3()?,
                d: r.f32()?,
            })
        }
    }
}

/// Read the leading file id and check it against the id the caller asked for.
fn expect_id(r: &mut Reader, expected: u32) -> Result<u32> {
    let found = r.u32()?;
    if found != expected {
        return Err(Error::IdMismatch { expected, found });
    }
    Ok(found)
}
