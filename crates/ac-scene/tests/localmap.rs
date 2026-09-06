//! Local maps against real data. Needs AC_DATA_DIR.
use ac_scene::{lbid, localmap};
use glam::Vec2;

fn alpha_at(map: &localmap::LocalMap, world: Vec2) -> u8 {
    let p = map.image.to_pixel(world);
    map.image
        .get(p.x.floor() as i64, p.y.floor() as i64)
        .unwrap_or_else(|| {
            panic!(
                "{world} is outside the {}x{} map",
                map.image.width, map.image.height
            )
        })[3]
}

/// The Holtburg meeting hall (0x0125): a hall floor at z = 0 with a
/// balcony at z = 6 whose entrance is at local (30, -60, 6).
#[test]
fn meeting_hall_plan_shows_the_balcony_but_not_the_hall_below() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = ac_scene::Assets::open(dir).unwrap();
    let block = 0x0125_0000;
    let origin = lbid::world_origin(block);
    let o = Vec2::new(origin.x, origin.y);
    let map = localmap::render(&assets, block, 2.0, Some((5.0, 8.0))).unwrap();
    assert!(map.dungeon);
    assert_eq!(map.image.scale, 2.0);
    assert!(
        map.z_min < 1.0 && map.z_max > 5.0,
        "{}..{}",
        map.z_min,
        map.z_max
    );
    // The entrance stands on the balcony level.
    assert_eq!(alpha_at(&map, o + Vec2::new(30.0, -60.0)), 255);
    // Over the middle of the hall there is no floor at the balcony level.
    assert_eq!(alpha_at(&map, o + Vec2::new(30.0, -38.0)), 0);
    // The hall floor is there once every storey is drawn.
    let all = localmap::render(&assets, block, 2.0, None).unwrap();
    assert_eq!(alpha_at(&all, o + Vec2::new(30.0, -38.0)), 255);
    assert_eq!(
        (all.image.width, all.image.height),
        (map.image.width, map.image.height)
    );

    // The image covers the geometry: both points map inside it and the
    // pixel/world transform round-trips.
    let img = &map.image;
    for w in [o + Vec2::new(30.0, -60.0), o + Vec2::new(30.0, -38.0)] {
        assert!(img.contains(w), "{w} outside the map");
        let back = img.to_world(img.to_pixel(w));
        assert!((back - w).length() < 1e-2);
    }
    // Dungeon geometry lies south of the block origin, so the map does too.
    assert!(img.origin.y < o.y, "origin {} vs block {}", img.origin, o);
    assert!(img.contains(img.origin + img.size() * 0.5));
}

#[test]
fn holtburg_outdoors_is_the_block_square_and_opaque() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = ac_scene::Assets::open(dir).unwrap();
    let block = 0xA9B4_0000;
    let map = localmap::render(&assets, block, 2.0, None).unwrap();
    assert!(!map.dungeon);
    assert_eq!((map.image.width, map.image.height), (384, 384));
    let origin = lbid::world_origin(block);
    assert_eq!(map.image.origin, Vec2::new(origin.x, origin.y));
    assert_eq!(map.image.size(), Vec2::splat(192.0));
    let clear = map.image.rgba.chunks(4).filter(|p| p[3] != 255).count();
    assert_eq!(clear, 0, "{clear} transparent pixels");
    // Buildings leave grey footprints somewhere in town.
    let grey = map
        .image
        .rgba
        .chunks(4)
        .filter(|p| p[0] == 150 && p[1] == 150 && p[2] == 150)
        .count();
    assert!(grey > 100, "{grey} footprint pixels");
}
