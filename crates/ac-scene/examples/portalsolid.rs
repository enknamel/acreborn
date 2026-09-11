//! Are a cell's doorways solid in its physics mesh?
//!
//! A cell structure lists the polygons that are portals -- the holes
//! between one cell and the next. The drawing skips them. If the
//! physics mesh keeps them, every doorway in the game has an invisible
//! pane across it: you can stand on either side and never walk through.
//!
//! Usage: portalsolid <block hex>
use ac_scene::Assets;

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let arg = std::env::args().nth(1).unwrap_or_else(|| "ABB3".into());
    let block = u32::from_str_radix(arg.trim_start_matches("0x"), 16).unwrap() << 16;
    let scene = assets.landblock(block).unwrap();
    let (mut portals, mut solid_portals, mut structures) = (0, 0, 0);
    for cell in &scene.cells {
        let Ok(env) = assets.environment(cell.environment_id) else {
            continue;
        };
        let Some((_, cs)) = env
            .cells
            .iter()
            .find(|(k, _)| *k == cell.cell_structure as u32)
        else {
            continue;
        };
        structures += 1;
        for id in &cs.portals {
            portals += 1;
            if cs.physics_polygons.iter().any(|(k, _)| k == id) {
                solid_portals += 1;
            }
        }
    }
    println!("{structures} cell structures, {portals} portal polygons");
    println!("  of which solid in the physics mesh: {solid_portals}");
}
