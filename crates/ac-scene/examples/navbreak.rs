//! Why is a dungeon's nav graph disconnected? Builds the graph, finds
//! the connected components, then reports every near-miss pair of nodes
//! that sit in different components with the reason the edge failed.
//! `AC_DATA_DIR=... cargo run --release -p ac-scene --example navbreak BLOCK`
use std::collections::HashMap;

use ac_scene::collision::Capsule;
use ac_scene::nav::{line_clear, Ground};
use ac_scene::navarea::Area;
use ac_scene::Assets;
use glam::{Vec2, Vec3};

fn flat(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.y)
}

/// Repeat `nav::Ground::walkable` step by step, naming the first failure.
fn why(ground: &Ground, a: Vec3, b: Vec3, cap: &Capsule) -> Option<(String, Vec3)> {
    let d = b - a;
    let len = flat(d).length();
    let steps = (len / 0.5).ceil().max(1.0) as usize;
    let range = cap.step_up.max(cap.step_down);
    let probe = Capsule {
        step_up: range,
        step_down: range,
        ..*cap
    };
    let mut prev = a;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let at = Vec3::new(a.x + d.x * t, a.y + d.y * t, prev.z);
        let Some((z, _)) = ground.surface_at(at, &probe) else {
            return Some(("no floor".into(), at));
        };
        let here = Vec3::new(at.x, at.y, z);
        if ground
            .collision
            .wall_contact(here, cap.radius, cap.height, cap.step_up)
        {
            return Some(("wall".into(), here));
        }
        match ground.collision.ceiling_at(here, cap.radius) {
            Some(cz) if cz - here.z < cap.height => {
                return Some((format!("head room {:.2}", cz - here.z), here));
            }
            _ => {}
        }
        if !line_clear(ground.collision, prev, here) {
            return Some(("chest ray".into(), here));
        }
        let dz = here.z - prev.z;
        if (dz > cap.step_up || dz < -cap.step_down) && (-dz > cap.step_up || -dz < -cap.step_down)
        {
            return Some((format!("step {dz:.2}"), here));
        }
        prev = here;
    }
    if (prev.z - b.z).abs() > 0.3 {
        return Some((format!("landed at {:.2} not {:.2}", prev.z, b.z), prev));
    }
    None
}

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let cap = Capsule {
        radius: 0.5,
        height: 1.8,
        step_up: 0.8,
        step_down: 1.5,
    };
    let arg = std::env::args().nth(1).unwrap_or_else(|| "01F6".into());
    let block = u32::from_str_radix(arg.trim_start_matches("0x"), 16).unwrap() << 16;
    // Ground truth first: how many pieces the dungeon's own cell-portal
    // graph has. No nav graph can join more than these.
    let piece_of: HashMap<u32, usize> = {
        let scene = ac_scene::landblock::load(&assets, block).unwrap();
        let index: HashMap<u32, usize> = scene
            .cells
            .iter()
            .enumerate()
            .map(|(i, c)| (c.cell_id, i))
            .collect();
        let mut seen = vec![usize::MAX; scene.cells.len()];
        let mut cell_sizes: Vec<usize> = Vec::new();
        for start in 0..scene.cells.len() {
            if seen[start] != usize::MAX {
                continue;
            }
            let c = cell_sizes.len();
            seen[start] = c;
            let mut stack = vec![start];
            let mut size = 0;
            while let Some(v) = stack.pop() {
                size += 1;
                for other in &scene.cells[v].portal_cells {
                    if let Some(&w) = index.get(other) {
                        if seen[w] == usize::MAX {
                            seen[w] = c;
                            stack.push(w);
                        }
                    }
                }
            }
            cell_sizes.push(size);
        }
        let mut shown = cell_sizes.clone();
        shown.sort_by_key(|&s| std::cmp::Reverse(s));
        println!(
            "{} cells joined by portals into {} pieces, biggest {:?}",
            scene.cells.len(),
            cell_sizes.len(),
            &shown[..shown.len().min(8)]
        );
        scene
            .cells
            .iter()
            .enumerate()
            .map(|(i, c)| (c.cell_id, seen[i]))
            .collect()
    };
    let mut area = Area::build(&assets, &[block], &cap, block).unwrap();
    area.build_everything();
    let n = area.nav.len();
    // Undirected components.
    let mut undirected: Vec<Vec<u32>> = vec![Vec::new(); n];
    for v in 0..n {
        for &w in area.nav.neighbours(v as u32) {
            undirected[v].push(w);
            undirected[w as usize].push(v as u32);
        }
    }
    let mut comp = vec![usize::MAX; n];
    let mut sizes: Vec<usize> = Vec::new();
    for s in 0..n {
        if comp[s] != usize::MAX {
            continue;
        }
        let c = sizes.len();
        let mut stack = vec![s];
        comp[s] = c;
        let mut size = 0;
        while let Some(v) = stack.pop() {
            size += 1;
            for &w in &undirected[v] {
                if comp[w as usize] == usize::MAX {
                    comp[w as usize] = c;
                    stack.push(w as usize);
                }
            }
        }
        sizes.push(size);
    }
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&c| std::cmp::Reverse(sizes[c]));
    println!(
        "{n} nodes, {} undirected components, biggest {} ({:.0}%)",
        sizes.len(),
        sizes[order[0]],
        100.0 * sizes[order[0]] as f32 / n as f32
    );
    // The decisive split: for each piece of the dungeon that IS connected
    // by its own portals, how much of it does one nav component cover?
    // A piece served by one component is faithful; a piece broken into
    // several is our graph failing to walk a way the game says exists.
    {
        let mut by_piece: HashMap<usize, HashMap<usize, usize>> = HashMap::new();
        for (i, &c) in comp.iter().enumerate() {
            let Some(&piece) = piece_of.get(&area.nav.nodes[i].cell) else {
                continue;
            };
            *by_piece.entry(piece).or_default().entry(c).or_default() += 1;
        }
        let mut pieces: Vec<(usize, Vec<(usize, usize)>)> = by_piece
            .into_iter()
            .map(|(p, m)| {
                let mut v: Vec<(usize, usize)> = m.into_iter().collect();
                v.sort_by_key(|(_, k)| std::cmp::Reverse(*k));
                (p, v)
            })
            .collect();
        pieces.sort_by_key(|(_, v)| std::cmp::Reverse(v.iter().map(|(_, k)| *k).sum::<usize>()));
        println!("nodes per portal-connected piece, and how our graph splits it:");
        for (_, v) in pieces.iter().take(6) {
            let total: usize = v.iter().map(|(_, k)| *k).sum();
            let biggest = v[0].1;
            println!(
                "  {total:>5} nodes in {} nav components; biggest covers {biggest} ({:.0}%){}",
                v.len(),
                100.0 * biggest as f32 / total as f32,
                if v.len() > 1 {
                    format!(
                        ", next {:?}",
                        v[1..v.len().min(5)]
                            .iter()
                            .map(|(_, k)| *k)
                            .collect::<Vec<_>>()
                    )
                } else {
                    String::new()
                }
            );
        }
    }
    let main = order[0];
    let only_main = std::env::args().nth(2).as_deref() != Some("all");
    let ground = Ground {
        collision: &area.collision,
        terrain: None,
        sea: None,
        no_go: None,
        outdoors_only: false,
    };
    // Near-miss pairs across the boundary of the main component.
    let spacing = area.nav.spacing;
    let mut reasons: HashMap<String, (usize, Vec3)> = HashMap::new();
    let mut pairs = 0;
    for i in 0..n {
        for j in 0..n {
            if i == j || comp[i] == comp[j] {
                continue;
            }
            if only_main && comp[i] != main && comp[j] != main {
                continue;
            }
            let (a, b) = (area.nav.nodes[i].pos, area.nav.nodes[j].pos);
            if flat(b - a).length() > spacing * 1.5 || (b.z - a.z).abs() > 2.0 {
                continue;
            }
            pairs += 1;
            let (r, at) = match why(&ground, a, b, &cap) {
                Some(x) => x,
                None => ("nothing (edge should exist)".into(), a),
            };
            let e = reasons.entry(r).or_insert((0, at));
            e.0 += 1;
        }
    }
    println!("{pairs} near-miss pairs across the main component's edge:");
    let mut rs: Vec<_> = reasons.into_iter().collect();
    rs.sort_by_key(|(_, (c, _))| std::cmp::Reverse(*c));
    for (r, (c, at)) in rs.iter().take(20) {
        println!(
            "  {c:>5}  {r}  e.g. at ({:.2}, {:.2}, {:.2})",
            at.x, at.y, at.z
        );
    }
    // Nearest pair between each of the top components.
    let top: Vec<usize> = order.iter().take(8).copied().collect();
    println!("nearest node pair between the top components:");
    for (ai, &ca) in top.iter().enumerate() {
        for &cb in top.iter().skip(ai + 1) {
            let mut best: Option<(f32, usize, usize)> = None;
            for i in 0..n {
                if comp[i] != ca {
                    continue;
                }
                for (j, &cj) in comp.iter().enumerate() {
                    if cj != cb {
                        continue;
                    }
                    let d = (area.nav.nodes[j].pos - area.nav.nodes[i].pos).length();
                    if best.map(|(b, _, _)| d < b).unwrap_or(true) {
                        best = Some((d, i, j));
                    }
                }
            }
            if let Some((d, i, j)) = best {
                if d > 12.0 {
                    continue;
                }
                let (a, b) = (area.nav.nodes[i].pos, area.nav.nodes[j].pos);
                let r = why(&ground, a, b, &cap)
                    .map(|(r, at)| format!("{r} at ({:.1},{:.1},{:.1})", at.x, at.y, at.z))
                    .unwrap_or_else(|| "nothing".into());
                println!(
                    "  [{:>5}]-[{:>5}] {d:.1} m: ({:.1},{:.1},{:.1}) -> ({:.1},{:.1},{:.1}): {r}",
                    sizes[ca], sizes[cb], a.x, a.y, a.z, b.x, b.y, b.z
                );
            }
        }
    }
    println!("component sizes (top 12), and where each comes closest to the biggest:");
    for &c in order.iter().take(12) {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        let mut best: Option<(f32, usize, usize)> = None;
        for i in 0..n {
            if comp[i] != c {
                continue;
            }
            lo = lo.min(area.nav.nodes[i].pos);
            hi = hi.max(area.nav.nodes[i].pos);
            if c == main {
                continue;
            }
            for (j, &cj) in comp.iter().enumerate() {
                if cj != main {
                    continue;
                }
                let d = (area.nav.nodes[j].pos - area.nav.nodes[i].pos).length();
                if best.map(|(b, _, _)| d < b).unwrap_or(true) {
                    best = Some((d, i, j));
                }
            }
        }
        println!(
            "  {:>6} nodes, ({:.0},{:.0},{:.1})..({:.0},{:.0},{:.1})",
            sizes[c], lo.x, lo.y, lo.z, hi.x, hi.y, hi.z
        );
        if let Some((d, i, j)) = best {
            let (a, b) = (area.nav.nodes[i].pos, area.nav.nodes[j].pos);
            let r = why(&ground, a, b, &cap)
                .map(|(r, _)| r)
                .unwrap_or_else(|| "nothing".into());
            println!(
                "         nearest {d:.1} m: ({:.1},{:.1},{:.1}) cell {:#x} -> ({:.1},{:.1},{:.1}) cell {:#x}: {r}",
                a.x, a.y, a.z, area.nav.nodes[i].cell, b.x, b.y, b.z, area.nav.nodes[j].cell
            );
        }
    }
}
