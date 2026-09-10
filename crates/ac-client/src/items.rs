//! Carried items as searchable records: what an item is from its object
//! description, plus its numbers once it has been appraised (damage,
//! armor level, spells, wield requirement...). [`Query`] parses an
//! inventory search line ("dmg>10 spell:blood type:weapon", "slot:ring
//! epics>=2", "hauberk \"epic life magic\"", "(ring or bracelet) not
//! minors>0") and [`ItemStats::matches`] tests an item against it.

use crate::Client;
use ac_net::messages::Appraisal;
use ac_world::{item_type, WorldObject};
use serde::{Deserialize, Serialize};

/// The broad kind of an item, from its ItemType bits, as one word.
pub fn kind_name(item_type: u32) -> &'static str {
    let kinds = [
        (item_type::MELEE_WEAPON, "weapon"),
        (item_type::MISSILE_WEAPON, "missile"),
        (item_type::CASTER, "caster"),
        (item_type::ARMOR, "armor"),
        (item_type::CLOTHING, "clothing"),
        (item_type::JEWELRY, "jewelry"),
        (item_type::CONTAINER, "pack"),
        (item_type::FOOD, "food"),
        (item_type::MONEY, "money"),
        (item_type::GEM, "gem"),
        (item_type::KEY, "key"),
        (item_type::MANA_STONE, "manastone"),
        (item_type::SPELL_COMPONENTS, "comps"),
        (item_type::WRITABLE, "writable"),
        (item_type::PORTAL, "portal"),
        (item_type::TINKERING_MATERIAL, "salvage"),
        (item_type::TINKERING_TOOL, "tool"),
        (item_type::PROMISSORY_NOTE, "note"),
        (item_type::SERVICE, "service"),
        (item_type::LIFESTONE, "lifestone"),
        (item_type::GAMEBOARD, "gameboard"),
        (item_type::MAGIC_WIELDABLE, "trinket"),
        (item_type::MISC, "misc"),
        (item_type::USELESS, "junk"),
    ];
    kinds
        .iter()
        .find(|(bit, _)| item_type & bit != 0)
        .map(|(_, n)| *n)
        .unwrap_or("misc")
}

/// One carried item with everything a search or a sort can ask about.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemStats {
    pub guid: u32,
    pub name: String,
    pub wcid: u32,
    pub item_type: u32,
    /// See [`kind_name`].
    pub kind: &'static str,
    pub stack: u32,
    pub wielded: bool,
    /// Where it can be worn or held (see `ac_world::equip`): what tells
    /// a shield from the rest of the armour.
    pub valid_locations: u32,
    /// The pack holding it (our own guid for the main pack).
    pub container: u32,
    pub value: u32,
    pub burden: u32,
    pub workmanship: f32,
    pub material: &'static str,
    /// Uses left and their maximum (a salvage bag's units), 0 when none.
    pub structure: u32,
    pub max_structure: u32,
    /// True once the server has told us the numbers below.
    pub appraised: bool,
    /// Weapon damage range and words ("Slashing"), 0 when not a weapon.
    pub damage_low: u32,
    pub damage_high: u32,
    pub damage_type: String,
    /// The same as bits, for matching against what a creature resists.
    pub damage_type_bits: u32,
    /// What the weapon was imbued with: critical strike, crippling blow,
    /// rending of one element (see `ac_world::elements::imbue`).
    pub imbued: u32,
    /// A caster's elemental damage bonus, 1.0 when it has none.
    pub elemental_damage: f32,
    /// How often and how hard criticals land, 0 when not said.
    pub crit_frequency: f32,
    pub crit_multiplier: f32,
    /// A launcher's damage modifier (a bow multiplies its arrows' damage
    /// by this), 0 when not said.
    pub damage_mod: f32,
    /// What ammunition it takes or is (see `ac_world::fletching::ammo_type`),
    /// 0 for a weapon that needs none.
    pub ammo_type: u32,
    /// What it is for in a fight (see `ac_world::fletching::combat_use`).
    pub combat_use: u32,
    pub speed: u32,
    pub weapon_skill: String,
    /// The id of the skill the weapon is used with, 0 when not said.
    pub weapon_skill_id: u32,
    pub attack_bonus: f64,
    pub defense_bonus: f64,
    pub armor_level: u32,
    pub shield: u32,
    /// Spell names on the item (cast on use, or on wield).
    pub spells: Vec<String>,
    /// Skill and level needed to wield it, in words.
    pub wield_skill: String,
    pub wield_level: u32,
    /// Every requirement for wielding it, as `(kind, what, difficulty)`:
    /// see `ac_world::wield`. A weapon may carry up to four, and all of
    /// them must be met.
    pub wield_reqs: Vec<(u32, u32, u32)>,
    pub mana: u32,
    pub max_mana: u32,
    pub spellcraft: u32,
    pub tinks: u32,
    pub bonded: bool,
    pub attuned: bool,
}

/// Damage type bits as words ("Slashing, Fire").
pub fn damage_type_name(bits: u32) -> String {
    let names = [
        (0x1, "Slashing"),
        (0x2, "Piercing"),
        (0x4, "Bludgeoning"),
        (0x8, "Cold"),
        (0x10, "Fire"),
        (0x20, "Acid"),
        (0x40, "Electric"),
        (0x400, "Nether"),
    ];
    let parts: Vec<&str> = names
        .iter()
        .filter(|(b, _)| bits & b != 0)
        .map(|(_, n)| *n)
        .collect();
    if parts.is_empty() {
        "?".into()
    } else {
        parts.join(", ")
    }
}

impl ItemStats {
    /// From the object alone (name, kind, value, burden, material).
    pub fn of(o: &WorldObject, me: Option<u32>) -> Self {
        ItemStats {
            guid: o.guid,
            name: o.name.clone(),
            wcid: o.weenie_class_id,
            item_type: o.item_type,
            kind: kind_name(o.item_type),
            stack: if o.name.starts_with("Salvaged ") && o.structure > 0 {
                o.structure
            } else {
                o.stack_size.max(1)
            },
            wielded: me.is_some() && o.wielder == me,
            valid_locations: o.valid_locations,
            container: o.container.unwrap_or(0),
            value: o.value,
            burden: o.burden,
            workmanship: o.workmanship,
            material: if o.material != 0 {
                ac_world::material::name(o.material)
            } else {
                ""
            },
            structure: o.structure,
            max_structure: o.max_structure,
            ammo_type: o.ammo_type,
            combat_use: o.combat_use,
            ..Default::default()
        }
        .with_launcher_guess()
    }

    /// A bow, crossbow or atlatl the server did not describe: the header
    /// fields are optional and appraisal never carries them (ACE sends
    /// neither), so what it shoots is worked out from its skill or name.
    fn with_launcher_guess(mut self) -> Self {
        use ac_world::fletching::{ammo_type, combat_use};
        use ac_world::item_type;
        if self.ammo_type == 0
            && self.item_type & item_type::MISSILE_WEAPON != 0
            && self.combat_use != combat_use::AMMO
        {
            let guess = ammo_type::guess(&self.name, self.weapon_skill_id);
            if guess != 0 {
                self.ammo_type = guess;
                if self.combat_use == 0 {
                    self.combat_use = combat_use::LAUNCHER;
                }
            }
        }
        self
    }

    /// Add what an appraisal says; `skill_name`/`spell_name` resolve ids.
    pub fn with_appraisal(
        mut self,
        a: &Appraisal,
        skill_name: &dyn Fn(u32) -> String,
        spell_name: &dyn Fn(u32) -> String,
    ) -> Self {
        self.appraised = a.success;
        if let Some(v) = a.int(19) {
            self.value = v.max(0) as u32;
        }
        if let Some(b) = a.int(5) {
            self.burden = b.max(0) as u32;
        }
        if let Some(w) = a.int(105) {
            self.workmanship = w as f32;
        }
        if let Some(m) = a.int(131) {
            self.material = ac_world::material::name(m as u32);
        }
        if let Some(al) = a.int(28) {
            self.armor_level = al.max(0) as u32;
        }
        if let Some(s) = a.int(56) {
            self.shield = s.max(0) as u32;
        }
        if let Some(w) = &a.weapon {
            self.damage_high = w.damage;
            self.damage_low = (w.damage as f64 * (1.0 - w.variance)).round() as u32;
            self.damage_type = damage_type_name(w.damage_type);
            self.damage_type_bits = w.damage_type;
            self.speed = w.speed;
            self.weapon_skill = skill_name(w.skill);
            self.weapon_skill_id = w.skill;
            self.attack_bonus = w.offense;
            self.defense_bonus = a.float(29).unwrap_or(1.0);
        }
        if let (Some(skill), Some(level)) = (a.int(159), a.int(160)) {
            self.wield_skill = skill_name(skill as u32);
            self.wield_level = level.max(0) as u32;
        }
        // Up to four requirement sets, each a kind, what it is about and
        // how much of it is needed. A top wand asks for War Magic 275.
        self.wield_reqs = [
            (158, 159, 160),
            (270, 271, 272),
            (273, 274, 275),
            (276, 277, 278),
        ]
        .into_iter()
        .filter_map(|(kind, what, difficulty)| {
            let kind = a.int(kind)?.max(0) as u32;
            (kind != 0).then(|| {
                (
                    kind,
                    a.int(what).unwrap_or(0).max(0) as u32,
                    a.int(difficulty).unwrap_or(0).max(0) as u32,
                )
            })
        })
        .collect();
        if let (Some(cur), Some(max)) = (a.int(107), a.int(108)) {
            self.mana = cur.max(0) as u32;
            self.max_mana = max.max(0) as u32;
        }
        if let Some(c) = a.int(106) {
            self.spellcraft = c.max(0) as u32;
        }
        if let (Some(s), Some(m)) = (a.int(92), a.int(91)) {
            self.structure = s.max(0) as u32;
            self.max_structure = m.max(0) as u32;
        }
        // A weapon may carry up to five imbued effects, in five
        // separate properties; they are one word as far as we care.
        self.imbued = [179, 303, 304, 305, 306]
            .into_iter()
            .filter_map(|p| a.int(p))
            .fold(0u32, |all, v| all | v as u32);
        // A caster with no elemental bonus is worth 1.0, not 0.
        if let Some(m) = a.float(152) {
            self.elemental_damage = m as f32;
        }
        if let Some(f) = a.float(147) {
            self.crit_frequency = f as f32;
        }
        if let Some(m) = a.float(136) {
            self.crit_multiplier = m as f32;
        }
        if let Some(m) = a.float(63) {
            self.damage_mod = m as f32;
        }
        if let Some(t) = a.int(50) {
            self.ammo_type = t.max(0) as u32;
        }
        if let Some(u) = a.int(51) {
            self.combat_use = u.max(0) as u32;
        }
        // A caster's own damage type is not in the weapon profile.
        if self.damage_type_bits == 0 {
            if let Some(t) = a.int(45) {
                self.damage_type_bits = t.max(0) as u32;
                self.damage_type = damage_type_name(self.damage_type_bits);
            }
        }
        self.tinks = a.int(171).unwrap_or(0).max(0) as u32;
        self.bonded = a.int(33).unwrap_or(0) != 0;
        self.attuned = a.int(114).unwrap_or(0) != 0;
        self.spells = a.spells.iter().map(|s| spell_name(*s)).collect();
        self.with_launcher_guess()
    }

    /// The short lines a tooltip shows: damage, armor, spells, requirement,
    /// value and burden.
    pub fn summary(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.damage_high > 0 {
            out.push(format!(
                "Damage {}-{} {} (speed {})",
                self.damage_low, self.damage_high, self.damage_type, self.speed
            ));
        }
        if self.armor_level > 0 {
            out.push(format!("Armor level {}", self.armor_level));
        }
        if self.shield > 0 {
            out.push(format!("Shield {}", self.shield));
        }
        if !self.spells.is_empty() {
            out.push(format!("Spells: {}", self.spells.join(", ")));
        }
        if self.wield_level > 0 {
            out.push(format!(
                "Requires {} {}",
                self.wield_skill, self.wield_level
            ));
        }
        if self.max_mana > 0 {
            out.push(format!("Mana {} / {}", self.mana, self.max_mana));
        }
        if self.workmanship > 0.0 {
            let mat = if self.material.is_empty() {
                String::new()
            } else {
                format!(" {}", self.material)
            };
            out.push(format!("Workmanship {:.0}{mat}", self.workmanship));
        } else if !self.material.is_empty() {
            out.push(self.material.to_string());
        }
        if self.max_structure > 0 {
            out.push(format!("Uses {} / {}", self.structure, self.max_structure));
        }
        let mut money = Vec::new();
        if self.value > 0 {
            money.push(format!("{} py", self.value));
        }
        if self.burden > 0 {
            money.push(format!("{} bu", self.burden));
        }
        if !money.is_empty() {
            out.push(money.join(", "));
        }
        let mut flags = Vec::new();
        if self.bonded {
            flags.push("bonded");
        }
        if self.attuned {
            flags.push("attuned");
        }
        if self.tinks > 0 {
            out.push(format!("Tinkered {} times", self.tinks));
        }
        if !flags.is_empty() {
            out.push(flags.join(", "));
        }
        if !self.appraised {
            out.push("(not appraised)".into());
        }
        out
    }

    /// The numeric field a query or a sort names, if this item has it.
    pub fn number(&self, key: NumKey) -> Option<f64> {
        let some = |v: u32| (v > 0).then_some(v as f64);
        match key {
            NumKey::Damage => some(self.damage_high),
            NumKey::Armor => some(self.armor_level),
            NumKey::Value => Some(self.value as f64),
            NumKey::Burden => Some(self.burden as f64),
            NumKey::Workmanship => (self.workmanship > 0.0).then_some(self.workmanship as f64),
            NumKey::Speed => some(self.speed),
            NumKey::Wield => some(self.wield_level),
            NumKey::Mana => some(self.max_mana),
            NumKey::Spellcraft => some(self.spellcraft),
            NumKey::Uses => some(self.structure),
            NumKey::Tinks => Some(self.tinks as f64),
            NumKey::Stack => Some(self.stack as f64),
            NumKey::Attack => Some((self.attack_bonus - 1.0) * 100.0),
            NumKey::Defense => Some((self.defense_bonus - 1.0) * 100.0),
            // Spell counts are known once appraised (or when the
            // description itself names a spell, a scroll's say); an
            // unknown list is not an empty one.
            NumKey::Spells => self.spells_known().then_some(self.spells.len() as f64),
            NumKey::Cantrips => self.spells_known().then_some(self.tiers().count() as f64),
            NumKey::Minors => self.tier_count(Tier::Minor),
            NumKey::Moderates => self.tier_count(Tier::Moderate),
            NumKey::Majors => self.tier_count(Tier::Major),
            NumKey::Epics => self.tier_count(Tier::Epic),
            NumKey::Legendaries => self.tier_count(Tier::Legendary),
        }
    }

    fn spells_known(&self) -> bool {
        self.appraised || !self.spells.is_empty()
    }

    fn tier_count(&self, tier: Tier) -> Option<f64> {
        self.spells_known().then_some(self.cantrips(tier) as f64)
    }

    /// Whether the item matches the query (an empty one matches all).
    pub fn matches(&self, q: &Query) -> bool {
        q.expr.as_ref().is_none_or(|e| self.matches_expr(e))
    }

    fn matches_expr(&self, e: &Expr) -> bool {
        match e {
            Expr::Term(t) => self.matches_term(t),
            Expr::Not(inner) => !self.matches_expr(inner),
            Expr::And(all) => all.iter().all(|e| self.matches_expr(e)),
            Expr::Or(any) => any.iter().any(|e| self.matches_expr(e)),
        }
    }

    /// The cantrip tier of each spell on the item that is one.
    pub fn tiers(&self) -> impl Iterator<Item = Tier> + '_ {
        self.spells.iter().filter_map(|s| Tier::of_spell(s))
    }

    /// How many cantrips of `tier` the item carries.
    pub fn cantrips(&self, tier: Tier) -> usize {
        self.tiers().filter(|t| *t == tier).count()
    }

    /// Whether the item goes in `slot` (see [`slot_mask`]).
    pub fn fits_slot(&self, slot: u32) -> bool {
        self.valid_locations & slot != 0
    }

    pub fn matches_term(&self, t: &Term) -> bool {
        let has = |hay: &str, needle: &str| hay.to_lowercase().contains(needle);
        match t {
            Term::Word(w) => {
                has(&self.name, w)
                    || has(self.material, w)
                    || has(self.kind, w)
                    || self.spells.iter().any(|s| has(s, w))
                    || slot_mask(w).is_some_and(|m| self.fits_slot(m))
            }
            Term::Spell(w) => self.spells.iter().any(|s| has(s, w)),
            Term::Kind(w) => self.kind == w || (w == "weapon" && self.damage_high > 0),
            Term::Material(w) => has(self.material, w),
            Term::Skill(w) => has(&self.weapon_skill, w) || has(&self.wield_skill, w),
            Term::Slot(mask) => self.fits_slot(*mask),
            Term::Tier(tier) => self.cantrips(*tier) > 0,
            Term::Wielded => self.wielded,
            Term::Unappraised => !self.appraised,
            Term::Num(key, op, v) => self.number(*key).is_some_and(|x| op.test(x, *v)),
        }
    }
}

/// A cantrip's tier, read off the front of its name ("Epic Strength",
/// "Legendary Life Magic Aptitude"): what an item's spell list says, not
/// what is cast.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tier {
    Minor,
    Moderate,
    Major,
    Epic,
    Legendary,
}

impl Tier {
    pub const ALL: [Tier; 5] = [
        Tier::Minor,
        Tier::Moderate,
        Tier::Major,
        Tier::Epic,
        Tier::Legendary,
    ];

    /// The word a query uses for the tier.
    pub fn word(self) -> &'static str {
        match self {
            Tier::Minor => "minor",
            Tier::Moderate => "moderate",
            Tier::Major => "major",
            Tier::Epic => "epic",
            Tier::Legendary => "legendary",
        }
    }

    /// The tier named by a query word ("epic"), if any.
    pub fn parse(word: &str) -> Option<Tier> {
        Tier::ALL.into_iter().find(|t| t.word() == word)
    }

    /// The tier a spell name starts with, if it is a cantrip's.
    pub fn of_spell(name: &str) -> Option<Tier> {
        let lower = name.to_lowercase();
        Tier::ALL.into_iter().find(|t| {
            lower
                .strip_prefix(t.word())
                .is_some_and(|rest| rest.starts_with(' '))
        })
    }
}

/// Where an item is worn, as `valid_locations` bits (ACE `EquipMask`).
/// The weapon and shield bits live in `ac_world::equip`.
pub mod slot {
    pub const HEAD: u32 = 0x1;
    pub const CHEST_WEAR: u32 = 0x2;
    pub const ABDOMEN_WEAR: u32 = 0x4;
    pub const UPPER_ARM_WEAR: u32 = 0x8;
    pub const LOWER_ARM_WEAR: u32 = 0x10;
    pub const HANDS: u32 = 0x20;
    pub const UPPER_LEG_WEAR: u32 = 0x40;
    pub const LOWER_LEG_WEAR: u32 = 0x80;
    pub const FEET: u32 = 0x100;
    pub const CHEST_ARMOR: u32 = 0x200;
    pub const ABDOMEN_ARMOR: u32 = 0x400;
    pub const UPPER_ARM_ARMOR: u32 = 0x800;
    pub const LOWER_ARM_ARMOR: u32 = 0x1000;
    pub const UPPER_LEG_ARMOR: u32 = 0x2000;
    pub const LOWER_LEG_ARMOR: u32 = 0x4000;
    pub const NECK: u32 = 0x8000;
    pub const WRIST: u32 = 0x1_0000 | 0x2_0000;
    pub const FINGER: u32 = 0x4_0000 | 0x8_0000;
    pub const TRINKET: u32 = 0x400_0000;
    pub const CLOAK: u32 = 0x800_0000;
    pub const SIGIL: u32 = 0x1000_0000 | 0x2000_0000 | 0x4000_0000;
    pub const CHEST: u32 = CHEST_WEAR | CHEST_ARMOR;
    pub const ABDOMEN: u32 = ABDOMEN_WEAR | ABDOMEN_ARMOR;
    pub const UPPER_ARMS: u32 = UPPER_ARM_WEAR | UPPER_ARM_ARMOR;
    pub const LOWER_ARMS: u32 = LOWER_ARM_WEAR | LOWER_ARM_ARMOR;
    pub const UPPER_LEGS: u32 = UPPER_LEG_WEAR | UPPER_LEG_ARMOR;
    pub const LOWER_LEGS: u32 = LOWER_LEG_WEAR | LOWER_LEG_ARMOR;
    pub const ARMS: u32 = UPPER_ARMS | LOWER_ARMS;
    pub const LEGS: u32 = UPPER_LEGS | LOWER_LEGS;
}

/// The slot words a query may use (`slot:ring`), with their bits.
pub const SLOTS: [(&str, u32); 27] = [
    ("head", slot::HEAD),
    ("chest", slot::CHEST),
    ("abdomen", slot::ABDOMEN),
    ("arms", slot::ARMS),
    ("upperarms", slot::UPPER_ARMS),
    ("lowerarms", slot::LOWER_ARMS),
    ("hands", slot::HANDS),
    ("legs", slot::LEGS),
    ("upperlegs", slot::UPPER_LEGS),
    ("lowerlegs", slot::LOWER_LEGS),
    ("feet", slot::FEET),
    ("neck", slot::NECK),
    ("necklace", slot::NECK),
    ("bracelet", slot::WRIST),
    ("wrist", slot::WRIST),
    ("ring", slot::FINGER),
    ("finger", slot::FINGER),
    ("trinket", slot::TRINKET),
    ("cloak", slot::CLOAK),
    ("sigil", slot::SIGIL),
    ("aetheria", slot::SIGIL),
    ("melee", ac_world::equip::MELEE_WEAPON),
    ("shield", ac_world::equip::SHIELD),
    ("missile", ac_world::equip::MISSILE_WEAPON),
    ("ammo", ac_world::equip::MISSILE_AMMO),
    ("wand", ac_world::equip::HELD),
    ("twohanded", ac_world::equip::TWO_HANDED),
];

/// The `valid_locations` bits a slot word names, if it is one.
pub fn slot_mask(word: &str) -> Option<u32> {
    SLOTS.iter().find(|(w, _)| *w == word).map(|(_, m)| *m)
}

/// Numeric fields a query can compare and a list can sort by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NumKey {
    Damage,
    Armor,
    Value,
    Burden,
    Workmanship,
    Speed,
    Wield,
    Mana,
    Spellcraft,
    Uses,
    Tinks,
    Stack,
    Attack,
    Defense,
    /// Spells on the item, all of them.
    Spells,
    /// Cantrips of any tier, and of each.
    Cantrips,
    Minors,
    Moderates,
    Majors,
    Epics,
    Legendaries,
}

impl NumKey {
    /// The words a query may use for the field.
    pub fn parse(word: &str) -> Option<NumKey> {
        Some(match word {
            "dmg" | "damage" => NumKey::Damage,
            "al" | "armor" => NumKey::Armor,
            "value" | "val" | "py" => NumKey::Value,
            "burden" | "bu" => NumKey::Burden,
            "ws" | "workmanship" | "work" => NumKey::Workmanship,
            "speed" => NumKey::Speed,
            "wield" | "req" | "level" | "lvl" => NumKey::Wield,
            "mana" => NumKey::Mana,
            "spellcraft" | "sc" => NumKey::Spellcraft,
            "uses" | "structure" => NumKey::Uses,
            "tinks" | "tinkered" => NumKey::Tinks,
            "stack" | "count" => NumKey::Stack,
            "attack" | "atk" => NumKey::Attack,
            "defense" | "def" => NumKey::Defense,
            "spells" => NumKey::Spells,
            "cantrips" => NumKey::Cantrips,
            "minors" => NumKey::Minors,
            "moderates" => NumKey::Moderates,
            "majors" => NumKey::Majors,
            "epics" => NumKey::Epics,
            "legendaries" => NumKey::Legendaries,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            NumKey::Damage => "damage",
            NumKey::Armor => "armor",
            NumKey::Value => "value",
            NumKey::Burden => "burden",
            NumKey::Workmanship => "workmanship",
            NumKey::Speed => "speed",
            NumKey::Wield => "wield level",
            NumKey::Mana => "mana",
            NumKey::Spellcraft => "spellcraft",
            NumKey::Uses => "uses",
            NumKey::Tinks => "tinks",
            NumKey::Stack => "stack",
            NumKey::Attack => "attack bonus",
            NumKey::Defense => "defense bonus",
            NumKey::Spells => "spells",
            NumKey::Cantrips => "cantrips",
            NumKey::Minors => "minor cantrips",
            NumKey::Moderates => "moderate cantrips",
            NumKey::Majors => "major cantrips",
            NumKey::Epics => "epic cantrips",
            NumKey::Legendaries => "legendary cantrips",
        }
    }

    /// Whether the field only exists once the item is appraised.
    pub fn needs_appraisal(self) -> bool {
        !matches!(self, NumKey::Value | NumKey::Burden | NumKey::Stack)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

impl Op {
    /// The comparison in words, for anything that shows a rule to a
    /// person rather than running it.
    pub fn word(self) -> &'static str {
        match self {
            Op::Lt => "is under",
            Op::Le => "is at most",
            Op::Gt => "is over",
            Op::Ge => "is at least",
            Op::Eq => "is",
        }
    }

    pub fn test(self, x: f64, v: f64) -> bool {
        match self {
            Op::Lt => x < v,
            Op::Le => x <= v,
            Op::Gt => x > v,
            Op::Ge => x >= v,
            Op::Eq => (x - v).abs() < 0.5,
        }
    }
}

/// One term of a query.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Term {
    /// Matches the name, material, kind, a spell name or a slot word. A
    /// quoted phrase (`"epic life magic"`) is one word, spaces and all.
    Word(String),
    /// `spell:blood`
    Spell(String),
    /// `type:armor`
    Kind(String),
    /// `mat:iron`
    Material(String),
    /// `skill:sword` (the weapon's skill or the wield requirement)
    Skill(String),
    /// `slot:ring` (see [`SLOTS`])
    Slot(u32),
    /// `tier:epic`: at least one cantrip of the tier.
    Tier(Tier),
    /// `wielded`
    Wielded,
    /// `unappraised`
    Unappraised,
    /// `dmg>10`, `al>=200`, `value<100`, `epics>=2`
    Num(NumKey, Op, f64),
}

/// A query as a tree: terms joined by `and` (a space), `or`, `not` (or a
/// leading `-`) and parentheses.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Term(Term),
    Not(Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

impl Expr {
    /// Every term in the tree.
    pub fn terms(&self) -> Vec<&Term> {
        match self {
            Expr::Term(t) => vec![t],
            Expr::Not(e) => e.terms(),
            Expr::And(es) | Expr::Or(es) => es.iter().flat_map(|e| e.terms()).collect(),
        }
    }
}

/// A parsed search line. Words are matched case-insensitively as
/// substrings; a space between terms means both must hold, `or` means
/// either, `not x` (or `-x`) means the opposite, and parentheses group:
/// `a b or c` is `(a and b) or c`. Bad input never fails to parse, it
/// just means less (see [`Query::check`] for what was wrong with it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub expr: Option<Expr>,
}

/// A piece of a search line as the tokenizer cuts it.
#[derive(Clone, Debug, PartialEq)]
enum Token {
    Open,
    Close,
    Or,
    And,
    Not,
    /// A word, and whether it was quoted (a quoted `or` is a word).
    Word(String),
}

/// Cut a line into tokens: parentheses stand alone even when stuck to a
/// word, double quotes hold a phrase together (`"epic life"` or
/// `spell:"life magic"`), and a leading `-` on a word is a `not`.
fn tokenize(line: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut chars = line.chars().peekable();
    let flush = |word: &mut String, out: &mut Vec<Token>| {
        if word.is_empty() {
            return;
        }
        let w = std::mem::take(word);
        let lower = w.to_lowercase();
        out.push(match lower.as_str() {
            "or" | "|" | "||" => Token::Or,
            "and" | "&" | "&&" => Token::And,
            "not" | "!" => Token::Not,
            _ => Token::Word(lower),
        });
    };
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => flush(&mut word, &mut out),
            '(' => {
                flush(&mut word, &mut out);
                out.push(Token::Open);
            }
            ')' => {
                flush(&mut word, &mut out);
                out.push(Token::Close);
            }
            '-' | '!' if word.is_empty() && chars.peek().is_some_and(|n| !n.is_whitespace()) => {
                out.push(Token::Not);
            }
            '"' => {
                // A phrase: whatever was typed before the quote (a
                // `spell:` key) stays in front of it.
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                    word.push(c);
                }
                let w = std::mem::take(&mut word).to_lowercase();
                if !w.is_empty() {
                    out.push(Token::Word(w));
                }
            }
            c => word.push(c),
        }
    }
    flush(&mut word, &mut out);
    out
}

/// A recursive-descent parser over the tokens that never fails: a
/// stray `)` is skipped, a missing one closes at the end of the line,
/// an `or` with nothing on one side joins what there is.
struct Parser<'a> {
    tokens: &'a [Token],
    at: usize,
    problems: Vec<String>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn or_expr(&mut self) -> Option<Expr> {
        let mut parts = Vec::new();
        loop {
            if let Some(e) = self.and_expr() {
                parts.push(e);
            }
            if self.peek() == Some(&Token::Or) {
                self.at += 1;
                if self.peek().is_none() || self.peek() == Some(&Token::Close) {
                    self.problems.push("nothing after `or`".into());
                }
                continue;
            }
            break;
        }
        match parts.len() {
            0 => None,
            1 => parts.pop(),
            _ => Some(Expr::Or(parts)),
        }
    }

    fn and_expr(&mut self) -> Option<Expr> {
        let mut parts = Vec::new();
        loop {
            match self.peek() {
                None | Some(Token::Or) | Some(Token::Close) => break,
                Some(Token::And) => {
                    self.at += 1;
                }
                _ => {
                    if let Some(e) = self.unary() {
                        parts.push(e);
                    }
                }
            }
        }
        match parts.len() {
            0 => None,
            1 => parts.pop(),
            _ => Some(Expr::And(parts)),
        }
    }

    fn unary(&mut self) -> Option<Expr> {
        let tok = self.peek()?.clone();
        self.at += 1;
        match tok {
            Token::Not => {
                let inner = self.unary();
                if inner.is_none() {
                    self.problems.push("nothing after `not`".into());
                }
                inner.map(|e| Expr::Not(Box::new(e)))
            }
            Token::Open => {
                let inner = self.or_expr();
                if self.peek() == Some(&Token::Close) {
                    self.at += 1;
                } else {
                    self.problems.push("a `(` without its `)`".into());
                }
                if inner.is_none() {
                    self.problems.push("empty parentheses".into());
                }
                inner
            }
            Token::Close => {
                self.problems.push("a `)` without its `(`".into());
                None
            }
            // `and`/`or` here is a stray one; the loops above eat the
            // meaningful ones.
            Token::And | Token::Or => None,
            Token::Word(w) => {
                let (term, problem) = parse_term(&w);
                if let Some(p) = problem {
                    self.problems.push(p);
                }
                Some(Expr::Term(term))
            }
        }
    }
}

/// One word as a term, and what was doubtful about it (an unknown key
/// still becomes a plain word, so the query keeps working).
fn parse_term(w: &str) -> (Term, Option<String>) {
    match parse_num(w) {
        Ok(Some(t)) => return (t, None),
        Ok(None) => {}
        Err(problem) => return (Term::Word(w.to_string()), Some(problem)),
    }
    if let Some((k, v)) = w.split_once(':') {
        let v = v.to_string();
        if v.is_empty() {
            return (
                Term::Word(w.to_string()),
                Some(format!("nothing after `{k}:`")),
            );
        }
        return match k {
            "spell" | "spells" => (Term::Spell(v), None),
            "type" | "kind" | "is" => (Term::Kind(v), None),
            "mat" | "material" => (Term::Material(v), None),
            "skill" => (Term::Skill(v), None),
            "slot" | "wear" => match slot_mask(&v) {
                Some(m) => (Term::Slot(m), None),
                None => (
                    Term::Word(w.to_string()),
                    Some(format!(
                        "unknown slot `{v}` (try {})",
                        SLOTS.iter().map(|(w, _)| *w).collect::<Vec<_>>().join(", ")
                    )),
                ),
            },
            "tier" | "cantrip" => match Tier::parse(&v) {
                Some(t) => (Term::Tier(t), None),
                None => (
                    Term::Word(w.to_string()),
                    Some(format!(
                        "unknown tier `{v}` (minor, moderate, major, epic, legendary)"
                    )),
                ),
            },
            _ => (
                Term::Word(w.to_string()),
                Some(format!("unknown key `{k}:`")),
            ),
        };
    }
    (
        match w {
            "wielded" | "worn" => Term::Wielded,
            "unappraised" => Term::Unappraised,
            _ => Term::Word(w.to_string()),
        },
        None,
    )
}

impl Parser<'_> {
    /// The whole line. Anything left over after the first expression (a
    /// stray `)`) is skipped and what follows it joined on with `and`,
    /// so nothing typed is silently dropped.
    fn line(&mut self) -> Option<Expr> {
        let mut expr = self.or_expr();
        while self.at < self.tokens.len() {
            if self.peek() == Some(&Token::Close) {
                self.problems.push("a `)` without its `(`".into());
                self.at += 1;
            }
            if let Some(more) = self.or_expr() {
                expr = Some(match expr {
                    Some(Expr::And(mut all)) => {
                        all.push(more);
                        Expr::And(all)
                    }
                    Some(e) => Expr::And(vec![e, more]),
                    None => more,
                });
            } else if self.peek() != Some(&Token::Close) {
                self.at += 1;
            }
        }
        expr
    }
}

impl Query {
    pub fn parse(line: &str) -> Query {
        let tokens = tokenize(line);
        let mut p = Parser {
            tokens: &tokens,
            at: 0,
            problems: Vec::new(),
        };
        Query { expr: p.line() }
    }

    /// What is wrong with a line, for a rule editor to show: unbalanced
    /// parentheses, an unknown key (`foo:bar`, `x<3`), a dangling `or`.
    /// The line still parses (see [`Query::parse`]); this only says
    /// whether it means what was typed.
    pub fn check(line: &str) -> Result<(), String> {
        let tokens = tokenize(line);
        let mut p = Parser {
            tokens: &tokens,
            at: 0,
            problems: Vec::new(),
        };
        let _ = p.line();
        match p.problems.first() {
            None => Ok(()),
            Some(first) => Err(first.clone()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.expr.is_none()
    }

    /// Every term of the query, whatever joins them.
    pub fn terms(&self) -> Vec<&Term> {
        self.expr.as_ref().map(|e| e.terms()).unwrap_or_default()
    }

    /// Whether any term needs the items to have been appraised.
    pub fn needs_appraisal(&self) -> bool {
        self.terms().iter().any(|t| match t {
            Term::Num(k, _, _) => k.needs_appraisal(),
            Term::Spell(_) | Term::Skill(_) | Term::Tier(_) => true,
            _ => false,
        })
    }
}

/// `dmg>10` as a term: `Ok(None)` when the word has no comparison in
/// it, `Err` when it has one but the key or the number is not right.
fn parse_num(w: &str) -> Result<Option<Term>, String> {
    for (sym, op) in [
        (">=", Op::Ge),
        ("<=", Op::Le),
        (">", Op::Gt),
        ("<", Op::Lt),
        ("=", Op::Eq),
    ] {
        if let Some((k, v)) = w.split_once(sym) {
            let Some(key) = NumKey::parse(k) else {
                return Err(format!("unknown number `{k}`"));
            };
            let Ok(v) = v.parse::<f64>() else {
                return Err(format!("`{v}` is not a number"));
            };
            return Ok(Some(Term::Num(key, op, v)));
        }
    }
    Ok(None)
}

/// The language in a few lines, for a help popover.
pub const HELP: &[(&str, &str)] = &[
    ("word", "name, material, kind, spell or slot contains it"),
    ("\"epic life magic\"", "a phrase, matched whole"),
    ("a b", "both must hold"),
    ("a or b", "either; a b or c is (a and b) or c"),
    ("not a, -a", "the opposite"),
    ("(a or b) c", "parentheses group"),
    ("value>250, al>=200, dmg>10", "numbers: value, burden, ws, dmg, al, speed, wield, mana, sc, uses, tinks, stack, atk, def"),
    ("epics>=2, legendaries>=1, majors, minors, cantrips, spells>=3", "how many cantrips of a tier (or spells at all) are on it"),
    ("tier:epic", "at least one cantrip of the tier"),
    ("slot:ring", "where it is worn: head, chest, abdomen, arms, hands, legs, feet, neck, bracelet, ring, trinket, cloak, sigil, melee, shield, missile, ammo, wand, twohanded"),
    ("spell:blood, type:armor, mat:iron, skill:sword", "one field"),
    ("wielded, unappraised", "flags"),
];

/// The order of a sorted list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Num(NumKey),
}

/// Sort items by a key; items lacking the number go last, ties by name.
pub fn sort(items: &mut [ItemStats], key: SortKey, descending: bool) {
    items.sort_by(|a, b| {
        let ord = match key {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Num(k) => match (a.number(k), b.number(k)) {
                (Some(x), Some(y)) => {
                    let o = x.total_cmp(&y);
                    if descending {
                        o.reverse()
                    } else {
                        o
                    }
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            },
        };
        let ord = if descending && key == SortKey::Name {
            ord.reverse()
        } else {
            ord
        };
        ord.then_with(|| a.name.cmp(&b.name))
    });
}

impl Client {
    /// Every carried and worn item as [`ItemStats`], appraised where the
    /// server has answered.
    pub fn item_stats(&self) -> Vec<ItemStats> {
        let me = self.world.player_guid;
        let skills = self.assets.skill_table().ok();
        let spells = self.assets.spell_table().ok();
        let skill_name = |id: u32| {
            skills
                .as_ref()
                .and_then(|t| t.get(id).map(|s| s.name.clone()))
                .unwrap_or_else(|| format!("skill {id}"))
        };
        let spell_name = |id: u32| {
            spells
                .as_ref()
                .and_then(|t| t.get(id).map(|s| s.name.clone()))
                .or_else(|| self.known_spells.get(&id).cloned())
                .unwrap_or_else(|| format!("spell {id}"))
        };
        self.world
            .wielded()
            .chain(self.world.inventory())
            .map(|o| {
                let s = ItemStats::of(o, me);
                match self.appraisals.get(&o.guid) {
                    Some(a) => s.with_appraisal(a, &skill_name, &spell_name),
                    None => s,
                }
            })
            .collect()
    }

    /// Carried items matching a search line (see [`Query`]).
    pub fn find_items(&self, line: &str) -> Vec<ItemStats> {
        let q = Query::parse(line);
        self.item_stats()
            .into_iter()
            .filter(|s| s.matches(&q))
            .collect()
    }

    /// Ask the server about every carried item we have not appraised,
    /// one at a time (see `tick_appraise`). Returns how many were queued.
    pub fn appraise_all(&mut self) -> usize {
        let todo: Vec<u32> = self
            .world
            .wielded()
            .chain(self.world.inventory())
            .map(|o| o.guid)
            .filter(|g| {
                !self.appraisals.contains_key(g)
                    && !self.appraise_queue.contains(g)
                    && self.appraise_inflight.map(|(i, _)| i) != Some(*g)
            })
            .collect();
        let n = todo.len();
        self.appraise_queue.extend(todo);
        n
    }

    /// How many carried items still lack an appraisal.
    pub fn unappraised_count(&self) -> usize {
        self.world
            .wielded()
            .chain(self.world.inventory())
            .filter(|o| !self.appraisals.contains_key(&o.guid))
            .count()
    }

    /// Send the next queued appraisal once the previous one has answered
    /// (or gone stale).
    pub fn tick_appraise(&mut self) {
        if let Some((guid, since)) = self.appraise_inflight {
            if self.appraisals.contains_key(&guid)
                || since.elapsed() > std::time::Duration::from_secs(2)
            {
                self.appraise_inflight = None;
            }
        }
        if self.appraise_inflight.is_none() {
            while let Some(guid) = self.appraise_queue.pop_front() {
                if self.appraisals.contains_key(&guid) || !self.world.objects.contains_key(&guid) {
                    continue;
                }
                self.appraise(guid);
                self.appraise_inflight = Some((guid, std::time::Instant::now()));
                break;
            }
        }
    }
}

impl ItemStats {
    /// From a vendor's stock description (a `WeenieDesc` with no world
    /// object behind it): name, kind, value, burden, material.
    pub fn of_desc(guid: u32, d: &ac_world::object::WeenieDesc) -> Self {
        ItemStats {
            guid,
            name: d.name.clone(),
            wcid: d.weenie_class_id,
            item_type: d.item_type,
            kind: kind_name(d.item_type),
            stack: d.stack_size.max(1),
            container: d.container.unwrap_or(0),
            value: d.value,
            burden: d.burden,
            workmanship: d.workmanship,
            ammo_type: d.ammo_type,
            combat_use: d.combat_use,
            material: if d.material != 0 {
                ac_world::material::name(d.material)
            } else {
                ""
            },
            structure: d.structure,
            max_structure: d.max_structure,
            ..Default::default()
        }
        .with_launcher_guess()
    }
}

impl Client {
    /// The stats of any object we know about: a carried item, a loot item
    /// in the open corpse or chest, a vendor's stock line, something on
    /// the ground or offered in a trade. Appraised where the cache has
    /// the answer; before that the object description's own spell (a
    /// scroll's, a wand's) still shows. `None` for an unknown guid.
    pub fn stats_of(&self, guid: u32) -> Option<ItemStats> {
        let me = self.world.player_guid;
        let (mut stats, own_spell) = match self.world.objects.get(&guid) {
            Some(o) => (ItemStats::of(o, me), o.spell_id),
            None => {
                let it = self
                    .world
                    .open_vendor
                    .as_ref()?
                    .items
                    .iter()
                    .find(|it| it.guid == guid)?;
                (ItemStats::of_desc(guid, &it.desc), it.desc.spell_id)
            }
        };
        let skills = self.assets.skill_table().ok();
        let spells = self.assets.spell_table().ok();
        let skill_name = |id: u32| {
            skills
                .as_ref()
                .and_then(|t| t.get(id).map(|s| s.name.clone()))
                .unwrap_or_else(|| format!("skill {id}"))
        };
        let spell_name = |id: u32| {
            spells
                .as_ref()
                .and_then(|t| t.get(id).map(|s| s.name.clone()))
                .or_else(|| self.known_spells.get(&id).cloned())
                .unwrap_or_else(|| format!("spell {id}"))
        };
        match self.appraisals.get(&guid) {
            Some(a) => stats = stats.with_appraisal(a, &skill_name, &spell_name),
            None if own_spell != 0 => stats.spells.push(spell_name(own_spell)),
            None => {}
        }
        Some(stats)
    }

    /// Queue background appraisals (see `tick_appraise`) for the given
    /// objects that are not appraised, queued or in flight: the items of
    /// an open corpse, a trade offer, anything in `world.objects`. Guids
    /// the world does not hold (a vendor's stock lines, which the queue
    /// cannot serve; a click on one appraises it) are skipped. Returns
    /// how many were queued.
    pub fn appraise_many(&mut self, guids: impl IntoIterator<Item = u32>) -> usize {
        let mut n = 0;
        for g in guids {
            let known = self.world.objects.contains_key(&g);
            let pending = self.appraisals.contains_key(&g)
                || self.appraise_queue.contains(&g)
                || self.appraise_inflight.map(|(i, _)| i) == Some(g);
            if known && !pending {
                self.appraise_queue.push_back(g);
                n += 1;
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sword() -> ItemStats {
        ItemStats {
            guid: 1,
            name: "Fine Sword".into(),
            kind: "weapon",
            item_type: item_type::MELEE_WEAPON,
            appraised: true,
            damage_low: 8,
            damage_high: 14,
            damage_type: "Slashing".into(),
            speed: 40,
            weapon_skill: "Sword".into(),
            attack_bonus: 1.05,
            defense_bonus: 1.0,
            spells: vec!["Blood Drinker IV".into(), "Heart Seeker III".into()],
            wield_skill: "Sword".into(),
            wield_level: 250,
            value: 1200,
            burden: 300,
            material: "Iron",
            workmanship: 6.0,
            ..Default::default()
        }
    }

    fn tunic() -> ItemStats {
        ItemStats {
            guid: 2,
            name: "Leather Tunic".into(),
            kind: "armor",
            item_type: item_type::ARMOR,
            appraised: true,
            armor_level: 120,
            value: 300,
            burden: 500,
            wielded: true,
            ..Default::default()
        }
    }

    fn unknown() -> ItemStats {
        ItemStats {
            guid: 3,
            name: "Mystery Wand".into(),
            kind: "caster",
            item_type: item_type::CASTER,
            value: 50,
            burden: 20,
            ..Default::default()
        }
    }

    #[test]
    fn parses_terms() {
        let q = Query::parse("Sword dmg>10 al>=100 spell:blood type:armor mat:iron wielded x<3");
        assert_eq!(
            q.terms().into_iter().cloned().collect::<Vec<_>>(),
            vec![
                Term::Word("sword".into()),
                Term::Num(NumKey::Damage, Op::Gt, 10.0),
                Term::Num(NumKey::Armor, Op::Ge, 100.0),
                Term::Spell("blood".into()),
                Term::Kind("armor".into()),
                Term::Material("iron".into()),
                Term::Wielded,
                // An unknown key is just a word.
                Term::Word("x<3".into()),
            ]
        );
        assert!(matches!(q.expr, Some(Expr::And(_))));
        assert!(q.needs_appraisal());
        assert!(!Query::parse("sword value>10").needs_appraisal());
        assert!(Query::parse("").is_empty());
        assert!(Query::parse("   ").is_empty());
        assert!(Query::parse("()").is_empty());
    }

    #[test]
    fn parses_or_not_and_parentheses() {
        let word = |w: &str| Expr::Term(Term::Word(w.into()));
        // A space is `and`, and binds tighter than `or`.
        assert_eq!(
            Query::parse("a b or c").expr,
            Some(Expr::Or(vec![
                Expr::And(vec![word("a"), word("b")]),
                word("c")
            ]))
        );
        assert_eq!(
            Query::parse("a (b or c)").expr,
            Some(Expr::And(vec![
                word("a"),
                Expr::Or(vec![word("b"), word("c")])
            ]))
        );
        // Parentheses stuck to a word still count.
        assert_eq!(
            Query::parse("(b or c) a").expr,
            Some(Expr::And(vec![
                Expr::Or(vec![word("b"), word("c")]),
                word("a")
            ]))
        );
        assert_eq!(
            Query::parse("not a").expr,
            Some(Expr::Not(Box::new(word("a"))))
        );
        assert_eq!(
            Query::parse("-a").expr,
            Some(Expr::Not(Box::new(word("a"))))
        );
        assert_eq!(
            Query::parse("a and -b").expr,
            Some(Expr::And(vec![word("a"), Expr::Not(Box::new(word("b")))]))
        );
        // A quoted phrase is one word, alone or after a key.
        assert_eq!(
            Query::parse("\"epic life magic\" ring").expr,
            Some(Expr::And(vec![word("epic life magic"), word("ring")]))
        );
        assert_eq!(
            Query::parse("spell:\"life magic\"").expr,
            Some(Expr::Term(Term::Spell("life magic".into())))
        );
        // Bad input still means something.
        assert_eq!(Query::parse("a or").expr, Some(word("a")));
        assert_eq!(Query::parse("or a").expr, Some(word("a")));
        assert_eq!(Query::parse("a)").expr, Some(word("a")));
        assert_eq!(
            Query::parse("(a b").expr,
            Some(Expr::And(vec![word("a"), word("b")]))
        );
        assert_eq!(Query::parse("not").expr, None);
        // The new keys.
        assert_eq!(
            Query::parse("slot:ring epics>=2 tier:epic spells<4")
                .terms()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec![
                Term::Slot(slot::FINGER),
                Term::Num(NumKey::Epics, Op::Ge, 2.0),
                Term::Tier(Tier::Epic),
                Term::Num(NumKey::Spells, Op::Lt, 4.0),
            ]
        );
        assert!(Query::parse("epics>=2").needs_appraisal());
        assert!(Query::parse("tier:epic").needs_appraisal());
        assert!(!Query::parse("slot:ring").needs_appraisal());
    }

    #[test]
    fn check_reports_what_is_wrong() {
        assert_eq!(Query::check("sword dmg>10 (a or b) -c"), Ok(()));
        assert_eq!(Query::check("\"epic life\" slot:ring epics>=2"), Ok(()));
        assert_eq!(Query::check(""), Ok(()));
        assert!(Query::check("(a or b").unwrap_err().contains("`(`"));
        assert!(Query::check("a or b)").unwrap_err().contains("`)`"));
        assert!(Query::check("x<3")
            .unwrap_err()
            .contains("unknown number `x`"));
        assert!(Query::check("dmg>ten")
            .unwrap_err()
            .contains("not a number"));
        assert!(Query::check("foo:bar")
            .unwrap_err()
            .contains("unknown key `foo:`"));
        assert!(Query::check("slot:tail")
            .unwrap_err()
            .contains("unknown slot"));
        assert!(Query::check("tier:huge")
            .unwrap_err()
            .contains("unknown tier"));
        assert!(Query::check("a or").unwrap_err().contains("`or`"));
        assert!(Query::check("a not").unwrap_err().contains("`not`"));
        assert!(Query::check("()").unwrap_err().contains("empty"));
        assert!(Query::check("spell:")
            .unwrap_err()
            .contains("nothing after"));
    }

    /// A ring, a bracelet and a hauberk with cantrips on them.
    fn ring(spells: &[&str]) -> ItemStats {
        ItemStats {
            guid: 10,
            name: "Ornate Ring".into(),
            kind: "jewelry",
            item_type: item_type::JEWELRY,
            valid_locations: slot::FINGER,
            appraised: true,
            spells: spells.iter().map(|s| s.to_string()).collect(),
            value: 4000,
            ..Default::default()
        }
    }

    fn hauberk(spells: &[&str]) -> ItemStats {
        ItemStats {
            guid: 11,
            name: "Chainmail Hauberk".into(),
            kind: "armor",
            item_type: item_type::ARMOR,
            valid_locations: slot::CHEST_ARMOR | slot::ABDOMEN_ARMOR | slot::UPPER_ARM_ARMOR,
            appraised: true,
            armor_level: 300,
            spells: spells.iter().map(|s| s.to_string()).collect(),
            value: 9000,
            ..Default::default()
        }
    }

    #[test]
    fn cantrip_tiers_are_read_off_spell_names() {
        assert_eq!(Tier::of_spell("Epic Strength"), Some(Tier::Epic));
        assert_eq!(
            Tier::of_spell("legendary life magic aptitude"),
            Some(Tier::Legendary)
        );
        assert_eq!(Tier::of_spell("Minor Coordination"), Some(Tier::Minor));
        assert_eq!(Tier::of_spell("Moderate Focus"), Some(Tier::Moderate));
        assert_eq!(Tier::of_spell("Major Impregnability"), Some(Tier::Major));
        // Only at the front, and only as a whole word.
        assert_eq!(Tier::of_spell("Blood Drinker IV"), None);
        assert_eq!(Tier::of_spell("Epicurean's Delight"), None);
        assert_eq!(Tier::of_spell("Strength of the Epic"), None);
        let r = ring(&[
            "Epic Strength",
            "Epic Coordination",
            "Major Focus",
            "Blood Drinker IV",
        ]);
        assert_eq!(r.cantrips(Tier::Epic), 2);
        assert_eq!(r.number(NumKey::Cantrips), Some(3.0));
        assert_eq!(r.number(NumKey::Spells), Some(4.0));
        assert_eq!(r.number(NumKey::Legendaries), Some(0.0));
        // Not appraised, no spells known: the counts are unknown, not 0.
        let mut blank = ring(&[]);
        blank.appraised = false;
        assert_eq!(blank.number(NumKey::Epics), None);
        assert!(!blank.matches(&Query::parse("epics<1")));
        assert!(ring(&[]).matches(&Query::parse("epics<1")));
    }

    #[test]
    fn the_users_three_examples() {
        let h = hauberk(&[
            "Epic Life Magic Aptitude",
            "Major Armor Self",
            "Epic Impregnability",
        ]);
        let plain = hauberk(&["Minor Strength"]);
        // "a Hauberk with Epic Life Mastery" (however the cantrip is
        // spelt, a phrase finds it).
        let q = Query::parse("hauberk \"epic life\"");
        assert!(h.matches(&q));
        assert!(!plain.matches(&q));
        assert!(h.matches(&Query::parse("hauberk spell:\"epic life magic\"")));
        assert!(h.matches(&Query::parse("slot:chest tier:epic \"life magic\"")));
        // "any Ring with multiple epics".
        let two = ring(&["Epic Strength", "Epic Coordination", "Minor Focus"]);
        let one = ring(&["Epic Strength", "Major Coordination"]);
        let q = Query::parse("slot:ring epics>=2");
        assert!(two.matches(&q));
        assert!(!one.matches(&q));
        assert!(!h.matches(&q), "a hauberk is not a ring");
        assert!(
            two.matches(&Query::parse("ring epics>1")),
            "a bare slot word"
        );
        // "more than 2 epics".
        let three = ring(&["Epic Strength", "Epic Coordination", "Epic Focus"]);
        let q = Query::parse("epics>2");
        assert!(three.matches(&q));
        assert!(!two.matches(&q));
        assert!(three.matches(&Query::parse("epics>=3 or legendaries>=1")));
        // Structure round the examples.
        let q = Query::parse("(slot:ring or slot:bracelet) epics>=2 not minors>0");
        assert!(two.matches(&Query::parse("(slot:ring or slot:bracelet) epics>=2")));
        assert!(!two.matches(&q), "it has a minor on it");
        assert!(three.matches(&q));
        assert!(h.matches(&Query::parse("slot:chest or slot:ring")));
        assert!(!h.matches(&Query::parse("-hauberk")));
        assert!(h.matches(&Query::parse("armor -slot:ring")));
        // Slot words fit any of a slot's bits.
        assert!(h.matches(&Query::parse("slot:abdomen")));
        assert!(!h.matches(&Query::parse("slot:legs")));
        assert!(!h.matches(&Query::parse("slot:hands")));
        assert_eq!(slot_mask("bracelet"), Some(slot::WRIST));
        assert_eq!(slot_mask("tail"), None);
    }

    #[test]
    fn matches_words_numbers_and_spells() {
        let s = sword();
        assert!(s.matches(&Query::parse("sword")));
        assert!(s.matches(&Query::parse("FINE")));
        assert!(s.matches(&Query::parse("iron")));
        assert!(s.matches(&Query::parse("blood")));
        assert!(s.matches(&Query::parse("spell:heart")));
        assert!(s.matches(&Query::parse("dmg>10 dmg<=14")));
        assert!(!s.matches(&Query::parse("dmg>14")));
        assert!(s.matches(&Query::parse("type:weapon skill:sword wield<=250")));
        assert!(s.matches(&Query::parse("atk>=5")));
        assert!(!s.matches(&Query::parse("wielded")));
        assert!(tunic().matches(&Query::parse("wielded al=120")));
        // Numbers the item does not have never match.
        assert!(!tunic().matches(&Query::parse("dmg>0")));
        assert!(!unknown().matches(&Query::parse("dmg>0")));
        assert!(unknown().matches(&Query::parse("unappraised value<100")));
    }

    #[test]
    fn sorts_missing_numbers_last() {
        let mut items = vec![unknown(), sword(), tunic()];
        sort(&mut items, SortKey::Num(NumKey::Value), true);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Fine Sword", "Leather Tunic", "Mystery Wand"]);
        sort(&mut items, SortKey::Num(NumKey::Damage), false);
        assert_eq!(items[0].name, "Fine Sword");
        sort(&mut items, SortKey::Name, false);
        assert_eq!(items[0].name, "Fine Sword");
        assert_eq!(items[2].name, "Mystery Wand");
    }

    #[test]
    fn summary_lines() {
        let lines = sword().summary();
        assert_eq!(lines[0], "Damage 8-14 Slashing (speed 40)");
        assert!(lines
            .iter()
            .any(|l| l == "Spells: Blood Drinker IV, Heart Seeker III"));
        assert!(lines.iter().any(|l| l == "Requires Sword 250"));
        assert!(lines.iter().any(|l| l == "Workmanship 6 Iron"));
        assert!(unknown().summary().contains(&"(not appraised)".to_string()));
        assert_eq!(kind_name(item_type::ARMOR | item_type::CLOTHING), "armor");
        assert_eq!(kind_name(item_type::MISC), "misc");
        assert_eq!(kind_name(0), "misc");
        // The bits that used to be mislabelled: a book is Writable, a
        // healing kit is Misc, 0x10000 is Portal (not a healer) and
        // 0x20000 is Lockable (a chest, not a lockpick).
        assert_eq!(kind_name(item_type::WRITABLE), "writable");
        assert_eq!(kind_name(item_type::PORTAL), "portal");
        assert_eq!(kind_name(item_type::SPELL_COMPONENTS), "comps");
        assert_eq!(kind_name(item_type::TINKERING_MATERIAL), "salvage");
        assert_eq!(kind_name(item_type::CREATURE | item_type::LOCKABLE), "misc");
    }

    #[test]
    fn a_launcher_is_told_by_the_header_or_guessed_from_its_name() {
        use ac_world::fletching::{ammo_type, combat_use};
        let mut bow = ac_world::object::WeenieDesc {
            name: "Longbow".into(),
            item_type: item_type::MISSILE_WEAPON,
            ..Default::default()
        };
        // The header said what it shoots.
        bow.ammo_type = ammo_type::ARROW;
        bow.combat_use = combat_use::LAUNCHER;
        let s = ItemStats::of_desc(1, &bow);
        assert_eq!(
            (s.ammo_type, s.combat_use),
            (ammo_type::ARROW, combat_use::LAUNCHER)
        );
        assert!(crate::weapons::is_launcher(&s));
        // The header said nothing (ACE): the name decides.
        bow.ammo_type = 0;
        bow.combat_use = 0;
        let s = ItemStats::of_desc(1, &bow);
        assert_eq!(
            (s.ammo_type, s.combat_use),
            (ammo_type::ARROW, combat_use::LAUNCHER)
        );
        assert!(crate::weapons::is_launcher(&s));
        // Ammunition is never a launcher, whatever its name.
        let arrows = ac_world::object::WeenieDesc {
            name: "Arrow".into(),
            item_type: item_type::MISSILE_WEAPON,
            combat_use: combat_use::AMMO,
            ..Default::default()
        };
        let s = ItemStats::of_desc(2, &arrows);
        assert_eq!(s.ammo_type, 0);
        assert!(!crate::weapons::is_launcher(&s));
    }

    #[test]
    fn stats_of_desc_reads_the_description() {
        let d = ac_world::object::WeenieDesc {
            name: "Dagger".into(),
            weenie_class_id: 21,
            icon_id: 0x0600_1234,
            item_type: item_type::MELEE_WEAPON,
            object_desc_flags: 0,
            weenie_flags: 0,
            value: 15,
            stack_size: 0,
            container: None,
            wielder: None,
            valid_locations: 0,
            wielded_location: 0,
            icon_overlay: 0,
            icon_underlay: 0,
            spell_id: 0,
            material: 0,
            workmanship: 0.0,
            ammo_type: 0,
            combat_use: 0,
            structure: 0,
            max_structure: 0,
            max_stack_size: 1,
            usable: 0,
            burden: 60,
            items_capacity: 0,
            containers_capacity: 0,
        };
        let s = ItemStats::of_desc(0x100, &d);
        assert_eq!(s.guid, 0x100);
        assert_eq!(s.name, "Dagger");
        assert_eq!(s.kind, "weapon");
        assert_eq!(s.stack, 1);
        assert_eq!((s.value, s.burden), (15, 60));
        assert!(!s.appraised);
        assert!(s.matches(&Query::parse("type:weapon value<20")));
        assert!(!s.matches(&Query::parse("dmg>0")));
    }
}
