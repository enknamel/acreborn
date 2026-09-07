//! Plan walks across a neighbourhood of landblocks and report what they
//! cost. Sanamar is the case that prompted it: the town's walls stopped
//! a character walking in from outside, because a route planned on one
//! landblock at a time cannot see the gate in the next one.
//!
//! Usage: areapath [block_x block_y [local_x local_y]]
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
    let a: Vec<String> = std::env::args().skip(1).collect();
    let num = |i: usize, d: f32| a.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    let (bx, by) = (num(0, 51.0) as u32, num(1, 217.0) as u32);
    let block = lbid::from_xy(bx, by);
    let origin = lbid::world_origin(block);
    let centre = Vec3::new(origin.x + num(2, 59.0), origin.y + num(3, 100.0), 0.0);

    let t0 = std::time::Instant::now();
    let mut area = Area::build(&assets, &blocks_for(centre, centre), &cap, block).unwrap();
    println!(
        "area {:#06x} ({} blocks, dungeon: {}): {} triangles in {:?}",
        block >> 16,
        area.blocks.len(),
        area.dungeon,
        area.collision.tris.len(),
        t0.elapsed()
    );
    let cz = area
        .terrain_at(centre.x, centre.y)
        .or_else(|| {
            area.collision
                .floor_at(centre + Vec3::Z * 5.0, 0.0, 20.0)
                .map(|(z, _)| z)
        })
        .unwrap_or(0.0);
    let centre = Vec3::new(centre.x, centre.y, cz);

    let (bearings, radius) = (16usize, 130.0f32);
    let (mut blocked, mut failed) = (0, 0);
    for i in 0..bearings {
        let ang = i as f32 / bearings as f32 * std::f32::consts::TAU;
        let p = Vec3::new(
            centre.x + radius * ang.cos(),
            centre.y + radius * ang.sin(),
            0.0,
        );
        let Some(z) = area.terrain_at(p.x, p.y) else {
            println!("{:>3.0} deg: no ground", ang.to_degrees());
            continue;
        };
        let from = Vec3::new(p.x, p.y, z);
        if area.line_clear(from + Vec3::Z, centre + Vec3::Z) {
            println!("{:>3.0} deg: line clear", ang.to_degrees());
            continue;
        }
        blocked += 1;
        let t0 = std::time::Instant::now();
        let path = area.path(from, centre);
        let dt = t0.elapsed();
        match path {
            Some(p) => {
                let mut len = 0.0;
                let mut prev = from;
                for w in &p {
                    len += prev.distance(*w);
                    prev = *w;
                }
                println!(
                    "{:>3.0} deg: blocked -> {} waypoints, {len:.0} m for {:.0} m straight, {dt:?}",
                    ang.to_degrees(),
                    p.len(),
                    from.distance(centre),
                );
            }
            None => {
                failed += 1;
                println!("{:>3.0} deg: blocked -> NO PATH, {dt:?}", ang.to_degrees());
            }
        }
    }
    println!(
        "{blocked} of {bearings} bearings blocked, {failed} with no route; \
         graph now {} nodes in {} chunks, {:?} building",
        area.nav.len(),
        area.nav.chunk_count(),
        area.nav.build_time
    );
}
