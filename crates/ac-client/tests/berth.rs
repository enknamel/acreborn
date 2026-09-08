//! Walking to one portal in the Town Network hub must not brush another:
//! a portal takes whoever touches it. Needs AC_DATA_DIR.

use ac_client::pathfinder::{portal_mouths_to_avoid, PORTAL_BERTH};
use ac_scene::collision::Capsule;
use ac_scene::navarea::Area;
use ac_scene::Assets;
use glam::{Vec2, Vec3};

#[test]
fn a_walk_through_the_hub_keeps_its_berth_from_the_other_portals() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(dir).unwrap();
    // The hub is landblock 0007: arrivals land at (70, -80) and the
    // Portal to Sanamar's mouth stands at (86, -120), with the Portal to
    // Redspire ten metres from it.
    let origin = ac_scene::lbid::world_origin(0x0007_0000);
    let from = origin + Vec3::new(70.0, -80.0, 0.0);
    let to = origin + Vec3::new(86.0, -120.0, 0.0);
    let cap = Capsule {
        radius: 0.5,
        height: 1.8,
        step_up: 0.8,
        step_down: 1.5,
    };
    let mut area = Area::build(&assets, &[0x0007_0000], &cap, 0x0007_0000).unwrap();
    assert!(area.dungeon, "the hub is an interior block");
    let avoid = portal_mouths_to_avoid(from, to);
    assert!(avoid.len() > 30, "the hub's other portals: {}", avoid.len());
    assert!(
        avoid
            .iter()
            .all(|m| m.distance(Vec2::new(to.x, to.y)) > PORTAL_BERTH),
        "the portal we are walking to is not kept off"
    );
    area.avoid = avoid.clone();
    area.berth = PORTAL_BERTH;
    // The hub floor is at z 0; the graph finds its own floors.
    let path = area
        .path(from + Vec3::Z * 0.1, to + Vec3::Z * 0.1)
        .expect("a way to the Sanamar portal");
    let mut prev = from;
    for w in &path {
        let steps = (prev.distance(*w) / 0.5).ceil().max(1.0) as i32;
        for k in 0..=steps {
            let q = prev.lerp(*w, k as f32 / steps as f32);
            let q2 = Vec2::new(q.x, q.y);
            let too_close = avoid.iter().find(|m| m.distance(q2) < PORTAL_BERTH - 0.6);
            assert!(too_close.is_none(), "the walk brushes a portal at {q:?}");
        }
        prev = *w;
    }
    let end = *path.last().unwrap();
    assert!(end.distance(to) < 1.5, "ends at the mouth: {end:?}");
}
