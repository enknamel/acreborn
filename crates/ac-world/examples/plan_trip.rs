//! Print the journey from one place to another.
//! `cargo run -p ac-world --example plan_trip Holtburg Arwic`
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    // plan_trip FROM TO [level]   or   plan_trip ns,ew TO [level]
    let from_xy = match a[0].split_once(',') {
        Some((ns, ew)) => ac_world::towns::world_xy(
            ns.trim().parse().expect("ns"),
            ew.trim().parse().expect("ew"),
        ),
        None => ac_world::towns::find(&a[0]).expect("from").world_xy(),
    };
    let to = ac_world::towns::find(&a[1]).expect("to");
    let level: u32 = a.get(2).and_then(|l| l.parse().ok()).unwrap_or(0);
    let t0 = std::time::Instant::now();
    let trip = ac_world::trip::plan_for(from_xy, 0, to.world_xy(), level, &[], &[]);
    println!(
        "{:?} -> {} at level {level}, planned in {:?}",
        a[0],
        to.name,
        t0.elapsed()
    );
    match trip {
        Some(t) => {
            println!("{} ({:.0} s)", t.summary(), t.seconds);
            for s in &t.steps {
                match s {
                    ac_world::trip::Step::Walk(p) => {
                        println!("  walk to {}", ac_world::towns::map_of(*p).0)
                    }
                    ac_world::trip::Step::Portal {
                        name, mouth, exit, ..
                    } => println!(
                        "  portal {name:?} at {:?} -> {:?}",
                        ac_world::towns::map_of(*mouth),
                        ac_world::towns::map_of(*exit)
                    ),
                }
            }
        }
        None => println!("no trip"),
    }
}
