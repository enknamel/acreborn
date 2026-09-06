//! Render the world map of Dereth to a PNG so it can be looked at:
//!
//! ```text
//! AC_DATA_DIR=... cargo run --release -p ac-scene --example world_map OUT.png [px_per_block]
//! ```
//!
//! The terrain grid comes from (or goes into) `WorldGrid::cache_dir()`;
//! the map itself is rendered fresh each run so palette changes show.

use ac_scene::{worldgrid::WorldGrid, worldmap, Assets};
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "worldmap.png".to_string());
    let px: u32 = args
        .next()
        .map(|s| s.parse().expect("px_per_block must be a number"))
        .unwrap_or(8);
    let dir = std::env::var_os("AC_DATA_DIR").expect("set AC_DATA_DIR to the client data");
    let assets = Assets::open(dir).expect("open the archives");

    let t = Instant::now();
    let grid = WorldGrid::load_cached(&assets, &WorldGrid::cache_dir()).expect("world grid");
    eprintln!("grid ready in {:.2?}", t.elapsed());
    let region = assets.region().expect("region");

    let t = Instant::now();
    let map = worldmap::render(&grid, &region, px);
    eprintln!(
        "rendered {}x{} at {px} px/block in {:.2?}",
        map.width,
        map.height,
        t.elapsed()
    );

    image::RgbaImage::from_raw(map.width, map.height, map.rgba)
        .expect("pixel buffer matches its size")
        .save(&out)
        .expect("write the PNG");
    eprintln!("wrote {out}");
}
