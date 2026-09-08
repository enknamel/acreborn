//! Paths through the Town Network hub, with and without the portal berth.
use ac_client::pathfinder::{portal_mouths_to_avoid, PORTAL_BERTH};
use ac_scene::collision::Capsule;
use ac_scene::navarea::Area;
use ac_scene::Assets;
use glam::{Vec2, Vec3};
fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let origin = ac_scene::lbid::world_origin(0x0007_0000);
    let from = origin + Vec3::new(70.0, -60.0, 0.1);
    let to = origin + Vec3::new(86.0, -120.0, 0.1);
    let cap = Capsule {
        radius: 0.5,
        height: 1.8,
        step_up: 0.8,
        step_down: 1.5,
    };
    let mut area = Area::build(&assets, &[0x0007_0000], &cap, 0x0007_0000).unwrap();
    println!(
        "hub: dungeon={} tris={} floors near from: {:?}",
        area.dungeon,
        area.collision.tris.len(),
        area.collision.floors_at_xy(from.x, from.y)
    );
    println!(
        "floors near to: {:?}",
        area.collision.floors_at_xy(to.x, to.y)
    );
    println!(
        "bounds: {:?}",
        area.collision
            .bounds()
            .map(|(lo, hi)| (lo - origin, hi - origin))
    );
    for (x, y) in [
        (70.0, -60.0),
        (70.0, -80.0),
        (86.0, -118.0),
        (80.0, -100.0),
        (60.0, -60.0),
    ] {
        let p = origin + Vec3::new(x, y, 0.0);
        println!(
            "  floors at ({x},{y}): {:?}  floor_at(z5): {:?}",
            area.collision.floors_at_xy(p.x, p.y),
            area.collision.floor_at(p + Vec3::Z * 5.0, 0.0, 20.0)
        );
    }
    let p = area.path(from, to);
    println!("no berth: {:?}", p.as_ref().map(|p| p.len()));
    let avoid = portal_mouths_to_avoid(from, to);
    let f2 = Vec2::new(from.x, from.y);
    let mut d: Vec<(f32, Vec2)> = avoid.iter().map(|m| (m.distance(f2), *m)).collect();
    d.sort_by(|a, b| a.0.total_cmp(&b.0));
    println!(
        "{} mouths to avoid; nearest to from: {:?}",
        avoid.len(),
        &d[..3.min(d.len())]
    );
    let t2 = Vec2::new(to.x, to.y);
    let mut dt: Vec<f32> = avoid.iter().map(|m| m.distance(t2)).collect();
    dt.sort_by(|a, b| a.total_cmp(b));
    println!(
        "nearest avoided mouth to the goal: {:?}",
        &dt[..3.min(dt.len())]
    );
    area.avoid = avoid;
    area.berth = PORTAL_BERTH;
    let p = area.path(from, to);
    println!("with berth: {:?}", p.as_ref().map(|p| p.len()));
}
