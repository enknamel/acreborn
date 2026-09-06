//! Print the journey from one place to another.
//! `cargo run -p ac-world --example plan_trip Holtburg Arwic`
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let from = ac_world::towns::find(&a[0]).expect("from");
    let to = ac_world::towns::find(&a[1]).expect("to");
    let t0 = std::time::Instant::now();
    let trip = ac_world::trip::plan(from.world_xy(), 0, to.world_xy());
    println!("{} -> {} planned in {:?}", from.name, to.name, t0.elapsed());
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
