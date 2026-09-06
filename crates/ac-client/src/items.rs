//! Carried items as searchable records: what an item is from its object
//! description, plus its numbers once it has been appraised (damage,
//! armor level, spells, wield requirement...). [`Query`] parses an
//! inventory search line ("dmg>10 spell:blood type:weapon") and
//! [`ItemStats::matches`] tests an item against it.

use crate::Client;
use ac_net::messages::Appraisal;
use ac_world::{item_type, WorldObject};

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
        (item_type::LOCKPICK, "lockpick"),
        (item_type::HEALER, "healer"),
        (item_type::MANA_STONE, "manastone"),
        (0x80, "comps"),
        (0x400, "scroll"),
        (0x2000, "portal"),
        (0x40000, "salvage"),
        (0x200000, "trinket"),
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
    pub speed: u32,
    pub weapon_skill: String,
    pub attack_bonus: f64,
    pub defense_bonus: f64,
    pub armor_level: u32,
    pub shield: u32,
    /// Spell names on the item (cast on use, or on wield).
    pub spells: Vec<String>,
    /// Skill and level needed to wield it.
    pub wield_skill: String,
    pub wield_level: u32,
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
            ..Default::default()
        }
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
            self.speed = w.speed;
            self.weapon_skill = skill_name(w.skill);
            self.attack_bonus = w.offense;
            self.defense_bonus = a.float(29).unwrap_or(1.0);
        }
        if let (Some(skill), Some(level)) = (a.int(159), a.int(160)) {
            self.wield_skill = skill_name(skill as u32);
            self.wield_level = level.max(0) as u32;
        }
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
        self.tinks = a.int(171).unwrap_or(0).max(0) as u32;
        self.bonded = a.int(33).unwrap_or(0) != 0;
        self.attuned = a.int(114).unwrap_or(0) != 0;
        self.spells = a.spells.iter().map(|s| spell_name(*s)).collect();
        self
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
        }
    }

    /// Whether the item matches every term of the query.
    pub fn matches(&self, q: &Query) -> bool {
        q.terms.iter().all(|t| self.matches_term(t))
    }

    fn matches_term(&self, t: &Term) -> bool {
        let has = |hay: &str, needle: &str| hay.to_lowercase().contains(needle);
        match t {
            Term::Word(w) => {
                has(&self.name, w)
                    || has(self.material, w)
                    || has(self.kind, w)
                    || self.spells.iter().any(|s| has(s, w))
            }
            Term::Spell(w) => self.spells.iter().any(|s| has(s, w)),
            Term::Kind(w) => self.kind == w || (w == "weapon" && self.damage_high > 0),
            Term::Material(w) => has(self.material, w),
            Term::Skill(w) => has(&self.weapon_skill, w) || has(&self.wield_skill, w),
            Term::Wielded => self.wielded,
            Term::Unappraised => !self.appraised,
            Term::Num(key, op, v) => self.number(*key).is_some_and(|x| op.test(x, *v)),
        }
    }
}

/// Numeric fields a query can compare and a list can sort by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
        }
    }

    /// Whether the field only exists once the item is appraised.
    pub fn needs_appraisal(self) -> bool {
        !matches!(self, NumKey::Value | NumKey::Burden | NumKey::Stack)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

impl Op {
    fn test(self, x: f64, v: f64) -> bool {
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
#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    /// Matches the name, material, kind or a spell name.
    Word(String),
    /// `spell:blood`
    Spell(String),
    /// `type:armor`
    Kind(String),
    /// `mat:iron`
    Material(String),
    /// `skill:sword` (the weapon's skill or the wield requirement)
    Skill(String),
    /// `wielded`
    Wielded,
    /// `unappraised`
    Unappraised,
    /// `dmg>10`, `al>=200`, `value<100`
    Num(NumKey, Op, f64),
}

/// A parsed search line. Words are matched case-insensitively as
/// substrings; every term must hold.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub terms: Vec<Term>,
}

impl Query {
    pub fn parse(line: &str) -> Query {
        let mut terms = Vec::new();
        for raw in line.split_whitespace() {
            let w = raw.to_lowercase();
            if let Some(t) = parse_num(&w) {
                terms.push(t);
                continue;
            }
            if let Some((k, v)) = w.split_once(':') {
                if v.is_empty() {
                    continue;
                }
                let v = v.to_string();
                terms.push(match k {
                    "spell" | "spells" => Term::Spell(v),
                    "type" | "kind" | "is" => Term::Kind(v),
                    "mat" | "material" => Term::Material(v),
                    "skill" => Term::Skill(v),
                    _ => Term::Word(w.clone()),
                });
                continue;
            }
            terms.push(match w.as_str() {
                "wielded" | "worn" => Term::Wielded,
                "unappraised" => Term::Unappraised,
                _ => Term::Word(w),
            });
        }
        Query { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Whether any term needs the items to have been appraised.
    pub fn needs_appraisal(&self) -> bool {
        self.terms.iter().any(|t| match t {
            Term::Num(k, _, _) => k.needs_appraisal(),
            Term::Spell(_) | Term::Skill(_) => true,
            _ => false,
        })
    }
}

fn parse_num(w: &str) -> Option<Term> {
    for (sym, op) in [
        (">=", Op::Ge),
        ("<=", Op::Le),
        (">", Op::Gt),
        ("<", Op::Lt),
        ("=", Op::Eq),
    ] {
        if let Some((k, v)) = w.split_once(sym) {
            let key = NumKey::parse(k)?;
            let v: f64 = v.parse().ok()?;
            return Some(Term::Num(key, op, v));
        }
    }
    None
}

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
            q.terms,
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
        assert!(q.needs_appraisal());
        assert!(!Query::parse("sword value>10").needs_appraisal());
        assert!(Query::parse("").is_empty());
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
        assert_eq!(kind_name(0x80), "comps");
        assert_eq!(kind_name(0), "misc");
    }
}
