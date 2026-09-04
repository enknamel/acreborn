//! GfxObj (0x01): a mesh. Vertices with normals and UV sets, polygons that
//! index them, and two BSP trees: one for physics (collision), one for
//! drawing (sorting/portals).

use glam::Vec3;
use serde::Serialize;

use crate::geom::{Plane, Sphere};
use crate::{expect_id, Error, Reader, Result};

pub mod flags {
    pub const HAS_PHYSICS: u32 = 0x1;
    pub const HAS_DRAWING: u32 = 0x2;
    pub const HAS_DID_DEGRADE: u32 = 0x8;
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Uv {
    pub u: f32,
    pub v: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Vertex {
    pub origin: Vec3,
    pub normal: Vec3,
    pub uvs: Vec<Uv>,
}

impl Vertex {
    pub fn parse(r: &mut Reader) -> Result<Self> {
        let n_uv = r.u16()? as usize;
        let origin = r.vec3()?;
        let normal = r.vec3()?;
        let uvs = r.fixed(n_uv, &mut |r: &mut Reader| {
            Ok(Uv {
                u: r.f32()?,
                v: r.f32()?,
            })
        })?;
        Ok(Vertex {
            origin,
            normal,
            uvs,
        })
    }
}

/// Bits of `Polygon::stippling`.
pub mod stippling {
    pub const POSITIVE: u8 = 0x1;
    pub const NEGATIVE: u8 = 0x2;
    pub const NO_POS: u8 = 0x4;
    pub const NO_NEG: u8 = 0x8;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CullMode {
    Landblock,
    None,
    Clockwise,
    CounterClockwise,
    Other(i32),
}

impl From<i32> for CullMode {
    fn from(v: i32) -> Self {
        match v {
            0 => CullMode::Landblock,
            1 => CullMode::None,
            2 => CullMode::Clockwise,
            3 => CullMode::CounterClockwise,
            o => CullMode::Other(o),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Polygon {
    pub stippling: u8,
    pub cull: CullMode,
    /// Index into `GfxObj::surfaces` for the front face.
    pub pos_surface: i16,
    /// Index into `GfxObj::surfaces` for the back face (same as front when
    /// `cull == None`).
    pub neg_surface: i16,
    /// Keys into `GfxObj::vertices`.
    pub vertex_ids: Vec<i16>,
    /// Per-vertex UV set index for the front face (empty if `NO_POS`).
    pub pos_uv_indices: Vec<u8>,
    /// Per-vertex UV set index for the back face.
    pub neg_uv_indices: Vec<u8>,
}

impl Polygon {
    pub fn parse(r: &mut Reader) -> Result<Self> {
        let n = r.u8()? as usize;
        let stippling = r.u8()?;
        let cull = CullMode::from(r.i32()?);
        let pos_surface = r.i16()?;
        let mut neg_surface = r.i16()?;
        let vertex_ids = r.fixed(n, &mut |r: &mut Reader| r.i16())?;
        let pos_uv_indices = if stippling & stippling::NO_POS == 0 {
            r.fixed(n, &mut |r: &mut Reader| r.u8())?
        } else {
            Vec::new()
        };
        let mut neg_uv_indices =
            if cull == CullMode::Clockwise && stippling & stippling::NO_NEG == 0 {
                r.fixed(n, &mut |r: &mut Reader| r.u8())?
            } else {
                Vec::new()
            };
        if cull == CullMode::None {
            neg_surface = pos_surface;
            neg_uv_indices = pos_uv_indices.clone();
        }
        Ok(Polygon {
            stippling,
            cull,
            pos_surface,
            neg_surface,
            vertex_ids,
            pos_uv_indices,
            neg_uv_indices,
        })
    }
}

/// Which flavor of BSP tree is being read; the node payloads differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BspKind {
    Physics,
    Drawing,
    Cell,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PortalPoly {
    pub portal_index: i16,
    pub polygon_id: i16,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum BspNode {
    /// Interior splitting node. The tag (BPnn, BPIn, BpIN, BpnN, BPIN, BPnN,
    /// BPOL, BpIn) encodes which children exist.
    Split {
        tag: String,
        plane: Plane,
        pos: Option<Box<BspNode>>,
        neg: Option<Box<BspNode>>,
        /// Absent for cell trees.
        sphere: Option<Sphere>,
        /// Drawing trees only.
        polys: Vec<u16>,
    },
    Leaf {
        index: i32,
        /// Physics trees only.
        solid: Option<i32>,
        sphere: Option<Sphere>,
        polys: Vec<u16>,
    },
    Portal {
        plane: Plane,
        pos: Box<BspNode>,
        neg: Box<BspNode>,
        sphere: Option<Sphere>,
        polys: Vec<u16>,
        portals: Vec<PortalPoly>,
    },
}

impl BspNode {
    pub fn parse(r: &mut Reader, kind: BspKind) -> Result<Self> {
        let raw = r.bytes(4)?;
        // Tags are stored reversed ("nnPB" on disk for "BPnn").
        let tag: String = raw.iter().rev().map(|&b| b as char).collect();
        match tag.as_str() {
            "LEAF" => {
                let index = r.i32()?;
                if kind == BspKind::Physics {
                    let solid = r.i32()?;
                    let sphere = Sphere::parse(r)?;
                    let polys = r.list(|r| r.u16())?;
                    Ok(BspNode::Leaf {
                        index,
                        solid: Some(solid),
                        sphere: Some(sphere),
                        polys,
                    })
                } else {
                    Ok(BspNode::Leaf {
                        index,
                        solid: None,
                        sphere: None,
                        polys: Vec::new(),
                    })
                }
            }
            "PORT" => {
                let plane = Plane::parse(r)?;
                let pos = Box::new(BspNode::parse(r, kind)?);
                let neg = Box::new(BspNode::parse(r, kind)?);
                let (mut sphere, mut polys, mut portals) = (None, Vec::new(), Vec::new());
                if kind == BspKind::Drawing {
                    sphere = Some(Sphere::parse(r)?);
                    let n_polys = r.u32()? as usize;
                    let n_portals = r.u32()? as usize;
                    polys = r.fixed(n_polys, &mut |r: &mut Reader| r.u16())?;
                    portals = r.fixed(n_portals, &mut |r: &mut Reader| {
                        Ok(PortalPoly {
                            portal_index: r.i16()?,
                            polygon_id: r.i16()?,
                        })
                    })?;
                }
                Ok(BspNode::Portal {
                    plane,
                    pos,
                    neg,
                    sphere,
                    polys,
                    portals,
                })
            }
            // Interior nodes. Second char P = has positive child (except the
            // childless BPOL "polygon" node), fourth char N = has negative
            // child. Observed tags: BPnn BPIn (pos only), BpIN BpnN (neg only),
            // BPIN BPnN (both), BPOL BpIn (none).
            t if t.starts_with('B') => {
                let plane = Plane::parse(r)?;
                let has_pos = matches!(t, "BPnn" | "BPIn" | "BPIN" | "BPnN");
                let has_neg = matches!(t, "BpIN" | "BpnN" | "BPIN" | "BPnN");
                let pos = if has_pos {
                    Some(Box::new(BspNode::parse(r, kind)?))
                } else {
                    None
                };
                let neg = if has_neg {
                    Some(Box::new(BspNode::parse(r, kind)?))
                } else {
                    None
                };
                let (mut sphere, mut polys) = (None, Vec::new());
                if kind != BspKind::Cell {
                    sphere = Some(Sphere::parse(r)?);
                    if kind == BspKind::Drawing {
                        polys = r.list(|r| r.u16())?;
                    }
                }
                Ok(BspNode::Split {
                    tag,
                    plane,
                    pos,
                    neg,
                    sphere,
                    polys,
                })
            }
            _ => Err(Error::Invalid {
                what: "bsp node tag",
                detail: format!("{tag:?} at {}", r.pos() - 4),
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GfxObj {
    pub id: u32,
    pub flags: u32,
    /// Surface (0x08) ids referenced by polygon surface indices.
    pub surfaces: Vec<u32>,
    pub vertex_type: i32,
    /// `(vertex id, vertex)`; ids are what polygons reference.
    pub vertices: Vec<(u16, Vertex)>,
    pub physics_polygons: Vec<(u16, Polygon)>,
    pub physics_bsp: Option<BspNode>,
    pub sort_center: Vec3,
    pub polygons: Vec<(u16, Polygon)>,
    pub drawing_bsp: Option<BspNode>,
    /// GfxObjDegradeInfo (0x11) id.
    pub did_degrade: Option<u32>,
}

impl GfxObj {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let flags = r.u32()?;
        let surfaces = r.packed_list(|r| r.u32())?;
        let vertex_type = r.i32()?;
        let n_verts = r.u32()? as usize;
        if vertex_type != 1 {
            return Err(Error::Unsupported {
                what: "vertex type",
                value: vertex_type as u32,
            });
        }
        let vertices = r.fixed(n_verts, &mut |r: &mut Reader| {
            Ok((r.u16()?, Vertex::parse(r)?))
        })?;
        let (mut physics_polygons, mut physics_bsp) = (Vec::new(), None);
        if flags & flags::HAS_PHYSICS != 0 {
            physics_polygons = r.packed_map(|r| r.u16(), Polygon::parse)?;
            physics_bsp = Some(BspNode::parse(&mut r, BspKind::Physics)?);
        }
        let sort_center = r.vec3()?;
        let (mut polygons, mut drawing_bsp) = (Vec::new(), None);
        if flags & flags::HAS_DRAWING != 0 {
            polygons = r.packed_map(|r| r.u16(), Polygon::parse)?;
            drawing_bsp = Some(BspNode::parse(&mut r, BspKind::Drawing)?);
        }
        let did_degrade = if flags & flags::HAS_DID_DEGRADE != 0 {
            Some(r.u32()?)
        } else {
            None
        };
        r.finish()?;
        Ok(GfxObj {
            id,
            flags,
            surfaces,
            vertex_type,
            vertices,
            physics_polygons,
            physics_bsp,
            sort_center,
            polygons,
            drawing_bsp,
            did_degrade,
        })
    }
}
