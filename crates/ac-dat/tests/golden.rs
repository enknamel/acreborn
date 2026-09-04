//! Differential tests against manifests produced by ACE's DatLoader
//! (`tools/regen-golden.sh`). They need the real archives and are skipped
//! unless `AC_DATA_DIR` is set.

use std::path::PathBuf;

use ac_dat::DatArchive;
use sha2::{Digest, Sha256};

fn data_dir() -> Option<PathBuf> {
    std::env::var_os("AC_DATA_DIR").map(PathBuf::from)
}

fn golden(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden/dat")
        .join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn manifest_line(dat: &DatArchive, e: &ac_dat::Entry) -> String {
    let bytes = dat.read(e.id).unwrap();
    format!(
        "{:08X}\t{}\t{}\t{}\t{}",
        e.id,
        e.offset,
        e.size,
        e.iteration,
        hex::encode(Sha256::digest(&bytes))
    )
}

fn check(archive: &str) {
    let Some(dir) = data_dir() else {
        eprintln!("AC_DATA_DIR unset; skipping");
        return;
    };
    let dat = DatArchive::open(dir.join(format!("client_{archive}.dat"))).unwrap();

    // Sampled rows must match exactly.
    for line in golden(&format!("{archive}_sample.tsv")).lines().skip(1) {
        let id = u32::from_str_radix(line.split('\t').next().unwrap(), 16).unwrap();
        let e = dat.entry(id).unwrap_or_else(|| panic!("{id:08X} missing"));
        assert_eq!(manifest_line(&dat, e), line);
    }

    // The whole manifest must hash to what ACE produced.
    let mut h = Sha256::new();
    for e in dat.entries() {
        h.update(manifest_line(&dat, e));
        h.update(b"\n");
    }
    assert_eq!(
        hex::encode(h.finalize()),
        golden(&format!("{archive}_manifest.sha256")).trim()
    );
}

#[test]
fn portal_matches_ace() {
    check("portal");
}

#[test]
fn cell_matches_ace() {
    check("cell_1");
}
