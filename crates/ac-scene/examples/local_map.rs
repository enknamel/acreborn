//! Render a landblock's local map (dungeon floor plan or outdoor block)
//! to a PNG.
//!
//! `AC_DATA_DIR=... cargo run --release -p ac-scene --example local_map BLOCK OUT.png [px_per_metre] [zlo zhi]`
//!
//! `BLOCK` is a hex landblock id such as `0125` or `A9B4`; `zlo zhi`
//! limits a dungeon plan to floors between those heights.
use ac_scene::{localmap, Assets};

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: local_map BLOCK OUT.png [px_per_metre] [zlo zhi]");
        std::process::exit(2);
    }
    let block = u32::from_str_radix(args[0].trim_start_matches("0x"), 16).unwrap() << 16;
    let out = &args[1];
    let scale: f32 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(2.0);
    let z_range = match (args.get(3), args.get(4)) {
        (Some(lo), Some(hi)) => Some((lo.parse().unwrap(), hi.parse().unwrap())),
        _ => None,
    };
    let map = localmap::render(&assets, block, scale, z_range).unwrap();
    let img = &map.image;
    println!(
        "{:#010x}: {} {}x{} px, origin ({:.1}, {:.1}), {:.1} m square, floors z {:.1}..{:.1}",
        block,
        if map.dungeon { "dungeon" } else { "outdoors" },
        img.width,
        img.height,
        img.origin.x,
        img.origin.y,
        img.size().x,
        map.z_min,
        map.z_max
    );
    image::save_buffer(
        out,
        &img.rgba,
        img.width,
        img.height,
        image::ColorType::Rgba8,
    )
    .unwrap();
    println!("wrote {out}");
}
