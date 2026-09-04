//! cell.dat terrain files.
//!
//! * `CellLandblock` (`LLLLFFFF`): the 9x9 height/terrain grid of one
//!   landblock (192 m square, 8x8 cells of 24 m).
//! * `LandblockInfo` (`LLLLFFFE`): static objects and buildings placed on
//!   the landblock; present only when the block has any.
//! * `EnvCell` (`LLLL0100+`): one interior cell (a room of a building or
//!   dungeon), referencing an `Environment` cell structure.

use serde::Serialize;

use crate::geom::Frame;
use crate::{expect_id, Reader, Result};

/// Bit layout of `CellLandblock::terrain` entries.
pub mod terrain {
    pub const ROAD_MASK: u16 = 0x3;
    pub const TYPE_MASK: u16 = 0x7C;
    pub const TYPE_SHIFT: u16 = 2;
    pub const SCENERY_MASK: u16 = 0xF800;
    pub const SCENERY_SHIFT: u16 = 11;

    pub fn road(t: u16) -> u16 {
        t & ROAD_MASK
    }
    pub fn terrain_type(t: u16) -> u16 {
        (t & TYPE_MASK) >> TYPE_SHIFT
    }
    pub fn scenery(t: u16) -> u16 {
        (t & SCENERY_MASK) >> SCENERY_SHIFT
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CellLandblock {
    pub id: u32,
    /// True when a `LandblockInfo` file exists for this block.
    pub has_objects: bool,
    /// 81 entries, x-major: index = x * 9 + y.
    pub terrain: Vec<u16>,
    /// 81 entries indexing `Region::land_defs.land_height_table`.
    pub height: Vec<u8>,
}

impl CellLandblock {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let has_objects = r.u32()? == 1;
        let terrain = r.fixed(81, &mut |r: &mut Reader| r.u16())?;
        let height = r.fixed(81, &mut |r: &mut Reader| r.u8())?;
        r.align4()?;
        r.finish()?;
        Ok(CellLandblock {
            id,
            has_objects,
            terrain,
            height,
        })
    }
}

/// A placed static object: model (GfxObj or Setup) id and its frame.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stab {
    pub id: u32,
    pub frame: Frame,
}

impl Stab {
    fn parse(r: &mut Reader) -> Result<Self> {
        Ok(Stab {
            id: r.u32()?,
            frame: Frame::parse(r)?,
        })
    }
}

pub mod portal_flags {
    pub const EXACT_MATCH: u16 = 0x1;
    pub const PORTAL_SIDE: u16 = 0x2;
}

/// A building's connection to the interior cells behind one of its portals.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BuildingPortal {
    pub flags: u16,
    pub other_cell_id: u16,
    pub other_portal_id: u16,
    pub stabs: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BuildingInfo {
    pub model_id: u32,
    pub frame: Frame,
    pub num_leaves: u32,
    pub portals: Vec<BuildingPortal>,
}

impl BuildingInfo {
    fn parse(r: &mut Reader) -> Result<Self> {
        let model_id = r.u32()?;
        let frame = Frame::parse(r)?;
        let num_leaves = r.u32()?;
        let portals = r.list(|r| {
            let flags = r.u16()?;
            let other_cell_id = r.u16()?;
            let other_portal_id = r.u16()?;
            let n = r.u16()? as usize;
            let stabs = r.fixed(n, &mut |r: &mut Reader| r.u16())?;
            r.align4()?;
            Ok(BuildingPortal {
                flags,
                other_cell_id,
                other_portal_id,
                stabs,
            })
        })?;
        Ok(BuildingInfo {
            model_id,
            frame,
            num_leaves,
            portals,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LandblockInfo {
    pub id: u32,
    /// Number of `EnvCell` files (`LLLL0100 ..`) belonging to this block.
    pub num_cells: u32,
    pub objects: Vec<Stab>,
    pub pack_mask: u16,
    pub buildings: Vec<BuildingInfo>,
    /// `(cell id, restriction object id)`; present when `pack_mask & 1`.
    pub restriction_tables: Vec<(u32, u32)>,
}

impl LandblockInfo {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let num_cells = r.u32()?;
        let objects = r.list(Stab::parse)?;
        let n_buildings = r.u16()? as usize;
        let pack_mask = r.u16()?;
        let buildings = r.fixed(n_buildings, &mut BuildingInfo::parse)?;
        let restriction_tables = if pack_mask & 1 != 0 {
            r.packed_hash_table(|r| r.u32(), |r| r.u32())?
        } else {
            Vec::new()
        };
        r.finish()?;
        Ok(LandblockInfo {
            id,
            num_cells,
            objects,
            pack_mask,
            buildings,
            restriction_tables,
        })
    }
}

pub mod env_cell_flags {
    pub const SEEN_OUTSIDE: u32 = 0x1;
    pub const HAS_STATIC_OBJS: u32 = 0x2;
    pub const HAS_RESTRICTION_OBJ: u32 = 0x8;
}

/// Connection from one interior cell to another through a polygon.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CellPortal {
    pub flags: u16,
    pub polygon_id: u16,
    pub other_cell_id: u16,
    pub other_portal_id: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvCell {
    pub id: u32,
    pub flags: u32,
    /// Surface (0x08) ids, one per surface slot of the cell structure.
    pub surfaces: Vec<u32>,
    /// Environment (0x0D) id.
    pub environment_id: u32,
    /// Index of the `CellStruct` within the environment.
    pub cell_structure: u16,
    pub position: Frame,
    pub portals: Vec<CellPortal>,
    /// Other cells (low 16 bits of their ids) visible from this one.
    pub visible_cells: Vec<u16>,
    pub static_objects: Vec<Stab>,
    pub restriction_obj: Option<u32>,
}

impl EnvCell {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let flags = r.u32()?;
        let _cell_id_again = r.u32()?;
        let n_surfaces = r.u8()? as usize;
        let n_portals = r.u8()? as usize;
        let n_visible = r.u16()? as usize;
        let surfaces = r.fixed(n_surfaces, &mut |r: &mut Reader| {
            Ok(0x0800_0000 | r.u16()? as u32)
        })?;
        let environment_id = 0x0D00_0000 | r.u16()? as u32;
        let cell_structure = r.u16()?;
        let position = Frame::parse(&mut r)?;
        let portals = r.fixed(n_portals, &mut |r: &mut Reader| {
            Ok(CellPortal {
                flags: r.u16()?,
                polygon_id: r.u16()?,
                other_cell_id: r.u16()?,
                other_portal_id: r.u16()?,
            })
        })?;
        let visible_cells = r.fixed(n_visible, &mut |r: &mut Reader| r.u16())?;
        let static_objects = if flags & env_cell_flags::HAS_STATIC_OBJS != 0 {
            r.list(Stab::parse)?
        } else {
            Vec::new()
        };
        let restriction_obj = if flags & env_cell_flags::HAS_RESTRICTION_OBJ != 0 {
            Some(r.u32()?)
        } else {
            None
        };
        r.finish()?;
        Ok(EnvCell {
            id,
            flags,
            surfaces,
            environment_id,
            cell_structure,
            position,
            portals,
            visible_cells,
            static_objects,
            restriction_obj,
        })
    }
}
