//! Assemble Holtburg (landblock A9B4) and its neighbours. Needs AC_DATA_DIR.

use ac_scene::{landblock, model, Assets};

#[test]
fn holtburg_assembles() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(dir).unwrap();
    let scene = landblock::load(&assets, 0xA9B4_0000).unwrap();
    assert_eq!(scene.terrain.vertices.len(), 81);
    assert_eq!(scene.terrain.indices.len(), 64 * 6);
    assert!(scene.has_info, "Holtburg has buildings");
    assert!(!scene.parts.is_empty());
    let mut tris = 0;
    let mut textured = 0;
    for p in &scene.parts {
        let g = assets.gfxobj(p.gfxobj_id).unwrap();
        let m = model::build_mesh(&assets, &g).unwrap();
        for s in &m.submeshes {
            tris += s.indices.len() / 3;
            if s.solid_color.is_none() && s.surface_id != 0 {
                let img = assets.surface_rgba(s.surface_id).unwrap();
                assert!(img.is_some());
                textured += 1;
            }
        }
    }
    eprintln!(
        "parts {} triangles {tris} textured submeshes {textured}",
        scene.parts.len()
    );
    assert!(tris > 1000);
}
