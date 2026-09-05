//! Time the CPU side of world loading: opening the archives, assembling
//! landblocks, building collision, and decoding the textures they use.
//!
//! `AC_DATA_DIR=... cargo run --release -p ac-scene --example bench_landblock [BLOCK...]`
use std::time::Instant;

use ac_scene::{collision::CollisionWorld, landblock, model, Assets};

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let mut blocks: Vec<u32> = std::env::args()
        .skip(1)
        .map(|a| u32::from_str_radix(a.trim_start_matches("0x"), 16).unwrap() << 16)
        .collect();
    if blocks.is_empty() {
        blocks = vec![0xA9B4_0000, 0xA9B3_0000, 0xA8B4_0000, 0x8602_0000];
    }
    let t = Instant::now();
    let assets = Assets::open(dir).unwrap();
    println!("open archives: {:.1} ms", ms(t));
    let t = Instant::now();
    let region = assets.region().unwrap();
    println!("region: {:.1} ms", ms(t));

    for &id in &blocks {
        // First load: parses everything; second: whatever is cached.
        let t = Instant::now();
        let scene = landblock::load(&assets, id).unwrap();
        let first = ms(t);
        let t = Instant::now();
        let scene2 = landblock::load(&assets, id).unwrap();
        let second = ms(t);
        drop(scene2);
        let t = Instant::now();
        let mut meshes = 0;
        let mut tris = 0;
        for p in scene
            .parts
            .iter()
            .chain(scene.cells.iter().flat_map(|c| c.parts.iter()))
        {
            let g = assets.gfxobj(p.gfxobj_id).unwrap();
            let m = model::build_mesh(&assets, &g).unwrap();
            meshes += 1;
            tris += m
                .submeshes
                .iter()
                .map(|s| s.indices.len() / 3)
                .sum::<usize>();
        }
        let mesh_ms = ms(t);
        let t = Instant::now();
        let w = CollisionWorld::from_scene(&assets, &scene).unwrap();
        let coll_first = ms(t);
        let t = Instant::now();
        let _ = CollisionWorld::from_scene(&assets, &scene).unwrap();
        let coll_second = ms(t);
        println!(
            "block {:#010x}: load {first:.1} ms (again {second:.1} ms), {} cells, {} parts; \
             {meshes} meshes / {tris} tris {mesh_ms:.1} ms; collision {} tris {coll_first:.1} ms \
             (again {coll_second:.1} ms)",
            id,
            scene.cells.len(),
            scene.parts.len(),
            w.tris.len()
        );
    }

    // Terrain texture layers, as the viewer decodes them for its texture arrays.
    if let Some(tables) = ac_scene::texmerge::Tables::from_region(&region) {
        let t = Instant::now();
        let mut bytes = 0;
        for &id in tables.texture_ids.iter().chain(tables.alpha_ids.iter()) {
            if let Ok(img) = assets.texture_rgba(id, None) {
                bytes += img.pixels.len();
            }
        }
        let first = ms(t);
        let t = Instant::now();
        for &id in tables.texture_ids.iter().chain(tables.alpha_ids.iter()) {
            let _ = assets.texture_rgba(id, None);
        }
        println!(
            "terrain layers: {} textures, {:.1} MB, decode {first:.1} ms (again {:.1} ms)",
            tables.texture_ids.len() + tables.alpha_ids.len(),
            bytes as f64 / 1e6,
            ms(t)
        );
    }
}
