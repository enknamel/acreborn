//! Does every placed object that should stop a character still stop one?
//!
//! For each model placed in a landblock: how big it is, what the client
//! would collide with it by (physics polygons, the Setup's cylinders or
//! spheres, or nothing), whether our collision world actually holds
//! triangles inside its bounds, and whether a capsule standing at its
//! middle reports wall contact.
//!
//! `AC_DATA_DIR=... cargo run --release -p ac-scene --example solidcheck BLOCK [minsize]`
use std::collections::HashMap;

use ac_formats::landblock::{CellLandblock, EnvCell, LandblockInfo};
use ac_scene::collision::{Capsule, CollisionWorld};
use ac_scene::model::{frame_to_mat, place};
use ac_scene::{landblock, lbid, Assets};
use glam::{Mat4, Vec3};

struct Placed {
    id: u32,
    source: &'static str,
    world: Mat4,
}

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Only report objects at least this big; the world is full of
    // 10 cm candle flames and they are not the question.
    let minsize: f32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.8);
    if args[0] == "all" {
        world_scan(&assets, minsize);
        return;
    }
    if args[0] == "model" {
        let m = u32::from_str_radix(args[1].trim_start_matches("0x"), 16).unwrap();
        let parts: Vec<u32> = if m >> 24 == 0x02 {
            assets.setup(m).unwrap().parts.clone()
        } else {
            vec![m]
        };
        for g in parts {
            let o = assets.gfxobj(g).unwrap();
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for (_, v) in &o.vertices {
                lo = lo.min(v.origin);
                hi = hi.max(v.origin);
            }
            println!(
                "  {g:#010x} flags {:#x} phys {} draw {} verts {} bbox ({:.1},{:.1},{:.1})..({:.1},{:.1},{:.1})",
                o.flags,
                o.physics_polygons.len(),
                o.polygons.len(),
                o.vertices.len(),
                lo.x, lo.y, lo.z, hi.x, hi.y, hi.z
            );
        }
        return;
    }
    if args[0] == "find" {
        // Which landblocks place a given model.
        let want = u32::from_str_radix(args[1].trim_start_matches("0x"), 16).unwrap();
        let ids: Vec<u32> = assets
            .cell
            .entries()
            .map(|e| e.id)
            .filter(|id| id & 0xFFFF == 0xFFFE)
            .collect();
        let mut found = 0;
        for info_id in ids {
            let Ok(bytes) = assets.cell.read(info_id) else {
                continue;
            };
            let Ok(info) = LandblockInfo::parse(info_id, &bytes) else {
                continue;
            };
            let mut in_cell = false;
            for i in 0..info.num_cells {
                let cid = (info_id & 0xFFFF_0000) | (0x100 + i);
                if let Ok(b) = assets.cell.read(cid) {
                    if let Ok(c) = EnvCell::parse(cid, &b) {
                        if c.static_objects.iter().any(|s| s.id == want) {
                            in_cell = true;
                            break;
                        }
                    }
                }
            }
            if in_cell
                || info.objects.iter().any(|s| s.id == want)
                || info.buildings.iter().any(|b| b.model_id == want)
            {
                println!("  block {:#06x} places {want:#010x}", info_id >> 16);
                found += 1;
                if found >= 6 {
                    break;
                }
            }
        }
        return;
    }
    let block = u32::from_str_radix(args[0].trim_start_matches("0x"), 16).unwrap() << 16;
    let scene = landblock::load(&assets, block).unwrap();
    let world_collision = CollisionWorld::from_scene(&assets, &scene).unwrap();
    let origin = Mat4::from_translation(lbid::world_origin(block));

    // Re-walk the placements the scene was built from.
    let lb_id = block | 0xFFFF;
    let lb = CellLandblock::parse(lb_id, &assets.cell.read(lb_id).unwrap()).unwrap();
    let info_id = block | 0xFFFE;
    let info = assets
        .cell
        .read(info_id)
        .ok()
        .and_then(|b| LandblockInfo::parse(info_id, &b).ok());
    let mut placed: Vec<Placed> = Vec::new();
    if let Some(info) = &info {
        for s in &info.objects {
            placed.push(Placed {
                id: s.id,
                source: "block static",
                world: origin * frame_to_mat(&s.frame),
            });
        }
        for b in &info.buildings {
            placed.push(Placed {
                id: b.model_id,
                source: "BUILDING",
                world: origin * frame_to_mat(&b.frame),
            });
        }
        for i in 0..info.num_cells {
            let cid = block | (0x100 + i);
            if let Ok(bytes) = assets.cell.read(cid) {
                if let Ok(c) = EnvCell::parse(cid, &bytes) {
                    for s in &c.static_objects {
                        placed.push(Placed {
                            id: s.id,
                            source: "cell static",
                            world: origin * frame_to_mat(&s.frame),
                        });
                    }
                }
            }
        }
    }
    for inst in ac_scene::scenery::generate(&assets, &lb, info.as_ref()).unwrap() {
        placed.push(Placed {
            id: inst.obj_id,
            source: "scenery",
            world: origin * inst.local,
        });
    }

    let cap = Capsule::default();
    let mut rows: Vec<(f32, String)> = Vec::new();
    let mut summary: HashMap<(&str, &str), usize> = HashMap::new();
    let mut hollow_big = 0usize;
    for p in &placed {
        // What the client would collide with it by.
        let (parts, cyl, sph) = match p.id >> 24 {
            0x02 => match assets.setup(p.id) {
                Ok(s) => (s.parts.clone(), s.cyl_spheres.len(), s.spheres.len()),
                Err(_) => continue,
            },
            0x01 => (vec![p.id], 0, 0),
            _ => continue,
        };
        let has_phys = parts.iter().any(|&g| {
            assets
                .gfxobj(g)
                .map(|o| !o.physics_polygons.is_empty())
                .unwrap_or(false)
        });
        let expect = if has_phys {
            "physics polygons"
        } else if cyl > 0 {
            "cylspheres"
        } else if sph > 0 {
            "spheres"
        } else {
            "nothing"
        };

        // World bounds of the placed model.
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let Ok(ps) = place(&assets, p.id, p.world) else {
            continue;
        };
        for part in &ps {
            if let Ok(g) = assets.gfxobj(part.gfxobj_id) {
                for (_, v) in &g.vertices {
                    let q = part.transform.transform_point3(v.origin);
                    lo = lo.min(q);
                    hi = hi.max(q);
                }
            }
        }
        if lo.x > hi.x {
            continue;
        }
        let width = (hi.x - lo.x).max(hi.y - lo.y);
        let height = hi.z - lo.z;
        if width.max(height) < minsize {
            continue;
        }

        // Does our collision world actually hold anything inside it?
        let mid = (lo + hi) * 0.5;
        let reach = (hi - lo).truncate().length() * 0.5 + 1.0;
        let inside = world_collision
            .nearby(mid, reach)
            .filter(|t| {
                let c = (t.a + t.b + t.c) / 3.0;
                c.cmpge(lo - Vec3::splat(0.05)).all() && c.cmple(hi + Vec3::splat(0.05)).all()
            })
            .count();

        // Would a capsule standing at its middle be pushed out of it?
        let feet = Vec3::new(mid.x, mid.y, lo.z);
        let contact = world_collision.wall_contact(feet, cap.radius, cap.height, 0.0);
        let pushed = (world_collision.resolve(feet, cap.radius, cap.height) - feet).length();

        let solid = if inside > 0 { "solid" } else { "HOLLOW" };
        if inside == 0 && width.max(height) >= 1.0 {
            hollow_big += 1;
        }
        *summary.entry((expect, solid)).or_default() += 1;
        rows.push((
            width * height,
            format!(
                "  {solid:>6} {:>16} {:#010x} {width:5.2} x {height:5.2} m  expect {expect:<16} tris {inside:>5}  contact {contact:<5} push {pushed:.2}",
                p.source, p.id
            ),
        ));
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!(
        "{block:#010x}: {} placements at least {minsize} m",
        rows.len()
    );
    for (_, r) in rows
        .iter()
        .take(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30))
    {
        println!("{r}");
    }
    let mut v: Vec<_> = summary.into_iter().collect();
    v.sort();
    println!("summary (objects >= {minsize} m):");
    for ((expect, solid), n) in v {
        println!("  {n:>5}  expect {expect:<16} -> {solid}");
    }
    println!("{hollow_big} objects of 1 m or more have no collision at all");
}

/// Every landblock in the world: which models at least `minsize` big
/// would have no collision at all under the client's own rule (no part
/// with physics polygons, no Setup cylinder or sphere). These are the
/// models the drawing-polygon fallback is currently keeping solid.
fn world_scan(assets: &Assets, minsize: f32) {
    let mut kind: HashMap<u32, Option<(f32, f32)>> = HashMap::new();
    let mut counts: HashMap<u32, usize> = HashMap::new();
    let mut blocks = 0usize;
    let ids: Vec<u32> = assets
        .cell
        .entries()
        .map(|e| e.id)
        .filter(|id| id & 0xFFFF == 0xFFFE)
        .collect();
    for info_id in ids {
        let Ok(bytes) = assets.cell.read(info_id) else {
            continue;
        };
        let Ok(info) = LandblockInfo::parse(info_id, &bytes) else {
            continue;
        };
        blocks += 1;
        let mut models: Vec<u32> = info.objects.iter().map(|s| s.id).collect();
        models.extend(info.buildings.iter().map(|b| b.model_id));
        for i in 0..info.num_cells {
            let cid = (info_id & 0xFFFF_0000) | (0x100 + i);
            if let Ok(b) = assets.cell.read(cid) {
                if let Ok(c) = EnvCell::parse(cid, &b) {
                    models.extend(c.static_objects.iter().map(|s| s.id));
                }
            }
        }
        for m in models {
            *counts.entry(m).or_default() += 1;
            if kind.contains_key(&m) {
                continue;
            }
            let (parts, cyl, sph) = match m >> 24 {
                0x02 => match assets.setup(m) {
                    Ok(s) => (s.parts.clone(), s.cyl_spheres.len(), s.spheres.len()),
                    Err(_) => {
                        kind.insert(m, None);
                        continue;
                    }
                },
                0x01 => (vec![m], 0, 0),
                _ => {
                    kind.insert(m, None);
                    continue;
                }
            };
            let solid = parts.iter().any(|&g| {
                assets
                    .gfxobj(g)
                    .map(|o| !o.physics_polygons.is_empty())
                    .unwrap_or(false)
            });
            if solid || cyl > 0 || sph > 0 {
                kind.insert(m, None);
                continue;
            }
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for &g in &parts {
                if let Ok(o) = assets.gfxobj(g) {
                    for (_, v) in &o.vertices {
                        lo = lo.min(v.origin);
                        hi = hi.max(v.origin);
                    }
                }
            }
            kind.insert(
                m,
                (lo.x <= hi.x).then(|| ((hi.x - lo.x).max(hi.y - lo.y), hi.z - lo.z)),
            );
        }
    }
    let mut rows: Vec<(u32, usize, f32, f32)> = kind
        .iter()
        .filter_map(|(m, k)| k.map(|(w, h)| (*m, counts[m], w, h)))
        .filter(|(_, _, w, h)| w.max(*h) >= minsize)
        .collect();
    rows.sort_by(|a, b| (b.2 * b.3).total_cmp(&(a.2 * a.3)));
    let total: usize = rows.iter().map(|r| r.1).sum();
    println!(
        "{blocks} landblocks scanned; {} distinct models of {minsize} m or more have no collision under the client's rule ({total} placements):",
        rows.len()
    );
    for (m, n, w, h) in rows.iter().take(30) {
        println!("  {m:#010x} x{n:<6} {w:6.2} x {h:6.2} m");
    }
}
