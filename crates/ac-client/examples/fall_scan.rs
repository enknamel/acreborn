//! Jump from a grid of spots across an outdoor landblock and report any
//! that end below the terrain: falling through the ground.
//!
//! `AC_DATA_DIR=... cargo run --release -p ac-client --example fall_scan BLOCK [step_m]`

use ac_client::player::{Input, Player};
use ac_scene::{landblock, Assets};
use glam::{Quat, Vec3};

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let block = u32::from_str_radix(args[0].trim_start_matches("0x"), 16).unwrap() << 16;
    let step: f32 = args.get(1).map(|s| s.parse().unwrap()).unwrap_or(8.0);
    let scene = landblock::load(&assets, block).unwrap();
    let origin = ac_scene::lbid::world_origin(block);
    let mut spots = 0;
    let mut bad = 0;
    let mut y = 4.0;
    while y < 188.0 {
        let mut x = 4.0;
        while x < 188.0 {
            let Some(z) = scene.terrain.height_at(Vec3::new(x, y, 0.0)) else {
                x += step;
                continue;
            };
            spots += 1;
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
                let end = pl.world_position() - origin;
                // Where should the ground be under the ending?
                let ground = scene.terrain.height_at(Vec3::new(end.x, end.y, 0.0));
                let below = ground.map(|g| g - end.z).unwrap_or(0.0);
                if pl.is_airborne() || below > 2.0 {
                    bad += 1;
                    println!(
                        "THROUGH from ({x:.0}, {y:.0}, {z:.1}) heading {deg:3}: ended ({:.1}, {:.1}, {:.1}) ground {:?} below by {below:.1} airborne {}",
                        end.x, end.y, end.z, ground.map(|g| (g * 10.0).round() / 10.0), pl.is_airborne()
                    );
                }
            }
            x += step;
        }
        y += step;
    }
    println!("{spots} spots, {bad} endings under the ground");
}
