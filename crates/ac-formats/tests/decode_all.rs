//! Decode every file of each supported kind from the real archives and
//! require exact byte consumption. Skipped unless `AC_DATA_DIR` is set.

use std::path::PathBuf;

use ac_dat::{DatArchive, FileKind};
use ac_formats::*;

fn archive(name: &str) -> Option<DatArchive> {
    let dir = PathBuf::from(std::env::var_os("AC_DATA_DIR")?);
    Some(DatArchive::open(dir.join(name)).unwrap())
}

fn check(kind: FileKind, parse: impl Fn(u32, &[u8]) -> Result<()>) {
    check_in("client_portal.dat", kind, parse)
}

fn check_in(name: &str, kind: FileKind, parse: impl Fn(u32, &[u8]) -> Result<()>) {
    let Some(dat) = archive(name) else {
        eprintln!("AC_DATA_DIR unset; skipping");
        return;
    };
    let mut n = 0;
    let mut failures = Vec::new();
    for e in dat.entries().filter(|e| dat.kind(e.id) == kind) {
        let bytes = dat.read(e.id).unwrap();
        if let Err(err) = parse(e.id, &bytes) {
            failures.push(format!("{:08X}: {err}", e.id));
        }
        n += 1;
    }
    assert!(n > 0, "no {kind:?} files found");
    assert!(
        failures.is_empty(),
        "{} of {n} {kind:?} files failed:\n{}",
        failures.len(),
        failures
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    eprintln!("{kind:?}: {n} ok");
}

#[test]
fn palettes() {
    check(FileKind::Palette, |id, b| {
        palette::Palette::parse(id, b).map(|_| ())
    });
}

#[test]
fn textures() {
    check(FileKind::Texture, |id, b| {
        texture::Texture::parse(id, b).map(|_| ())
    });
}

#[test]
fn surface_textures() {
    check(FileKind::SurfaceTexture, |id, b| {
        surface_texture::SurfaceTexture::parse(id, b).map(|_| ())
    });
}

#[test]
fn surfaces() {
    check(FileKind::Surface, |id, b| {
        surface::Surface::parse(id, b).map(|_| ())
    });
}

#[test]
fn gfxobjs() {
    check(FileKind::GfxObj, |id, b| {
        gfxobj::GfxObj::parse(id, b).map(|_| ())
    });
}

#[test]
fn setups() {
    check(FileKind::Setup, |id, b| {
        setup::Setup::parse(id, b).map(|_| ())
    });
}

#[test]
fn animations() {
    check(FileKind::Animation, |id, b| {
        animation::Animation::parse(id, b).map(|_| ())
    });
}

#[test]
fn environments() {
    check(FileKind::Environment, |id, b| {
        environment::Environment::parse(id, b).map(|_| ())
    });
}

#[test]
fn scenes() {
    check(FileKind::Scene, |id, b| {
        scene::Scene::parse(id, b).map(|_| ())
    });
}

#[test]
fn region() {
    check(FileKind::Region, |id, b| {
        region::Region::parse(id, b).map(|_| ())
    });
}

#[test]
fn cell_landblocks() {
    check_in("client_cell_1.dat", FileKind::LandBlock, |id, b| {
        landblock::CellLandblock::parse(id, b).map(|_| ())
    });
}

#[test]
fn landblock_infos() {
    check_in("client_cell_1.dat", FileKind::LandBlockInfo, |id, b| {
        landblock::LandblockInfo::parse(id, b).map(|_| ())
    });
}

#[test]
fn env_cells() {
    check_in("client_cell_1.dat", FileKind::EnvCell, |id, b| {
        landblock::EnvCell::parse(id, b).map(|_| ())
    });
}

/// Every texture must expand to RGBA of the declared size.
#[test]
fn textures_to_rgba() {
    let Some(dat) = archive("client_portal.dat") else {
        return;
    };
    let mut palettes = std::collections::HashMap::new();
    let mut formats = std::collections::BTreeMap::new();
    let mut failures = Vec::new();
    for e in dat
        .entries()
        .filter(|e| dat.kind(e.id) == FileKind::Texture)
    {
        let t = texture::Texture::parse(e.id, &dat.read(e.id).unwrap()).unwrap();
        let pal = t.default_palette.map(|pid| {
            palettes
                .entry(pid)
                .or_insert_with(|| {
                    palette::Palette::parse(pid, &dat.read(pid).unwrap())
                        .unwrap()
                        .colors
                        .clone()
                })
                .clone()
        });
        *formats.entry(format!("{:?}", t.format)).or_insert(0usize) += 1;
        if t.data_len == 0 {
            continue;
        }
        match t.to_rgba8(pal.as_deref()) {
            Ok(img) => {
                if t.format != texture::PixelFormat::CustomRawJpeg {
                    assert_eq!(
                        img.pixels.len(),
                        (t.width * t.height * 4) as usize,
                        "{:08X}",
                        e.id
                    );
                }
            }
            Err(err) => failures.push(format!("{:08X} {:?}: {err}", e.id, t.format)),
        }
    }
    eprintln!("formats: {formats:?}");
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn skill_table() {
    let Some(dat) = archive("client_portal.dat") else {
        return;
    };
    use skill_table::{attribute, SkillTable};
    let t = SkillTable::parse(SkillTable::ID, &dat.read(SkillTable::ID).unwrap()).unwrap();
    // 38 live skills; the retired weapon skills are not in the table.
    assert_eq!(t.skills.len(), 38, "{} skills", t.skills.len());
    // Melee Defense = (Quickness + Coordination) / 3
    let md = t.get(6).unwrap();
    assert_eq!(md.name, "Melee Defense");
    assert_eq!(md.formula.attr1, attribute::QUICKNESS);
    assert_eq!(md.formula.attr2, attribute::COORDINATION);
    assert_eq!(md.formula.divisor, 3);
    // Run = Quickness
    let run = t.get(24).unwrap();
    assert_eq!(run.name, "Run");
    assert_eq!(run.formula.attr1, attribute::QUICKNESS);
    assert_eq!(run.formula.attr2, attribute::NONE);
    assert_eq!(run.formula.divisor, 1);
    assert_eq!(run.min_level, 1, "Run is usable untrained");
    // Life Magic = (Focus + Self) / 4, needs training
    let lm = t.get(33).unwrap();
    assert_eq!(lm.name, "Life Magic");
    assert_eq!(lm.formula.attr1, attribute::FOCUS);
    assert_eq!(lm.formula.attr2, attribute::SELF);
    assert_eq!(lm.formula.divisor, 4);
    assert_eq!(lm.min_level, 2);
    assert!(t.get(999).is_none());
}

#[test]
fn chargen() {
    let Some(dat) = archive("client_portal.dat") else {
        return;
    };
    let cg = chargen::CharGen::parse(
        chargen::CharGen::ID,
        &dat.read(chargen::CharGen::ID).unwrap(),
    )
    .unwrap();
    assert!(
        cg.starter_areas.iter().any(|a| a.name == "Holtburg"),
        "{:?}",
        cg.starter_areas.iter().map(|a| &a.name).collect::<Vec<_>>()
    );
    let (id, aluvian) = cg
        .heritage_groups
        .iter()
        .find(|(_, h)| h.name == "Aluvian")
        .unwrap();
    assert_eq!(*id, 1);
    assert_eq!(aluvian.attribute_credits, 330);
    assert!(
        aluvian
            .templates
            .iter()
            .any(|t| t.name == "Swashbuckler" || t.name == "Warrior"),
        "{:?}",
        aluvian
            .templates
            .iter()
            .map(|t| &t.name)
            .collect::<Vec<_>>()
    );
    assert_eq!(aluvian.genders.len(), 2);
}

#[test]
fn motion_tables() {
    check(FileKind::MotionTable, |id, b| {
        motion_table::MotionTable::parse(id, b).map(|_| ())
    });
}

#[test]
fn particle_emitters() {
    check(FileKind::ParticleEmitter, |id, b| {
        particle_emitter::ParticleEmitterInfo::parse(id, b).map(|_| ())
    });
}

#[test]
fn physics_scripts() {
    check(FileKind::PhysicsScript, |id, b| {
        physics_script::PhysicsScript::parse(id, b).map(|_| ())
    });
}

#[test]
fn physics_script_tables() {
    check(FileKind::PhysicsScriptTable, |id, b| {
        physics_script_table::PhysicsScriptTable::parse(id, b).map(|_| ())
    });
}

#[test]
fn waves() {
    check(FileKind::Wave, |id, b| wave::Wave::parse(id, b).map(|_| ()));
}

#[test]
fn sound_tables() {
    check(FileKind::SoundTable, |id, b| {
        sound_table::SoundTable::parse(id, b).map(|_| ())
    });
}

/// Every wave must be a format the audio layer knows how to play, and its
/// RIFF form must be self-consistent.
#[test]
fn wave_formats() {
    let Some(dat) = archive("client_portal.dat") else {
        return;
    };
    let mut tags = std::collections::BTreeMap::new();
    for e in dat.entries().filter(|e| dat.kind(e.id) == FileKind::Wave) {
        let w = wave::Wave::parse(e.id, &dat.read(e.id).unwrap()).unwrap();
        let f = &w.format;
        *tags
            .entry((f.format_tag, f.channels, f.bits_per_sample, f.extra.len()))
            .or_insert(0usize) += 1;
        assert!(
            w.is_pcm() || w.is_mp3(),
            "{:08X}: unexpected format tag {:#x}",
            e.id,
            f.format_tag
        );
        let riff = w.to_riff();
        assert_eq!(&riff[0..4], b"RIFF", "{:08X}", e.id);
        assert_eq!(
            u32::from_le_bytes(riff[4..8].try_into().unwrap()) as usize + 8,
            riff.len(),
            "{:08X}",
            e.id
        );
    }
    eprintln!("wave (tag, channels, bits, extra): {tags:?}");
}

/// The human sound table (weenie DID 0x20000001; the Setup 0x02000001
/// leaves `default_sound_table` at 0) resolves the common combat and
/// movement sound types to existing waves.
#[test]
fn human_sound_table_resolves() {
    let Some(dat) = archive("client_portal.dat") else {
        return;
    };
    let setup = setup::Setup::parse(0x0200_0001, &dat.read(0x0200_0001).unwrap()).unwrap();
    let table_id = if setup.default_sound_table != 0 {
        setup.default_sound_table
    } else {
        0x2000_0001
    };
    let t = sound_table::SoundTable::parse(table_id, &dat.read(table_id).unwrap()).unwrap();
    eprintln!(
        "sound table {table_id:08X}: {} types: {:?}",
        t.sounds.len(),
        t.sound_types().collect::<Vec<_>>()
    );
    // Sound::Attack1 = 3, Sound::Wound1 = 0x0C, Sound::Death1 = 0x0F (the
    // client's Sound enum). Footsteps come from the terrain, not this table.
    for sound_type in [3u32, 0x0C, 0x0F] {
        let d = t
            .get(sound_type)
            .unwrap_or_else(|| panic!("sound type {sound_type} missing"));
        assert!(!d.entries.is_empty());
        for e in &d.entries {
            assert_eq!(dat.kind(e.wave_id), FileKind::Wave, "{:08X}", e.wave_id);
            assert!(e.probability > 0.0 && e.probability <= 1.0);
        }
    }
}

#[test]
fn spell_table() {
    let Some(dat) = archive("client_portal.dat") else {
        return;
    };
    use spell_table::{flags, school, spell_type, SpellTable, TypeData};
    let t = SpellTable::parse(SpellTable::ID, &dat.read(SpellTable::ID).unwrap()).unwrap();
    assert_eq!(t.spells.len(), 6266, "{} spells", t.spells.len());
    assert_eq!(t.spell_sets.len(), 139, "{} spell sets", t.spell_sets.len());
    let comps = spell_components::SpellComponentTable::parse(
        spell_components::SpellComponentTable::ID,
        &dat.read(spell_components::SpellComponentTable::ID).unwrap(),
    )
    .unwrap();
    // The descrambled formula of every spell must index the component
    // table; nearly every scarab-led formula ends with a talisman (a few
    // creature-only spells such as "Exploding Magma" do not).
    let mut levels = [0usize; 9];
    let (mut scarab_led, mut talisman_last) = (0, 0);
    for (id, s) in &t.spells {
        for c in s.formula() {
            assert!(
                comps.get(c).is_some(),
                "spell {id} {:?}: component {c} unknown ({:?})",
                s.name,
                s.components
            );
        }
        levels[s.level() as usize] += 1;
        if s.scarab_level() != 0 {
            scarab_led += 1;
            let last = s.formula().last().unwrap();
            if comps.get(last).unwrap().kind == spell_components::component_type::TALISMAN {
                talisman_last += 1;
            }
        }
    }
    eprintln!(
        "spells per level 0..8: {levels:?}; {scarab_led} scarab-led, {talisman_last} end in a talisman"
    );
    assert_eq!(levels[0], 0, "every spell has a level");
    assert!(scarab_led > 5000 && talisman_last * 100 > scarab_led * 95);
    // Strength Other I: creature enchantment, targeted, level 1.
    let (id, s) = t.find_by_name("Strength Other I").unwrap();
    assert_eq!(id, 1);
    assert_eq!(s.school, school::CREATURE);
    assert_eq!(s.level(), 1);
    assert_eq!(s.scarab_level(), 1);
    assert!(s.is_beneficial() && s.needs_target() && !s.is_self_targeted());
    assert_eq!(s.meta_spell_type, spell_type::ENCHANTMENT);
    assert!(matches!(s.type_data, TypeData::Enchantment { duration, .. } if duration > 0.0));
    assert_eq!(s.formula().collect::<Vec<_>>(), [1, 7, 33, 44, 49]);
    assert_eq!(comps.spell_words(s.formula()), "Malar Cazael");
    // Heal Self I: life boost, self only, level 1.
    let s = t.get(6).unwrap();
    assert_eq!(s.name, "Heal Self I");
    assert_eq!(s.school, school::LIFE);
    assert_eq!(s.level(), 1);
    assert!(s.is_self_targeted() && !s.needs_target());
    assert_eq!(s.meta_spell_type, spell_type::BOOST);
    assert_eq!(s.type_data, TypeData::None);
    assert_eq!(comps.spell_words(s.formula()), "Malar Zhapaj");
    // Acid Stream III..VI: war projectiles, levels 3..6 by power.
    for (id, level) in [(60, 3), (61, 4), (62, 5), (63, 6)] {
        let s = t.get(id).unwrap();
        assert!(s.name.starts_with("Acid Stream "), "{id}: {}", s.name);
        assert_eq!(s.level(), level, "{id}: power {}", s.power);
        assert_eq!(s.scarab_level(), level);
        assert_eq!(s.school, school::WAR);
        assert!(s.has_flag(flags::RESISTABLE) && s.needs_target());
        assert_eq!(s.meta_spell_type, spell_type::PROJECTILE);
        assert!(s.is_projectile());
    }
    // Shock Wave II: level 2 at power 50.
    let s = t.get(65).unwrap();
    assert_eq!(s.name, "Shock Wave II");
    assert_eq!((s.power, s.level()), (50, 2));
    // Mind Blossom: level 7 (power 300), platinum scarab.
    let s = t.get(2091).unwrap();
    assert_eq!(s.name, "Mind Blossom");
    assert_eq!((s.level(), s.scarab_level(), s.components[0]), (7, 7, 112));
    assert!(s.is_self_targeted());
    // Level 8 spells exist (power 400) and the client's scarab rule
    // disagrees with the power rule for them, as ACE notes.
    let s = t.get(4001).unwrap();
    assert_eq!(s.name, "Burning Earth");
    assert_eq!((s.level(), s.scarab_level()), (8, 6));
    assert!(t.get(0).is_none());
    // Spell sets: tiers are sorted and `active` picks the highest reached.
    let (_, set) = t.spell_sets.first().unwrap();
    assert!(set.tiers.windows(2).all(|w| w[0].0 < w[1].0));
    let (top, spells) = set.tiers.last().unwrap();
    assert_eq!(set.active(*top), spells.as_slice());
    assert_eq!(set.active(*top + 10), spells.as_slice());
}

#[test]
fn spell_components() {
    let Some(dat) = archive("client_portal.dat") else {
        return;
    };
    use spell_components::{component_type, SpellComponentTable};
    let t = SpellComponentTable::parse(
        SpellComponentTable::ID,
        &dat.read(SpellComponentTable::ID).unwrap(),
    )
    .unwrap();
    assert_eq!(t.components.len(), 163, "{} components", t.components.len());
    let lead = t.get(1).unwrap();
    assert_eq!(lead.name, "Lead Scarab");
    assert_eq!(lead.kind, component_type::SCARAB);
    assert_eq!(lead.icon_id >> 24, 0x06);
    let hyssop = t.get(7).unwrap();
    assert_eq!(
        (hyssop.name.as_str(), hyssop.text.as_str()),
        ("Hyssop", "Malar")
    );
    assert_eq!(hyssop.kind, component_type::HERB);
    assert_eq!(t.get(63).unwrap().name, "Red Taper");
    assert_eq!(t.get(63).unwrap().kind, component_type::TAPER);
    let poplar = t.get(49).unwrap();
    assert_eq!(poplar.kind, component_type::TALISMAN);
    assert!(poplar.gesture & 0x4000_0000 != 0 && poplar.time > 1.0);
    assert_eq!(t.get(198).unwrap().name, "Essence of Kemeroi");
    assert!(t.get(199).is_none());
    for (_, c) in &t.components {
        assert!(c.icon_id >> 24 == 0x06);
        assert!(c.kind >= 1 && c.kind <= 7, "{}: kind {}", c.name, c.kind);
    }
}
