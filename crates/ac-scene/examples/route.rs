//! Route on foot between two map coordinates.
//! `cargo run --release -p ac-scene --example route -- 69.6,-62.4 -60.5,-88.0`
use ac_scene::{worldgrid::WorldGrid, worldroute, Assets};
use glam::Vec2;

fn xy(s: &str) -> Vec2 {
    let (ns, ew) = s.split_once(',').expect("ns,ew");
    let ns: f32 = ns.trim().parse().unwrap();
    let ew: f32 = ew.trim().parse().unwrap();
    Vec2::new((ew + 102.0) * 240.0, (ns + 102.0) * 240.0)
}

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let grid = WorldGrid::load_cached(&assets, &WorldGrid::cache_dir()).unwrap();
    let region = assets.region().unwrap();
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (from, to) = (xy(&a[0]), xy(&a[1]));
    println!("straight line {:.0} m", from.distance(to));
    let t0 = std::time::Instant::now();
    match worldroute::find(&grid, &region, from, to) {
        Some(r) => {
            let len: f32 = r.windows(2).map(|w| w[0].distance(w[1])).sum();
            println!(
                "route: {} waypoints, {len:.0} m, {:.0} min on foot, found in {:?}",
                r.len(),
                len / 5.0 / 60.0,
                t0.elapsed()
            );
        }
        None => println!("no route on foot ({:?})", t0.elapsed()),
    }
}
