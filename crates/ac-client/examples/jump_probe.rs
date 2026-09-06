//! Simulate running jumps from one spot in every direction and report
//! where the character ends up, to catch falls into places a capsule
//! cannot stand (under a porch, between floors).
//!
//! `AC_DATA_DIR=... cargo run --release -p ac-client --example jump_probe BLOCK x y z [power]`
use ac_client::player::{Input, Player};
use ac_scene::{collision::CollisionWorld, landblock, Assets};
use glam::{Quat, Vec3};

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let block = u32::from_str_radix(args[0].trim_start_matches("0x"), 16).unwrap() << 16;
    let f = |i: usize| args[i].parse::<f32>().unwrap();
    let start = Vec3::new(f(1), f(2), f(3));
    let power = args
        .get(4)
        .map(|p| p.parse::<f32>().unwrap())
        .unwrap_or(1.0);
    let cell = ac_world::outdoor_cell(block, start);
    let scene = landblock::load(&assets, block).unwrap();
    let world = CollisionWorld::from_scene(&assets, &scene).unwrap();
    let origin = ac_scene::lbid::world_origin(block);
    let mut bad = 0;
    for deg in (0..360).step_by(15) {
        let heading = (deg as f32).to_radians();
        let mut pl = Player::new(&assets, cell, start, Quat::from_rotation_z(heading));
        pl.set_motion_table(&assets, 0x0200_0001, 0x0900_0001);
        pl.max_jump_power = 1.0;
        let dt = 1.0 / 60.0;
        // Run for half a second, jump, then keep running for three seconds.
        let mut lowest = start.z;
        for frame in 0..210 {
            let input = Input {
                forward: 1.0,
                strafe: 0.0,
                run: true,
                jump: frame == 30,
                jump_held: false,
            };
            if frame == 30 {
                pl.jump(power);
            }
            pl.update(&assets, &input, dt);
            lowest = lowest.min(pl.world_position().z);
        }
        let end = pl.world_position();
        let headroom = world
            .ceiling_at(end, 0.4)
            .map(|cz| cz - end.z)
            .unwrap_or(f32::INFINITY);
        let stuck = headroom < 1.7;
        if stuck {
            bad += 1;
        }
        let l = end - origin;
        println!(
            "heading {deg:3}: end cell {:#010x} local ({:.1}, {:.1}, {:.1}) lowest {:.1} headroom {:.1}{}",
            pl.cell,
            l.x,
            l.y,
            l.z,
            lowest - origin.z,
            headroom,
            if stuck { "  <- STUCK" } else { "" }
        );
    }
    println!("{bad} of 24 headings ended somewhere the capsule does not fit");
}
