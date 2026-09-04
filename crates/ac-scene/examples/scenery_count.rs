//! Print scenery counts per landblock around a center block.
use ac_scene::{landblock, Assets};

fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(dir).unwrap();
    let center =
        u32::from_str_radix(&std::env::args().nth(1).unwrap_or("A9B4".into()), 16).unwrap();
    let (cx, cy) = (center >> 8, center & 0xFF);
    // Detailed stats for the center block.
    {
        let id = ac_scene::lbid::from_xy(cx, cy);
        let lb_id = id | 0xFFFF;
        let lb =
            ac_formats::landblock::CellLandblock::parse(lb_id, &assets.cell.read(lb_id).unwrap())
                .unwrap();
        let info_id = id | 0xFFFE;
        let info = assets
            .cell
            .read(info_id)
            .ok()
            .map(|b| ac_formats::landblock::LandblockInfo::parse(info_id, &b).unwrap());
        let (_, st) = ac_scene::scenery::generate_with_stats(&assets, &lb, info.as_ref()).unwrap();
        println!("{:04X}: {st:?}", id >> 16);
        if let Some(info) = &info {
            for b in &info.buildings {
                let r = assets
                    .setup(b.model_id)
                    .map(|s| s.sorting_sphere.radius)
                    .unwrap_or(-1.0);
                println!(
                    "  building {:08X} at {:?} sorting radius {r:.1}",
                    b.model_id, b.frame.origin
                );
            }
        }
        let mut types = std::collections::BTreeMap::new();
        for t in &lb.terrain {
            *types
                .entry((
                    ac_formats::landblock::terrain::terrain_type(*t),
                    ac_formats::landblock::terrain::scenery(*t),
                ))
                .or_insert(0) += 1;
        }
        println!("  (terrain type, scene type) -> count: {types:?}");
    }
    for by in (cy.saturating_sub(1)..=cy + 1).rev() {
        let mut row = String::new();
        for bx in cx.saturating_sub(1)..=cx + 1 {
            let id = ac_scene::lbid::from_xy(bx, by);
            match landblock::load(&assets, id) {
                Ok(s) => row.push_str(&format!(
                    "{:04X}: {:3} scenery, info {}  ",
                    id >> 16,
                    s.scenery_count,
                    s.has_info as u8
                )),
                Err(e) => row.push_str(&format!("{:04X}: err {e}  ", id >> 16)),
            }
        }
        println!("{row}");
    }
}
