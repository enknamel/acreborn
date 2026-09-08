//! Walk a character offline from one spot to another the way a journey
//! does (terrain-grid waypoints, legs clipped to the landblock, the
//! steering around buildings), with the real player physics, to watch
//! how it copes with walls without a server.
//!
//! `AC_DATA_DIR=... cargo run --release -p ac-client --example walksim \
//!     BLOCK x y z GOAL_BLOCK gx gy [seconds]`
//! Coordinates are local to their landblock (`g` for z stands the
//! character on the terrain). `RUST_LOG=ac_client=debug` shows the route
//! decisions; `WALKSIM_TRACE=<seconds>` prints the position and the
//! steering aim every few frames from then on.
use std::time::{Duration, Instant};

use ac_client::pathfinder::Pathfinder;
use ac_client::player::{Input, Player};
use ac_client::route::Steering;
use ac_client::travel::{leg_end, ARRIVE, STUCK_AFTER};
use ac_scene::worldgrid::WorldGrid;
use ac_scene::{worldroute, Assets};
use glam::{Quat, Vec2, Vec3};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let hex = |s: &str| u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap() << 16;
    let f = |i: usize| args[i].parse::<f32>().unwrap();
    let block = hex(&args[0]);
    let goal_block = hex(&args[4]);
    let goal = ac_world::landblock_origin(goal_block) + Vec3::new(f(5), f(6), 0.0);
    let goal2 = Vec2::new(goal.x, goal.y);
    let seconds: f32 = args.get(7).map(|s| s.parse().unwrap()).unwrap_or(120.0);

    let grid = WorldGrid::load_cached(&assets, &WorldGrid::cache_dir()).unwrap();
    let region = assets.region().unwrap();
    // "g" for z puts the character on the terrain.
    let z = match args[3].as_str() {
        "g" => {
            let o = ac_world::landblock_origin(block);
            grid.height_at(Vec2::new(o.x + f(1), o.y + f(2))) + 0.1
        }
        _ => f(3),
    };
    let start = Vec3::new(f(1), f(2), z);
    let cell = ac_world::outdoor_cell(block, start);
    let mut pl = Player::new(&assets, cell, start, Quat::IDENTITY);
    pl.set_motion_table(&assets, 0x0200_0001, 0x0900_0001);
    let me0 = pl.world_position();
    let route =
        worldroute::find(&grid, &region, Vec2::new(me0.x, me0.y), goal2).expect("no terrain route");
    println!("terrain route: {} waypoints", route.len());
    for w in &route {
        let b = WorldGrid::block_of(*w);
        let l = *w
            - Vec2::new(
                ac_world::landblock_origin(b).x,
                ac_world::landblock_origin(b).y,
            );
        println!("  {:#010x} ({:.1}, {:.1})", b, l.x, l.y);
    }

    let mut steering = Steering::new(Instant::now());
    let mut wide = Pathfinder::new(assets.data_dir.clone());
    let dt = 1.0 / 20.0;
    let t0 = Instant::now();
    let mut next = 0usize;
    let mut leg: Option<Vec2> = None;
    let mut best = f32::INFINITY;
    let mut last_progress: Option<Instant> = None;
    let mut measured_on: Option<Instant> = None;
    let mut last_report = -1i32;
    let mut arrived = false;
    let mut frames = 0u32;
    let mut last_aim: Option<Vec3> = None;
    let frame_time = Duration::from_secs_f32(dt);
    while t0.elapsed().as_secs_f32() < seconds {
        let frame_start = Instant::now();
        let now = frame_start;
        let me3 = pl.world_position();
        let me = Vec2::new(me3.x, me3.y);
        while next < route.len() && me.distance(route[next]) <= ARRIVE {
            next += 1;
            leg = None;
            best = f32::INFINITY;
            last_progress = None;
            measured_on = None;
        }
        if next >= route.len() {
            arrived = true;
            break;
        }
        let wp = route[next];
        let (d, plan) = match steering.remaining(me3) {
            Some((along, end, planned)) => {
                (along + Vec2::new(end.x, end.y).distance(wp), Some(planned))
            }
            None => (me.distance(wp), None),
        };
        if plan != measured_on {
            measured_on = plan;
            best = d;
            last_progress.get_or_insert(now);
        }
        match last_progress {
            Some(t) if d >= best - 1.0 => {
                if now.duration_since(t) >= STUCK_AFTER {
                    println!(
                        "[{:6.1}s] no progress toward waypoint {next}/{} for {:?}, skipping it",
                        t0.elapsed().as_secs_f32(),
                        route.len(),
                        STUCK_AFTER
                    );
                    next += 1;
                    leg = None;
                    best = f32::INFINITY;
                    last_progress = None;
                    measured_on = None;
                    continue;
                }
            }
            _ => {
                best = d;
                last_progress = Some(now);
            }
        }
        let l = match leg {
            Some(l) if me.distance(l) > ARRIVE && me.distance(l) <= 120.0 => l,
            _ => {
                let l = leg_end(me, wp);
                leg = Some(l);
                l
            }
        };
        let g = Vec3::new(l.x, l.y, grid.height_at(l));
        let gblock = WorldGrid::block_of(l);
        let mut input = Input::default();
        let flat = Vec2::new(g.x - me3.x, g.y - me3.y);
        if flat.length() > 1.0 {
            let aim = steering.steer(&mut pl, &assets, &mut wide, g, gblock, now);
            last_aim = Some(aim);
            let d = aim - pl.world_position();
            let flat = Vec2::new(d.x, d.y);
            if flat.length() > 1e-3 {
                pl.heading = (-flat.x).atan2(flat.y);
            }
            input.forward = 1.0;
            input.run = true;
        }
        pl.update(&assets, &input, dt);
        let trace_from: f32 = std::env::var("WALKSIM_TRACE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(f32::INFINITY);
        if t0.elapsed().as_secs_f32() >= trace_from && frames % 5 == 0 {
            let p = pl.world_position() - ac_world::landblock_origin(pl.landblock());
            eprintln!(
                "frame {frames}: at ({:.2}, {:.2}, {:.2}) heading {:.2} aim {:?}",
                p.x,
                p.y,
                p.z,
                pl.heading,
                last_aim.map(|a| a - ac_world::landblock_origin(pl.landblock()))
            );
        }
        frames += 1;
        let sec = t0.elapsed().as_secs_f32() as i32;
        if sec != last_report && sec % 2 == 0 {
            last_report = sec;
            let p = pl.world_position();
            let b = pl.landblock();
            let lp = p - ac_world::landblock_origin(b);
            println!(
                "[{sec:4}s] {b:#010x} ({:.1}, {:.1}, {:.1})  to goal {:.0} m  waypoint {next}/{}  leg {:?}",
                lp.x,
                lp.y,
                lp.z,
                me.distance(goal2),
                route.len(),
                leg.map(|l| {
                    let o = ac_world::landblock_origin(WorldGrid::block_of(l));
                    ((l.x - o.x * 1.0) as i32, (l.y - o.y) as i32)
                })
            );
        }
        let spent = frame_start.elapsed();
        if spent < frame_time {
            std::thread::sleep(frame_time - spent);
        }
    }
    let p = pl.world_position();
    println!(
        "{} after {:.1} s ({frames} frames): {:.0} m from the goal",
        if arrived { "arrived" } else { "gave up" },
        t0.elapsed().as_secs_f32(),
        Vec2::new(p.x, p.y).distance(goal2)
    );
}
