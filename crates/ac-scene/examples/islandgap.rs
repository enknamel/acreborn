//! Why is a building's interior cut off from the street?
//!
//! Builds a landblock's navigation graph, finds the groups of nodes
//! that can reach one another, and for the biggest island reports the
//! nearest node of the mainland and what the walk between the two
//! fails on. A doorway that no node stands in, a sill too tall to step
//! over and a floor the walk loses track of all look the same from the
//! outside -- a monster in plain sight that no path leads to -- and
//! quite different here.
//!
//! Usage: islandgap <block hex, e.g. A9B4> [how many islands]
use ac_scene::collision::Capsule;
use ac_scene::navarea::Area;
use ac_scene::Assets;
use glam::Vec3;

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    // The character's own capsule, not a slim probe: a doorway that
    // admits a 0.5 m radius and not a 0.679 m one is the whole story.
    let cap = Capsule {
        radius: 0.679,
        height: 1.835,
        step_up: 0.600,
        step_down: 1.500,
    };
    let arg = std::env::args().nth(1).unwrap_or_else(|| "A9B4".into());
    let want: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let block = u32::from_str_radix(arg.trim_start_matches("0x"), 16).unwrap() << 16;
    let mut area = Area::build(&assets, &[block], &cap, block).unwrap();
    area.build_everything();
    let origin = Vec3::new(
        ((block >> 24) & 0xFF) as f32 * 192.0,
        ((block >> 16) & 0xFF) as f32 * 192.0,
        0.0,
    );
    let n = area.nav.len();
    println!("{block:#010x}: {n} nodes, {} edges", area.nav.edge_count());

    // Groups, biggest first.
    let mut group = vec![usize::MAX; n];
    let mut groups: Vec<Vec<u32>> = Vec::new();
    for s in 0..n {
        if group[s] != usize::MAX {
            continue;
        }
        let id = groups.len();
        let mut members = Vec::new();
        let mut stack = vec![s as u32];
        group[s] = id;
        while let Some(v) = stack.pop() {
            members.push(v);
            for &w in area.nav.neighbours(v) {
                if group[w as usize] == usize::MAX {
                    group[w as usize] = id;
                    stack.push(w);
                }
            }
        }
        groups.push(members);
    }
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by_key(|i| std::cmp::Reverse(groups[*i].len()));
    println!("{} groups", groups.len());
    let mainland = order[0];

    for &g in order.iter().skip(1).take(want) {
        // Not the nearest node in three dimensions -- that is
        // whatever lies directly under an upper floor, through the
        // floorboards -- but the nearest pair that could plausibly be
        // joined: close by on the ground and within a step of each
        // other in height. That is what a doorway or the foot of a
        // staircase looks like.
        let mut pairs: Vec<(f32, u32, u32)> = Vec::new();
        for &a in &groups[g] {
            let pa = area.nav.nodes[a as usize].pos;
            for &b in &groups[mainland] {
                let pb = area.nav.nodes[b as usize].pos;
                let flat = ((pa.x - pb.x).powi(2) + (pa.y - pb.y).powi(2)).sqrt();
                let dz = (pa.z - pb.z).abs();
                if flat <= 3.0 && dz <= cap.step_up + cap.step_down {
                    pairs.push((flat + dz, a, b));
                }
            }
        }
        pairs.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        println!(
            "\nisland of {} nodes, floors z={:.1}..{:.1}",
            groups[g].len(),
            groups[g].iter().map(|&i| area.nav.nodes[i as usize].pos.z).fold(f32::MAX, f32::min),
            groups[g].iter().map(|&i| area.nav.nodes[i as usize].pos.z).fold(f32::MIN, f32::max),
        );
        if pairs.is_empty() {
            println!("  no node of the mainland comes within 3 m and a step of it");
            println!("  -- nothing to join: the way in has no node standing in it");
            continue;
        }
        println!("  {} plausible joins; the closest few:", pairs.len());
        for (score, a, b) in pairs.iter().take(4) {
            let pa = area.nav.nodes[*a as usize].pos;
            let pb = area.nav.nodes[*b as usize].pos;
            let (there, back) = area.with_ground(|ground, _| ground.walkable(pa, pb, &cap));
            println!(
                "   {:?} <-> {:?}  (gap {score:.2})  in->out {there} out->in {back}",
                pa - origin,
                pb - origin
            );
        }
    }
}
