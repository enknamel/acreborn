//! Collision sanity against real data. Needs AC_DATA_DIR.
use ac_scene::{collision::CollisionWorld, landblock, Assets};
use glam::Vec3;

#[test]
fn holtburg_walls_push_and_floors_hold() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(dir).unwrap();
    let scene = landblock::load(&assets, 0xA9B4_0000).unwrap();
    let w = CollisionWorld::from_scene(&assets, &scene).unwrap();
    assert!(w.tris.len() > 1000, "{} tris", w.tris.len());
    // Take a wall triangle of a building and stand 0.1 m inside it: we must be pushed out.
    let wall = w
        .tris
        .iter()
        .find(|t| t.normal.z.abs() < 0.2 && (t.b - t.a).length() > 2.0 && t.a.z > 60.0)
        .unwrap();
    let mid = (wall.a + wall.b + wall.c) / 3.0;
    let inside = mid - wall.normal * 0.1 - Vec3::new(0.0, 0.0, 1.0);
    let out = w.resolve(inside, 0.4, 1.7);
    let d = (out - inside).dot(wall.normal);
    assert!(d > 0.2, "pushed {d} along the wall normal");

    // The Training Academy: a floor under the spawn point at z ~ 0.
    let academy = landblock::load(&assets, 0x8602_0000).unwrap();
    let wa = CollisionWorld::from_scene(&assets, &academy).unwrap();
    let spawn = ac_world_origin(0x8602_01AD) + Vec3::new(12.32, -28.48, 1.0);
    let (z, cell) = wa.floor_at(spawn, 1.0, 3.0).expect("floor under the spawn");
    assert!(z.abs() < 0.5, "floor z {z}");
    assert_eq!(cell, 0x8602_01AD, "spawn cell from the floor triangle");
}

fn ac_world_origin(cell: u32) -> Vec3 {
    Vec3::new(
        (cell >> 24) as f32 * 192.0,
        ((cell >> 16) & 0xFF) as f32 * 192.0,
        0.0,
    )
}

#[test]
fn segment_hit_finds_a_wall() {
    let a = Vec3::new(5.0, -1.0, 0.0);
    let b = Vec3::new(5.0, 1.0, 0.0);
    let c = Vec3::new(5.0, 0.0, 3.0);
    let mut w = CollisionWorld::default();
    w.add_tri(a, b, c, 0, false);
    let f = w
        .segment_hit(Vec3::new(0.0, 0.0, 1.0), Vec3::new(10.0, 0.0, 1.0))
        .expect("hit");
    assert!((f - 0.5).abs() < 1e-5, "{f}");
    // From the other side too, and a miss above the triangle.
    assert!(w
        .segment_hit(Vec3::new(10.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0))
        .is_some());
    assert!(w
        .segment_hit(Vec3::new(0.0, 0.0, 5.0), Vec3::new(10.0, 0.0, 5.0))
        .is_none());
}
