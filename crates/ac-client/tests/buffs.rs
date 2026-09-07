//! The buffs a character should wear, worked out from its training and
//! from what it can cast. Needs AC_DATA_DIR for the SpellTable.

use ac_client::buffs::{wanted, Character, Target};
use ac_client::Stance;
use ac_scene::Assets;

// Spell ids, from the table: the same buff at two levels, skill buffs
// for two skills, an armour spell, and the two kinds of weapon aura.
const REJUVENATION_I: u32 = 54;
const REJUVENATION_VI: u32 = 193;
const HEAVY_MASTERY_VI: u32 = 423;
const WAR_MASTERY_VI: u32 = 634;
const STRENGTH_VI: u32 = 1332;
const BLADE_PROTECTION_VI: u32 = 1114;
const IMPENETRABILITY_VI: u32 = 1486;
const BLOOD_DRINKER_SELF_VI: u32 = 1616;
const SPIRIT_DRINKER_SELF_VI: u32 = 3258;
const HEAVY_WEAPONS: u32 = 44;
const WAR_MAGIC: u32 = 34;

#[test]
fn a_swordsman_gets_his_own_masteries_and_the_highest_level_he_can_land() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(dir).unwrap();
    let table = assets.spell_table().unwrap();
    let known = [
        REJUVENATION_I,
        REJUVENATION_VI,
        HEAVY_MASTERY_VI,
        WAR_MASTERY_VI,
        STRENGTH_VI,
        BLADE_PROTECTION_VI,
        IMPENETRABILITY_VI,
        BLOOD_DRINKER_SELF_VI,
        SPIRIT_DRINKER_SELF_VI,
    ];
    let armour = [0x8000_0001u32, 0x8000_0002];
    let all = |_: u32| true;
    let me = Character {
        known: &known,
        trained: &[HEAVY_WEAPONS],
        stance: Stance::Melee,
        armour: &armour,
        usable: &all,
    };
    let wants = wanted(&table, &me);
    let spells: Vec<u32> = wants.iter().map(|w| w.spell).collect();
    // One Rejuvenation, the sixth.
    assert!(spells.contains(&REJUVENATION_VI));
    assert!(!spells.contains(&REJUVENATION_I), "{spells:?}");
    // His mastery, not the mage's.
    assert!(spells.contains(&HEAVY_MASTERY_VI));
    assert!(!spells.contains(&WAR_MASTERY_VI));
    // Attributes and protections for anyone.
    assert!(spells.contains(&STRENGTH_VI));
    assert!(spells.contains(&BLADE_PROTECTION_VI));
    // The weapon aura, not the caster's.
    assert!(spells.contains(&BLOOD_DRINKER_SELF_VI));
    assert!(!spells.contains(&SPIRIT_DRINKER_SELF_VI));
    // Impenetrability on each piece of armour.
    let on_armour: Vec<u32> = wants
        .iter()
        .filter(|w| w.spell == IMPENETRABILITY_VI)
        .filter_map(|w| match w.target {
            Target::Item(g) => Some(g),
            Target::Me => None,
        })
        .collect();
    assert_eq!(on_armour, armour);

    // The same character as a mage: the other mastery and the other aura.
    let mage = Character {
        trained: &[WAR_MAGIC],
        stance: Stance::Magic,
        ..me
    };
    let spells: Vec<u32> = wanted(&table, &mage).iter().map(|w| w.spell).collect();
    assert!(spells.contains(&WAR_MASTERY_VI) && !spells.contains(&HEAVY_MASTERY_VI));
    assert!(spells.contains(&SPIRIT_DRINKER_SELF_VI) && !spells.contains(&BLOOD_DRINKER_SELF_VI));

    // A caster who cannot land the sixth gets the first instead: the
    // highest level that can be cast, not the highest known.
    let weak = |id: u32| id != REJUVENATION_VI;
    let novice = Character {
        usable: &weak,
        ..me
    };
    let spells: Vec<u32> = wanted(&table, &novice).iter().map(|w| w.spell).collect();
    assert!(spells.contains(&REJUVENATION_I) && !spells.contains(&REJUVENATION_VI));
}
