//! Does the navigation graph climb a dungeon's stairs? Builds the whole
//! graph of one dungeon landblock, then asks for a path between the
//! lowest floor it found and the highest.
//!
//! Usage: dungeonnav <block hex, e.g. 01EA>
use ac_scene::collision::Capsule;
use ac_scene::navarea::Area;
use ac_scene::Assets;

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let cap = Capsule {
        radius: 0.5,
        height: 1.8,
        step_up: 0.8,
        step_down: 1.5,
    };
    let arg = std::env::args().nth(1).unwrap_or_else(|| "01EA".into());
    let block = u32::from_str_radix(arg.trim_start_matches("0x"), 16).unwrap() << 16;
    let t0 = std::time::Instant::now();
    let mut area = Area::build(&assets, &[block], &cap, block).unwrap();
    println!(
        "{block:#010x}: dungeon={}, {} triangles, assembled in {:?}",
        area.dungeon,
        area.collision.tris.len(),
        t0.elapsed()
    );
    let t0 = std::time::Instant::now();
    area.build_everything();
    println!(
        "graph: {} nodes, {} edges, {} chunks, built in {:?}",
        area.nav.len(),
        area.nav.edge_count(),
        area.nav.chunk_count(),
        t0.elapsed()
    );
    if area.nav.is_empty() {
        println!("no walkable nodes");
        return;
    }
    // Which nodes can actually reach which: components of the directed
    // graph, biggest first, with the range of floors each one spans.
    let n = area.nav.len();
    let mut seen = vec![false; n];
    let mut comps: Vec<(usize, f32, f32, u32)> = Vec::new();
    for s in 0..n {
        if seen[s] {
            continue;
        }
        let mut stack = vec![s as u32];
        seen[s] = true;
        let (mut lo, mut hi, mut size) = (f32::MAX, f32::MIN, 0usize);
        let root = s as u32;
        while let Some(v) = stack.pop() {
            size += 1;
            let z = area.nav.nodes[v as usize].pos.z;
            lo = lo.min(z);
            hi = hi.max(z);
            for &w in area.nav.neighbours(v) {
                if !seen[w as usize] {
                    seen[w as usize] = true;
                    stack.push(w);
                }
            }
        }
        comps.push((size, lo, hi, root));
    }
    comps.sort_by(|a, b| b.0.cmp(&a.0));
    println!("{} reachable groups; the five biggest:", comps.len());
    for (size, lo, hi, root) in comps.iter().take(5) {
        println!(
            "  {size:>5} nodes, floors z={lo:.1}..{hi:.1} ({:.0} m of rise), from {:?}",
            hi - lo,
            area.nav.nodes[*root as usize].pos
        );
    }
}
