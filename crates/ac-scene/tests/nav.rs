//! Pathfinding through the Training Academy (landblock 8602). Needs
//! AC_DATA_DIR.

use ac_scene::{
    collision::{Capsule, CollisionWorld},
    landblock, lbid,
    nav::{line_clear, Ground, NavGraph},
    Assets,
};
use glam::Vec3;

#[test]
fn academy_start_room_to_the_far_end() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(dir).unwrap();
    let block = 0x8602_0000;
    let scene = landblock::load(&assets, block).unwrap();
    let collision = CollisionWorld::from_scene(&assets, &scene).unwrap();
    let ground = Ground {
        collision: &collision,
        terrain: None,
    };
    let cap = Capsule::default();
    let origin = lbid::world_origin(block);
    let start = origin + Vec3::new(12.3, -28.5, 0.0);

    // The whole block, for the numbers.
    let mut whole = NavGraph::for_scene(&scene, &collision, &cap);
    whole.build_all(&ground);
    eprintln!(
        "academy nav: {} nodes, {} edges, {} chunks, built in {:.0} ms",
        whole.len(),
        whole.edge_count(),
        whole.chunk_count(),
        whole.build_time.as_secs_f64() * 1e3
    );
    assert!(whole.len() > 5000, "{} nodes", whole.len());

    for target in [0x8602_0215, 0x8602_026D] {
        assert!(
            scene.cells.iter().any(|c| c.cell_id == target),
            "cell {target:#010x} missing"
        );
        let goal = whole
            .nodes
            .iter()
            .find(|n| n.cell == target)
            .unwrap_or_else(|| panic!("no walkable node in {target:#010x}"))
            .pos;
        assert!(
            !line_clear(&collision, start, goal),
            "the straight line to {target:#010x} is clear"
        );
        // A fresh graph builds only the chunks the search reaches.
        let mut graph = NavGraph::for_scene(&scene, &collision, &cap);
        let t = std::time::Instant::now();
        let path = graph
            .find_path(&ground, start, goal)
            .unwrap_or_else(|| panic!("no path to {target:#010x}"));
        eprintln!(
            "to {target:#010x}: {} waypoints in {:.1} ms; built {} nodes in {} chunks ({:.0} ms)",
            path.len(),
            t.elapsed().as_secs_f64() * 1e3,
            graph.len(),
            graph.chunk_count(),
            graph.build_time.as_secs_f64() * 1e3
        );
        assert_eq!(*path.last().unwrap(), goal);
        assert!(path.len() >= 2, "{path:?}");
        // The spawn point sits inside a thin marker post; the planner
        // steps out of it first, as the player does on its first move.
        let mut from = collision.resolve_above(start, cap.radius, cap.height, cap.step_up);
        assert!(from.distance(start) < 0.5, "start pushed to {from}");
        for &p in &path {
            assert!(
                line_clear(&collision, from, p),
                "{from} -> {p} crosses a wall"
            );
            assert!(
                ground.walkable(from, p, &cap).0,
                "{from} -> {p} cannot be walked"
            );
            assert!(from.distance(p) > 0.1, "degenerate step at {p}");
            from = p;
        }
        // Every waypoint stands on an Academy floor.
        for &p in &path {
            let (_, cell) = ground.surface_at(p, &cap).expect("floor under waypoint");
            assert_eq!(cell & 0xFFFF_0000, block, "{p} in {cell:#010x}");
        }
    }
}
