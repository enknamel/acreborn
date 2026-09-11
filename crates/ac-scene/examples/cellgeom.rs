//! Dump one interior cell: its structure's physics polygons and its
//! static objects, in landblock-local space.
//! `AC_DATA_DIR=... cargo run --release -p ac-scene --example cellgeom CELLID`
use ac_formats::landblock::EnvCell;
use ac_scene::model::frame_to_mat;
use ac_scene::Assets;
use glam::Vec3;

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let Some(arg) = std::env::args().nth(1) else {
        eprintln!("usage: cellgeom CELLID   (e.g. 01f6027a)");
        return;
    };
    let Ok(cell_id) = u32::from_str_radix(arg.trim_start_matches("0x"), 16) else {
        eprintln!("{arg} is not a cell id");
        return;
    };
    let bytes = assets.cell.read(cell_id).unwrap();
    let cell = EnvCell::parse(cell_id, &bytes).unwrap();
    println!(
        "cell {cell_id:#010x} env {:#x} struct {} pos {:?} statics {}",
        cell.environment_id,
        cell.cell_structure,
        cell.position,
        cell.static_objects.len()
    );
    let t = frame_to_mat(&cell.position);
    let env = assets.environment(cell.environment_id).unwrap();
    let (_, cs) = env
        .cells
        .iter()
        .find(|(k, _)| *k == cell.cell_structure as u32)
        .unwrap();
    let table: std::collections::HashMap<u16, Vec3> =
        cs.vertices.iter().map(|(k, v)| (*k, v.origin)).collect();
    let bbox = |ps: &[Vec3]| {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for p in ps {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        (lo, hi)
    };
    let mut all: Vec<Vec3> = Vec::new();
    for (id, p) in &cs.physics_polygons {
        let pts: Vec<Vec3> = p
            .vertex_ids
            .iter()
            .filter_map(|&v| table.get(&(v as u16)))
            .map(|v| t.transform_point3(*v))
            .collect();
        all.extend(pts.iter().copied());
        let (lo, hi) = bbox(&pts);
        println!(
            "  physpoly {id} cull {:?} n {} bbox ({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2}) portal {}",
            p.cull,
            pts.len(),
            lo.x,
            lo.y,
            lo.z,
            hi.x,
            hi.y,
            hi.z,
            cs.portals.contains(id)
        );
    }
    let (lo, hi) = bbox(&all);
    println!(
        "  struct physics bbox ({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2}); {} draw polys, {} portals {:?}",
        lo.x, lo.y, lo.z, hi.x, hi.y, hi.z, cs.polygons.len(), cs.portals.len(), cs.portals
    );
    for s in &cell.static_objects {
        let m = frame_to_mat(&s.frame);
        let o = m.w_axis.truncate();
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        let mut nphys = 0;
        let mut ndraw = 0;
        if let Ok(parts) = ac_scene::model::place(&assets, s.id, m) {
            for part in &parts {
                if let Ok(g) = assets.gfxobj(part.gfxobj_id) {
                    nphys += g.physics_polygons.len();
                    ndraw += g.polygons.len();
                    for (_, v) in &g.vertices {
                        let p = part.transform.transform_point3(v.origin);
                        lo = lo.min(p);
                        hi = hi.max(p);
                    }
                }
            }
        }
        println!(
            "  static {:#010x} at ({:.2},{:.2},{:.2}) bbox ({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2}) phys {nphys} draw {ndraw}",
            s.id, o.x, o.y, o.z, lo.x, lo.y, lo.z, hi.x, hi.y, hi.z
        );
    }
}
