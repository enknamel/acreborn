//! What the skill table says it costs to train a skill. Needs AC_DATA_DIR.
use ac_scene::Assets;

#[test]
fn the_table_has_a_training_cost_for_the_magic_schools() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(dir).unwrap();
    let table = assets.skill_table().unwrap();
    for (id, name) in [
        (31u32, "Creature Enchantment"),
        (32, "Item Enchantment"),
        (33, "Life Magic"),
        (34, "War Magic"),
    ] {
        let base = table.get(id);
        println!(
            "{id} {name}: {:?}",
            base.map(|b| (b.trained_cost, b.specialized_cost))
        );
        assert!(base.is_some(), "{name} ({id}) missing from the skill table");
    }
}
