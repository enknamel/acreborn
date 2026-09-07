//! Find a shoreline and check that routes will not cross the water.
//! Scans landblocks for ones that are part sea and part land, then, in
//! the first good one, plans a walk between two points on the shore
//! that are separated by open water.
use ac_scene::collision::Capsule;
use ac_scene::navarea::{blocks_for, Area};
use ac_scene::{lbid, Assets};
use glam::Vec3;

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let cap = Capsule {
        radius: 0.5,
        height: 1.8,
        step_up: 0.8,
        step_down: 1.5,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (bx, by) = if args.len() >= 2 {
        (
            args[0].parse::<u32>().unwrap(),
            args[1].parse::<u32>().unwrap(),
        )
    } else {
        (51, 217)
    };
    let block = lbid::from_xy(bx, by);
    let origin = lbid::world_origin(block);
    let centre = origin + Vec3::new(96.0, 96.0, 0.0);
    let mut area = Area::build(&assets, &blocks_for(centre, centre), &cap, block).unwrap();
    // How much of the neighbourhood is open sea, sampled on a 4 m grid.
    let (mut sea, mut total) = (0, 0);
    let step = 4.0;
    let lo = lbid::world_origin(*area.blocks.first().unwrap());
    for i in 0..(3 * 192 / step as i32) {
        for j in 0..(3 * 192 / step as i32) {
            let (x, y) = (lo.x + i as f32 * step, lo.y + j as f32 * step);
            total += 1;
            if area.is_sea(x, y) {
                sea += 1;
            }
        }
    }
    println!(
        "block {:#06x} ({}N {}E): {sea} of {total} sampled points are open sea ({:.0}%)",
        block >> 16,
        (centre.y / 240.0 - 102.0).round(),
        (centre.x / 240.0 - 102.0).round(),
        100.0 * sea as f32 / total as f32
    );

    if sea == 0 {
        return;
    }
    // A pair of shore points with open water between them: a route must
    // go round the head of the bay, not across it.
    let shore: Vec<(f32, f32)> = (0..(3 * 192 / step as i32))
        .flat_map(|i| (0..(3 * 192 / step as i32)).map(move |j| (i, j)))
        .map(|(i, j)| (lo.x + i as f32 * step, lo.y + j as f32 * step))
        .filter(|&(x, y)| {
            !area.is_sea(x, y)
                && [(step, 0.0), (-step, 0.0), (0.0, step), (0.0, -step)]
                    .iter()
                    .any(|(dx, dy)| area.is_sea(x + dx, y + dy))
        })
        .collect();
    println!("{} shore points", shore.len());
    let mut tested = 0;
    for a in shore.iter().step_by(7) {
        for b in shore.iter().step_by(11) {
            let d = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
            if !(60.0..140.0).contains(&d) {
                continue;
            }
            // Water on the straight line between them.
            let n = (d / 4.0) as i32;
            let across = (1..n).any(|k| {
                let t = k as f32 / n as f32;
                area.is_sea(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
            });
            if !across {
                continue;
            }
            let Some(za) = area.terrain_at(a.0, a.1) else {
                continue;
            };
            let Some(zb) = area.terrain_at(b.0, b.1) else {
                continue;
            };
            let (pa, pb) = (Vec3::new(a.0, a.1, za), Vec3::new(b.0, b.1, zb));
            let t0 = std::time::Instant::now();
            let path = area.path(pa, pb);
            let dt = t0.elapsed();
            match path {
                Some(p) => {
                    let mut len = 0.0;
                    let mut prev = pa;
                    let mut wet = 0;
                    for w in &p {
                        len += prev.distance(*w);
                        // Sample the leg: no part of it may be at sea.
                        let steps = (prev.distance(*w) / 3.0).ceil() as i32;
                        for k in 0..=steps {
                            let t = k as f32 / steps.max(1) as f32;
                            let q = prev.lerp(*w, t);
                            if area.is_sea(q.x, q.y) {
                                wet += 1;
                            }
                        }
                        prev = *w;
                    }
                    println!(
                        "  {d:.0} m apart across water -> {} waypoints, {len:.0} m round, \
                         {wet} samples at sea, {dt:?}",
                        p.len()
                    );
                }
                None => println!("  {d:.0} m apart across water -> no route ({dt:?})"),
            }
            tested += 1;
            if tested >= 6 {
                return;
            }
        }
    }
    if tested == 0 {
        println!("  no pair of shore points with water between them");
    }
}
