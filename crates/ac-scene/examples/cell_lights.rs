//! List the interior cells of a landblock with their lights.
//!
//!     AC_DATA_DIR=~/Downloads/ac_data cargo run -p ac-scene --example cell_lights -- 8602

use ac_scene::{landblock, Assets};

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let arg = std::env::args().nth(1).expect("landblock id, e.g. 8602");
    let id = u32::from_str_radix(arg.trim_start_matches("0x"), 16).expect("hex id") << 16;
    let assets = Assets::open(dir).unwrap();
    let scene = landblock::load(&assets, id).unwrap();
    println!(
        "{id:#010x}: {} cells, {} lights, dungeon {}",
        scene.cells.len(),
        scene.lights.light_count(),
        scene.is_dungeon
    );
    for c in &scene.cells {
        let o = c.transform.transform_point3(glam::Vec3::ZERO);
        let reach = scene.lights.cell(c.cell_id).map_or(0, |l| l.lights.len());
        println!(
            "cell {:#010x} at ({:.1}, {:.1}, {:.1}) portals {:?} lights {} (+{} through portals)",
            c.cell_id,
            o.x,
            o.y,
            o.z,
            c.portal_cells
                .iter()
                .map(|p| p & 0xFFFF)
                .collect::<Vec<_>>(),
            c.lights.len(),
            reach.saturating_sub(c.lights.len()),
        );
        for l in &c.lights {
            println!(
                "    light at ({:.1}, {:.1}, {:.1}) colour ({:.2}, {:.2}, {:.2}) radius {}",
                l.position.x, l.position.y, l.position.z, l.color.x, l.color.y, l.color.z, l.radius
            );
        }
    }
}
