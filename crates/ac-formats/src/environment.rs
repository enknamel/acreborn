//! Environment (0x0D): the geometry of interior cells. One file holds
//! several `CellStruct`s (rooms/corridors) that `EnvCell`s instantiate.

use serde::Serialize;

use crate::gfxobj::{BspKind, BspNode, Polygon, Vertex};
use crate::{expect_id, Error, Reader, Result};

#[derive(Debug, Clone, Serialize)]
pub struct CellStruct {
    pub vertex_type: i32,
    pub vertices: Vec<(u16, Vertex)>,
    pub polygons: Vec<(u16, Polygon)>,
    /// Polygon ids that are portals to neighbouring cells.
    pub portals: Vec<u16>,
    pub cell_bsp: BspNode,
    pub physics_polygons: Vec<(u16, Polygon)>,
    pub physics_bsp: BspNode,
    pub drawing_bsp: Option<BspNode>,
}

impl CellStruct {
    fn parse(r: &mut Reader) -> Result<Self> {
        let n_polygons = r.u32()? as usize;
        let n_physics_polygons = r.u32()? as usize;
        let n_portals = r.u32()? as usize;
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
        let polygons = r.fixed(n_polygons, &mut |r: &mut Reader| {
            Ok((r.u16()?, Polygon::parse(r)?))
        })?;
        let portals = r.fixed(n_portals, &mut |r: &mut Reader| r.u16())?;
        r.align4()?;
        let cell_bsp = BspNode::parse(r, BspKind::Cell)?;
        let physics_polygons = r.fixed(n_physics_polygons, &mut |r: &mut Reader| {
            Ok((r.u16()?, Polygon::parse(r)?))
        })?;
        let physics_bsp = BspNode::parse(r, BspKind::Physics)?;
        let drawing_bsp = if r.u32()? != 0 {
            Some(BspNode::parse(r, BspKind::Drawing)?)
        } else {
            None
        };
        r.align4()?;
        Ok(CellStruct {
            vertex_type,
            vertices,
            polygons,
            portals,
            cell_bsp,
            physics_polygons,
            physics_bsp,
            drawing_bsp,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Environment {
    pub id: u32,
    pub cells: Vec<(u32, CellStruct)>,
}

impl Environment {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let cells = r.map(|r| r.u32(), CellStruct::parse)?;
        r.finish()?;
        Ok(Environment { id, cells })
    }
}
