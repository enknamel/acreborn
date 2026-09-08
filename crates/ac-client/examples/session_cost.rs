//! What the Nth session in a process costs, in memory, with no server.
//!
//! `AC_DATA_DIR=... cargo run --release -p ac-client --example session_cost [N] [BLOCK]`
//!
//! Opens the archives once, then N sessions the way `acbot` does
//! (`Client::connect` to a port nobody listens on, so no packet is
//! answered), stands each character in `BLOCK` (Holtburg by default),
//! and makes each one do the things that allocate: walk into a wall
//! (the block's collision), plan a path around it (the block's nav
//! graph), plan a journey (the world grid), and ask the wide planner
//! for a route (the pathfinder thread and its neighbourhood). After
//! every phase it prints the process RSS and the share of the growth
//! that each session accounts for.
//!
//! `ps` reads the RSS, so the numbers are the whole process's: the
//! archive pages touched so far are in there too, file-backed and
//! shared with every other process on the machine. On macOS `vmmap`
//! adds the physical footprint, which leaves those out and is what the
//! machine actually runs short of.
use std::rc::Rc;
use std::time::{Duration, Instant};

use ac_client::player::Player;
use ac_client::{Client, Config};
use ac_scene::Assets;
use glam::{Quat, Vec2, Vec3};

fn rss_kb() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

/// The physical footprint in KB as `vmmap --summary` reports it
/// (macOS), or 0 where there is no such tool.
fn footprint_kb() -> u64 {
    let Ok(out) = std::process::Command::new("vmmap")
        .args(["--summary", &std::process::id().to_string()])
        .output()
    else {
        return 0;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(line) = text.lines().find(|l| l.starts_with("Physical footprint:")) else {
        return 0;
    };
    let v = line.split(':').nth(1).unwrap_or("").trim();
    let (num, unit) = v.split_at(v.len().saturating_sub(1));
    let num: f64 = num.parse().unwrap_or(0.0);
    (match unit {
        "K" => num,
        "M" => num * 1024.0,
        "G" => num * 1024.0 * 1024.0,
        _ => v.parse::<f64>().unwrap_or(0.0) / 1024.0,
    }) as u64
}

struct Meter {
    last: u64,
    last_fp: u64,
    n: u64,
}

impl Meter {
    fn phase(&mut self, what: &str) {
        let now = rss_kb();
        let fp = footprint_kb();
        let delta = now as i64 - self.last as i64;
        let dfp = fp as i64 - self.last_fp as i64;
        print!(
            "{what:<44} rss {:>7.1} MB  +{:>6.1} MB  ({:>5.1} MB per session)",
            now as f64 / 1024.0,
            delta as f64 / 1024.0,
            delta as f64 / 1024.0 / self.n as f64
        );
        if fp > 0 {
            print!(
                "  footprint {:>7.1} MB  +{:>6.1} MB",
                fp as f64 / 1024.0,
                dfp as f64 / 1024.0
            );
        }
        println!();
        self.last = now;
        self.last_fp = fp;
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n: usize = args.first().map(|s| s.parse().unwrap()).unwrap_or(4);
    let block = args
        .get(1)
        .map(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap() << 16)
        .unwrap_or(0xA9B4_0000);
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let mut m = Meter {
        last: rss_kb(),
        last_fp: footprint_kb(),
        n: n as u64,
    };
    println!("{n} sessions in landblock {block:#010x}");
    println!("{:<44} rss {:>7.1} MB", "start", m.last as f64 / 1024.0);

    let assets = Rc::new(Assets::open(std::path::Path::new(&dir)).unwrap());
    let region = assets.region().unwrap();
    m.n = 1;
    m.phase("archives opened (shared)");
    m.n = n as u64;
    let after_archives = (m.last, m.last_fp);

    let mut clients: Vec<Client> = (0..n)
        .map(|i| {
            Client::connect(
                Config {
                    host: "127.0.0.1:1".into(),
                    account: format!("cost{i}"),
                    password: "x".into(),
                    character: None,
                    auto_enter: true,
                },
                assets.clone(),
            )
            .unwrap()
        })
        .collect();
    m.phase("Client::connect x N");

    // Stand on the terrain in the middle of the block.
    let origin = ac_scene::lbid::world_origin(block);
    let lb_id = block | 0xFFFF;
    let lb = ac_formats::landblock::CellLandblock::parse(lb_id, &assets.cell.read(lb_id).unwrap())
        .unwrap();
    let sampler = ac_scene::scenery::TerrainSampler::new(&lb, &region.land_defs.land_height_table);
    let local = |x: f32, y: f32| {
        let z = sampler.height_at(Vec3::new(x, y, 0.0)).unwrap_or(0.0);
        Vec3::new(x, y, z)
    };
    let start = local(96.0, 96.0);
    let cell = ac_world::outdoor_cell(block, start);
    for c in clients.iter_mut() {
        let mut pl = Player::new(&assets, cell, start, Quat::IDENTITY);
        pl.set_motion_table(&assets, 0x0200_0001, 0x0900_0001);
        c.player = Some(pl);
    }
    m.phase("Player::new x N");

    let far = origin + local(20.0, 170.0);
    for c in clients.iter_mut() {
        let pl = c.player.as_mut().unwrap();
        let me = pl.world_position();
        pl.line_blocked(&assets, block, me, far);
    }
    m.phase("block collision (line_blocked) x N");

    let mut found = 0;
    for c in clients.iter_mut() {
        let pl = c.player.as_mut().unwrap();
        let me = pl.world_position();
        found += pl.find_path(&assets, block, me, far).is_some() as usize;
    }
    m.phase(&format!("block nav graph (find_path, {found} found) x N"));

    let goal = Vec2::new(far.x, far.y) + Vec2::new(0.0, 900.0);
    let mut planned = 0;
    for c in clients.iter_mut() {
        planned += c.travel_to(goal) as usize;
    }
    m.phase(&format!("world grid (travel_to, {planned} planned) x N"));

    let cap = clients[0].player.as_ref().unwrap().capsule();
    let now = Instant::now();
    for c in clients.iter_mut() {
        let me = c.player.as_ref().unwrap().world_position();
        c.pathfinder.ask(me, far, cap, block, true, now);
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut answered = vec![false; n];
    while answered.iter().any(|a| !a) && Instant::now() < deadline {
        for (i, c) in clients.iter_mut().enumerate() {
            if answered[i] {
                continue;
            }
            let me = c.player.as_ref().unwrap().world_position();
            if c.pathfinder.take(me, far).is_some() || !c.pathfinder.busy() {
                answered[i] = true;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let done = answered.iter().filter(|a| **a).count();
    m.phase(&format!("wide pathfinder ({done}/{n} answered) x N"));

    // A tick each, as the bot loop would do with nothing to do.
    let t0 = Instant::now();
    for _ in 0..100 {
        for c in clients.iter_mut() {
            c.tick(None, 0.05, Instant::now());
        }
    }
    let per_tick = t0.elapsed().as_secs_f64() / 100.0 / n as f64;
    m.phase("100 idle ticks x N");
    println!("idle tick: {:.1} us per session", per_tick * 1e6);
    println!(
        "total: rss {:.1} MB, {:.1} MB per session on top of the archives",
        m.last as f64 / 1024.0,
        (m.last as f64 - after_archives.0 as f64) / 1024.0 / n as f64
    );
    if m.last_fp > 0 {
        println!(
            "       footprint {:.1} MB, {:.1} MB per session on top of the archives",
            m.last_fp as f64 / 1024.0,
            (m.last_fp as f64 - after_archives.1 as f64) / 1024.0 / n as f64
        );
    }
}
