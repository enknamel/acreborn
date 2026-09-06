//! Playing the character on its own: keeping it alive, keeping its buffs
//! up, fighting what it is told to fight and taking the loot worth
//! taking.
//!
//! This is a set of rules, not a script: [`Config`] says what to do and
//! [`Client::tick_autoplay`] does the highest-priority thing that needs
//! doing this moment. In order:
//!
//! 1. **Stay alive**: below a fraction of health, use a healing kit or
//!    cast a healing spell; below a lower fraction, break off the fight.
//! 2. **Loot**: a corpse of something we killed is opened, the items
//!    that pass the filters are taken, and it is closed again.
//! 3. **Fight**: pick the nearest creature that passes the name rules
//!    and attack it.
//! 4. **Buff**: out of combat, recast anything that has run out or is
//!    about to.
//!
//! Loot is filtered with the inventory's own search language
//! (`crate::items::Query`), so a rule reads `value>500`,
//! `type:armor al>=200` or `spell:blood`. Items are appraised first when
//! a rule needs numbers.
//!
//! Nothing here talks to the UI: the panel edits a [`Config`] and reads
//! [`Autoplay::status`].

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::items::Query;
use crate::Client;

/// How long to wait for a corpse to open before giving up.
const LOOT_TIMEOUT: Duration = Duration::from_secs(6);
/// Least time between two heals, so one is not spammed.
const HEAL_EVERY: Duration = Duration::from_millis(2500);
/// Least time between two casts of the same buff.
const BUFF_EVERY: Duration = Duration::from_millis(1500);
/// Least time between two attack orders.
const ATTACK_EVERY: Duration = Duration::from_millis(1200);

/// Staying alive.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Survive {
    /// Heal when health falls below this fraction of its maximum.
    pub heal_below: f32,
    /// Stop fighting below this fraction (0 to keep fighting).
    pub flee_below: f32,
    /// Use a carried healing kit.
    pub use_kits: bool,
    /// Cast this spell to heal (by name, "Heal Self"); empty for none.
    pub heal_spell: String,
}

impl Default for Survive {
    fn default() -> Self {
        Survive {
            heal_below: 0.6,
            flee_below: 0.25,
            use_kits: true,
            heal_spell: "Heal Self".into(),
        }
    }
}

/// Buffs to keep up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Buffs {
    /// Spell names to keep on the character.
    pub spells: Vec<String>,
    /// Recast when this many seconds or fewer are left.
    pub recast_within: f32,
    /// Only buff out of combat.
    pub out_of_combat_only: bool,
}

impl Default for Buffs {
    fn default() -> Self {
        Buffs {
            spells: Vec::new(),
            recast_within: 30.0,
            out_of_combat_only: true,
        }
    }
}

/// What to fight.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fight {
    pub enabled: bool,
    /// Only attack creatures whose name contains one of these; empty
    /// means anything that can be attacked.
    pub only: Vec<String>,
    /// Never attack creatures whose name contains one of these.
    pub avoid: Vec<String>,
    /// Farthest creature to pick, metres.
    pub radius: f32,
}

impl Default for Fight {
    fn default() -> Self {
        Fight {
            enabled: true,
            only: Vec::new(),
            avoid: Vec::new(),
            radius: 25.0,
        }
    }
}

/// What loot is worth taking.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Loot {
    pub enabled: bool,
    /// Take an item that matches any of these searches (the inventory's
    /// language: `value>500`, `type:armor al>=200`, `spell:blood`).
    pub filters: Vec<String>,
    /// Always take these, whatever the filters say (by name).
    pub always: Vec<String>,
    /// Never take these (by name), even when a filter matches.
    pub never: Vec<String>,
    /// Ask the server about the corpse's items before deciding, so that
    /// filters on damage, armour and spells can be judged.
    pub appraise: bool,
}

impl Default for Loot {
    fn default() -> Self {
        Loot {
            enabled: true,
            filters: vec!["value>250".into()],
            always: vec!["Pyreal".into()],
            never: Vec::new(),
            appraise: true,
        }
    }
}

/// Everything the character does on its own.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    pub survive: Survive,
    pub buffs: Buffs,
    pub fight: Fight,
    pub loot: Loot,
}

/// Whether `name` contains any of `list`, case-insensitively. An empty
/// list matches nothing.
pub fn name_matches(name: &str, list: &[String]) -> bool {
    let name = name.to_lowercase();
    list.iter()
        .any(|w| !w.trim().is_empty() && name.contains(&w.trim().to_lowercase()))
}

/// Whether a creature called `name` is one to fight.
pub fn wanted_target(name: &str, f: &Fight) -> bool {
    if name_matches(name, &f.avoid) {
        return false;
    }
    f.only.iter().all(|w| w.trim().is_empty()) || name_matches(name, &f.only)
}

/// Whether an item is worth taking.
pub fn wanted_loot(stats: &crate::items::ItemStats, l: &Loot) -> bool {
    if name_matches(&stats.name, &l.never) {
        return false;
    }
    if name_matches(&stats.name, &l.always) {
        return true;
    }
    l.filters.iter().any(|f| {
        let q = Query::parse(f);
        !q.is_empty() && stats.matches(&q)
    })
}

/// What the character is doing on its own right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Doing {
    #[default]
    Idle,
    Healing,
    Fleeing,
    Fighting,
    Looting,
    Buffing,
}

impl Doing {
    pub fn label(self) -> &'static str {
        match self {
            Doing::Idle => "waiting",
            Doing::Healing => "healing",
            Doing::Fleeing => "breaking off",
            Doing::Fighting => "fighting",
            Doing::Looting => "looting",
            Doing::Buffing => "buffing",
        }
    }
}

/// The running state of the rules.
#[derive(Default)]
pub struct Autoplay {
    pub config: Config,
    pub doing: Doing,
    /// A line for the panel: "fighting Drudge Skulker".
    pub status: String,
    last_heal: Option<Instant>,
    last_attack: Option<Instant>,
    last_buff: Option<Instant>,
    /// The corpse being looted and when we started.
    corpse: Option<(u32, Instant)>,
    /// Corpses already emptied.
    looted: Vec<u32>,
    /// Corpse items we asked the server about.
    appraising: bool,
}

impl Autoplay {
    fn say(&mut self, doing: Doing, status: impl Into<String>) {
        let status = status.into();
        if self.doing != doing || self.status != status {
            tracing::info!("autoplay: {status}");
        }
        self.doing = doing;
        self.status = status;
    }
}

impl Client {
    /// Health as a fraction of its maximum, 1.0 when unknown.
    pub fn health_fraction(&self) -> f32 {
        let stats = &self.world.stats;
        let max = stats.vital_max(0);
        if max == 0 {
            return 1.0;
        }
        stats.vitals[0].current as f32 / max as f32
    }

    /// The id of a known spell whose name starts with `name`, preferring
    /// the highest level learnt (the last in the spellbook order).
    pub fn spell_by_name(&self, name: &str) -> Option<u32> {
        let want = name.trim().to_lowercase();
        if want.is_empty() {
            return None;
        }
        let table = self.assets.spell_table().ok();
        let mut best: Option<(u32, u32)> = None;
        for id in &self.world.stats.spells {
            let full = table
                .as_ref()
                .and_then(|t| t.get(*id).map(|s| s.name.clone()))
                .or_else(|| self.known_spells.get(id).cloned())
                .unwrap_or_default()
                .to_lowercase();
            if !full.starts_with(&want) && !full.contains(&want) {
                continue;
            }
            // Later spells in a family are the stronger ones.
            if best.map(|(b, _)| *id > b).unwrap_or(true) {
                best = Some((*id, *id));
            }
        }
        best.map(|(id, _)| id)
    }

    /// Seconds left on the enchantment of a spell family, if any is up.
    fn buff_left(&self, spell: u32) -> Option<f32> {
        let now = self.session.server_time()?;
        self.world
            .stats
            .enchantments
            .iter()
            .filter(|e| e.spell_id as u32 == spell)
            .map(|e| (e.start_time + e.duration - now) as f32)
            .fold(None, |acc: Option<f32>, left| {
                Some(acc.map_or(left, |a| a.max(left)))
            })
    }

    /// Run the rules for this moment. Call it once a frame; it does at
    /// most one thing.
    pub fn tick_autoplay(&mut self, now: Instant) {
        if !self.autoplay.config.enabled || self.world.player_guid.is_none() {
            if !self.autoplay.status.is_empty() && !self.autoplay.config.enabled {
                self.autoplay.doing = Doing::Idle;
                self.autoplay.status.clear();
            }
            return;
        }
        if self.autoplay_survive(now) {
            return;
        }
        if self.autoplay_loot(now) {
            return;
        }
        if self.autoplay_fight(now) {
            return;
        }
        if self.autoplay_buff(now) {
            return;
        }
        let doing = self.autoplay.doing;
        if doing != Doing::Idle {
            self.autoplay.say(Doing::Idle, "waiting");
        }
    }

    /// Heal, and break off a losing fight. True when it acted.
    fn autoplay_survive(&mut self, now: Instant) -> bool {
        let cfg = self.autoplay.config.survive.clone();
        let health = self.health_fraction();
        if health >= cfg.heal_below || health <= 0.0 {
            return false;
        }
        if cfg.flee_below > 0.0 && health < cfg.flee_below && self.attack_target.is_some() {
            self.attack_target = None;
            if self.combat {
                self.toggle_combat();
            }
            self.autoplay.say(
                Doing::Fleeing,
                format!("breaking off at {:.0}% health", health * 100.0),
            );
            return true;
        }
        if self
            .autoplay
            .last_heal
            .is_some_and(|t| now.duration_since(t) < HEAL_EVERY)
        {
            return false;
        }
        // A kit is quicker and cheaper than a spell.
        if cfg.use_kits {
            let kit = self
                .world
                .inventory()
                .filter(|o| ac_world::usable::on_self(o.usable) && o.name.contains("Healing Kit"))
                .map(|o| o.guid)
                .next();
            if let Some(kit) = kit {
                let me = self.world.player_guid.unwrap_or(0);
                self.use_on(kit, me);
                self.autoplay.last_heal = Some(now);
                self.autoplay
                    .say(Doing::Healing, format!("healing at {:.0}%", health * 100.0));
                return true;
            }
        }
        if let Some(spell) = self.spell_by_name(&cfg.heal_spell) {
            if matches!(self.can_cast(spell), crate::magic::CastCheck::Ok) {
                self.cast(spell);
                self.autoplay.last_heal = Some(now);
                self.autoplay
                    .say(Doing::Healing, format!("healing at {:.0}%", health * 100.0));
                return true;
            }
        }
        false
    }

    /// Open the corpse of something we killed and take what is worth
    /// taking. True while looting.
    fn autoplay_loot(&mut self, now: Instant) -> bool {
        if !self.autoplay.config.loot.enabled {
            return false;
        }
        // Already at one: wait for its contents, then empty it.
        if let Some((guid, since)) = self.autoplay.corpse {
            if now.duration_since(since) > LOOT_TIMEOUT {
                tracing::info!("autoplay: corpse {guid:#010x} did not open");
                self.autoplay.corpse = None;
                self.autoplay.looted.push(guid);
                self.autoplay.appraising = false;
                return false;
            }
            let open = self.world.open_container.clone();
            let Some((open_guid, items)) = open else {
                self.autoplay.say(Doing::Looting, "opening a corpse");
                return true;
            };
            if open_guid != guid {
                return true;
            }
            let cfg = self.autoplay.config.loot.clone();
            // The stat filters need the numbers first.
            let needs = cfg
                .filters
                .iter()
                .any(|f| Query::parse(f).needs_appraisal());
            if cfg.appraise && needs && !self.autoplay.appraising {
                let missing: Vec<u32> = items
                    .iter()
                    .copied()
                    .filter(|g| !self.appraisals.contains_key(g))
                    .collect();
                if !missing.is_empty() {
                    self.appraise_many(missing);
                    self.autoplay.appraising = true;
                    self.autoplay.say(Doing::Looting, "looking over the loot");
                    return true;
                }
            }
            if self.autoplay.appraising
                && items.iter().any(|g| !self.appraisals.contains_key(g))
                && now.duration_since(since) < LOOT_TIMEOUT
            {
                return true;
            }
            let mut took = 0;
            for g in &items {
                let Some(stats) = self.stats_of(*g) else {
                    continue;
                };
                if wanted_loot(&stats, &cfg) {
                    tracing::info!("autoplay: taking {}", stats.name);
                    self.take(*g);
                    took += 1;
                }
            }
            self.close_container();
            self.autoplay.looted.push(guid);
            self.autoplay.corpse = None;
            self.autoplay.appraising = false;
            self.autoplay
                .say(Doing::Looting, format!("took {took} item(s)"));
            return true;
        }
        // Look for one nearby that we have not emptied.
        if self.attack_target.is_some() {
            return false;
        }
        let me = self.player.as_ref().map(|p| p.world_position());
        let Some(me) = me else { return false };
        let looted = self.autoplay.looted.clone();
        let corpse = self
            .world
            .objects
            .values()
            .filter(|o| o.object_desc_flags & ac_world::object_desc_flags::CORPSE != 0)
            .filter(|o| !looted.contains(&o.guid))
            .filter_map(|o| {
                let p = o.world_pos()?;
                let d = p.distance(me);
                (d <= 20.0).then_some((d, o.guid, o.name.clone()))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((_, guid, name)) = corpse else {
            return false;
        };
        if self.combat {
            self.toggle_combat();
        }
        self.interact(guid);
        self.autoplay.corpse = Some((guid, now));
        self.autoplay.say(Doing::Looting, format!("looting {name}"));
        true
    }

    /// Pick something to fight and attack it. True when fighting.
    fn autoplay_fight(&mut self, now: Instant) -> bool {
        let cfg = self.autoplay.config.fight.clone();
        if !cfg.enabled {
            return false;
        }
        // Already on one that is still alive.
        if let Some(t) = self.attack_target {
            if let Some(o) = self.world.objects.get(&t) {
                if o.health.unwrap_or(1.0) > 0.0 {
                    let name = o.name.clone();
                    self.autoplay
                        .say(Doing::Fighting, format!("fighting {name}"));
                    return true;
                }
            }
        }
        if self
            .autoplay
            .last_attack
            .is_some_and(|t| now.duration_since(t) < ATTACK_EVERY)
        {
            return false;
        }
        let me = self.player.as_ref().map(|p| p.world_position());
        let Some(me) = me else { return false };
        let target = self
            .world
            .objects
            .values()
            .filter(|o| {
                o.item_type & ac_world::item_type::CREATURE != 0
                    && o.object_desc_flags & ac_world::object_desc_flags::ATTACKABLE != 0
                    && o.object_desc_flags & ac_world::object_desc_flags::PLAYER == 0
                    && o.health.unwrap_or(1.0) > 0.0
                    && !o.is_player
            })
            .filter(|o| wanted_target(&o.name, &cfg))
            .filter_map(|o| {
                let p = o.world_pos()?;
                let d = p.distance(me);
                (d <= cfg.radius).then_some((d, o.guid, o.name.clone()))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((_, guid, name)) = target else {
            return false;
        };
        if !self.combat {
            self.toggle_combat();
        }
        self.attack(guid);
        self.autoplay.last_attack = Some(now);
        self.autoplay
            .say(Doing::Fighting, format!("attacking {name}"));
        true
    }

    /// Put a buff back up. True when it cast one.
    fn autoplay_buff(&mut self, now: Instant) -> bool {
        let cfg = self.autoplay.config.buffs.clone();
        if cfg.spells.is_empty() {
            return false;
        }
        if cfg.out_of_combat_only && self.attack_target.is_some() {
            return false;
        }
        if self
            .autoplay
            .last_buff
            .is_some_and(|t| now.duration_since(t) < BUFF_EVERY)
        {
            return false;
        }
        for name in &cfg.spells {
            let Some(spell) = self.spell_by_name(name) else {
                continue;
            };
            let left = self.buff_left(spell);
            if left.is_some_and(|l| l > cfg.recast_within) {
                continue;
            }
            if !matches!(self.can_cast(spell), crate::magic::CastCheck::Ok) {
                continue;
            }
            self.cast(spell);
            self.autoplay.last_buff = Some(now);
            let name = name.clone();
            self.autoplay.say(Doing::Buffing, format!("casting {name}"));
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::ItemStats;

    #[test]
    fn target_rules_read_names() {
        let mut f = Fight::default();
        assert!(wanted_target("Drudge Skulker", &f));
        f.only = vec!["drudge".into()];
        assert!(wanted_target("Drudge Skulker", &f));
        assert!(!wanted_target("Olthoi Grub", &f));
        f.only = vec!["  ".into()];
        assert!(wanted_target("anything", &f), "a blank rule means any");
        f.only = Vec::new();
        f.avoid = vec!["Olthoi".into()];
        assert!(!wanted_target("Olthoi Grub", &f));
        assert!(wanted_target("Drudge Skulker", &f));
        // Avoid wins over only.
        f.only = vec!["olthoi".into()];
        assert!(!wanted_target("Olthoi Grub", &f));
    }

    fn item(name: &str, value: u32, armor: u32) -> ItemStats {
        ItemStats {
            name: name.into(),
            value,
            armor_level: armor,
            appraised: true,
            kind: if armor > 0 { "armor" } else { "misc" },
            ..Default::default()
        }
    }

    #[test]
    fn loot_rules_use_the_search_language() {
        let mut l = Loot::default();
        // The default: anything over 250 pyreals, and pyreals themselves.
        assert!(wanted_loot(&item("Ornate Ring", 900, 0), &l));
        assert!(!wanted_loot(&item("Rusty Nail", 3, 0), &l));
        assert!(wanted_loot(&item("Pyreal", 12, 0), &l));
        // A stat filter.
        l.filters = vec!["type:armor al>=200".into()];
        assert!(wanted_loot(&item("Platemail", 100, 240), &l));
        assert!(!wanted_loot(&item("Platemail", 100, 120), &l));
        // Never wins over always and the filters.
        l.never = vec!["platemail".into()];
        assert!(!wanted_loot(&item("Platemail", 100, 240), &l));
        // A blank filter matches nothing.
        l.filters = vec!["".into()];
        l.never = Vec::new();
        assert!(!wanted_loot(&item("Ornate Ring", 900, 0), &l));
        assert!(wanted_loot(&item("Pyreal", 12, 0), &l), "always still wins");
    }

    #[test]
    fn config_round_trips_through_json() {
        let mut c = Config::default();
        c.enabled = true;
        c.buffs.spells = vec!["Strength Self".into()];
        c.fight.avoid = vec!["Olthoi".into()];
        let text = serde_json::to_string(&c).unwrap();
        let back: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(back, c);
        // Missing fields fall back to the defaults.
        let partial: Config = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(partial.enabled);
        assert_eq!(partial.survive.heal_below, Survive::default().heal_below);
        assert_eq!(Doing::Fighting.label(), "fighting");
    }
}
