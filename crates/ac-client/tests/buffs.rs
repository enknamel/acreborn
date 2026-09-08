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
// Two thirty-second quest spells that outrank the numbered buffs of
// their categories in power, and the level-eight buff for one of them.
const LICORICE_LEAP: u32 = 4211;
const TUSKER_SPRINT: u32 = 2933;
const PRODIGAL_JUMPING: u32 = 3715;
const JUMP: u32 = 22;
const RUN: u32 = 24;

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
    let all = |_: u32| true;
    let me = Character {
        known: &known,
        trained: &[HEAVY_WEAPONS],
        stance: Stance::Melee,
        guid: 0x5000_0001,
        wears_armour: true,
        usable: &all,
        weapon_skill: Some(HEAVY_WEAPONS),
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
    // Impenetrability once, cast at ourselves: the server spreads it
    // over everything worn.
    let on_armour: Vec<u32> = wants
        .iter()
        .filter(|w| w.spell == IMPENETRABILITY_VI)
        .filter_map(|w| match w.target {
            Target::Item(g) => Some(g),
            Target::Me => None,
        })
        .collect();
    assert_eq!(on_armour, vec![0x5000_0001]);
    // Nothing worn: nothing to harden.
    let bare = Character {
        wears_armour: false,
        ..me
    };
    assert!(!wanted(&table, &bare)
        .iter()
        .any(|w| w.spell == IMPENETRABILITY_VI));

    // The same character as a mage: the other mastery and the other aura.
    let mage = Character {
        trained: &[WAR_MAGIC],
        stance: Stance::Magic,
        weapon_skill: Some(0),
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

#[test]
fn short_lived_spells_are_not_buffs() {
    let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
        return;
    };
    let assets = Assets::open(dir).unwrap();
    let table = assets.spell_table().unwrap();
    let known = [LICORICE_LEAP, TUSKER_SPRINT, PRODIGAL_JUMPING];
    let all = |_: u32| true;
    let me = Character {
        known: &known,
        trained: &[JUMP, RUN],
        stance: Stance::Melee,
        guid: 0x5000_0001,
        wears_armour: false,
        usable: &all,
        weapon_skill: None,
    };
    let spells: Vec<u32> = wanted(&table, &me).iter().map(|w| w.spell).collect();
    assert_eq!(spells, vec![PRODIGAL_JUMPING], "{spells:?}");
}
