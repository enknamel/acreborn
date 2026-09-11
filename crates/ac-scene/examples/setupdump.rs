//! Dump a Setup's placement frames and the resulting part transforms,
//! to compare our placement against the client's.
//! `AC_DATA_DIR=... cargo run --release -p ac-scene --example setupdump 0x020001E7`
use ac_scene::model::frame_to_mat;
use ac_scene::Assets;
use glam::Vec3;

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let id = u32::from_str_radix(
        std::env::args().nth(1).unwrap().trim_start_matches("0x"),
        16,
    )
    .unwrap();
    if id == 0 {
        // Survey mode: how many setups would we place wrongly?
        let ids: Vec<u32> = assets
            .portal
            .entries()
            .map(|e| e.id)
            .filter(|i| i >> 24 == 0x02)
            .collect();
        let (mut total, mut has65, mut has0, mut neither, mut differ, mut moved) =
            (0, 0, 0, 0, 0, 0);
        let mut worst = 0.0f32;
        let mut worst_id = 0u32;
        for i in ids {
            let Ok(s) = assets.setup(i) else { continue };
            if s.placement_frames.is_empty() {
                continue;
            }
            total += 1;
            let f65 = s.placement_frames.iter().find(|(k, _)| *k == 0x65);
            let f0 = s.placement_frames.iter().find(|(k, _)| *k == 0);
            match (f65, f0) {
                (Some(_), _) => has65 += 1,
                (None, Some(_)) => has0 += 1,
                (None, None) => neither += 1,
            }
            if let (Some((_, a)), Some((_, b))) = (f65, f0) {
                let mut d = 0.0f32;
                for (x, y) in a.frames.iter().zip(b.frames.iter()) {
                    d = d.max((x.origin - y.origin).length());
                    d = d.max((x.orientation.xyz() - y.orientation.xyz()).length());
                }
                if d > 1e-4 {
                    differ += 1;
                    if d > 0.05 {
                        moved += 1;
                    }
                    if d > worst {
                        worst = d;
                        worst_id = i;
                    }
                }
            }
        }
        println!("{total} setups with placement frames: {has65} have 0x65, {has0} have only 0, {neither} have neither");
        println!("of those with both, {differ} differ between 0x65 and 0 ({moved} by more than 5 cm); worst {worst:.2} m at {worst_id:#010x}");
        return;
    }
    let s = assets.setup(id).unwrap();
    println!(
        "setup {id:#010x} flags {:#x}, {} parts, {} placement frames: keys {:?}",
        s.flags,
        s.parts.len(),
        s.placement_frames.len(),
        s.placement_frames
            .iter()
            .map(|(k, _)| *k)
            .collect::<Vec<_>>()
    );
    println!("  default_scale {:?}", s.default_scale);
    println!("  parent_index {:?}", s.parent_index);
    for (key, af) in &s.placement_frames {
        println!("  placement {key} ({key:#x}): {} frames", af.frames.len());
        for (i, f) in af.frames.iter().enumerate() {
            let m = frame_to_mat(f);
            // Where this part's geometry lands under this placement.
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            if let Some(&part) = s.parts.get(i) {
                if let Ok(g) = assets.gfxobj(part) {
                    for (_, v) in &g.vertices {
                        let p = m.transform_point3(v.origin);
                        lo = lo.min(p);
                        hi = hi.max(p);
                    }
                }
            }
            println!(
                "    part {i} {:#010x}: origin ({:.3},{:.3},{:.3}) quat ({:.3},{:.3},{:.3},{:.3}) -> bbox ({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2})",
                s.parts.get(i).copied().unwrap_or(0),
                f.origin.x, f.origin.y, f.origin.z,
                f.orientation.x, f.orientation.y, f.orientation.z, f.orientation.w,
                lo.x, lo.y, lo.z, hi.x, hi.y, hi.z
            );
        }
    }
    // Raw geometry, unplaced.
    for (i, &part) in s.parts.iter().enumerate() {
        if let Ok(g) = assets.gfxobj(part) {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for (_, v) in &g.vertices {
                lo = lo.min(v.origin);
                hi = hi.max(v.origin);
            }
            println!(
                "  raw part {i} {part:#010x}: bbox ({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2}) sort_center ({:.2},{:.2},{:.2})",
                lo.x, lo.y, lo.z, hi.x, hi.y, hi.z,
                g.sort_center.x, g.sort_center.y, g.sort_center.z
            );
        }
    }
}
