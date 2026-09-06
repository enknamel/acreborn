//! Jump from a grid of standing spots in every cell of a landblock and
//! report the ones that never land: the character hovering in the air
//! is the bug this catches (stairs, ledges, low ceilings).
//!
//! `AC_DATA_DIR=... cargo run --release -p ac-client --example jump_scan BLOCK [power]`

use ac_client::player::{Input, Player};
use ac_scene::collision::CollisionWorld;
use ac_scene::{landblock, Assets};
use glam::{Quat, Vec3};

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let block = u32::from_str_radix(args[0].trim_start_matches("0x"), 16).unwrap() << 16;
    let power = args
        .get(1)
        .map(|p| p.parse::<f32>().unwrap())
        .unwrap_or(1.0);
    let scene = landblock::load(&assets, block).unwrap();
    let world = CollisionWorld::from_scene(&assets, &scene).unwrap();
    let origin = ac_scene::lbid::world_origin(block);
    let mut spots = 0;
    let mut bad = 0;
    for cs in &scene.cells {
        // The cell transform is world-space.
        let c = cs.transform.transform_point3(Vec3::ZERO) - origin;
        let mut seen = 0;
        let (mut zmin, mut zmax) = (f32::INFINITY, f32::NEG_INFINITY);
        for ix in -4..=4 {
            for iy in -4..=4 {
                let p = Vec3::new(c.x + ix as f32 * 1.5, c.y + iy as f32 * 1.5, c.z + 3.0);
                let Some((z, cell)) = world.floor_at(origin + p, 0.0, 12.0) else {
                    continue;
                };
                if cell != cs.cell_id {
                    continue;
                }
                // Only level floors: a jump from a slope that only counts
                // as a floor by the skin of its normal is not a real case.
                let level = [(0.3, 0.0), (-0.3, 0.0), (0.0, 0.3), (0.0, -0.3)]
                    .iter()
                    .all(|(dx, dy)| {
                        world
                            .floor_at(
                                origin + Vec3::new(p.x + dx, p.y + dy, z - origin.z + 0.3),
                                0.0,
                                0.6,
                            )
                            .is_some_and(|(fz, _)| (fz - z).abs() < 0.05)
                    });
                if !level && std::env::var_os("SLOPES").is_none() {
                    continue;
                }
                // Every spot inside a dungeon has a ceiling; the top of a
                // roof cap (a vaulted ceiling seen from above) has none and
                // is not somewhere a player can be.
                if world
                    .ceiling_at(origin + Vec3::new(p.x, p.y, z - origin.z), 0.4)
                    .is_none()
                {
                    continue;
                }
                zmin = zmin.min(z - origin.z);
                zmax = zmax.max(z - origin.z);
                let start = Vec3::new(p.x, p.y, z - origin.z);
                seen += 1;
                spots += 1;
                for deg in (0..360).step_by(45) {
                    let heading = (deg as f32).to_radians();
                    let mut pl =
                        Player::new(&assets, cs.cell_id, start, Quat::from_rotation_z(heading));
                    pl.set_motion_table(&assets, 0x0200_0001, 0x0900_0001);
                    pl.max_jump_power = 1.0;
                    let dt = 1.0 / 60.0;
                    let mut airborne_frames = 0;
                    for frame in 0..360 {
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
                        if pl.is_airborne() {
                            airborne_frames += 1;
                        } else {
                            airborne_frames = 0;
                        }
                    }
                    let end_w = pl.world_position();
                    let on_nothing = !pl.is_airborne() && world.floor_at(end_w, 0.1, 0.1).is_none();
                    if on_nothing {
                        bad += 1;
                        let l = end_w - origin;
                        println!(
                            "ONNOTHING cell {:#010x} from ({:.1}, {:.1}, {:.1}) heading {deg:3}: at cell {:#010x} ({:.2}, {:.2}, {:.2}) floor below {:?}",
                            cs.cell_id, start.x, start.y, start.z, pl.cell, l.x, l.y, l.z,
                            world.floor_at(end_w, 0.1, 50.0).map(|(z, c)| (z - origin.z, format!("{c:#x}")))
                        );
                    }
                    if pl.is_airborne() && airborne_frames > 180 {
                        bad += 1;
                        let l = pl.world_position() - origin;
                        println!(
                            "HOVER cell {:#010x} from ({:.1}, {:.1}, {:.1}) heading {deg:3}: at cell {:#010x} ({:.1}, {:.1}, {:.1}) airborne {airborne_frames} frames",
                            cs.cell_id, start.x, start.y, start.z, pl.cell, l.x, l.y, l.z
                        );
                    }
                }
            }
        }
        if seen > 0 {
            println!(
                "cell {:#010x}: {seen} level spots, floor z {zmin:.1}..{zmax:.1}",
                cs.cell_id
            );
        }
    }
    println!(
        "{spots} spots in {} cells, {bad} hovering endings",
        scene.cells.len()
    );
}
