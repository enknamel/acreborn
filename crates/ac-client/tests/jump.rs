//! Running jumps against real geometry. Needs AC_DATA_DIR.
//!
//! A jump from the ground in front of the Shoushi tailor's porch that
//! arrived a few centimetres under the porch top used to pass down
//! through the slab and leave the character stuck in the one-metre gap
//! beneath it (the server then refused every move).
use ac_client::player::{Input, Player};
use ac_scene::{collision::CollisionWorld, landblock, Assets};
use glam::{Quat, Vec3};

#[test]
fn jumps_at_the_shoushi_porch_never_end_under_it() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let block = 0xDA55_0000;
    let scene = landblock::load(&assets, block).unwrap();
    let world = CollisionWorld::from_scene(&assets, &scene).unwrap();
    let start = Vec3::new(129.5, 175.0, 20.0);
    let cell = ac_world::outdoor_cell(block, start);
    let mut stuck = Vec::new();
    for power in [0.9f32, 0.95, 1.0] {
        for deg in (0..360).step_by(15) {
            let heading = (deg as f32).to_radians();
            let mut pl = Player::new(&assets, cell, start, Quat::from_rotation_z(heading));
            pl.set_motion_table(&assets, 0x0200_0001, 0x0900_0001);
            pl.max_jump_power = 1.0;
            for frame in 0..210 {
                let input = Input {
                    forward: 1.0,
                    strafe: 0.0,
                    run: true,
                    jump: false,
                    jump_held: false,
                };
                if frame == 30 {
                    pl.jump(power);
                }
                pl.update(&assets, &input, 1.0 / 60.0);
            }
            let end = pl.world_position();
            let headroom = world
                .ceiling_at(end, 0.4)
                .map(|cz| cz - end.z)
                .unwrap_or(f32::INFINITY);
            if headroom < 1.7 {
                stuck.push((
                    power,
                    deg,
                    end - ac_scene::lbid::world_origin(block),
                    headroom,
                ));
            }
        }
    }
    assert!(stuck.is_empty(), "ended without headroom: {stuck:?}");
}

/// The Holtburg meeting hall (0x0125) has one-sided cell walls with
/// separate back faces and staircases with railings a body width from
/// the wall. Running jumps from beside those walls used to leave the
/// building and fall forever ("stuck in the air" in the client, the
/// server refusing every position).
#[test]
fn jumps_beside_meeting_hall_walls_stay_inside() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let cases = [
        // South wall of the ground-floor hall.
        (0x0125_010F, Vec3::new(25.5, -44.5, 0.0), 180.0),
        // Beside the west staircase railing, into the wall.
        (0x0125_0108, Vec3::new(15.5, -41.5, 0.0), 90.0),
        // The east one, mirrored.
        (0x0125_0113, Vec3::new(44.5, -41.5, 0.0), 270.0),
        // A corner of the entrance corridor.
        (0x0125_0100, Vec3::new(-4.5, -34.5, 0.0), 90.0),
    ];
    let mut bad = Vec::new();
    for (cell, start, deg) in cases {
        let heading = (deg as f32).to_radians();
        let mut pl = Player::new(&assets, cell, start, Quat::from_rotation_z(heading));
        pl.set_motion_table(&assets, 0x0200_0001, 0x0900_0001);
        pl.max_jump_power = 1.0;
        for frame in 0..360 {
            let input = Input {
                forward: 1.0,
                strafe: 0.0,
                run: true,
                jump: false,
                jump_held: false,
            };
            if frame == 30 {
                pl.jump(1.0);
            }
            pl.update(&assets, &input, 1.0 / 60.0);
        }
        let end = pl.world_position() - ac_world::landblock_origin(cell);
        if pl.is_airborne() || end.z < -0.5 || end.z > 20.0 {
            bad.push((cell, start, deg, end, pl.is_airborne()));
        }
    }
    assert!(bad.is_empty(), "left the building: {bad:?}");
}

/// The cells above the meeting hall's floor (0x01250124 and its like)
/// are air: every face is a portal and there are no physics polygons.
/// Their portal polygons used to be built into the collision as walls
/// and a floor, so a jump from the balcony landed on an invisible floor
/// six metres above the hall, boxed in by invisible walls.
#[test]
fn air_cells_above_the_meeting_hall_have_no_floor() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let block = 0x0125_0000;
    let scene = landblock::load(&assets, block).unwrap();
    let world = CollisionWorld::from_scene(&assets, &scene).unwrap();
    let origin = ac_scene::lbid::world_origin(block);
    // Over the hall at balcony height: nothing to stand on until the
    // ramp well below.
    let over_hall = origin + Vec3::new(30.0, -38.0, 6.3);
    assert_eq!(world.floor_at(over_hall, 0.6, 1.5), None);
    let deep = world.floor_at(over_hall, 0.6, 50.0).map(|f| f.0 - origin.z);
    assert!(deep.is_some_and(|z| z < 5.0), "floor below: {deep:?}");

    // A running jump north off the balcony ends on the hall floor.
    let cell = 0x0125_010F;
    let mut pl = Player::new(&assets, cell, Vec3::new(30.0, -43.0, 6.0), Quat::IDENTITY);
    pl.set_motion_table(&assets, 0x0200_0001, 0x0900_0001);
    pl.max_jump_power = 1.0;
    for frame in 0..300 {
        let input = Input {
            forward: 1.0,
            strafe: 0.0,
            run: true,
            jump: false,
            jump_held: false,
        };
        if frame == 30 {
            pl.jump(1.0);
        }
        pl.update(&assets, &input, 1.0 / 60.0);
    }
    let end = pl.world_position() - origin;
    assert!(!pl.is_airborne(), "still airborne at {end}");
    assert!(end.z.abs() < 0.05 && end.y > -35.0, "ended at {end}");
}

/// A jump off a Holtburg hill must land on the ground, not under it, and
/// walking out of a building and jumping must not fall for ever (the
/// character still carries the building's interior cell id, which used to
/// stop the terrain from catching them).
#[test]
fn jumps_on_holtburg_hills_land_on_the_ground() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let block = 0xA9B4_0000;
    let scene = landblock::load(&assets, block).unwrap();
    let mut bad = Vec::new();
    for (x, y) in [(76.0, 88.0), (16.0, 100.0), (28.0, 136.0), (100.0, 36.0)] {
        let z = scene.terrain.height_at(Vec3::new(x, y, 0.0)).unwrap();
        for deg in (0..360).step_by(45) {
            let start = Vec3::new(x, y, z);
            let cell = ac_world::outdoor_cell(block, start);
            let mut pl = Player::new(
                &assets,
                cell,
                start,
                Quat::from_rotation_z((deg as f32).to_radians()),
            );
            pl.set_motion_table(&assets, 0x0200_0001, 0x0900_0001);
            pl.max_jump_power = 1.0;
            for frame in 0..300 {
                let input = Input {
                    forward: 1.0,
                    strafe: 0.0,
                    run: true,
                    jump: false,
                    jump_held: false,
                };
                if frame == 30 {
                    pl.jump(1.0);
                }
                pl.update(&assets, &input, 1.0 / 60.0);
            }
            let end = pl.world_position() - ac_scene::lbid::world_origin(block);
            let ground = scene.terrain.height_at(Vec3::new(end.x, end.y, 0.0));
            let under = ground.map(|g| g - end.z).unwrap_or(0.0);
            if pl.is_airborne() || under > 2.0 {
                bad.push((x, y, deg, end, ground, pl.is_airborne()));
            }
        }
    }
    assert!(bad.is_empty(), "ended under the ground: {bad:?}");
}
