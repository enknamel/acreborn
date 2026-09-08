//! The Training Academy tutorial's legs are walkable on the academy
//! landblock's own navigation graph (see `ac_client::academy`). Needs
//! AC_DATA_DIR; skipped without it.

use ac_scene::{
    collision::{Capsule, CollisionWorld},
    landblock,
    nav::{Ground, NavGraph},
    Assets,
};
use glam::Vec3;

#[test]
fn academy_legs_are_walkable() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(dir).unwrap();
    let block = ac_client::academy::LANDBLOCK;
    let scene = landblock::load(&assets, block).unwrap();
    let collision = CollisionWorld::from_scene(&assets, &scene).unwrap();
    let ground = Ground {
        collision: &collision,
        terrain: None,
        sea: None,
        no_go: None,
        outdoors_only: false,
    };
    let cap = Capsule::default();
    let origin = ac_world::landblock_origin(block);
    // Every walk within one leg of the tutorial (a leg ends at a
    // portal, which the graph cannot follow) must find a path from the
    // previous spot.
    let mut from: Option<Vec3> = None;
    let mut failures = Vec::new();
    for step in ac_client::academy::steps() {
        let Some(local) = step.at else {
            continue;
        };
        let here = origin + local;
        // A ramp the graph cannot climb is walked straight up by the
        // physics; the table says so in the step's name.
        let ramp = step.kind == ac_client::academy::Kind::Walk && step.target.contains("ramp top");
        if let Some(prev) = from.filter(|_| !ramp) {
            let mut graph = NavGraph::for_scene(&scene, &collision, &cap);
            let t = std::time::Instant::now();
            match graph.find_path(&ground, prev, here) {
                Some(path) => eprintln!(
                    "{:?} {}: {} waypoints in {:.0} ms",
                    step.kind,
                    step.target,
                    path.len(),
                    t.elapsed().as_secs_f64() * 1e3
                ),
                None => failures.push(format!(
                    "{:?} {}: no path from {:?} to {:?}",
                    step.kind,
                    step.target,
                    prev - origin,
                    local
                )),
            }
        }
        // A portal ends the leg where it drops the character.
        from = match step.kind {
            ac_client::academy::Kind::Portal => step.lands.map(|l| origin + l),
            _ => Some(here),
        };
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
