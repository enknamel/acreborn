//! Walking into Sanamar past its city walls, and climbing a dungeon's
//! stairs. Needs AC_DATA_DIR.
//!
//! Sanamar (landblock 33D9) is the case that prompted the neighbourhood
//! planner: a character walking in from outside the walls would run at
//! them and stop, because the gate is in the next landblock and a route
//! planned on one block at a time could not see it.

use ac_scene::{
    collision::{Capsule, CollisionWorld},
    landblock, lbid,
    nav::NavGraph,
    navarea::{blocks_for, Area},
    Assets,
};
use glam::Vec3;

fn capsule() -> Capsule {
    Capsule {
        radius: 0.5,
        height: 1.8,
        step_up: 0.8,
        step_down: 1.5,
    }
}

#[test]
fn a_walk_into_sanamar_goes_round_the_walls_not_into_them() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(dir).unwrap();
    let cap = capsule();
    let block = lbid::from_xy(0x33, 0xD9);
    let origin = lbid::world_origin(block);
    // The town square, where the portal from the town network lands.
    let square = origin + Vec3::new(59.0, 100.0, 0.0);

    let mut area = Area::build(&assets, &blocks_for(square, square), &cap, block).unwrap();
    assert!(!area.dungeon);
    assert_eq!(area.blocks.len(), 9, "a full neighbourhood around the town");
    assert!(
        area.collision.tris.len() > 50_000,
        "the walls and buildings are there: {} triangles",
        area.collision.tris.len()
    );
    let square = Vec3::new(
        square.x,
        square.y,
        area.terrain_at(square.x, square.y).unwrap(),
    );

    // Approach from the south-west, where the wall stands between us and
    // the square. Straight in is blocked; a route exists all the same.
    let bearing = 225f32.to_radians();
    let from = square + Vec3::new(130.0 * bearing.cos(), 130.0 * bearing.sin(), 0.0);
    let from = Vec3::new(from.x, from.y, area.terrain_at(from.x, from.y).unwrap());
    assert!(
        !area.line_clear(from + Vec3::Z, square + Vec3::Z),
        "the wall should be between us and the square"
    );
    let path = area.path(from, square).expect("a way in past the wall");
    let end = *path.last().unwrap();
    assert!(
        end.distance(square) < 1.0,
        "the route should reach the square, not stop short at {end:?}"
    );
    // The way round is longer than the way through, and leaves the block
    // the goal is in: that is the whole point of planning wide.
    let mut walked = 0.0;
    let mut prev = from;
    for w in &path {
        walked += prev.distance(*w);
        prev = *w;
    }
    assert!(
        walked > from.distance(square),
        "going round should be further than the straight line"
    );

    // The same walk planned on the goal's landblock alone: the start is
    // outside that block, so the single-block planner has nothing to
    // stand on and cannot answer at all.
    let scene = landblock::load(&assets, block).unwrap();
    let collision = CollisionWorld::from_scene(&assets, &scene).unwrap();
    let mut one = NavGraph::for_scene(&scene, &collision, &cap);
    let sampler = |x: f32, y: f32| {
        scene
            .terrain
            .height_at(Vec3::new(x - origin.x, y - origin.y, 0.0))
    };
    let ground = ac_scene::nav::Ground {
        collision: &collision,
        terrain: Some(&sampler),
    };
    assert!(
        one.find_path(&ground, from, square).is_none(),
        "one landblock alone should not be able to plan this walk"
    );
}

#[test]
fn a_dungeon_is_planned_on_its_own_and_its_stairs_connect_its_floors() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(dir).unwrap();
    let cap = capsule();
    // The Old Talisman dungeon: several floors joined by stairs.
    let block = 0x01EA_0000;
    let square = lbid::world_origin(block) + Vec3::new(96.0, 96.0, 0.0);
    // Asking for a neighbourhood still gets just the dungeon: the blocks
    // beside it on the lattice are unrelated pieces of the surface.
    let mut area = Area::build(&assets, &blocks_for(square, square), &cap, block).unwrap();
    assert!(area.dungeon);
    assert_eq!(area.blocks, vec![block]);
    assert_eq!(area.terrain_at(square.x, square.y), None);

    area.build_everything();
    assert!(
        !area.nav.is_empty(),
        "the dungeon should have walkable floors"
    );

    // The biggest group of floors that can all reach each other should
    // span more than one storey: that only happens if the graph climbs.
    let n = area.nav.len();
    let mut seen = vec![false; n];
    let mut best_rise = 0.0f32;
    let mut best_size = 0usize;
    for s in 0..n {
        if seen[s] {
            continue;
        }
        seen[s] = true;
        let mut stack = vec![s as u32];
        let (mut lo, mut hi, mut size) = (f32::MAX, f32::MIN, 0usize);
        while let Some(v) = stack.pop() {
            size += 1;
            let z = area.nav.nodes[v as usize].pos.z;
            lo = lo.min(z);
            hi = hi.max(z);
            for &w in area.nav.neighbours(v) {
                if !seen[w as usize] {
                    seen[w as usize] = true;
                    stack.push(w);
                }
            }
        }
        if size > best_size {
            best_size = size;
            best_rise = hi - lo;
        }
    }
    assert!(
        best_size > n / 2,
        "most of the dungeon should be one connected place, got {best_size} of {n}"
    );
    assert!(
        best_rise > 20.0,
        "the connected part should climb several floors, got {best_rise:.1} m"
    );
}
