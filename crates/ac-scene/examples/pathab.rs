//! Is there a walk from A to B? pathab ax ay bx by (world x, y)
use ac_scene::collision::Capsule;
use ac_scene::navarea::{block_at, blocks_for, Area};
use ac_scene::Assets;
use glam::Vec3;
fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let a: Vec<f32> = std::env::args()
        .skip(1)
        .map(|s| s.parse().unwrap())
        .collect();
    // `pathab ax ay bx by` walks between two spots on the ground;
    // `pathab ax ay az bx by bz` between two exact points, for a goal
    // that is a floor up rather than wherever the terrain happens to be.
    let exact = a.len() >= 6;
    let (from, to) = if exact {
        (Vec3::new(a[0], a[1], a[2]), Vec3::new(a[3], a[4], a[5]))
    } else {
        (Vec3::new(a[0], a[1], 0.0), Vec3::new(a[2], a[3], 0.0))
    };
    // The shape matters as much as the map: a doorway that admits a
    // slim probe and not the character is a path that plans and cannot
    // be walked. AC_CAPSULE=r,h,up,down overrides it.
    let cap = match std::env::var("AC_CAPSULE") {
        Ok(v) => {
            let n: Vec<f32> = v.split(',').map(|s| s.trim().parse().unwrap()).collect();
            Capsule {
                radius: n[0],
                height: n[1],
                step_up: n[2],
                step_down: n[3],
            }
        }
        Err(_) => Capsule {
            radius: 0.5,
            height: 1.8,
            step_up: 0.8,
            step_down: 1.5,
        },
    };
    let mut area = Area::build(&assets, &blocks_for(from, to), &cap, block_at(from)).unwrap();
    let (from, to) = if exact {
        (from, to)
    } else {
        let fz = area.terrain_at(from.x, from.y).unwrap_or(0.0);
        let tz = area.terrain_at(to.x, to.y).unwrap_or(0.0);
        (Vec3::new(from.x, from.y, fz), Vec3::new(to.x, to.y, tz))
    };
    println!(
        "from {from:?} to {to:?}, {} blocks, {} tris, sea at from: {}, line clear: {}",
        area.blocks.len(),
        area.collision.tris.len(),
        area.is_sea(from.x, from.y),
        area.line_clear(from + Vec3::Z, to + Vec3::Z)
    );
    let t0 = std::time::Instant::now();
    match area.path_to(from, to, exact) {
        Some(p) => println!(
            "path: {} waypoints in {:?}: {:?}",
            p.len(),
            t0.elapsed(),
            p.iter()
                .map(|w| (w.x.round(), w.y.round(), w.z.round()))
                .collect::<Vec<_>>()
        ),
        None => println!(
            "NO PATH ({:?}); nearest node to from exists: {}",
            t0.elapsed(),
            area.nav.len()
        ),
    }
}
