//! A player character's look from the CharGen table (0x0E000002), composed
//! into the same palette / texture / part overrides the server sends in an
//! ObjDesc (ACE `WorldObject.AddBaseModelData` and `PlayerFactory`):
//!
//! * the hair style swaps the head part (index 16) for a head GfxObj and
//!   retextures its hair surface;
//! * eyes, nose and mouth are texture swaps on the head part, keyed by the
//!   placeholder SurfaceTexture ids the head GfxObj's surfaces reference;
//! * skin, hair and eye colours are sub-palettes over the sex's base
//!   palette at colour ranges 0..24, 24..32 and 32..40.

use ac_formats::chargen::{CharGen, HeritageGroupCg, SexCg};

use crate::model::Appearance;
use crate::{Assets, Error, Result};

/// Part index of the head in humanoid Setups.
pub const HEAD_PART: u8 = 16;

/// Character-creation choices. Style fields index the per-sex option lists;
/// shades are `0..=1` hues into a palette set.
#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    /// Heritage group id (1 = Aluvian, 2 = Gharu'ndim, 3 = Sho, ...).
    pub heritage: u32,
    /// 1 = male, 2 = female.
    pub gender: u32,
    pub hair_style: usize,
    pub hair_color: usize,
    pub hair_shade: f32,
    pub eyes: usize,
    pub eye_color: usize,
    pub nose: usize,
    pub mouth: usize,
    pub skin_shade: f32,
}

impl Default for Look {
    fn default() -> Self {
        Look {
            heritage: 1,
            gender: 1,
            hair_style: 0,
            hair_color: 0,
            hair_shade: 0.5,
            eyes: 0,
            eye_color: 0,
            nose: 0,
            mouth: 0,
            skin_shade: 0.5,
        }
    }
}

/// The wire-shaped description of a look: what a server would put in the
/// character's ObjectCreate. Sub-palette offset and length are in units of
/// 8 colours, as on the wire.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CharacterDesc {
    pub setup_id: u32,
    pub palette_id: u32,
    pub sub_palettes: Vec<(u32, u8, u8)>,
    pub texture_changes: Vec<(u8, u32, u32)>,
    pub part_changes: Vec<(u8, u32)>,
}

impl CharacterDesc {
    pub fn appearance(&self, assets: &Assets) -> Appearance {
        Appearance::from_obj_desc(
            assets,
            self.palette_id,
            &self.sub_palettes,
            &self.texture_changes,
            &self.part_changes,
        )
    }
}

/// Heritage group id from a name (case-insensitive prefix, e.g. "aluvian")
/// or a decimal id.
pub fn heritage_id(cg: &CharGen, name_or_id: &str) -> Option<u32> {
    if let Ok(id) = name_or_id.parse::<u32>() {
        return cg
            .heritage_groups
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(i, _)| *i);
    }
    let want = name_or_id.to_ascii_lowercase();
    cg.heritage_groups
        .iter()
        .find(|(_, g)| g.name.to_ascii_lowercase().starts_with(&want))
        .map(|(i, _)| *i)
}

fn options<'a>(cg: &'a CharGen, look: &Look) -> Result<(&'a HeritageGroupCg, &'a SexCg)> {
    let (_, group) = cg
        .heritage_groups
        .iter()
        .find(|(id, _)| *id == look.heritage)
        .ok_or_else(|| Error::Other(format!("chargen: no heritage group {}", look.heritage)))?;
    let (_, sex) = group
        .genders
        .iter()
        .find(|(id, _)| *id == look.gender as i32)
        .ok_or_else(|| {
            Error::Other(format!(
                "chargen: {} has no gender {}",
                group.name, look.gender
            ))
        })?;
    Ok((group, sex))
}

/// A Palette (0x04) id from a palette id or a PaletteSet (0x0F) id at `shade`.
fn palette_at(assets: &Assets, id: u32, shade: f32) -> Result<u32> {
    if id >> 24 == 0x0F {
        assets
            .palette_set(id)?
            .palette_for_shade(shade)
            .ok_or_else(|| Error::Other(format!("{id:#010x}: empty palette set")))
    } else {
        Ok(id)
    }
}

fn pick<'a, T>(list: &'a [T], idx: usize, what: &str) -> Result<&'a T> {
    list.get(idx).ok_or_else(|| {
        Error::Other(format!(
            "chargen: {what} {idx} out of range (have {})",
            list.len()
        ))
    })
}

/// Compose a look into a Setup id plus ObjDesc-shaped overrides.
pub fn describe(assets: &Assets, look: &Look) -> Result<CharacterDesc> {
    let cg = assets.chargen()?;
    let (_, sex) = options(&cg, look)?;
    let hair = pick(&sex.hair_styles, look.hair_style, "hair style")?;
    let mut d = CharacterDesc {
        setup_id: if hair.alternate_setup != 0 {
            hair.alternate_setup
        } else {
            sex.setup_id
        },
        palette_id: sex.base_palette,
        ..Default::default()
    };

    // Head and hair. One part change is a head object; several (Gear Knight,
    // Olthoi "body styles") are applied wholesale.
    let hd = &hair.obj_desc;
    if hd.anim_part_changes.len() == 1 {
        d.part_changes.push((HEAD_PART, hd.anim_part_changes[0].1));
    } else {
        d.part_changes.extend(hd.anim_part_changes.iter().copied());
        d.texture_changes.extend(hd.texture_changes.iter().copied());
    }
    if let Some(&(_, old, new)) = hd.texture_changes.first() {
        d.texture_changes.push((HEAD_PART, old, new));
    }
    let hair_set = pick(&sex.hair_colors, look.hair_color, "hair colour")?;
    d.sub_palettes
        .push((palette_at(assets, *hair_set, look.hair_shade)?, 3, 1));

    // Skin.
    d.sub_palettes
        .push((palette_at(assets, sex.skin_pal_set, look.skin_shade)?, 0, 3));

    // Face strips.
    let eyes = pick(&sex.eye_strips, look.eyes, "eyes")?;
    let eyes_desc = if hair.bald {
        &eyes.obj_desc_bald
    } else {
        &eyes.obj_desc
    };
    if let Some(&(_, old, new)) = eyes_desc.texture_changes.first() {
        d.texture_changes.push((HEAD_PART, old, new));
    }
    let eye_color = pick(&sex.eye_colors, look.eye_color, "eye colour")?;
    d.sub_palettes
        .push((palette_at(assets, *eye_color, 0.0)?, 4, 1));
    let nose = pick(&sex.nose_strips, look.nose, "nose")?;
    if let Some(&(_, old, new)) = nose.obj_desc.texture_changes.first() {
        d.texture_changes.push((HEAD_PART, old, new));
    }
    let mouth = pick(&sex.mouth_strips, look.mouth, "mouth")?;
    if let Some(&(_, old, new)) = mouth.obj_desc.texture_changes.first() {
        d.texture_changes.push((HEAD_PART, old, new));
    }
    Ok(d)
}
