//! Region (0x13, singleton `0x13000000`): world constants. Landblock
//! geometry, the terrain height table, game calendar, sky/day cycles,
//! ambient sounds, scenery per terrain type, and terrain texture merging.

use serde::Serialize;

use crate::{expect_id, Reader, Result};

#[derive(Debug, Clone, Serialize)]
pub struct LandDefs {
    pub num_block_length: i32,
    pub num_block_width: i32,
    pub square_length: f32,
    pub lblock_length: i32,
    pub vertex_per_cell: i32,
    pub max_obj_height: f32,
    pub sky_height: f32,
    pub road_width: f32,
    /// 256 entries; `CellLandblock::height` indexes into this.
    pub land_height_table: Vec<f32>,
}

impl LandDefs {
    fn parse(r: &mut Reader) -> Result<Self> {
        Ok(LandDefs {
            num_block_length: r.i32()?,
            num_block_width: r.i32()?,
            square_length: r.f32()?,
            lblock_length: r.i32()?,
            vertex_per_cell: r.i32()?,
            max_obj_height: r.f32()?,
            sky_height: r.f32()?,
            road_width: r.f32()?,
            land_height_table: r.fixed(256, &mut |r: &mut Reader| r.f32())?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TimeOfDay {
    pub start: f32,
    pub is_night: bool,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Season {
    pub start_date: u32,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GameTime {
    pub zero_time_of_year: f64,
    pub zero_year: u32,
    pub day_length: f32,
    pub days_per_year: u32,
    pub year_spec: String,
    pub times_of_day: Vec<TimeOfDay>,
    pub days_of_week: Vec<String>,
    pub seasons: Vec<Season>,
}

fn aligned_string(r: &mut Reader) -> Result<String> {
    let s = r.pstring16()?;
    r.align4()?;
    Ok(s)
}

impl GameTime {
    fn parse(r: &mut Reader) -> Result<Self> {
        Ok(GameTime {
            zero_time_of_year: r.f64()?,
            zero_year: r.u32()?,
            day_length: r.f32()?,
            days_per_year: r.u32()?,
            year_spec: aligned_string(r)?,
            times_of_day: r.list(|r| {
                Ok(TimeOfDay {
                    start: r.f32()?,
                    is_night: r.u32()? == 1,
                    name: aligned_string(r)?,
                })
            })?,
            days_of_week: r.list(aligned_string)?,
            seasons: r.list(|r| {
                Ok(Season {
                    start_date: r.u32()?,
                    name: aligned_string(r)?,
                })
            })?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SkyObject {
    pub begin_time: f32,
    pub end_time: f32,
    pub begin_angle: f32,
    pub end_angle: f32,
    pub tex_velocity_x: f32,
    pub tex_velocity_y: f32,
    pub default_gfxobj_id: u32,
    pub default_pes_id: u32,
    pub properties: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkyObjectReplace {
    pub object_index: u32,
    pub gfxobj_id: u32,
    pub rotate: f32,
    pub transparent: f32,
    pub luminosity: f32,
    pub max_bright: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkyTimeOfDay {
    pub begin: f32,
    pub dir_bright: f32,
    pub dir_heading: f32,
    pub dir_pitch: f32,
    pub dir_color: u32,
    pub amb_bright: f32,
    pub amb_color: u32,
    pub min_world_fog: f32,
    pub max_world_fog: f32,
    pub world_fog_color: u32,
    pub world_fog: u32,
    pub sky_obj_replace: Vec<SkyObjectReplace>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DayGroup {
    pub chance_of_occur: f32,
    pub day_name: String,
    pub sky_objects: Vec<SkyObject>,
    pub sky_time: Vec<SkyTimeOfDay>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkyDesc {
    pub tick_size: f64,
    pub light_tick_size: f64,
    pub day_groups: Vec<DayGroup>,
}

impl SkyDesc {
    fn parse(r: &mut Reader) -> Result<Self> {
        let tick_size = r.f64()?;
        let light_tick_size = r.f64()?;
        r.align4()?;
        let day_groups = r.list(|r| {
            let chance_of_occur = r.f32()?;
            let day_name = aligned_string(r)?;
            let sky_objects = r.list(|r| {
                let o = SkyObject {
                    begin_time: r.f32()?,
                    end_time: r.f32()?,
                    begin_angle: r.f32()?,
                    end_angle: r.f32()?,
                    tex_velocity_x: r.f32()?,
                    tex_velocity_y: r.f32()?,
                    default_gfxobj_id: r.u32()?,
                    default_pes_id: r.u32()?,
                    properties: r.u32()?,
                };
                r.align4()?;
                Ok(o)
            })?;
            let sky_time = r.list(|r| {
                let mut t = SkyTimeOfDay {
                    begin: r.f32()?,
                    dir_bright: r.f32()?,
                    dir_heading: r.f32()?,
                    dir_pitch: r.f32()?,
                    dir_color: r.u32()?,
                    amb_bright: r.f32()?,
                    amb_color: r.u32()?,
                    min_world_fog: r.f32()?,
                    max_world_fog: r.f32()?,
                    world_fog_color: r.u32()?,
                    world_fog: r.u32()?,
                    sky_obj_replace: Vec::new(),
                };
                r.align4()?;
                t.sky_obj_replace = r.list(|r| {
                    let s = SkyObjectReplace {
                        object_index: r.u32()?,
                        gfxobj_id: r.u32()?,
                        rotate: r.f32()?,
                        transparent: r.f32()?,
                        luminosity: r.f32()?,
                        max_bright: r.f32()?,
                    };
                    r.align4()?;
                    Ok(s)
                })?;
                Ok(t)
            })?;
            Ok(DayGroup {
                chance_of_occur,
                day_name,
                sky_objects,
                sky_time,
            })
        })?;
        Ok(SkyDesc {
            tick_size,
            light_tick_size,
            day_groups,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AmbientSound {
    pub sound_type: u32,
    pub volume: f32,
    pub base_chance: f32,
    pub min_rate: f32,
    pub max_rate: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct AmbientStb {
    pub stb_id: u32,
    pub sounds: Vec<AmbientSound>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SceneType {
    pub stb_index: u32,
    /// Scene (0x12) ids.
    pub scenes: Vec<u32>,
}

impl SceneType {
    fn parse(r: &mut Reader) -> Result<Self> {
        Ok(SceneType {
            stb_index: r.u32()?,
            scenes: r.list(|r| r.u32())?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TerrainType {
    pub name: String,
    pub color: u32,
    /// Indices into `Region::scene_types`.
    pub scene_types: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerrainTex {
    pub tex_gid: u32,
    pub tex_tiling: u32,
    pub max_vert_bright: u32,
    pub min_vert_bright: u32,
    pub max_vert_saturate: u32,
    pub min_vert_saturate: u32,
    pub max_vert_hue: u32,
    pub min_vert_hue: u32,
    pub detail_tex_tiling: u32,
    pub detail_tex_gid: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TexMerge {
    pub base_tex_size: u32,
    /// `(tcode, texture id)` alpha maps for terrain corners.
    pub corner_terrain_maps: Vec<(u32, u32)>,
    pub side_terrain_maps: Vec<(u32, u32)>,
    /// `(rcode, texture id)` alpha maps for roads.
    pub road_maps: Vec<(u32, u32)>,
    /// `(terrain type, texture description)`.
    pub terrain_desc: Vec<(u32, TerrainTex)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionMisc {
    pub version: u32,
    pub game_map_id: u32,
    pub autotest_map_id: u32,
    pub autotest_map_size: u32,
    pub clear_cell_id: u32,
    pub clear_monster_id: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Region {
    pub id: u32,
    pub region_number: u32,
    pub version: u32,
    pub region_name: String,
    pub land_defs: LandDefs,
    pub game_time: GameTime,
    pub parts_mask: u32,
    pub sky: Option<SkyDesc>,
    pub ambient_sounds: Vec<AmbientStb>,
    pub scene_types: Vec<SceneType>,
    pub terrain_types: Vec<TerrainType>,
    pub land_surf_type: u32,
    pub tex_merge: Option<TexMerge>,
    pub misc: Option<RegionMisc>,
}

impl Region {
    pub const ID: u32 = 0x1300_0000;

    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let region_number = r.u32()?;
        let version = r.u32()?;
        let region_name = aligned_string(&mut r)?;
        let land_defs = LandDefs::parse(&mut r)?;
        let game_time = GameTime::parse(&mut r)?;
        let parts_mask = r.u32()?;
        let sky = if parts_mask & 0x10 != 0 {
            Some(SkyDesc::parse(&mut r)?)
        } else {
            None
        };
        let ambient_sounds = if parts_mask & 0x01 != 0 {
            r.list(|r| {
                Ok(AmbientStb {
                    stb_id: r.u32()?,
                    sounds: r.list(|r| {
                        Ok(AmbientSound {
                            sound_type: r.u32()?,
                            volume: r.f32()?,
                            base_chance: r.f32()?,
                            min_rate: r.f32()?,
                            max_rate: r.f32()?,
                        })
                    })?,
                })
            })?
        } else {
            Vec::new()
        };
        let scene_types = if parts_mask & 0x02 != 0 {
            r.list(SceneType::parse)?
        } else {
            Vec::new()
        };
        let terrain_types = r.list(|r| {
            Ok(TerrainType {
                name: aligned_string(r)?,
                color: r.u32()?,
                scene_types: r.list(|r| r.u32())?,
            })
        })?;
        let land_surf_type = r.u32()?;
        let tex_merge = if land_surf_type == 1 {
            None
        } else {
            let pair = |r: &mut Reader| Ok((r.u32()?, r.u32()?));
            Some(TexMerge {
                base_tex_size: r.u32()?,
                corner_terrain_maps: r.list(pair)?,
                side_terrain_maps: r.list(pair)?,
                road_maps: r.list(pair)?,
                terrain_desc: r.list(|r| {
                    Ok((
                        r.u32()?,
                        TerrainTex {
                            tex_gid: r.u32()?,
                            tex_tiling: r.u32()?,
                            max_vert_bright: r.u32()?,
                            min_vert_bright: r.u32()?,
                            max_vert_saturate: r.u32()?,
                            min_vert_saturate: r.u32()?,
                            max_vert_hue: r.u32()?,
                            min_vert_hue: r.u32()?,
                            detail_tex_tiling: r.u32()?,
                            detail_tex_gid: r.u32()?,
                        },
                    ))
                })?,
            })
        };
        let misc = if parts_mask & 0x0200 != 0 {
            Some(RegionMisc {
                version: r.u32()?,
                game_map_id: r.u32()?,
                autotest_map_id: r.u32()?,
                autotest_map_size: r.u32()?,
                clear_cell_id: r.u32()?,
                clear_monster_id: r.u32()?,
            })
        } else {
            None
        };
        r.finish()?;
        Ok(Region {
            id,
            region_number,
            version,
            region_name,
            land_defs,
            game_time,
            parts_mask,
            sky,
            ambient_sounds,
            scene_types,
            terrain_types,
            land_surf_type,
            tex_merge,
            misc,
        })
    }
}
