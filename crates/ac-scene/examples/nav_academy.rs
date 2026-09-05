//! Build the navigation graph of a landblock and walk a path across it,
//! printing timings, node counts and the waypoints.
//!
//! `AC_DATA_DIR=... cargo run --release -p ac-scene --example nav_academy [BLOCK [FROM_CELL x y z TO_CELL x y z]]`
//!
//! Without arguments: the Training Academy (8602), from the starting
//! room to the far end of the dungeon.
use std::time::Instant;

use ac_scene::{
    collision::{Capsule, CollisionWorld},
    landblock, lbid,
    nav::{Ground, NavGraph},
    Assets,
};
use glam::Vec3;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let hex = |s: &str| u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap();
    let block = args.first().map(|a| hex(a) << 16).unwrap_or(0x8602_0000);
    let (from_cell, from, to_cell, to) = if args.len() >= 9 {
        let f = |i: usize| args[i].parse::<f32>().unwrap();
        (
            hex(&args[1]),
            Vec3::new(f(2), f(3), f(4)),
            hex(&args[5]),
            Vec3::new(f(6), f(7), f(8)),
        )
    } else {
        (
            0x8602_01AD,
            Vec3::new(12.3, -28.5, 0.0),
            0x8602_026D,
            Vec3::ZERO,
        )
    };

    let assets = Assets::open(dir).unwrap();
    let t = Instant::now();
    let scene = landblock::load(&assets, block).unwrap();
    println!(
        "{block:#010x}: assembled in {:.1} ms, {} cells, dungeon {}",
        ms(t),
        scene.cells.len(),
        scene.is_dungeon
    );
    let t = Instant::now();
    let collision = CollisionWorld::from_scene(&assets, &scene).unwrap();
    println!(
        "collision: {} tris in {:.1} ms",
        collision.tris.len(),
        ms(t)
    );
    if let Some((lo, hi)) = collision.bounds() {
        println!("bounds: {lo} .. {hi}");
    }
    let cap = Capsule::default();
    let origin = lbid::world_origin(block);
    let terrain = |x: f32, y: f32| {
        scene
            .terrain
            .height_at(Vec3::new(x - origin.x, y - origin.y, 0.0))
    };
    let ground = Ground {
        collision: &collision,
        terrain: (!scene.is_dungeon).then_some(&terrain),
    };
    // The whole block at once, for the numbers...
    let mut whole = NavGraph::for_scene(&scene, &collision, &cap);
    whole.build_all(&ground);
    println!(
        "whole block: {} nodes, {} edges in {} chunks, spacing {} m, built in {:.1} ms",
        whole.len(),
        whole.edge_count(),
        whole.chunk_count(),
        whole.spacing,
        whole.build_time.as_secs_f64() * 1e3
    );
    let mut per_cell: std::collections::BTreeMap<u32, usize> = Default::default();
    for n in &whole.nodes {
        *per_cell.entry(n.cell).or_default() += 1;
    }
    println!("cells with nodes: {}", per_cell.len());
    // ...and a fresh one that builds only what the path touches.
    let mut graph = NavGraph::for_scene(&scene, &collision, &cap);
    let start = origin + from;
    // A goal cell alone: aim at the centre of one of its nodes.
    let goal = if to == Vec3::ZERO && to_cell != 0 {
        match whole.nodes.iter().find(|n| n.cell == to_cell) {
            Some(n) => n.pos,
            None => {
                println!("no node in cell {to_cell:#010x}");
                return;
            }
        }
    } else {
        origin + to
    };
    println!(
        "from {from_cell:#010x} {start} to {to_cell:#010x} {goal}; straight line clear: {}",
        ac_scene::nav::line_clear(&collision, start, goal)
    );
    let t = Instant::now();
    let path = graph.find_path(&ground, start, goal);
    let took = ms(t);
    println!(
        "lazy graph: {} nodes in {} chunks, built in {:.1} ms",
        graph.len(),
        graph.chunk_count(),
        graph.build_time.as_secs_f64() * 1e3
    );
    match path {
        Some(path) => {
            let mut len = 0.0;
            let mut prev = start;
            for p in &path {
                len += prev.distance(*p);
                prev = *p;
            }
            println!(
                "path: {} waypoints, {len:.1} m, found in {took:.2} ms",
                path.len()
            );
            for (i, p) in path.iter().enumerate() {
                let local = *p - origin;
                let cell = ground.surface_at(*p, &cap).map(|(_, c)| c).unwrap_or(0);
                println!(
                    "  {i:3}: {cell:#010x} ({:.1}, {:.1}, {:.1})",
                    local.x, local.y, local.z
                );
            }
        }
        None => println!("no path ({took:.2} ms)"),
    }
}
