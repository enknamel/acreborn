//! How many cell structures have no physics polygons: those get their
//! drawn solid faces as collision instead. `AC_DATA_DIR=... cargo run
//! --release -p ac-scene --example env_survey`
use ac_scene::Assets;

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let ids: Vec<u32> = assets
        .portal
        .entries()
        .map(|e| e.id)
        .filter(|id| id >> 24 == 0x0D)
        .collect();
    let (mut envs, mut cells, mut no_phys, mut no_phys_solid, mut all_portals) = (0, 0, 0, 0, 0);
    let mut examples = Vec::new();
    for id in ids {
        let Ok(env) = assets.environment(id) else {
            continue;
        };
        envs += 1;
        for (k, cs) in &env.cells {
            cells += 1;
            if cs.physics_polygons.is_empty() {
                no_phys += 1;
                let solid = cs
                    .polygons
                    .iter()
                    .filter(|(p, _)| !cs.portals.contains(p))
                    .count();
                if solid > 0 {
                    no_phys_solid += 1;
                    if examples.len() < 8 {
                        examples.push(format!(
                            "{id:#x}/{k}: {solid} solid of {} polys",
                            cs.polygons.len()
                        ));
                    }
                } else {
                    all_portals += 1;
                }
            }
        }
    }
    println!("{envs} environments, {cells} cell structs, {no_phys} without physics polygons: {all_portals} all portals, {no_phys_solid} with drawn solid faces");
    for e in examples {
        println!("  {e}");
    }
}
