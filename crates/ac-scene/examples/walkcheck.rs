//! What is stopping a character from stepping where it wants to?
//!
//! Takes a block and two landblock-local points, tries the step the
//! player physics would try, and when it is refused reports the
//! geometry standing in the way -- and, in particular, whether that
//! geometry came from an object's own physics mesh or from the mesh it
//! is merely drawn with.
//!
//! Usage: walkcheck <block hex> ax ay az bx by bz
use ac_scene::collision::Capsule;
use ac_scene::navarea::Area;
use ac_scene::Assets;
use glam::Vec3;

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let a: Vec<String> = std::env::args().skip(1).collect();
    let block = u32::from_str_radix(a[0].trim_start_matches("0x"), 16).unwrap() << 16;
    let n: Vec<f32> = a[1..].iter().map(|s| s.parse().unwrap()).collect();
    let origin = Vec3::new(
        ((block >> 24) & 0xFF) as f32 * 192.0,
        ((block >> 16) & 0xFF) as f32 * 192.0,
        0.0,
    );
    let from = origin + Vec3::new(n[0], n[1], n[2]);
    let to = origin + Vec3::new(n[3], n[4], n[5]);
    let cap = Capsule::default();
    println!("capsule r={} h={}", cap.radius, cap.height);
    let mut area = Area::build(&assets, &[block], &cap, block).unwrap();
    area.with_ground(|ground, _| {
        let w = ground.collision.walk(from, to, &cap);
        println!(
            "walk {:?} -> {:?}\n  gives {:?} blocked {}",
            from - origin,
            to - origin,
            w.pos - origin,
            w.pos.distance(from) < 1e-3
        );
        println!("  fits at start: {}", ground.fits_here(from, &cap));
        println!("  fits at end  : {}", ground.fits_here(to, &cap));
        // What the overhang test thinks is above the destination.
        let floor = ground
            .collision
            .floor_at(Vec3::new(to.x, to.y, from.z), cap.step_up, cap.step_down);
        let feet = Vec3::new(to.x, to.y, floor.map(|(z, _)| z).unwrap_or(from.z));
        println!("  feet at destination {:?} floor {:?}", feet - origin, floor.map(|(z, c)| (z, format!("{c:#x}"))));
        println!(
            "  overhang_escape: {:?}",
            ground.collision.overhang_escape(feet, cap.radius, cap.height)
        );
        println!("  ceiling_at: {:?}", ground.collision.ceiling_at(feet, cap.radius));
        for t in &ground.collision.tris {
            let facing_down = t.normal.z < -0.5 || (t.two_sided && t.normal.z > 0.5);
            if !facing_down {
                continue;
            }
            let lo = t.a.z.min(t.b.z).min(t.c.z);
            let hi = t.a.z.max(t.b.z).max(t.c.z);
            if hi < feet.z + 0.2 || lo > feet.z + cap.height {
                continue;
            }
            let c = (t.a + t.b + t.c) / 3.0;
            let d = ((c.x - feet.x).powi(2) + (c.y - feet.y).powi(2)).sqrt();
            if d < 1.5 {
                println!(
                    "    overhead cand {d:.2} m z={lo:.2}..{hi:.2} cell {:#x} two_sided {} n.z {:.2}",
                    t.cell, t.two_sided, t.normal.z
                );
            }
        }
        // Everything the capsule overlaps on the way, by the cell it
        // belongs to. Cell 0 is outdoor geometry: the terrain, the
        // buildings' outsides and every placed object.
        let mid = (from + to) * 0.5;
        let mut near: Vec<(f32, u32)> = Vec::new();
        for t in &ground.collision.tris {
            let c = (t.a + t.b + t.c) / 3.0;
            let d = ((c.x - mid.x).powi(2) + (c.y - mid.y).powi(2)).sqrt();
            if d < 2.0 && c.z > mid.z - 2.0 && c.z < mid.z + cap.height {
                near.push((d, t.cell));
            }
        }
        near.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        println!("  {} triangles within 2 m; nearest ten:", near.len());
        for (d, cell) in near.iter().take(10) {
            println!("    {d:.2} m  cell {cell:#x}");
        }
    });
}
