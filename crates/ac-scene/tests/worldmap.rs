//! The world map against real data. Needs AC_DATA_DIR.
use ac_scene::{worldgrid::WorldGrid, worldmap, Assets};
use glam::Vec2;

/// World xy of the centre of landblock `id` (`xxyy0000`).
fn block_centre(id: u32) -> Vec2 {
    Vec2::new(
        ((id >> 24) as f32 + 0.5) * 192.0,
        (((id >> 16) & 0xFF) as f32 + 0.5) * 192.0,
    )
}

#[test]
fn holtburg_is_land_and_the_open_sea_is_water() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(dir).unwrap();
    let grid = WorldGrid::load_cached(&assets, &WorldGrid::cache_dir()).unwrap();
    let region = assets.region().unwrap();
    let map = worldmap::render(&grid, &region, 1);
    assert_eq!((map.width, map.height), (255, 255));
    assert_eq!(map.origin, Vec2::ZERO);
    assert!((map.scale - 1.0 / 192.0).abs() < 1e-9);

    let p = map.to_pixel(block_centre(0xA9B4_0000));
    let holtburg = map.get(p.x as i64, p.y as i64).unwrap();
    assert!(
        !worldmap::is_water_color(holtburg),
        "Holtburg pixel {holtburg:?} should be land"
    );
    // Holtburg is in the north-east quarter: right of centre, upper rows.
    assert!(p.x > 128.0 && p.y < 128.0, "{p:?}");

    // Well inside the inland sea, south-west of the Obsidian Span. (0x0101
    // is not open sea: the archive keeps a small test island with a road
    // in its south-west corner, and 0x8080 is the sand peninsula's shore.)
    let p = map.to_pixel(block_centre(0x6070_0000));
    let sea = map.get(p.x as i64, p.y as i64).unwrap();
    assert!(
        worldmap::is_water_color(sea),
        "inland sea pixel {sea:?} should be water"
    );
    // Every landblock of the retail archive is present (open sea is stored
    // as WaterDeepSea blocks), so the missing-block path is covered by the
    // unit test in the module instead.
    assert!((0..255usize).all(|x| (0..255usize).all(|y| grid.has_block(x, y))));
}

#[test]
fn cached_round_trips_through_the_file() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(dir).unwrap();
    let tmp = std::env::temp_dir().join(format!("acreborn-worldmap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    // Reuse the grid cache when there is one so the test does not read
    // 65k landblocks twice.
    let _ = std::fs::copy(
        WorldGrid::cache_dir().join("worldgrid.bin"),
        tmp.join("worldgrid.bin"),
    );

    let first = worldmap::cached(&assets, &tmp, 2).unwrap();
    assert_eq!((first.width, first.height), (510, 510));
    let path = tmp.join(worldmap::cache_name(2));
    assert!(path.is_file(), "{} written", path.display());
    let second = worldmap::cached(&assets, &tmp, 2).unwrap();
    assert_eq!(first, second);
    let parsed = worldmap::from_bytes(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(parsed, first);
    let _ = std::fs::remove_dir_all(&tmp);
}
