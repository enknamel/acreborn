//! A character's look from the CharGen table dresses the head part.
//! Needs AC_DATA_DIR.

use ac_scene::chargen::{self, Look, HEAD_PART};
use ac_scene::{model, Assets};

fn assets() -> Option<Assets> {
    let dir = std::env::var_os("AC_DATA_DIR")?;
    Some(Assets::open(dir).unwrap())
}

/// Texture overrides the head part ends up with: (surface id, new texture).
fn head_overrides(assets: &Assets, setup_id: u32, app: &model::Appearance) -> Vec<(u32, u32)> {
    let parts = model::place_with(assets, setup_id, glam::Mat4::IDENTITY, app).unwrap();
    let head = parts
        .iter()
        .find(|p| p.part_index == HEAD_PART)
        .expect("humanoid Setup has a head part");
    let g = assets.gfxobj(head.gfxobj_id).unwrap();
    let mesh = model::build_mesh_with(assets, &g, HEAD_PART, app).unwrap();
    mesh.submeshes
        .iter()
        .filter_map(|s| s.texture_override.map(|t| (s.surface_id, t)))
        .collect()
}

#[test]
fn aluvian_male_head_gets_hair_and_face() {
    let Some(assets) = assets() else { return };
    let look = Look {
        hair_style: 3,
        ..Look::default()
    };
    let desc = chargen::describe(&assets, &look).unwrap();
    assert_eq!(desc.setup_id, 0x0200_0001, "human male Setup");
    assert_eq!(desc.palette_id, 0x0400_007E, "human base palette");
    // Hair style 3 swaps the head GfxObj; hair, eyes, nose and mouth are
    // texture changes on the head part, all keyed by the placeholder
    // SurfaceTexture ids that the head's surfaces reference.
    assert_eq!(desc.part_changes.len(), 1);
    assert_eq!(desc.part_changes[0].0, HEAD_PART);
    assert_ne!(desc.part_changes[0].1, 0x0100_005A, "not the bare head");
    assert_eq!(desc.texture_changes.len(), 4);
    assert!(desc.texture_changes.iter().all(|c| c.0 == HEAD_PART));
    assert_eq!(desc.sub_palettes.len(), 3, "hair, skin and eye colours");

    let app = desc.appearance(&assets);
    assert_eq!(
        app.part_swaps.get(&HEAD_PART),
        Some(&desc.part_changes[0].1)
    );
    let palette = app.palette.as_ref().expect("composed palette");
    assert_eq!(palette.len(), 2048, "Index16 textures use the full palette");

    let overrides = head_overrides(&assets, desc.setup_id, &app);
    assert_eq!(
        overrides.len(),
        4,
        "every head texture change matched a surface: {overrides:x?}"
    );
    for (_, tex) in &overrides {
        assert!(
            desc.texture_changes.iter().any(|c| c.2 == *tex),
            "override {tex:#010x} is one of the chosen textures"
        );
        // Each replacement decodes with the composed palette.
        let img = assets.texture_rgba_with_palette(*tex, palette).unwrap();
        assert!(img.width > 0 && img.height > 0);
    }
}

#[test]
fn bald_style_still_has_a_face() {
    let Some(assets) = assets() else { return };
    // Aluvian male hair style 0 is bald; the eye strip then uses its bald
    // variant, and the head is still retextured with eyes, nose and mouth.
    let desc = chargen::describe(&assets, &Look::default()).unwrap();
    let hairy = chargen::describe(
        &assets,
        &Look {
            hair_style: 3,
            ..Look::default()
        },
    )
    .unwrap();
    assert_ne!(
        desc.texture_changes[1], hairy.texture_changes[1],
        "bald eyes differ"
    );
    let app = desc.appearance(&assets);
    let overrides = head_overrides(&assets, desc.setup_id, &app);
    assert_eq!(overrides.len(), 4, "{overrides:x?}");
}

#[test]
fn later_texture_changes_replace_earlier_ones() {
    let Some(assets) = assets() else { return };
    // A server lists the base body before clothing: the client's
    // ObjDesc::AddTextureMapChange drops the earlier entry for the same
    // part and old texture, so the shirt (second entry) wins.
    let changes = [
        (0u8, 0x0500_0BB0u32, 0x0500_0BB0u32),
        (0, 0x0500_0CBE, 0x0500_0CBE),
        (0, 0x0500_0BB0, 0x0500_025D),
        (0, 0x0500_0CBE, 0x0500_0CEA),
    ];
    let app = model::Appearance::from_obj_desc(&assets, 0, &[], &changes, &[(0, 0x0100_004E)]);
    let swaps = &app.texture_swaps[&0];
    assert_eq!(
        swaps,
        &vec![(0x0500_0BB0, 0x0500_025D), (0x0500_0CBE, 0x0500_0CEA)]
    );
    let g = assets.gfxobj(0x0100_004E).unwrap();
    let mesh = model::build_mesh_with(&assets, &g, 0, &app).unwrap();
    let mut overrides: Vec<u32> = mesh
        .submeshes
        .iter()
        .filter_map(|s| s.texture_override)
        .collect();
    overrides.sort_unstable();
    assert_eq!(overrides, vec![0x0500_025D, 0x0500_0CEA]);
}

#[test]
fn heritage_lookup() {
    let Some(assets) = assets() else { return };
    let cg = assets.chargen().unwrap();
    assert_eq!(chargen::heritage_id(&cg, "aluvian"), Some(1));
    assert_eq!(chargen::heritage_id(&cg, "Sho"), Some(3));
    assert_eq!(chargen::heritage_id(&cg, "2"), Some(2));
    assert_eq!(chargen::heritage_id(&cg, "klingon"), None);
}
