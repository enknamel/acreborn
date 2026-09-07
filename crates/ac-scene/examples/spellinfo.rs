//! Print a few fields of spells by id: spellinfo 3405 1160
use ac_scene::Assets;
fn main() {
    let dir = std::env::var_os("AC_DATA_DIR").expect("AC_DATA_DIR");
    let assets = Assets::open(std::path::Path::new(&dir)).unwrap();
    let table = assets.spell_table().unwrap();
    for a in std::env::args().skip(1) {
        let id: u32 = a.parse().unwrap();
        match table.get(id) {
            Some(s) => println!(
                "{id} {:?}: school {} category {} power {} level {} flags {:#x} self_targeted {} beneficial {} duration {:?} components {:?}",
                s.name, s.school, s.category, s.power, s.level(), s.bitfield, s.is_self_targeted(), s.is_beneficial(), s.duration(), s.components
            ),
            None => println!("{id}: not in the table"),
        }
    }
}
