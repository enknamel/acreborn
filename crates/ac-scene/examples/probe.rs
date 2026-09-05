//! Probe the collision world at one point: wall contact, floor, and
//! walk results in the four compass directions.
//!
//! `AC_DATA_DIR=... cargo run --release -p ac-scene --example probe BLOCK x y z`
use ac_scene::{
    collision::{Capsule, CollisionWorld},
    landblock, Assets,
};
use glam::Vec3;

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let block = u32::from_str_radix(args[0].trim_start_matches("0x"), 16).unwrap() << 16;
    let f = |i: usize| args[i].parse::<f32>().unwrap();
    let local = Vec3::new(f(1), f(2), f(3));
    let origin = ac_scene::lbid::world_origin(block);
    let p = origin + local;
    let scene = landblock::load(&assets, block).unwrap();
    let c = CollisionWorld::from_scene(&assets, &scene).unwrap();
    let cap = Capsule::default();
    println!("capsule {cap:?}");
    println!(
        "at {local:?}: floor {:?}",
        c.floor_at(p, cap.step_up, cap.step_down)
    );
    for skirt in [0.0, 0.3, 0.5] {
        println!(
            "wall_contact skirt {skirt}: {}",
            c.wall_contact(p, cap.radius, cap.height, skirt)
        );
    }
    let r = c.resolve(p, cap.radius, cap.height);
    println!(
        "resolve -> {:?} (moved {:.3})",
        r - origin,
        (r - p).length()
    );
    for dx in [-0.6, -0.3, 0.0, 0.3, 0.6, 0.9, 1.2, 1.5, 2.0, 3.0] {
        let q = p + Vec3::X * dx;
        println!(
            "x{dx:+.1}: floor {:?} ceiling {:?}",
            c.floor_at(q, cap.step_up, cap.step_down).map(|f| f.0),
            c.ceiling_at(q, cap.radius)
        );
    }
    // Down-facing triangles below head height near the point.
    for t in &c.tris {
        let lo = t.a.z.min(t.b.z).min(t.c.z);
        let near = |v: Vec3| (v.x - p.x).abs() < 1.5 && (v.y - p.y).abs() < 1.5;
        if (t.normal.z < -0.5 || (t.two_sided && t.normal.z > 0.5))
            && lo > p.z + 0.2
            && lo < p.z + 1.7
            && (near(t.a) || near(t.b) || near(t.c))
        {
            println!(
                "overhang cell {:#010x} two_sided {} n {:?}: {:?} {:?} {:?}",
                t.cell,
                t.two_sided,
                t.normal,
                t.a - origin,
                t.b - origin,
                t.c - origin
            );
        }
    }
    for (name, d) in [
        ("+x", Vec3::X),
        ("-x", -Vec3::X),
        ("+y", Vec3::Y),
        ("-y", -Vec3::Y),
    ] {
        let w = c.walk(p, p + d * 0.3, &cap);
        println!("walk {name} 0.3 m: {w:?}");
    }
}
