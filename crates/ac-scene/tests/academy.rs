//! The Training Academy dungeon (landblock 8602) and its cell lights.
//! Needs AC_DATA_DIR.

use ac_scene::{landblock, Assets};

#[test]
fn academy_cells_carry_lights() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(dir).unwrap();
    let scene = landblock::load(&assets, 0x8602_0000).unwrap();
    assert!(scene.is_dungeon, "the Academy is underground");
    assert!(!scene.cells.is_empty());
    let lights = scene.lights.light_count();
    assert!(lights > 0, "no cell lights in the Academy");
    let lit_cells = scene.cells.iter().filter(|c| !c.lights.is_empty()).count();
    eprintln!(
        "academy: {} cells, {lights} lights in {lit_cells} cells",
        scene.cells.len()
    );
    // Every light has a colour and a finite reach, and sits in its cell
    // (static object frames are landblock-local; composing them with the
    // cell frame too used to throw torches 35 m away).
    for c in &scene.cells {
        let origin = c.transform.transform_point3(glam::Vec3::ZERO);
        for l in &c.lights {
            assert!(l.radius > 0.0 && l.radius < 100.0, "{l:?}");
            assert!(l.color.max_element() > 0.0, "{l:?}");
            assert!(
                l.position.distance(origin) < 20.0,
                "light {l:?} far from cell {:#010x} at {origin}",
                c.cell_id
            );
        }
    }
    // The block stays available to per-frame lookups without a rebuild.
    assert!(assets.cached_landblock(0x8602_01AD).is_some());
    // Standing at a torch, an object is brighter than the ambient, and
    // cells reach their neighbours' lights through the portals.
    let ambient = ac_scene::lighting::DUNGEON_AMBIENT;
    let cell = scene.cells.iter().find(|c| !c.lights.is_empty()).unwrap();
    let at_torch = scene
        .lights
        .sample(cell.cell_id, cell.lights[0].position)
        .unwrap();
    assert!(
        at_torch.max_element() > ambient.max_element() + 0.1,
        "{at_torch}"
    );
    let reach = scene.lights.cell(cell.cell_id).unwrap().lights.len();
    assert!(reach >= cell.lights.len());
    // Outdoors cells are not this block's business.
    assert!(scene.lights.sample(0x8602_0001, glam::Vec3::ZERO).is_none());
}
