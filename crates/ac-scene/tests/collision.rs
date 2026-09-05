//! Collision sanity against real data. Needs AC_DATA_DIR.
use ac_scene::{
    collision::{Capsule, CollisionWorld, Vertical, GRAVITY},
    landblock, Assets,
};
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

/// A quad `a b c d` (counter-clockwise seen from its normal's side).
fn quad(w: &mut CollisionWorld, a: Vec3, b: Vec3, c: Vec3, d: Vec3, cell: u32, two_sided: bool) {
    w.add_tri(a, b, c, cell, two_sided);
    w.add_tri(a, c, d, cell, two_sided);
}

/// Floor at `z` covering x0..x1 by -5..5, normal up.
fn floor(w: &mut CollisionWorld, x0: f32, x1: f32, z: f32, cell: u32) {
    quad(
        w,
        Vec3::new(x0, -5.0, z),
        Vec3::new(x1, -5.0, z),
        Vec3::new(x1, 5.0, z),
        Vec3::new(x0, 5.0, z),
        cell,
        false,
    );
}

/// Vertical face at `x` from z0 to z1, normal facing -x (toward a walker
/// coming from the left).
fn riser(w: &mut CollisionWorld, x: f32, z0: f32, z1: f32) {
    quad(
        w,
        Vec3::new(x, -5.0, z0),
        Vec3::new(x, -5.0, z1),
        Vec3::new(x, 5.0, z1),
        Vec3::new(x, 5.0, z0),
        0,
        false,
    );
}

/// Ceiling at `z` covering x0..x1, normal facing down.
fn ceiling(w: &mut CollisionWorld, x0: f32, x1: f32, z: f32) {
    quad(
        w,
        Vec3::new(x0, -5.0, z),
        Vec3::new(x0, 5.0, z),
        Vec3::new(x1, 5.0, z),
        Vec3::new(x1, -5.0, z),
        0,
        false,
    );
}

#[test]
fn falling_lands_on_the_first_floor() {
    let mut w = CollisionWorld::default();
    floor(&mut w, -5.0, 5.0, 0.0, 0x1234_0101);
    // A floor below the one we should land on must not catch us first.
    floor(&mut w, -5.0, 5.0, -3.0, 0x1234_0102);
    assert!(w.tris.iter().all(|t| t.normal.z > 0.99));
    let cap = Capsule::default();
    let mut pos = Vec3::new(1.0, 1.0, 5.0);
    let mut vz = 0.0f32;
    let dt = 1.0 / 60.0;
    let mut t = 0.0f32;
    let landed = loop {
        vz -= GRAVITY * dt;
        t += dt;
        match w.vertical(pos, vz * dt, &cap) {
            Vertical::Free(p) => pos = p,
            Vertical::Landed(p, cell) => break (p, cell),
            Vertical::Ceiling(p) => panic!("ceiling at {p}"),
        }
        assert!(t < 3.0, "never landed, at {pos}");
    };
    assert!(landed.0.z.abs() < 1e-4, "landed at {}", landed.0);
    assert_eq!(landed.1, 0x1234_0101);
    // sqrt(2 * 5 / 9.8) ~ 1.01 s of free fall.
    assert!((0.9..1.2).contains(&t), "fell for {t} s");
}

#[test]
fn walking_steps_up_a_low_ledge() {
    let mut w = CollisionWorld::default();
    floor(&mut w, -5.0, 0.0, 0.0, 0);
    riser(&mut w, 0.0, 0.0, 0.5);
    floor(&mut w, 0.0, 5.0, 0.5, 0);
    let cap = Capsule::default();
    // Walk straight into the step: not pushed back, feet on the ledge.
    let from = Vec3::new(-0.3, 0.0, 0.0);
    let to = Vec3::new(0.3, 0.0, 0.0);
    let s = w.walk(from, to, &cap);
    assert!(!s.blocked);
    assert!((s.pos.x - 0.3).abs() < 1e-4, "pushed back to {}", s.pos);
    assert_eq!(s.floor.map(|f| f.0), Some(0.5));
    assert!((s.pos.z - 0.5).abs() < 1e-4, "feet at {}", s.pos);
    // And back down without falling: a floor is found within step_down.
    let s = w.walk(Vec3::new(0.3, 0.0, 0.5), Vec3::new(-0.3, 0.0, 0.5), &cap);
    assert!(!s.blocked);
    assert_eq!(s.floor.map(|f| f.0), Some(0.0));
    assert!(s.pos.z.abs() < 1e-4, "feet at {}", s.pos);
}

#[test]
fn walking_is_blocked_by_a_tall_ledge() {
    let mut w = CollisionWorld::default();
    floor(&mut w, -5.0, 0.0, 0.0, 0);
    riser(&mut w, 0.0, 0.0, 1.5);
    floor(&mut w, 0.0, 5.0, 1.5, 0);
    let cap = Capsule::default();
    let s = w.walk(Vec3::new(-0.6, 0.0, 0.0), Vec3::new(0.3, 0.0, 0.0), &cap);
    assert!(
        s.pos.x <= -cap.radius + 1e-4,
        "walked into the ledge: {}",
        s.pos
    );
    assert_eq!(s.floor.map(|f| f.0), Some(0.0));
    assert!(s.pos.z.abs() < 1e-4, "feet at {}", s.pos);
}

#[test]
fn ceilings_block_walking_and_jumping() {
    let mut w = CollisionWorld::default();
    floor(&mut w, -5.0, 5.0, 0.0, 0);
    // A doorway too low for the capsule (1.7 m) starts at x = 0.
    ceiling(&mut w, 0.0, 5.0, 1.2);
    let cap = Capsule::default();
    assert_eq!(
        w.ceiling_at(Vec3::new(1.0, 0.0, 0.0), cap.radius),
        Some(1.2)
    );
    assert_eq!(w.ceiling_at(Vec3::new(-1.0, 0.0, 0.0), cap.radius), None);
    let from = Vec3::new(-1.0, 0.0, 0.0);
    let s = w.walk(from, Vec3::new(0.3, 0.0, 0.0), &cap);
    assert!(s.blocked);
    assert_eq!(s.pos, from);
    // Open floor next to it is fine.
    let s = w.walk(from, Vec3::new(-0.5, 0.0, 0.0), &cap);
    assert!(!s.blocked);

    // Jumping under a 3 m ceiling: the head stops at it.
    let mut w = CollisionWorld::default();
    floor(&mut w, -5.0, 5.0, 0.0, 0);
    ceiling(&mut w, -5.0, 5.0, 3.0);
    match w.vertical(Vec3::new(0.0, 0.0, 1.0), 0.5, &cap) {
        Vertical::Ceiling(p) => assert!((p.z - (3.0 - cap.height)).abs() < 1e-4, "{p}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        w.vertical(Vec3::new(0.0, 0.0, 0.5), 0.5, &cap),
        Vertical::Free(Vec3::new(0.0, 0.0, 1.0))
    );
}
