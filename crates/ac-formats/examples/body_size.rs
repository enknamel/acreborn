//! What size a character actually is, according to the game's own data.
//!
//! The walking rules need a capsule -- how wide, how tall, how big a
//! step it can take -- and those four numbers decide which doorways a
//! character fits through. They are not ours to invent: every Setup
//! carries them, and a character's Setup is chosen at creation.
//!
//! Usage: AC_DATA_DIR=~/Downloads/ac_data cargo run -p ac-formats --example body_size

fn main() {
    let Ok(dir) = std::env::var("AC_DATA_DIR") else {
        eprintln!("set AC_DATA_DIR");
        return;
    };
    let portal = match ac_dat::DatArchive::open(format!("{dir}/client_portal.dat")) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("cannot open the portal dat: {e}");
            return;
        }
    };
    let Ok(bytes) = portal.read(0x0E00_0002) else {
        eprintln!("no character generation table");
        return;
    };
    let gen = match ac_formats::chargen::CharGen::parse(0x0E000002, &bytes) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("cannot read the character generation table: {e}");
            return;
        }
    };
    println!(
        "{:<22} {:>10} {:>8} {:>8} {:>9} {:>10}",
        "who", "setup", "height", "radius", "step up", "step down"
    );
    for h in &gen.heritage_groups {
        for (_, s) in &h.1.genders {
            let Ok(bytes) = portal.read(s.setup_id) else {
                continue;
            };
            let Ok(setup) = ac_formats::setup::Setup::parse(s.setup_id, &bytes) else {
                continue;
            };
            if s.setup_id == 0x0200_0001 || s.setup_id == 0x0200_004e {
                println!(
                    "  {} cylsphere(s), {} sphere(s):",
                    setup.cyl_spheres.len(),
                    setup.spheres.len()
                );
                for c in &setup.cyl_spheres {
                    println!(
                        "    cylsphere at {:?} radius {:.3} height {:.3}",
                        c.origin, c.radius, c.height
                    );
                }
                for sp in &setup.spheres {
                    println!("    sphere at {:?} radius {:.3}", sp.origin, sp.radius);
                }
            }
            println!(
                "{:<22} {:#010x} {:>8.3} {:>8.3} {:>9.3} {:>10.3}",
                format!("{} {}", h.1.name, s.name),
                s.setup_id,
                setup.height,
                setup.radius,
                setup.step_up_height,
                setup.step_down_height
            );
        }
    }
}
