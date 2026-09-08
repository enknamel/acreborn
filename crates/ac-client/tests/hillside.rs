//! Walking over a hill that has a dungeon under it. Needs AC_DATA_DIR.
//!
//! The Halls of Metos lie beneath a mountain east of Loredane Villas
//! (landblock 6F8B). A character walking up the hillside used to drop
//! onto the halls' floor twenty metres below the turf, because an
//! interior floor within step range won over the terrain above it, and
//! then carry on under the mountain to the portal's map position, two
//! hundred metres beneath the portal.

use ac_client::player::{Input, Player};
use ac_scene::Assets;
use glam::{Quat, Vec3};

#[test]
fn walking_up_the_villas_hillside_stays_on_the_hill() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let block = 0x6F8B_0000;
    let origin = ac_scene::lbid::world_origin(block);
    // The villas' portal exit, on the turf at the foot of the hill, and
    // a point up the hill to the north-west where the terrain is 34 m.
    // `Player::new` takes the landblock-local position.
    let start = Vec3::new(108.7, 13.5, 4.2);
    let goal = origin + Vec3::new(-12.0, 212.0, 34.0);
    let cell = ac_world::outdoor_cell(block, start);
    let mut pl = Player::new(&assets, cell, start, Quat::IDENTITY);
    pl.set_motion_table(&assets, 0x0200_0001, 0x0900_0001);
    // Follow the route the planner gives, as the client would.
    let cap = ac_scene::collision::Capsule {
        radius: 0.5,
        height: 1.8,
        step_up: 0.8,
        step_down: 1.5,
    };
    let mut area = ac_scene::navarea::Area::build(
        &assets,
        &ac_scene::navarea::blocks_for(origin + start, goal),
        &cap,
        block,
    )
    .unwrap();
    // Outdoors to outdoors: the planner keeps out of the buildings,
    // which is what keeps it out of the halls beneath the hill.
    area.outdoors_only = true;
    let mut route = area.path(origin + start, goal).expect("a way up the hill");
    for w in &route {
        let t = area.terrain_at(w.x, w.y).expect("on the map");
        assert!(
            (w.z - t).abs() < 2.5,
            "the route goes indoors at {w:?} (terrain {t:.1})"
        );
    }
    route.push(goal);
    let mut next = 0;
    let mut lowest_under_turf = 0.0f32;
    for _ in 0..(60 * 120) {
        let me = pl.world_position();
        while next < route.len()
            && glam::Vec2::new(route[next].x - me.x, route[next].y - me.y).length() < 1.5
        {
            next += 1;
        }
        if next >= route.len() {
            break;
        }
        let to = route[next] - me;
        pl.heading = (-to.x).atan2(to.y);
        let input = Input {
            forward: 1.0,
            strafe: 0.0,
            run: true,
            jump: false,
            jump_held: false,
        };
        pl.update(&assets, &input, 1.0 / 60.0);
        let me = pl.world_position();
        // Never indoors, and never far under the ground.
        assert_eq!(
            pl.cell & 0xFFFF < 0x100,
            true,
            "walked into an interior cell {:#010x} at {me:?}",
            pl.cell
        );
        if let Some(t) = pl_terrain(&assets, me) {
            lowest_under_turf = lowest_under_turf.min(me.z - t);
            assert!(
                me.z > t - 2.0,
                "under the hill: z {:.1} with the ground at {t:.1} at {me:?}",
                me.z
            );
        }
    }
    let end = pl.world_position();
    assert!(
        glam::Vec2::new(end.x - goal.x, end.y - goal.y).length() < 3.0,
        "did not get up the hill: ended at {end:?}"
    );
}

/// Terrain height under a world position, from the landblock.
fn pl_terrain(assets: &Assets, world: Vec3) -> Option<f32> {
    let bx = (world.x / 192.0).floor() as u32;
    let by = (world.y / 192.0).floor() as u32;
    let id = ac_scene::lbid::from_xy(bx, by);
    let scene = ac_scene::landblock::load(assets, id).ok()?;
    let o = ac_scene::lbid::world_origin(id);
    scene
        .terrain
        .height_at(Vec3::new(world.x - o.x, world.y - o.y, 0.0))
}
