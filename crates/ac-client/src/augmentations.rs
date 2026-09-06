//! Augmentations: permanent character upgrades bought with experience by
//! using an augmentation gem (ACE `AugmentationDevice`). Using the gem
//! asks a kind 6 confirmation ("This action will augment your character
//! with X and will cost N available experience."); on yes the server
//! raises the matching character property (PropertyInt 218..328), takes
//! the XP and says "X has acquired the Y augmentation!". The gem's cost
//! is its `AugmentationCost` (PropertyInt64 3), shown by appraisal.

use crate::Client;

/// (PropertyInt id, name, times it can be taken).
pub const AUGMENTATIONS: &[(u32, &str, u32)] = &[
    (218, "Reinforcement of the Lugians (Strength)", 10),
    (219, "Bleeargh's Fortitude (Endurance)", 10),
    (220, "Oswald's Enhancement (Coordination)", 10),
    (221, "Siraluun's Blessing (Quickness)", 10),
    (222, "Enduring Calm (Focus)", 10),
    (223, "Steadfast Will (Self)", 10),
    (224, "Ciandra's Essence (Salvaging)", 1),
    (225, "Yoshi's Essence (Item Tinkering)", 1),
    (226, "Jibril's Essence (Armor Tinkering)", 1),
    (227, "Celdiseth's Essence (Magic Item Tinkering)", 1),
    (228, "Koga's Essence (Weapon Tinkering)", 1),
    (229, "Shadow of the Seventh Mule (pack slot)", 1),
    (230, "Might of the Seventh Mule (carrying capacity)", 5),
    (231, "Clutch of the Miser (less death item loss)", 3),
    (232, "Enduring Enchantment (spells past death)", 1),
    (233, "Critical Protection", 1),
    (234, "Quick Learner (bonus XP)", 1),
    (235, "Ciandra's Fortune (bonus salvage)", 4),
    (236, "Charmed Smith (imbue chance)", 1),
    (237, "Innate Renewal (faster regeneration)", 2),
    (238, "Archmage's Endurance (spell duration)", 5),
    (240, "Enhancement of the Blade Turner (slash)", 2),
    (241, "Enhancement of the Arrow Turner (pierce)", 2),
    (242, "Enhancement of the Mace Turner (bludgeon)", 2),
    (243, "Caustic Enhancement (acid)", 2),
    (244, "Fiery Enhancement (fire)", 2),
    (245, "Icy Enhancement (cold)", 2),
    (246, "Storm's Enhancement (lightning)", 2),
    (293, "Specialize Gearcraft", 1),
    (294, "Infused Creature Magic", 1),
    (295, "Infused Item Magic", 1),
    (296, "Infused Life Magic", 1),
    (297, "Infused War Magic", 1),
    (298, "Eye of the Remorseless (critical chance)", 1),
    (299, "Hand of the Remorseless (critical damage)", 1),
    (300, "Master of the Steel Circle (melee)", 1),
    (301, "Master of the Focused Eye (missile)", 1),
    (302, "Master of the Five Fold Path (magic)", 1),
    (309, "Frenzy of the Slayer (damage bonus)", 1),
    (310, "Iron Skin of the Invincible (damage reduction)", 1),
    (326, "Jack of All Trades", 1),
    (327, "Nether Enhancement", 2),
    (328, "Infused Void Magic", 1),
];

/// One augmentation the character has: name, times taken, maximum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owned {
    pub name: &'static str,
    pub count: u32,
    pub max: u32,
}

impl Client {
    /// The augmentations the character has taken, from the sheet's
    /// integer properties.
    pub fn augmentations(&self) -> Vec<Owned> {
        AUGMENTATIONS
            .iter()
            .filter_map(|&(id, name, max)| {
                let count = self
                    .world
                    .stats
                    .ints
                    .iter()
                    .find(|(k, _)| *k == id)
                    .map(|(_, v)| *v)
                    .unwrap_or(0);
                (count > 0).then_some(Owned {
                    name,
                    count: count as u32,
                    max,
                })
            })
            .collect()
    }
}
