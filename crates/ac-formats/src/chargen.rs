//! CharGen (0x0E000002): character creation tables. Starter areas, and per
//! heritage group the attribute/skill credits, skill costs, templates and
//! per-gender appearance option lists.

use serde::Serialize;

use crate::geom::Frame;
use crate::{expect_id, Reader, Result};

#[derive(Debug, Clone, Serialize)]
pub struct Position {
    pub cell_id: u32,
    pub frame: Frame,
}

#[derive(Debug, Clone, Serialize)]
pub struct StarterArea {
    pub name: String,
    pub locations: Vec<Position>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillCg {
    pub skill: u32,
    pub normal_cost: i32,
    pub primary_cost: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TemplateCg {
    pub name: String,
    pub icon: u32,
    pub title: u32,
    pub strength: u32,
    pub endurance: u32,
    pub coordination: u32,
    pub quickness: u32,
    pub focus: u32,
    pub self_: u32,
    pub normal_skills: Vec<u32>,
    pub primary_skills: Vec<u32>,
}

/// Appearance overrides: palette, texture and part swaps on a Setup.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ObjDesc {
    pub palette_id: u32,
    /// (sub palette id, offset, num colors), both in colors (the file stores
    /// them in units of 8; a zero length means the whole 2048-color palette).
    pub sub_palettes: Vec<(u32, u32, u32)>,
    /// (part index, old texture, new texture)
    pub texture_changes: Vec<(u8, u32, u32)>,
    /// (part index, gfxobj id)
    pub anim_part_changes: Vec<(u8, u32)>,
}

impl ObjDesc {
    pub fn parse(r: &mut Reader) -> Result<Self> {
        r.align4()?;
        let _eleven = r.u8()?;
        let n_pal = r.u8()? as usize;
        let n_tex = r.u8()? as usize;
        let n_parts = r.u8()? as usize;
        let palette_id = if n_pal > 0 {
            r.data_id_of_type(0x0400_0000)?
        } else {
            0
        };
        let sub_palettes = r.fixed(n_pal, &mut |r: &mut Reader| {
            let id = r.data_id_of_type(0x0400_0000)?;
            let offset = r.u8()? as u32 * 8;
            let n = r.u8()? as u32;
            Ok((id, offset, if n == 0 { 256 * 8 } else { n * 8 }))
        })?;
        let texture_changes = r.fixed(n_tex, &mut |r: &mut Reader| {
            Ok((
                r.u8()?,
                r.data_id_of_type(0x0500_0000)?,
                r.data_id_of_type(0x0500_0000)?,
            ))
        })?;
        let anim_part_changes = r.fixed(n_parts, &mut |r: &mut Reader| {
            Ok((r.u8()?, r.data_id_of_type(0x0100_0000)?))
        })?;
        r.align4()?;
        Ok(ObjDesc {
            palette_id,
            sub_palettes,
            texture_changes,
            anim_part_changes,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HairStyleCg {
    pub icon: u32,
    pub bald: bool,
    pub alternate_setup: u32,
    pub obj_desc: ObjDesc,
}

#[derive(Debug, Clone, Serialize)]
pub struct EyeStripCg {
    pub icon: u32,
    pub icon_bald: u32,
    pub obj_desc: ObjDesc,
    pub obj_desc_bald: ObjDesc,
}

#[derive(Debug, Clone, Serialize)]
pub struct FaceStripCg {
    pub icon: u32,
    pub obj_desc: ObjDesc,
}

#[derive(Debug, Clone, Serialize)]
pub struct GearCg {
    pub name: String,
    pub clothing_table: u32,
    pub weenie_default: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SexCg {
    pub name: String,
    pub scale: u32,
    pub setup_id: u32,
    pub sound_table: u32,
    pub icon: u32,
    pub base_palette: u32,
    pub skin_pal_set: u32,
    pub physics_table: u32,
    pub motion_table: u32,
    pub combat_table: u32,
    pub base_obj_desc: ObjDesc,
    pub hair_colors: Vec<u32>,
    pub hair_styles: Vec<HairStyleCg>,
    pub eye_colors: Vec<u32>,
    pub eye_strips: Vec<EyeStripCg>,
    pub nose_strips: Vec<FaceStripCg>,
    pub mouth_strips: Vec<FaceStripCg>,
    pub headgear: Vec<GearCg>,
    pub shirts: Vec<GearCg>,
    pub pants: Vec<GearCg>,
    pub footwear: Vec<GearCg>,
    pub clothing_colors: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HeritageGroupCg {
    pub name: String,
    pub icon: u32,
    pub setup_id: u32,
    pub environment_setup_id: u32,
    pub attribute_credits: u32,
    pub skill_credits: u32,
    pub primary_start_areas: Vec<i32>,
    pub secondary_start_areas: Vec<i32>,
    pub skills: Vec<SkillCg>,
    pub templates: Vec<TemplateCg>,
    /// (gender id, options); 1 = male, 2 = female.
    pub genders: Vec<(i32, SexCg)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CharGen {
    pub id: u32,
    pub starter_areas: Vec<StarterArea>,
    /// (heritage id, group); see `HeritageGroup` ids (1 = Aluvian ...).
    pub heritage_groups: Vec<(u32, HeritageGroupCg)>,
}

fn gear(r: &mut Reader) -> Result<GearCg> {
    Ok(GearCg {
        name: r.dotnet_string()?,
        clothing_table: r.u32()?,
        weenie_default: r.u32()?,
    })
}

fn face_strip(r: &mut Reader) -> Result<FaceStripCg> {
    Ok(FaceStripCg {
        icon: r.u32()?,
        obj_desc: ObjDesc::parse(r)?,
    })
}

impl CharGen {
    pub const ID: u32 = 0x0E00_0002;

    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let _ = r.u32()?;
        let starter_areas = r.packed_list(|r| {
            Ok(StarterArea {
                name: r.dotnet_string()?,
                locations: r.packed_list(|r| {
                    Ok(Position {
                        cell_id: r.u32()?,
                        frame: Frame::parse(r)?,
                    })
                })?,
            })
        })?;
        let _one = r.u8()?;
        let heritage_groups = r.packed_map(
            |r| r.u32(),
            |r| {
                Ok(HeritageGroupCg {
                    name: r.dotnet_string()?,
                    icon: r.u32()?,
                    setup_id: r.u32()?,
                    environment_setup_id: r.u32()?,
                    attribute_credits: r.u32()?,
                    skill_credits: r.u32()?,
                    primary_start_areas: r.packed_list(|r| r.i32())?,
                    secondary_start_areas: r.packed_list(|r| r.i32())?,
                    skills: r.packed_list(|r| {
                        Ok(SkillCg {
                            skill: r.u32()?,
                            normal_cost: r.i32()?,
                            primary_cost: r.i32()?,
                        })
                    })?,
                    templates: r.packed_list(|r| {
                        Ok(TemplateCg {
                            name: r.dotnet_string()?,
                            icon: r.u32()?,
                            title: r.u32()?,
                            strength: r.u32()?,
                            endurance: r.u32()?,
                            coordination: r.u32()?,
                            quickness: r.u32()?,
                            focus: r.u32()?,
                            self_: r.u32()?,
                            normal_skills: r.packed_list(|r| r.u32())?,
                            primary_skills: r.packed_list(|r| r.u32())?,
                        })
                    })?,
                    genders: {
                        let _one = r.u8()?;
                        r.packed_map(
                            |r| r.i32(),
                            |r| {
                                Ok(SexCg {
                                    name: r.dotnet_string()?,
                                    scale: r.u32()?,
                                    setup_id: r.u32()?,
                                    sound_table: r.u32()?,
                                    icon: r.u32()?,
                                    base_palette: r.u32()?,
                                    skin_pal_set: r.u32()?,
                                    physics_table: r.u32()?,
                                    motion_table: r.u32()?,
                                    combat_table: r.u32()?,
                                    base_obj_desc: ObjDesc::parse(r)?,
                                    hair_colors: r.packed_list(|r| r.u32())?,
                                    hair_styles: r.packed_list(|r| {
                                        Ok(HairStyleCg {
                                            icon: r.u32()?,
                                            bald: r.u8()? == 1,
                                            alternate_setup: r.u32()?,
                                            obj_desc: ObjDesc::parse(r)?,
                                        })
                                    })?,
                                    eye_colors: r.packed_list(|r| r.u32())?,
                                    eye_strips: r.packed_list(|r| {
                                        Ok(EyeStripCg {
                                            icon: r.u32()?,
                                            icon_bald: r.u32()?,
                                            obj_desc: ObjDesc::parse(r)?,
                                            obj_desc_bald: ObjDesc::parse(r)?,
                                        })
                                    })?,
                                    nose_strips: r.packed_list(face_strip)?,
                                    mouth_strips: r.packed_list(face_strip)?,
                                    headgear: r.packed_list(gear)?,
                                    shirts: r.packed_list(gear)?,
                                    pants: r.packed_list(gear)?,
                                    footwear: r.packed_list(gear)?,
                                    clothing_colors: r.packed_list(|r| r.u32())?,
                                })
                            },
                        )?
                    },
                })
            },
        )?;
        r.finish()?;
        Ok(CharGen {
            id,
            starter_areas,
            heritage_groups,
        })
    }
}
