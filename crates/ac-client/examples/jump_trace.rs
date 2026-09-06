//! Trace one running jump frame by frame from a spot in a cell.
//! `AC_DATA_DIR=... cargo run --release -p ac-client --example jump_trace CELL x y z heading_deg [power]`
use ac_client::player::{Input, Player};
use ac_scene::Assets;
use glam::{Quat, Vec3};

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cell = u32::from_str_radix(args[0].trim_start_matches("0x"), 16).unwrap();
    let f = |i: usize| args[i].parse::<f32>().unwrap();
    let start = Vec3::new(f(1), f(2), f(3));
    let heading = f(4).to_radians();
    let power = args
        .get(5)
        .map(|p| p.parse::<f32>().unwrap())
        .unwrap_or(1.0);
    let origin = ac_world::landblock_origin(cell);
    let scene = ac_scene::landblock::load(&assets, cell & 0xFFFF_0000).unwrap();
    let world = ac_scene::collision::CollisionWorld::from_scene(&assets, &scene).unwrap();
    let mut pl = Player::new(&assets, cell, start, Quat::from_rotation_z(heading));
    pl.set_motion_table(&assets, 0x0200_0001, 0x0900_0001);
    pl.max_jump_power = 1.0;
    let dt = 1.0 / 60.0;
    for frame in 0..240 {
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
        pl.update(&assets, &input, dt);
        let l = pl.world_position() - origin;
        if frame % 5 == 0 || (28..40).contains(&frame) {
            let w = pl.world_position();
            let below = world
                .floor_at(w, 0.6, 200.0)
                .map(|(z, c)| format!("{:.2} in {:#010x}", z - origin.z, c));
            let ceil = world.ceiling_at(w, 0.4).map(|z| z - origin.z);
            println!(
                "{frame:3} cell {:#010x} ({:.2}, {:.2}, {:.2}) air {} floor {:?} ceiling {:?}",
                pl.cell,
                l.x,
                l.y,
                l.z,
                pl.is_airborne(),
                below,
                ceil
            );
        }
    }
}
