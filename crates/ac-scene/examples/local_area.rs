//! Time and write the map of the landblocks around one.
//! `cargo run --release -p ac-scene --example local_area BLOCK OUT.png [radius] [px_per_m]`
use ac_scene::{localmap, Assets};

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let a: Vec<String> = std::env::args().skip(1).collect();
    let block = u32::from_str_radix(a[0].trim_start_matches("0x"), 16).unwrap() << 16;
    let radius: u32 = a.get(2).and_then(|r| r.parse().ok()).unwrap_or(1);
    let px: f32 = a.get(3).and_then(|p| p.parse().ok()).unwrap_or(2.0);
    let t0 = std::time::Instant::now();
    let m = localmap::render_area(&assets, block, radius, px, None).unwrap();
    let took = t0.elapsed();
    let blocks = (radius * 2 + 1) * (radius * 2 + 1);
    println!(
        "radius {radius} ({blocks} blocks): {}x{} px, {:.1} MB, {took:?}",
        m.image.width,
        m.image.height,
        m.image.rgba.len() as f32 / 1e6
    );
    image::save_buffer(
        &a[1],
        &m.image.rgba,
        m.image.width,
        m.image.height,
        image::ColorType::Rgba8,
    )
    .unwrap();
}
