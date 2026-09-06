//! List the collision triangles within `r` of a landblock-local point.
//! `AC_DATA_DIR=... cargo run --release -p ac-scene --example tris_near BLOCK x y z r`
use ac_scene::collision::CollisionWorld;
use ac_scene::{landblock, Assets};
use glam::Vec3;

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let block = u32::from_str_radix(args[0].trim_start_matches("0x"), 16).unwrap() << 16;
    let f = |i: usize| args[i].parse::<f32>().unwrap();
    let p = Vec3::new(f(1), f(2), f(3));
    let r = f(4);
    let scene = landblock::load(&assets, block).unwrap();
    for cs in &scene.cells {
        let c = cs.transform.transform_point3(Vec3::ZERO) - ac_scene::lbid::world_origin(block);
        if (c - p).length() < 15.0 {
            println!(
                "cell {:#010x} env {:#x} struct {} at ({:.1}, {:.1}, {:.1}) portals {:?} parts {}",
                cs.cell_id,
                cs.environment_id,
                cs.cell_structure,
                c.x,
                c.y,
                c.z,
                cs.portal_cells
                    .iter()
                    .map(|c| format!("{c:#x}"))
                    .collect::<Vec<_>>(),
                cs.parts.len()
            );
        }
    }
    let world = CollisionWorld::from_scene(&assets, &scene).unwrap();
    let origin = ac_scene::lbid::world_origin(block);
    let mut n = 0;
    for t in world.nearby(origin + p, r) {
        let a = t.a - origin;
        let b = t.b - origin;
        let c = t.c - origin;
        println!(
            "tri cell {:#010x} n ({:.2}, {:.2}, {:.2}) two_sided {} a ({:.1}, {:.1}, {:.1}) b ({:.1}, {:.1}, {:.1}) c ({:.1}, {:.1}, {:.1})",
            t.cell, t.normal.x, t.normal.y, t.normal.z, t.two_sided, a.x, a.y, a.z, b.x, b.y, b.z, c.x, c.y, c.z
        );
        n += 1;
    }
    println!("{n} triangles within {r} of ({}, {}, {})", p.x, p.y, p.z);
    // Which steep triangles push a capsule of radius 0.4 at this point.
    let cap_r = 0.4;
    for dz in [0.4, 0.85, 1.3] {
        let c = origin + p + Vec3::new(0.0, 0.0, dz);
        for t in world.nearby(origin + p, cap_r + 0.5) {
            if t.normal.z.abs() > 0.6 {
                continue;
            }
            let q = ac_scene::collision::closest_point_on_tri(c, t);
            let d = c - q;
            let dist = d.length();
            if dist >= cap_r {
                continue;
            }
            let sd = (c - t.a).dot(t.normal);
            println!(
                "push at dz {dz}: tri cell {:#010x} n ({:.2}, {:.2}, {:.2}) a ({:.1}, {:.1}, {:.1}) dist {dist:.2} sd {sd:.2} two_sided {}",
                t.cell, t.normal.x, t.normal.y, t.normal.z, t.a.x - origin.x, t.a.y - origin.y, t.a.z - origin.z, t.two_sided
            );
        }
    }
    let r1 = world.resolve_above(origin + p, cap_r, 1.7, 0.6) - origin;
    println!("resolve_above -> ({:.2}, {:.2}, {:.2})", r1.x, r1.y, r1.z);
}

#[allow(dead_code)]
fn unused() {}
