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
