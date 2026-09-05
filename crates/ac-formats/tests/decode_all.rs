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
