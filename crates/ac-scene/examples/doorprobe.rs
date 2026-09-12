//! Why does a doorway get no navigation node?
//!
//! For every opening the cell data names, runs the same checks the
//! graph does when it tries to stand a character there, and counts
//! which one turns it away.
//!
//! Usage: doorprobe <block hex, e.g. A9B4>
use ac_scene::collision::Capsule;
use ac_scene::navarea::Area;
use ac_scene::Assets;
use glam::Vec3;

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let cap = Capsule {
        radius: 0.679,
        height: 1.835,
        step_up: 0.600,
        step_down: 1.500,
    };
    let arg = std::env::args().nth(1).unwrap_or_else(|| "A9B4".into());
    let block = u32::from_str_radix(arg.trim_start_matches("0x"), 16).unwrap() << 16;
    let mut area = Area::build(&assets, &[block], &cap, block).unwrap();
    let doors = area.doorways.clone();
    let spacing = 1.5f32;
    let (mut shifted, mut no_floor, mut not_in_cell, mut no_fit, mut ok) = (0, 0, 0, 0, 0);
    let mut shift_sizes: Vec<f32> = Vec::new();
    area.with_ground(|ground, _| {
        for d in &doors {
            let q = ground
                .collision
                .resolve_above(*d, cap.radius, cap.height, cap.step_up);
            let shift = ((q.x - d.x).powi(2) + (q.y - d.y).powi(2)).sqrt();
            shift_sizes.push(shift);
            // The graph refuses a node it had to shove more than a
            // quarter of the lattice to stand up.
            if shift > 0.25 * spacing {
                shifted += 1;
                continue;
            }
            let Some((z, cell)) = ground.surface_at(Vec3::new(q.x, q.y, d.z), &cap) else {
                no_floor += 1;
                continue;
            };
            let feet = Vec3::new(q.x, q.y, z);
            if cell != 0
                && !ground
                    .collision
                    .inside_cell(feet + Vec3::new(0.0, 0.0, 0.1))
            {
                not_in_cell += 1;
                continue;
            }
            if !ground.fits_here(feet, &cap) {
                no_fit += 1;
                continue;
            }
            ok += 1;
        }
    });
    println!("{} doorways", doors.len());
    println!("  shoved too far to stand : {shifted}");
    println!("  no floor within reach   : {no_floor}");
    println!("  floor not inside a cell : {not_in_cell}");
    println!("  character does not fit  : {no_fit}");
    println!("  a node stands here      : {ok}");
    // Standing in a doorway is no use if nothing joins to it.
    area.build_everything();
    let mut lonely = 0;
    let mut joined = 0;
    let mut one_sided = 0;
    for d in &doors {
        let near = (0..area.nav.len())
            .filter(|&i| area.nav.nodes[i].pos.distance(*d) < 1.0)
            .collect::<Vec<_>>();
        for i in near {
            let out = area.nav.neighbours(i as u32).len();
            let back = (0..area.nav.len())
                .filter(|&j| area.nav.neighbours(j as u32).contains(&(i as u32)))
                .count();
            if out == 0 && back == 0 {
                lonely += 1;
            } else if out == 0 || back == 0 {
                one_sided += 1;
            } else {
                joined += 1;
            }
        }
    }
    println!("  doorway nodes joined both ways: {joined}, one way: {one_sided}, joined to nothing: {lonely}");
    shift_sizes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if !shift_sizes.is_empty() {
        let at = |f: f32| shift_sizes[((shift_sizes.len() - 1) as f32 * f) as usize];
        println!(
            "  sideways shove needed: median {:.2} m, 90th {:.2} m, worst {:.2} m",
            at(0.5),
            at(0.9),
            at(1.0)
        );
    }
}
