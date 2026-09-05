//! Item icons are 32x32 RenderSurfaces (0x06) that decode straight to RGBA.
//! Needs AC_DATA_DIR.

use ac_scene::Assets;

fn assets() -> Option<Assets> {
    let dir = std::env::var_os("AC_DATA_DIR")?;
    Some(Assets::open(dir).unwrap())
}

/// A handful of inventory icons of different pixel formats.
const ICONS: &[u32] = &[
    0x0600_0FAA,
    0x0600_1100, // R8G8B8
    0x0600_1A8A, // A8R8G8B8
    0x0600_1FB7,
    0x0600_2F40,
    0x0600_2C0D,
    0x0600_601C,
    0x0600_6A21,
];

#[test]
fn icons_decode_to_32x32_rgba() {
    let Some(assets) = assets() else { return };
    for &id in ICONS {
        let tex = assets.texture(id).unwrap();
        assert_eq!((tex.width, tex.height), (32, 32), "{id:#010x}");
        let img = assets.texture_rgba(id, None).unwrap();
        assert_eq!((img.width, img.height), (32, 32), "{id:#010x}");
        assert_eq!(img.pixels.len(), 32 * 32 * 4, "{id:#010x}");
        // An icon is a picture, not a blank: some pixel is visible and not
        // every visible pixel is the same color.
        let visible: Vec<&[u8]> = img.pixels.chunks_exact(4).filter(|p| p[3] > 0).collect();
        assert!(!visible.is_empty(), "{id:#010x}: fully transparent");
        assert!(
            visible.iter().any(|p| p[..3] != visible[0][..3]),
            "{id:#010x}: flat color"
        );
    }
}

#[test]
fn icon_ids_are_render_surfaces() {
    let Some(assets) = assets() else { return };
    for &id in ICONS {
        assert_eq!(id >> 24, 0x06);
        assert!(assets.portal.entry(id).is_some(), "{id:#010x} in portal");
    }
}
