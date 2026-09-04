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
