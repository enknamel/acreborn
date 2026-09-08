//! Geometry around a world point: how many triangles per cell and the z range. zprofile x y z r
use ac_scene::collision::Capsule;
use ac_scene::navarea::{block_at, blocks_for, Area};
use ac_scene::Assets;
use glam::Vec3;
use std::collections::BTreeMap;
fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let a: Vec<f32> = std::env::args()
        .skip(1)
        .map(|s| s.parse().unwrap())
        .collect();
    let p = Vec3::new(a[0], a[1], a[2]);
    let cap = Capsule {
        radius: 0.5,
        height: 1.8,
        step_up: 0.8,
        step_down: 1.5,
    };
    let area = Area::build(&assets, &blocks_for(p, p), &cap, block_at(p)).unwrap();
    println!(
        "terrain at xy: {:?}; floors at xy: {:?}",
        area.terrain_at(p.x, p.y),
        area.collision.floors_at_xy(p.x, p.y)
    );
    let mut per: BTreeMap<u32, (usize, f32, f32)> = BTreeMap::new();
    for t in area.collision.nearby(p, a[3]) {
        let e = per.entry(t.cell).or_insert((0, f32::MAX, f32::MIN));
        e.0 += 1;
        for v in [t.a, t.b, t.c] {
            e.1 = e.1.min(v.z);
            e.2 = e.2.max(v.z);
        }
    }
    for (cell, (n, lo, hi)) in per {
        println!("  cell {cell:#010x}: {n} tris, z {lo:.1}..{hi:.1}");
    }
}
