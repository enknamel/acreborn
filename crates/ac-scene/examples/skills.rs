//! Print the skill table: id and name.
use ac_scene::Assets;
fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let table = assets.skill_table().unwrap();
    let mut ids: Vec<u32> = (1..60).filter(|id| table.get(*id).is_some()).collect();
    ids.sort();
    for id in ids {
        if let Some(s) = table.get(id) {
            print!("{id}={} ", s.name.replace(' ', "_"));
        }
    }
    println!();
}
