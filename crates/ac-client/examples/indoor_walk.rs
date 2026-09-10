//! Can a character walk about inside a building?
//!
//! `AC_DATA_DIR=... cargo run --release -p ac-client --example indoor_walk`
//!
//! A landblock counts as a dungeon only when every terrain height is
//! zero. A town with buildings in it never is, so the outdoor ground
//! used to be treated as solid inside those buildings too, and it cuts
//! straight through the room. This walks a few short steps inside an
//! interior cell and says whether anything stopped them.
use std::rc::Rc;

use ac_client::player::Player;
use ac_scene::Assets;
use glam::{Quat, Vec3};

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Rc::new(Assets::open(std::path::Path::new(&dir)).unwrap());

    // Holtburg: real terrain, and interior cells for its buildings.
    let block = 0xA9B4_0000u32;
    // An interior cell of that block (the meeting hall's portal stands
    // in it), and a spot inside it.
    let indoor_cell = 0xA9B4_017A;
    let here = Vec3::new(159.2, 39.0, 96.0);

    for (what, cell) in [
        ("inside a building", indoor_cell),
        ("out on the grass", block | 0x19),
    ] {
        let mut pl = Player::new(&assets, cell, here, Quat::IDENTITY);
        pl.set_motion_table(&assets, 0x0200_0001, 0x0900_0001);
        let from = pl.world_position();
        let mut blocked = 0;
        let mut tried = 0;
        for (dx, dy) in [
            (3.0, 0.0),
            (-3.0, 0.0),
            (0.0, 3.0),
            (0.0, -3.0),
            (2.0, 2.0),
            (-2.0, -2.0),
        ] {
            let to = from + Vec3::new(dx, dy, 0.0);
            tried += 1;
            if pl.line_blocked(&assets, block, from, to) {
                blocked += 1;
            }
        }
        println!(
            "{what:<20} cell {cell:#010x}  indoors={}  {blocked}/{tried} short steps blocked",
            (cell & 0xFFFF) >= 0x100,
        );
    }
}
