//! `acdat`: inspect and extract Turbine DAT archives.

use std::io::Write;
use std::path::PathBuf;

use ac_dat::{DatArchive, FileKind, Iteration, ITERATION_FILE_ID};
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Path to a .dat archive
    dat: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print the archive header and summary counts
    Info,
    /// List files: id, size, kind
    Ls {
        /// Only list files of this kind (e.g. GfxObj, LandBlock)
        #[arg(long)]
        kind: Option<String>,
    },
    /// Write one file's bytes to stdout
    Cat { id: String },
    /// Extract one file (or all files with --all) into a directory
    Extract {
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        all: bool,
        id: Option<String>,
    },
    /// Decode one file with ac-formats and print it as JSON
    Decode { id: String },
    /// Emit a manifest: id, offset, size, iteration, sha256 (for golden diffs)
    Manifest,
    /// Compare the archive against a manifest produced by `manifest` or AceDump
    Diff { manifest: PathBuf },
}

fn parse_id(s: &str) -> Result<u32> {
    let s = s.trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(s, 16).with_context(|| format!("bad file id {s:?}"))
}

fn decode_json(kind: FileKind, id: u32, b: &[u8]) -> Result<serde_json::Value> {
    use ac_formats::*;
    let v = match kind {
        FileKind::GfxObj => serde_json::to_value(gfxobj::GfxObj::parse(id, b)?)?,
        FileKind::Setup => serde_json::to_value(setup::Setup::parse(id, b)?)?,
        FileKind::Animation => serde_json::to_value(animation::Animation::parse(id, b)?)?,
        FileKind::Palette => serde_json::to_value(palette::Palette::parse(id, b)?)?,
        FileKind::SurfaceTexture => {
            serde_json::to_value(surface_texture::SurfaceTexture::parse(id, b)?)?
        }
        FileKind::Texture => serde_json::to_value(texture::Texture::parse(id, b)?)?,
        FileKind::Surface => serde_json::to_value(surface::Surface::parse(id, b)?)?,
        FileKind::Environment => serde_json::to_value(environment::Environment::parse(id, b)?)?,
        FileKind::Region => serde_json::to_value(region::Region::parse(id, b)?)?,
        FileKind::LandBlock => serde_json::to_value(landblock::CellLandblock::parse(id, b)?)?,
        FileKind::LandBlockInfo => serde_json::to_value(landblock::LandblockInfo::parse(id, b)?)?,
        FileKind::EnvCell => serde_json::to_value(landblock::EnvCell::parse(id, b)?)?,
        other => bail!("no decoder for {other:?} yet"),
    };
    Ok(v)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let dat =
        DatArchive::open(&cli.dat).with_context(|| format!("opening {}", cli.dat.display()))?;
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    match cli.cmd {
        Cmd::Info => {
            let h = dat.header();
            writeln!(
                out,
                "data_set:            {:?} (subset {})",
                h.data_set, h.data_subset
            )?;
            writeln!(out, "block_size:          {:#x}", h.block_size)?;
            writeln!(out, "file_size:           {} bytes", h.file_size)?;
            writeln!(out, "btree_root:          {:#x}", h.btree)?;
            writeln!(
                out,
                "free list:           head {:#x} tail {:#x} count {}",
                h.free_head, h.free_tail, h.free_count
            )?;
            writeln!(
                out,
                "lru:                 new {} old {} use {}",
                h.new_lru, h.old_lru, h.use_lru
            )?;
            writeln!(out, "master_map_id:       {:#010x}", h.master_map_id)?;
            writeln!(out, "engine_pack_version: {}", h.engine_pack_version)?;
            writeln!(out, "game_pack_version:   {}", h.game_pack_version)?;
            writeln!(out, "version_major:       {}", hex::encode(h.version_major))?;
            writeln!(out, "version_minor:       {}", h.version_minor)?;
            writeln!(out, "files:               {}", dat.len())?;
            if let Ok(bytes) = dat.read(ITERATION_FILE_ID) {
                match Iteration::parse(&bytes) {
                    Some(it) => writeln!(out, "iteration:           {} {:?}", it.total, it.ranges)?,
                    None => writeln!(out, "iteration:           <unparseable>")?,
                }
            }
            let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
            for e in dat.entries() {
                *kinds.entry(format!("{:?}", dat.kind(e.id))).or_default() += 1;
            }
            writeln!(out, "by kind:")?;
            for (k, n) in kinds {
                writeln!(out, "  {k:<24} {n}")?;
            }
        }
        Cmd::Ls { kind } => {
            for e in dat.entries() {
                let k = dat.kind(e.id);
                if let Some(want) = &kind {
                    if !format!("{k:?}").eq_ignore_ascii_case(want) {
                        continue;
                    }
                }
                writeln!(out, "{:08X}\t{}\t{:?}", e.id, e.size, k)?;
            }
        }
        Cmd::Cat { id } => {
            let bytes = dat.read(parse_id(&id)?)?;
            out.write_all(&bytes)?;
        }
        Cmd::Extract { out: dir, all, id } => {
            std::fs::create_dir_all(&dir)?;
            let ids: Vec<u32> = if all {
                dat.entries().map(|e| e.id).collect()
            } else {
                vec![parse_id(id.as_deref().context("give an id or --all")?)?]
            };
            for id in ids {
                let bytes = dat.read(id)?;
                let kind = dat.kind(id);
                let sub = if all {
                    dir.join(format!("{kind:?}"))
                } else {
                    dir.clone()
                };
                std::fs::create_dir_all(&sub)?;
                std::fs::write(sub.join(format!("{id:08X}.bin")), bytes)?;
            }
        }
        Cmd::Decode { id } => {
            let id = parse_id(&id)?;
            let bytes = dat.read(id)?;
            let json = decode_json(dat.kind(id), id, &bytes)?;
            serde_json::to_writer_pretty(&mut out, &json)?;
            writeln!(out)?;
        }
        Cmd::Manifest => {
            writeln!(out, "id\toffset\tsize\titeration\tsha256")?;
            for e in dat.entries() {
                let bytes = dat.read(e.id)?;
                let hash = Sha256::digest(&bytes);
                writeln!(
                    out,
                    "{:08X}\t{}\t{}\t{}\t{}",
                    e.id,
                    e.offset,
                    e.size,
                    e.iteration,
                    hex::encode(hash)
                )?;
            }
        }
        Cmd::Diff { manifest } => {
            let text = std::fs::read_to_string(&manifest)?;
            let mut expected = std::collections::BTreeMap::new();
            for line in text.lines().skip(1) {
                let f: Vec<&str> = line.split('\t').collect();
                if f.len() < 5 {
                    continue;
                }
                expected.insert(
                    parse_id(f[0])?,
                    (f[2].parse::<u32>()?, f[4].to_ascii_lowercase()),
                );
            }
            let mut missing = 0usize;
            let mut mismatched = 0usize;
            let mut extra = 0usize;
            for (id, (size, hash)) in &expected {
                match dat.entry(*id) {
                    None => {
                        missing += 1;
                        writeln!(out, "MISSING  {id:08X}")?;
                    }
                    Some(e) => {
                        let bytes = dat.read(*id)?;
                        let got = hex::encode(Sha256::digest(&bytes));
                        if e.size != *size || &got != hash {
                            mismatched += 1;
                            writeln!(
                                out,
                                "MISMATCH {id:08X} size {} vs {} hash {} vs {}",
                                e.size, size, got, hash
                            )?;
                        }
                    }
                }
            }
            for e in dat.entries() {
                if !expected.contains_key(&e.id) {
                    extra += 1;
                    writeln!(out, "EXTRA    {:08X}", e.id)?;
                }
            }
            writeln!(out, "checked {} expected files: {missing} missing, {mismatched} mismatched, {extra} extra", expected.len())?;
            if missing + mismatched + extra > 0 {
                out.flush()?;
                bail!("manifest differs");
            }
        }
    }
    out.flush()?;
    Ok(())
}
