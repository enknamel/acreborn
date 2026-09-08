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
use crate::{Client, Stance};

/// How long to wait for a corpse to open before giving up.
const LOOT_TIMEOUT: Duration = Duration::from_secs(6);
/// Least time between two heals, so one is not spammed.
const HEAL_EVERY: Duration = Duration::from_millis(2500);
/// Least time between two casts of the same buff.
const BUFF_EVERY: Duration = Duration::from_millis(1500);
/// A target that takes no damage for this long is let go.
const STALL_AFTER: Duration = Duration::from_secs(20);
/// And left alone for this long afterwards.
const GIVE_UP_FOR: Duration = Duration::from_secs(90);
/// A change of weapon is asked for at most this often.
const REWIELD_EVERY: Duration = Duration::from_millis(1000);
/// Ammunition is made at most this often: a use takes a moment and
/// the bundles need to answer.
const CRAFT_EVERY: Duration = Duration::from_secs(4);
/// The same note is not logged again within this.
const NOTE_EVERY: Duration = Duration::from_secs(5);
/// How often the buffs are gone through to see what is due.
const BUFF_CHECK_EVERY: Duration = Duration::from_millis(1000);
/// Least time between two attack orders.
const ATTACK_EVERY: Duration = Duration::from_millis(1200);
/// Least time between two attack spells. A war spell takes about two
/// seconds to cast and the server refuses one sent over another, so
/// this is paced to the casting rather than to the frame.
const CAST_EVERY: Duration = Duration::from_millis(2600);

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
    /// Cast this spell to heal (by name, "Heal Self"); empty to use the
    /// strongest health boost known instead, whatever it is called.
    pub heal_spell: String,
    /// Keep mana up by pouring stamina into it, and stamina up with
    /// Revitalize, the way a caster does: the transfer gives more mana
    /// than the Revitalize costs, so the round is a gain.
    pub manage_mana: bool,
    /// Pour stamina into mana when mana is under this fraction.
    pub mana_below: f32,
    /// Revitalize when stamina is under this fraction.
    pub stamina_below: f32,
}

impl Default for Survive {
    fn default() -> Self {
        Survive {
            heal_below: 0.6,
            flee_below: 0.25,
            use_kits: true,
            heal_spell: String::new(),
            manage_mana: true,
            mana_below: 0.4,
            stamina_below: 0.3,
        }
    }
}

/// Buffs to keep up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Buffs {
    /// Work the buffs out from the character itself: every Life and
    /// Creature self-enchantment it knows for the skills it has trained,
    /// the Item auras for the way it fights, and the armour spells on
    /// each piece worn; the highest level of each (see `crate::buffs`).
    pub auto: bool,
    /// The lowest chance of a cast landing that is still worth the
    /// mana: a level that fizzles more often than this is passed over
    /// for the one below it. Half is the point where the school's skill
    /// equals the spell's power.
    pub least_chance: f32,
    /// Spell names to keep on the character as well, by hand.
    pub spells: Vec<String>,
    /// In a quiet moment, put back any buff with this many seconds or
    /// fewer left. Wide on purpose: refreshing a few at every lull
    /// spreads the work out, so the set never all runs out at once and
    /// the character is never stood still for twenty casts in a row.
    pub top_up_within: f32,
    /// A buff with this many seconds or fewer left is put back at once,
    /// fight or no fight, swapping to a wand for it if need be. A buff
    /// must never be allowed to run out: the protections going down in
    /// the middle of a fight is how a character dies.
    pub never_below: f32,
    /// Only top up out of combat (the urgent recasts happen regardless).
    pub out_of_combat_only: bool,
    /// Keep this fraction of mana back from buffing, for healing and
    /// fighting. A character that spends its last point on Quickness
    /// Self cannot heal, and buffs are the one thing that can wait.
    pub keep_mana: f32,
}

impl Default for Buffs {
    fn default() -> Self {
        Buffs {
            auto: true,
            least_chance: 0.5,
            spells: Vec::new(),
            top_up_within: 300.0,
            never_below: 60.0,
            out_of_combat_only: true,
            keep_mana: 0.35,
        }
    }
}

/// What to fight.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fight {
    pub enabled: bool,
    /// How to fight: with a weapon in hand, at range, or with spells.
    pub style: Style,
    /// Attack spells to throw, by name. Empty means every attack spell
    /// in the spellbook: of the ones that can be cast right now, the one
    /// the target is weakest to is used. Only read when fighting with
    /// magic.
    pub spells: Vec<String>,
    /// Wield the best weapon carried for whatever is being fought: the
    /// one whose element it takes most damage from, rending and
    /// criticals counted (see `crate::weapons`).
    pub pick_weapon: bool,
    /// Cast a vulnerability for the target's weakest element before
    /// fighting anything with at least this much health. 0 never does.
    pub vuln_above_health: u32,
    /// Only attack creatures whose name contains one of these; empty
    /// means anything that can be attacked.
    pub only: Vec<String>,
    /// Never attack creatures whose name contains one of these.
    pub avoid: Vec<String>,
    /// Farthest creature to pick, metres.
    pub radius: f32,
    /// Make more ammunition when out, from a bundle of heads and a
    /// bundle of shafts carried, if Fletching is up to it.
    pub craft_ammo: bool,
}

impl Default for Fight {
    fn default() -> Self {
        Fight {
            enabled: true,
            style: Style::Auto,
            spells: Vec::new(),
            pick_weapon: true,
            vuln_above_health: crate::weapons::LONG_FIGHT_HEALTH,
            craft_ammo: true,
            only: Vec::new(),
            avoid: Vec::new(),
            radius: 25.0,
        }
    }
}

/// Which weapon the character should be holding.
///
/// How a character fights is not a setting: it follows what is in its
/// hands. A wand, orb or staff casts, a bow or crossbow shoots, a sword
/// swings. So this does not choose a stance, it chooses a weapon:
/// [`Style::Auto`] fights with whatever is already held, and the other
/// three wield a weapon of that kind first, for a character that
/// carries more than one and should only use the one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Style {
    #[default]
    Auto,
    Melee,
    Missile,
    Magic,
}

impl Style {
    pub fn label(self) -> &'static str {
        match self {
            Style::Auto => "whatever is held",
            Style::Melee => "a melee weapon",
            Style::Missile => "a bow or thrown weapon",
            Style::Magic => "a wand or staff",
        }
    }

    pub const ALL: [Style; 4] = [Style::Auto, Style::Melee, Style::Missile, Style::Magic];
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

/// What this character does for the others playing alongside it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// Attacks the team's target.
    #[default]
    Fighter,
    /// Lands the debuffs on the team's target before the others hit it.
    Debuffer,
    /// Heals whoever is worst off, and fights only when everyone is
    /// healthy.
    Healer,
}

impl Role {
    pub fn label(self) -> &'static str {
        match self {
            Role::Fighter => "fighter",
            Role::Debuffer => "debuffer",
            Role::Healer => "healer",
        }
    }
}

/// Hunting with the other characters being played.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Team {
    pub enabled: bool,
    pub role: Role,
    /// Attack whatever the team is attacking rather than picking alone.
    pub focus_fire: bool,
    /// Form a fellowship and recruit the others.
    pub fellowship: bool,
    /// The fellowship's name.
    pub fellowship_name: String,
    /// Spells a debuffer lands on the team's target, in order
    /// ("Imperil", "Magic Yield Other").
    pub debuffs: Vec<String>,
    /// Hand a teammate standing next to us what they are short of.
    pub share_supplies: bool,
    /// Ask for more when fewer than this many are carried (by name).
    pub keep_stocked: Vec<(String, u32)>,
    /// A creature with at least this much health is a hard fight, and
    /// hard fights are planned: the teammate with the highest Life
    /// Magic softens it with a vulnerability and an imperil, and the
    /// rest hold their fire until it has. 0 plans nothing.
    pub hard_fight_health: u32,
    /// Whether the rest wait for the softening before opening fire on
    /// a hard target. Off by default: the shots and spells spent before
    /// the vulnerability lands cost next to nothing, and every second
    /// the target is not being hit is a second it is hitting someone.
    pub wait_for_debuff: bool,
    /// This character leads: the others come to it, follow it about and
    /// fly when it flies. The one played by hand, usually. Without one
    /// the leader is whoever's name sorts first, and nobody follows.
    pub lead: bool,
    /// Follow the leader about (a character that leads never does).
    pub follow: bool,
    /// How close to keep to the leader, metres.
    pub follow_distance: f32,
}

/// A leader further off than this is followed before anything else,
/// a fight included; nearer, the fight comes first.
const FOLLOW_BREAK: f32 = 40.0;
/// Up to this far the follower walks straight for the leader, letting
/// the steering find the way; further (the leader took a portal) a
/// journey is planned.
const FOLLOW_WALK: f32 = 120.0;

impl Default for Team {
    fn default() -> Self {
        Team {
            enabled: false,
            role: Role::Fighter,
            focus_fire: true,
            fellowship: true,
            fellowship_name: "acreborn".into(),
            debuffs: Vec::new(),
            share_supplies: true,
            keep_stocked: vec![("Healing Kit".into(), 1)],
            hard_fight_health: 400,
            wait_for_debuff: false,
            lead: false,
            follow: true,
            follow_distance: 4.0,
        }
    }
}

/// What one of the others has told us about itself. The host fills this
/// in from the bus every frame (see `ac_plugin::team`); the rules here
/// only read it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mate {
    pub name: String,
    pub guid: u32,
    /// Its session index in its own process.
    pub session: usize,
    pub world: glam::Vec3,
    pub health: f32,
    pub role: Role,
    pub target: Option<u32>,
    pub target_name: String,
    pub in_fellowship: bool,
    /// Items it is short of, by name.
    pub wants: Vec<String>,
    /// Targets it has already debuffed.
    pub debuffed: Vec<u32>,
    /// True for the one that picks the targets.
    pub leader: bool,
    /// Its Life Magic as it stands, buffs counted: what decides who
    /// softens a hard target.
    pub life_magic: u32,
    /// It knows a vulnerability or an imperil it can cast right now.
    pub can_soften: bool,
    /// It asked to lead (see `Team::lead`).
    pub leads: bool,
    /// It is flying (no-clip); followers fly too.
    pub flying: bool,
    /// The cell it stands in: indoors (a hub, a dungeon) is somewhere
    /// a journey cannot be planned to from outside.
    pub cell: u32,
}

/// The team as the host last saw it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TeamView {
    pub mates: Vec<Mate>,
    /// Whether this character is the one picking targets.
    pub leader: bool,
}

impl TeamView {
    /// The one leading, if it is one of the others.
    pub fn leader_mate(&self) -> Option<&Mate> {
        self.mates.iter().find(|m| m.leader)
    }

    /// The target the team is on: the leader's, else the first anyone has.
    pub fn target(&self) -> Option<(u32, String)> {
        let leader = self
            .mates
            .iter()
            .find(|m| m.leader)
            .and_then(|m| m.target.map(|t| (t, m.target_name.clone())));
        leader.or_else(|| {
            self.mates
                .iter()
                .find_map(|m| m.target.map(|t| (t, m.target_name.clone())))
        })
    }

    /// Whether anyone has already landed the debuffs on `target`.
    pub fn debuffed(&self, target: u32) -> bool {
        self.mates.iter().any(|m| m.debuffed.contains(&target))
    }

    /// The mate nearest `me` that is short of something we could hand
    /// over, within `radius` metres.
    pub fn wanting<'a>(&'a self, me: glam::Vec3, radius: f32) -> Option<&'a Mate> {
        self.mates
            .iter()
            .filter(|m| !m.wants.is_empty() && m.world.distance(me) <= radius)
            .min_by(|a, b| a.world.distance(me).total_cmp(&b.world.distance(me)))
    }

    /// The mate in the worst shape, for a healer.
    pub fn worst_hurt(&self) -> Option<&Mate> {
        self.mates
            .iter()
            .filter(|m| m.health > 0.0 && m.health < 1.0)
            .min_by(|a, b| a.health.total_cmp(&b.health))
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
    pub team: Team,
}

/// Why a cast is refused, in a few words for a status line.
pub fn cast_problem(check: &crate::magic::CastCheck) -> String {
    use crate::magic::CastCheck;
    match check {
        CastCheck::Ok => "fine".into(),
        CastCheck::NotKnown => "not known".into(),
        CastCheck::NoCaster => "no wand wielded".into(),
        CastCheck::MissingComponents(m) => format!("short of {} components", m.len()),
        CastCheck::NotEnoughMana { need, have } => format!("mana {have}/{need}"),
        CastCheck::TooHard { power, skill } => format!("power {power} over skill {skill}"),
    }
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
    Debuffing,
    Helping,
    Following,
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
            Doing::Debuffing => "debuffing",
            Doing::Helping => "helping the team",
            Doing::Following => "following the leader",
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
    /// When an attack spell last went out, and what it was thrown at:
    /// a spell keeps no `attack_target` of its own the way a swing
    /// does, so the engine remembers what it is working on.
    last_cast: Option<Instant>,
    casting_at: Option<u32>,
    /// The target the weapon in hand was chosen for, so it is chosen
    /// once a fight and not once a frame.
    armed_for: Option<u32>,
    /// Targets already made vulnerable this fight.
    vulned: Vec<u32>,
    /// Hard targets being softened, and how far along: 0 the
    /// vulnerability still to cast, 1 the imperil.
    softening: Vec<(u32, u8)>,
    last_vuln: Option<Instant>,
    last_buff: Option<Instant>,
    /// Enchantments put on items: `(item, category, when, seconds it
    /// lasts)`. An item's enchantments are not reported the way the
    /// character's own are, so the cast is remembered instead.
    item_buffs: Vec<(u32, u32, Instant, f32)>,
    /// The last note logged and when (see `note`).
    noted: Option<(String, Instant)>,
    /// When the buffs were last gone through. The urgent pass runs
    /// every tick, and working out what is due walks the whole
    /// spellbook, so it is only done once a second.
    buffs_checked: Option<Instant>,
    /// The weapon put down to cast an urgent buff mid-fight, to be taken
    /// up again the moment the buffing is done.
    put_down: Option<u32>,
    /// The ammunition chosen for the target, to be wielded with the bow.
    wanted_ammo: Option<u32>,
    /// When the hands were last asked to change weapon.
    last_rewield: Option<Instant>,
    /// A weapon to wield as soon as the hands are empty.
    pending_wield: Option<u32>,
    /// A journey put down for a fight, to be picked up again after it.
    resume_trip: Option<glam::Vec2>,
    /// The target being worked on, since when, and its health when
    /// last seen to drop: a target that takes no damage for a while is
    /// out of reach, and is let go.
    engaged: Option<(u32, Instant, f32)>,
    /// Targets let go, and when, so they are left alone for a while.
    given_up: Vec<(u32, Instant)>,
    /// When ammunition was last made.
    last_craft: Option<Instant>,
    /// When stamina was last poured into mana or Revitalize cast.
    last_vital: Option<Instant>,
    /// The corpse being looted and when we started.
    corpse: Option<(u32, Instant)>,
    /// Corpses already emptied.
    looted: Vec<u32>,
    /// Corpse items we asked the server about.
    appraising: bool,
    /// The other characters being played, as the host last saw them.
    pub team: TeamView,
    /// Targets this character has landed its debuffs on.
    pub debuffed: Vec<u32>,
    /// What this character is short of, for the others to hand over.
    pub wants: Vec<String>,
    last_debuff: Option<Instant>,
    last_give: Option<Instant>,
    last_recruit: Option<Instant>,
    /// Where the journey after a far-off leader was bound, to plan
    /// again once it has moved on.
    follow_trip: Option<glam::Vec2>,
    /// No journey after the leader is planned before this: planning
    /// costs a search, and one that found no way is not tried again for
    /// a while.
    next_follow_plan: Option<Instant>,
}

impl Autoplay {
    /// The creature spells are being thrown at, if any: the magic
    /// fighter's counterpart to `Client::attack_target`.
    pub fn casting_at(&self) -> Option<u32> {
        self.casting_at
    }

    /// Something worth knowing that is not what the character is doing:
    /// logged, at most every few seconds for the same words, and the
    /// status left as it was. Said every tick it would drown the log
    /// and flip the status back and forth with whatever else is going on.
    fn note(&mut self, text: impl Into<String>, now: Instant) {
        let text = text.into();
        let again = self
            .noted
            .as_ref()
            .is_some_and(|(t, when)| *t == text && now.duration_since(*when) < NOTE_EVERY);
        if !again {
            tracing::info!("autoplay: {text}");
            self.noted = Some((text, now));
        }
    }

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
        // Of the family, the strongest that can be cast right now: a
        // name like "Heal Self" means the best Heal Self we can manage,
        // not the best in the book. Failing any castable, the strongest
        // known, so the reason it cannot be cast can be reported.
        let mut best_castable: Option<(u32, u32)> = None;
        let mut best_known: Option<(u32, u32)> = None;
        for id in &self.world.stats.spells {
            let sp = table.as_ref().and_then(|t| t.get(*id));
            let full = sp
                .map(|s| s.name.clone())
                .or_else(|| self.known_spells.get(id).cloned())
                .unwrap_or_default()
                .to_lowercase();
            if !full.starts_with(&want) && !full.contains(&want) {
                continue;
            }
            // Power orders a family; a spell the table lacks ranks by id.
            let power = sp.map(|s| s.power).unwrap_or(*id);
            let castable = matches!(
                self.can_cast(*id),
                crate::magic::CastCheck::Ok | crate::magic::CastCheck::NoCaster
            );
            if castable && best_castable.is_none_or(|(_, p)| power > p) {
                best_castable = Some((*id, power));
            }
            if best_known.is_none_or(|(_, p)| power > p) {
                best_known = Some((*id, power));
            }
        }
        best_castable.or(best_known).map(|(id, _)| id)
    }

    /// The strongest known boost of a vital that can be cast right now,
    /// whatever it is called: Heal Self VI and Adja's Intervention are
    /// both health boosts, and the table says so where a name would not.
    fn best_boost(&self, vital: u32) -> Option<u32> {
        let table = self.assets.spell_table().ok()?;
        ac_world::vitals::boosts_of(vital)
            .into_iter()
            .filter(|b| self.world.stats.spells.contains(&b.spell))
            .filter(|b| table.get(b.spell).is_some_and(|s| s.is_self_targeted()))
            .filter(|b| {
                matches!(
                    self.can_cast(b.spell),
                    crate::magic::CastCheck::Ok | crate::magic::CastCheck::NoCaster
                )
            })
            .max_by_key(|b| table.get(b.spell).map(|s| s.power).unwrap_or(0))
            .map(|b| b.spell)
    }

    /// The strongest known self transfer from one vital into another
    /// that can be cast right now.
    fn best_transfer(&self, from: u32, to: u32) -> Option<u32> {
        let table = self.assets.spell_table().ok()?;
        ac_world::vitals::transfers_between(from, to)
            .into_iter()
            .filter(|t| self.world.stats.spells.contains(&t.spell))
            .filter(|t| table.get(t.spell).is_some_and(|s| s.is_self_targeted()))
            .filter(|t| {
                matches!(
                    self.can_cast(t.spell),
                    crate::magic::CastCheck::Ok | crate::magic::CastCheck::NoCaster
                )
            })
            .max_by_key(|t| table.get(t.spell).map(|s| s.power).unwrap_or(0))
            .map(|t| t.spell)
    }

    /// Keep mana and stamina up the way a caster does: stamina poured
    /// into mana when mana runs low, Revitalize when stamina does. True
    /// when a spell went out.
    fn autoplay_vitals(&mut self, now: Instant) -> bool {
        use ac_world::vitals::vital;
        let cfg = self.autoplay.config.survive.clone();
        if !cfg.manage_mana {
            return false;
        }
        if self
            .autoplay
            .last_vital
            .is_some_and(|t| now.duration_since(t) < HEAL_EVERY)
        {
            return false;
        }
        let frac = |i: usize| {
            let max = self.world.stats.vital_max(i).max(1) as f32;
            self.world.stats.vitals[i].current as f32 / max
        };
        let (stamina, mana) = (frac(1), frac(2));
        // Stamina first: it is what mana is made from, and Revitalize is
        // cheap next to what a transfer of a full bar returns.
        if stamina < cfg.stamina_below {
            if let Some(spell) = self.best_boost(vital::STAMINA) {
                if matches!(self.can_cast(spell), crate::magic::CastCheck::Ok) {
                    self.cast(spell);
                    self.autoplay.last_vital = Some(now);
                    self.autoplay.say(
                        Doing::Buffing,
                        format!("restoring stamina at {:.0}%", stamina * 100.0),
                    );
                    return true;
                }
            }
        }
        if mana < cfg.mana_below && stamina >= cfg.stamina_below.max(0.5) {
            if let Some(spell) = self.best_transfer(vital::STAMINA, vital::MANA) {
                if matches!(self.can_cast(spell), crate::magic::CastCheck::Ok) {
                    self.cast(spell);
                    self.autoplay.last_vital = Some(now);
                    self.autoplay.say(
                        Doing::Buffing,
                        format!("pouring stamina into mana at {:.0}%", mana * 100.0),
                    );
                    return true;
                }
            }
        }
        false
    }

    /// The lowest-power known spell whose name matches, for reporting
    /// why a whole family is out of reach.
    fn easiest_of_family(&self, name: &str) -> Option<u32> {
        let want = name.trim().to_lowercase();
        let table = self.assets.spell_table().ok()?;
        self.world
            .stats
            .spells
            .iter()
            .filter_map(|id| table.get(*id).map(|s| (*id, s)))
            .filter(|(_, s)| s.name.to_lowercase().starts_with(&want))
            .min_by_key(|(_, s)| s.power)
            .map(|(id, _)| id)
    }

    /// Seconds left on the enchantment of a spell family, if any is up.
    fn buff_left(&self, spell: u32) -> Option<f32> {
        let table = self.assets.spell_table().ok();
        let sp = table.as_ref().and_then(|t| t.get(spell));
        match sp {
            Some(sp) => self.category_left(sp.category, sp.power),
            None => self.spell_left(spell),
        }
    }

    /// Seconds left on this exact spell: `None` when it is not up,
    /// infinity when it never runs out.
    fn spell_left(&self, spell: u32) -> Option<f32> {
        self.longest_left(|e| e.spell_id as u32 == spell)
    }

    /// Seconds left on any enchantment of this category at least as
    /// strong as `power`: Strength Self VI already up means Strength
    /// Self IV is not wanted, and the other way round it is.
    fn category_left(&self, category: u32, power: u32) -> Option<f32> {
        self.longest_left(|e| e.category as u32 == category && e.power >= power)
    }

    /// The longest any enchantment passing `keep` has left. A quest or
    /// item enchantment with no end has a duration below zero and is
    /// worth infinity here: treating it as run out had a character
    /// recasting a permanent buff every two seconds. Without the
    /// server's clock nothing can be said, and nothing is due.
    fn longest_left(&self, keep: impl Fn(&ac_world::stats::Enchantment) -> bool) -> Option<f32> {
        let now = self.session.server_time()?;
        self.world
            .stats
            .enchantments
            .iter()
            .filter(|e| keep(e))
            .map(|e| match e.remaining(now) {
                Some(left) => left as f32,
                None => f32::INFINITY,
            })
            .fold(None, |acc: Option<f32>, left| {
                Some(acc.map_or(left, |a| a.max(left)))
            })
    }

    /// Seconds left on an enchantment we put on an item, by category,
    /// from when we cast it and how long the spell lasts.
    fn item_buff_left(&self, item: u32, category: u32, now: Instant) -> Option<f32> {
        self.autoplay
            .item_buffs
            .iter()
            .filter(|(g, c, _, _)| *g == item && *c == category)
            .map(|(_, _, when, lasts)| lasts - now.duration_since(*when).as_secs_f32())
            .fold(None, |acc: Option<f32>, left| {
                Some(acc.map_or(left, |a| a.max(left)))
            })
    }

    /// The buffs this character should be wearing right now, worked out
    /// from what it is (see `crate::buffs::wanted`).
    pub fn wanted_buffs(&self) -> Vec<crate::buffs::Want> {
        let Ok(table) = self.assets.spell_table() else {
            return Vec::new();
        };
        let trained = crate::buffs::trained_skills(&self.world.stats.skills);
        let least = self.autoplay.config.buffs.least_chance;
        // Likely enough to land, and with the components and mana to
        // try: a wand not yet in hand is the one lack that does not
        // count, since wielding one is the first thing done.
        let usable = |id: u32| {
            self.cast_chance(id) >= least
                && matches!(
                    self.can_cast(id),
                    crate::magic::CastCheck::Ok | crate::magic::CastCheck::NoCaster
                )
        };
        // Anything the armour spells could harden: armour, clothing or
        // a shield, since a cast at ourselves lands on all of it.
        let wears_armour = self.world.wielded().any(|o| {
            o.item_type & (ac_world::item_type::ARMOR | ac_world::item_type::CLOTHING) != 0
                || o.valid_locations & ac_world::equip::SHIELD != 0
        });
        // The weapon in hand decides which weapon skill is worth a buff.
        let weapon_skill = match self.combat_stance() {
            Stance::Magic => Some(0),
            _ => self
                .world
                .wielded()
                .find(|o| {
                    o.item_type
                        & (ac_world::item_type::MELEE_WEAPON | ac_world::item_type::MISSILE_WEAPON)
                        != 0
                })
                .and_then(|o| self.stats_of(o.guid))
                .map(|i| i.weapon_skill_id)
                .filter(|id| *id != 0),
        };
        let me = crate::buffs::Character {
            known: &self.world.stats.spells,
            trained: &trained,
            stance: self.combat_stance(),
            guid: self.world.player_guid.unwrap_or(0),
            wears_armour,
            usable: &usable,
            weapon_skill,
        };
        crate::buffs::wanted(&table, &me)
    }

    /// Run the rules for this moment. Call it once a frame; it does at
    /// most one thing.
    pub fn tick_autoplay(&mut self, now: Instant) {
        // The team's housekeeping runs whether or not the character
        // plays on its own: a leader played by hand still gathers the
        // fellowship, and everyone answers its invitations.
        if self.autoplay.config.team.enabled && self.world.player_guid.is_some() {
            self.autoplay_accept_invites();
            if self.autoplay.config.team.lead && !self.autoplay.config.enabled {
                self.autoplay_fellowship(now);
            }
        }
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
        // A weapon waiting for empty hands is taken up as soon as they
        // are.
        if let Some(g) = self.autoplay.pending_wield {
            let hands_full = self.world.wielded().any(|o| {
                o.item_type
                    & (ac_world::item_type::MELEE_WEAPON
                        | ac_world::item_type::MISSILE_WEAPON
                        | ac_world::item_type::CASTER)
                    != 0
                    && o.valid_locations & ac_world::equip::MISSILE_AMMO == 0
            });
            if !hands_full {
                self.autoplay.pending_wield = None;
                if self.world.is_carried(g) {
                    self.wield_guid(g);
                }
            } else if !self.world.is_carried(g) && !self.world.objects.contains_key(&g) {
                self.autoplay.pending_wield = None;
            }
        }
        // A buff about to run out goes back up before anything else is
        // done, fight or no fight.
        if self.autoplay_buff(now, true) {
            return;
        }
        // Mana and stamina are kept up between everything else.
        if self.autoplay_vitals(now) {
            return;
        }
        self.autoplay_rearm();
        self.autoplay_stock();
        if self.autoplay_loot(now) {
            return;
        }
        // A leader that has got well away is caught up with before
        // anything else; a nearby one is followed once the fighting is
        // done.
        if self.autoplay_follow(now, true) {
            return;
        }
        if self.autoplay_team(now) {
            return;
        }
        if self.autoplay_fight(now) {
            return;
        }
        if self.autoplay_buff(now, false) {
            return;
        }
        if self.autoplay_follow(now, false) {
            return;
        }
        if self.autoplay_resume_journey() {
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
            self.autoplay.casting_at = None;
            self.autoplay.armed_for = None;
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
                self.remember_journey();
                self.use_on(kit, me);
                self.autoplay.last_heal = Some(now);
                self.autoplay
                    .say(Doing::Healing, format!("healing at {:.0}%", health * 100.0));
                return true;
            }
        }
        let heal = if cfg.heal_spell.trim().is_empty() {
            self.best_boost(ac_world::vitals::vital::HEALTH)
        } else {
            self.spell_by_name(&cfg.heal_spell)
        };
        if let Some(spell) = heal {
            let check = self.can_cast(spell);
            // When no level can be cast the name resolves to the
            // strongest, whose reason ("short of components") is not
            // the one that matters; the easiest level says why even
            // that is out of reach.
            let check = if matches!(check, crate::magic::CastCheck::Ok) || cfg.heal_spell.is_empty()
            {
                check
            } else {
                self.easiest_of_family(&cfg.heal_spell)
                    .map(|id| self.can_cast(id))
                    .unwrap_or(check)
            };
            if matches!(check, crate::magic::CastCheck::Ok) {
                self.cast(spell);
                self.autoplay.last_heal = Some(now);
                self.autoplay
                    .say(Doing::Healing, format!("healing at {:.0}%", health * 100.0));
                return true;
            }
            // Hurt and unable to heal is worth saying out loud.
            let why = cast_problem(&check);
            self.autoplay.note(
                format!(
                    "cannot heal at {:.0}%: {} {why}",
                    health * 100.0,
                    cfg.heal_spell
                ),
                now,
            );
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
            // A full pack takes nothing, and asking the server anyway
            // only fills the log with refusals.
            if self.pack_full() {
                self.autoplay.note("pack full, leaving the loot", now);
                self.autoplay.looted.push(guid);
                self.autoplay.corpse = None;
                self.autoplay.appraising = false;
                return false;
            }
            let mut took = 0;
            for g in &items {
                let Some(stats) = self.stats_of(*g) else {
                    continue;
                };
                let own_corpse = self
                    .world
                    .open_container
                    .as_ref()
                    .and_then(|c| self.world.objects.get(&c.0))
                    .is_some_and(|o| {
                        let me = self.world.stats.name.to_lowercase();
                        !me.is_empty() && o.name.to_lowercase() == format!("corpse of {me}")
                    });
                if own_corpse || wanted_loot(&stats, &cfg) {
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
        // Names of the players about, ours excepted: anyone in view and
        // everyone on the team.
        let my_name = self.world.stats.name.to_lowercase();
        let mut players: Vec<String> = self
            .world
            .objects
            .values()
            .filter(|o| o.is_player)
            .map(|o| o.name.to_lowercase())
            .chain(
                self.autoplay
                    .team
                    .mates
                    .iter()
                    .map(|m| m.name.to_lowercase()),
            )
            .filter(|n| !n.is_empty() && *n != my_name)
            .collect();
        players.sort();
        players.dedup();
        // A corpse is a creature's when its name is a creature's; a
        // corpse of anyone else, whether or not we can see them, is a
        // player's, and a player's corpse is theirs.
        let someone_elses = |corpse: &str| {
            let lower = corpse.to_lowercase();
            let Some(who) = lower.strip_prefix("corpse of ") else {
                return false;
            };
            if who == my_name {
                return false;
            }
            players.iter().any(|p| p == who)
                || ac_world::elements::creature(corpse.get(10..).unwrap_or("")).is_none()
        };
        let corpse = self
            .world
            .objects
            .values()
            .filter(|o| o.object_desc_flags & ac_world::object_desc_flags::CORPSE != 0)
            .filter(|o| !looted.contains(&o.guid))
            // Another player's corpse is theirs: a teammate's gear taken
            // off their body is not loot, whatever the filters say. Our
            // own is emptied for everything on it (see below): the wand
            // and the components are on it, and a character without
            // them cannot fight or heal.
            .filter(|o| !someone_elses(&o.name))
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
        self.autoplay.casting_at = None;
        self.autoplay.armed_for = None;
        // Opening a corpse is "using something", which ends a journey.
        self.remember_journey();
        self.interact(guid);
        self.autoplay.corpse = Some((guid, now));
        self.autoplay.say(Doing::Looting, format!("looting {name}"));
        true
    }

    /// The stance the rules want this character in, and the weapon that
    /// gives it wielded if one is carried.
    ///
    /// Which way a character fights is not a setting: it follows what
    /// is in its hands, so [`Style::Auto`] simply reads them. The other
    /// three ask for a weapon of that kind to be wielded, and if none is
    /// carried the character fights with what it has and the rules say
    /// so rather than pretending.
    fn fighting_stance(&mut self) -> Stance {
        let want = match self.autoplay.config.fight.style {
            Style::Auto => return self.combat_stance(),
            Style::Melee => Stance::Melee,
            Style::Missile => Stance::Missile,
            Style::Magic => Stance::Magic,
        };
        if self.combat_stance() != want {
            // Wielding takes a moment; until the server confirms it, the
            // hands still say what they said. Asked once a second, not
            // once a tick: the server answers each ask, and refuses the
            // ones it cannot yet do.
            let now = Instant::now();
            if self
                .autoplay
                .last_rewield
                .is_none_or(|t| now.duration_since(t) >= REWIELD_EVERY)
            {
                self.autoplay.last_rewield = Some(now);
                self.wield_for(want);
            }
        }
        self.combat_stance()
    }

    /// Wield the best weapon carried for `target`, if a better one than
    /// the one in hand is carried. Done once per target: swapping
    /// weapons mid-swing is worse than a slightly wrong weapon.
    ///
    /// Told to fight with whatever suits, this looks across all three
    /// kinds of weapon at once, so a character skilled with a wand and
    /// poor with a sword reaches for the wand. Changing weapon changes
    /// the stance, which the next tick reads back out of its hands.
    fn arm_for(&mut self, target: u32, stance: Stance) {
        if !self.autoplay.config.fight.pick_weapon {
            return;
        }
        if self.autoplay.armed_for == Some(target) {
            return;
        }
        self.autoplay.armed_for = Some(target);
        let Some(known) = self.creature_known(target) else {
            return;
        };
        let carried: Vec<crate::items::ItemStats> = self.item_stats();
        // A weapon nobody has looked at has no element, no imbue and no
        // requirement to read, so it can neither be judged nor safely
        // reached for. Ask about the ones we are carrying; the answers
        // come back over the next few seconds and the choice improves
        // with them.
        let unknown: Vec<u32> = carried
            .iter()
            .filter(|i| !i.appraised && crate::weapons::stance_of(i).is_some())
            .map(|i| i.guid)
            .collect();
        if !unknown.is_empty() {
            self.appraise_many(unknown);
            // Come back to the choice once the answers are in.
            self.autoplay.armed_for = None;
        }
        let wielder = self.wielder();
        let free_choice = self.autoplay.config.fight.style == Style::Auto;
        let picked = if free_choice {
            crate::weapons::best_any(&carried, Some(known), &wielder).map(|(_, c)| c)
        } else {
            crate::weapons::best(&carried, stance, Some(known), &wielder)
        };
        // A bow's choice comes with the arrows to shoot from it.
        self.autoplay.wanted_ammo = picked
            .as_ref()
            .filter(|p| {
                carried
                    .iter()
                    .any(|i| i.guid == p.guid && crate::weapons::is_launcher(i))
            })
            .and_then(|_| crate::weapons::best_missile(&carried, Some(known), &wielder))
            .and_then(|(_, ammo)| ammo.map(|a| a.guid));
        let Some(pick) = picked else {
            return;
        };
        // What is in hand now, whichever kind it is when the choice is
        // free, so the comparison is between the two real options.
        let held = carried.iter().find(|i| {
            i.wielded
                && match crate::weapons::stance_of(i) {
                    Some(s) => free_choice || s == stance,
                    None => false,
                }
        });
        // Only swap for something meaningfully better: an appraisal we
        // have not done yet should not make us drop a good weapon.
        let now_worth = held
            .map(|i| crate::weapons::score(i, Some(known), &wielder))
            .unwrap_or(0.0);
        if held.map(|i| i.guid) == Some(pick.guid) || pick.score <= now_worth * 1.1 {
            return;
        }
        tracing::info!(
            "autoplay: wielding {} against {} ({})",
            pick.name,
            known.name,
            pick.why
        );
        // The server will not put a second weapon in full hands: the
        // old one goes back in the pack first and the new one is
        // wielded once the hands are empty.
        if self.put_weapons_away() {
            self.autoplay.pending_wield = Some(pick.guid);
        } else {
            self.wield_guid(pick.guid);
        }
    }

    /// A hard fight: the creature has at least the team's threshold of
    /// health. With no team, nothing is hard in this sense (the solo
    /// rules soften by their own threshold).
    fn is_hard_fight(&self, guid: u32) -> bool {
        let team = &self.autoplay.config.team;
        if !team.enabled || team.hard_fight_health == 0 {
            return false;
        }
        self.creature_known(guid)
            .is_some_and(|c| c.health >= team.hard_fight_health)
    }

    /// The name of whoever should soften a hard target: the one with
    /// the highest Life Magic of those who can, this character
    /// included. Every session works this out from the same roster,
    /// so they agree without a word.
    fn softener(&self) -> Option<String> {
        let mine = (
            self.world.stats.name.clone(),
            self.life_magic(),
            self.can_soften(),
        );
        let best = self
            .autoplay
            .team
            .mates
            .iter()
            .map(|m| (m.name.clone(), m.life_magic, m.can_soften))
            .chain(std::iter::once(mine))
            .filter(|(_, _, can)| *can)
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)));
        best.map(|(name, _, _)| name)
    }

    /// Whether anyone on the team, this character included, has
    /// softened `guid`.
    fn softened_by_anyone(&self, guid: u32) -> bool {
        self.autoplay.debuffed.contains(&guid)
            || self
                .autoplay
                .team
                .mates
                .iter()
                .any(|m| m.debuffed.contains(&guid))
    }

    /// This character's Life Magic as it stands (skill 33).
    pub fn life_magic(&self) -> u32 {
        let stats = &self.world.stats;
        let Some(sk) = stats.skill(33) else {
            return 0;
        };
        let table = self.assets.skill_table().ok();
        stats.skill_current(sk, table.as_ref().and_then(|t| t.get(33)))
    }

    /// Knows a vulnerability or an imperil it could cast right now (a
    /// wand not in hand does not count against it).
    pub fn can_soften(&self) -> bool {
        let castable = |id: &u32| {
            matches!(
                self.can_cast(*id),
                crate::magic::CastCheck::Ok | crate::magic::CastCheck::NoCaster
            )
        };
        self.imperil_spells().iter().any(castable)
            || ac_world::elements::ALL
                .into_iter()
                .flat_map(ac_world::elements::vulnerabilities)
                .filter(|id| self.world.stats.spells.contains(id))
                .any(|id| castable(&id))
    }

    /// The imperils known: spells cast on another that lower its
    /// armour. Found by effect, so level eight's "Incantation of
    /// Imperil Other" counts without its name being read.
    pub fn imperil_spells(&self) -> Vec<u32> {
        let Ok(table) = self.assets.spell_table() else {
            return Vec::new();
        };
        self.world
            .stats
            .spells
            .iter()
            .copied()
            .filter(|id| {
                ac_world::buffs::effect(*id)
                    .is_some_and(|e| e.kind() == ac_world::buffs::kind::BODY_ARMOR && e.value < 0.0)
            })
            .filter(|id| table.get(*id).is_some_and(|s| s.needs_target()))
            .collect()
    }

    /// See that the bow has something to shoot: the ammunition chosen
    /// for the target if it is still carried, else whatever fits. True
    /// when something is wielded or was already.
    fn ready_ammo(&mut self) -> bool {
        if self.wielded_ammo().is_some() {
            // The chosen kind, if it is not the one in the slot.
            if let Some(want) = self.autoplay.wanted_ammo {
                if self.wielded_ammo() != Some(want) && self.world.is_carried(want) {
                    self.wield_guid(want);
                }
            }
            return true;
        }
        if let Some(want) = self.autoplay.wanted_ammo {
            if self.world.is_carried(want) {
                return self.wield_guid(want);
            }
        }
        self.wield_ammo()
    }

    /// Make ammunition for the launcher in hand from a bundle of heads
    /// and a bundle of shafts carried, the recipe within Fletching, for
    /// the element the target is weakest to when there is a choice.
    /// True when a bundle was used this tick.
    fn autoplay_craft_ammo(&mut self, now: Instant) -> bool {
        if !self.autoplay.config.fight.craft_ammo {
            return false;
        }
        if self
            .autoplay
            .last_craft
            .is_some_and(|t| now.duration_since(t) < CRAFT_EVERY)
        {
            return false;
        }
        let launcher = self
            .wielded_missile_weapon()
            .and_then(|g| self.stats_of(g))
            .filter(crate::weapons::is_launcher);
        let Some(launcher) = launcher else {
            return false;
        };
        if launcher.ammo_type == 0 {
            return false;
        }
        // Fletching as it stands.
        let fletching = {
            let stats = &self.world.stats;
            let table = self.assets.skill_table().ok();
            stats
                .skill(37)
                .map(|sk| stats.skill_current(sk, table.as_ref().and_then(|t| t.get(37))))
                .unwrap_or(0)
        };
        let carried: Vec<(u32, u32)> = self
            .world
            .objects
            .values()
            .filter(|o| self.world.is_carried(o.guid))
            .map(|o| (o.weenie_class_id, o.guid))
            .collect();
        let held = |wcid: u32| carried.iter().find(|(w, _)| *w == wcid).map(|(_, g)| *g);
        let weakest = self
            .autoplay
            .casting_at
            .or(self.attack_target)
            .and_then(|g| self.creature_known(g))
            .and_then(|c| c.weakest_to());
        let mut options: Vec<(&ac_world::fletching::Recipe, u32, u32)> =
            ac_world::fletching::making(launcher.ammo_type)
                .filter(|r| r.difficulty <= fletching)
                .filter_map(|r| Some((r, held(r.source)?, held(r.target)?)))
                .collect();
        if options.is_empty() {
            self.autoplay.note(
                format!(
                    "out of {} and nothing to make more from",
                    ac_world::fletching::ammo_type::name(launcher.ammo_type)
                ),
                now,
            );
            return false;
        }
        // The target's weakness first, then whatever is hardest to make,
        // which is the better arrow.
        options.sort_by(|a, b| {
            let hits =
                |r: &ac_world::fletching::Recipe| weakest.is_some_and(|w| r.element() == Some(w));
            hits(b.0)
                .cmp(&hits(a.0))
                .then(b.0.difficulty.cmp(&a.0.difficulty))
        });
        let (recipe, source, target) = options[0];
        self.use_on(source, target);
        self.autoplay.last_craft = Some(now);
        self.autoplay.say(
            Doing::Looting,
            format!("making {} from {}", recipe.result_name, recipe.source_name),
        );
        true
    }

    /// The main pack has no room for another item.
    pub fn pack_full(&self) -> bool {
        let capacity = self
            .world
            .player()
            .map(|p| p.items_capacity)
            .filter(|c| *c > 0)
            .unwrap_or(102);
        self.world.main_pack().count() as u32 >= capacity
    }

    /// What is known about the kind of creature `guid` is.
    fn creature_known(&self, guid: u32) -> Option<&'static ac_world::elements::Creature> {
        let o = self.world.objects.get(&guid)?;
        ac_world::elements::known(o.weenie_class_id, &o.name)
    }

    /// How this character is fighting right now: what its hands give.
    pub fn fighting_style(&self) -> Style {
        match self.combat_stance() {
            Stance::Melee => Style::Melee,
            Stance::Missile => Style::Missile,
            Stance::Magic => Style::Magic,
        }
    }

    /// Pick something to fight and attack it. True when fighting.
    fn autoplay_fight(&mut self, now: Instant) -> bool {
        let cfg = self.autoplay.config.fight.clone();
        if !cfg.enabled {
            return false;
        }
        let stance = self.fighting_stance();
        if stance == Stance::Magic {
            return self.autoplay_fight_with_spells(now, &cfg);
        }
        // Already on one that is still alive.
        if let Some(t) = self.attack_target {
            if self.stalled_on(t, now) {
                return false;
            }
            if let Some(o) = self.world.objects.get(&t) {
                if o.health.unwrap_or(1.0) > 0.0 {
                    let name = o.name.clone();
                    self.autoplay
                        .say(Doing::Fighting, format!("fighting {name}"));
                    // A bow with an empty ammunition slot shoots
                    // nothing, and the slot empties mid-fight.
                    if stance == Stance::Missile
                        && !self.ready_ammo()
                        && self.autoplay_craft_ammo(now)
                    {
                        return true;
                    }
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
        // Fighting at range: the bow needs something to shoot, and
        // when there is nothing to shoot, something is made.
        let missile = stance == Stance::Missile;
        if missile && !self.ready_ammo() && self.autoplay_craft_ammo(now) {
            return true;
        }
        let me = self.player.as_ref().map(|p| p.world_position());
        let Some(me) = me else { return false };
        // Hunting together: hit what the team is hitting.
        let team = &self.autoplay.config.team;
        if team.enabled && team.focus_fire && !self.autoplay.team.leader {
            if let Some((guid, name)) = self.autoplay.team.target() {
                let alive = self
                    .world
                    .objects
                    .get(&guid)
                    .is_some_and(|o| o.health.unwrap_or(1.0) > 0.0);
                if alive {
                    if self.autoplay_plan_hard(guid, &name, now) {
                        return true;
                    }
                    self.remember_journey();
                    self.arm_for(guid, stance);
                    self.enter_combat();
                    self.attack(guid);
                    self.autoplay.last_attack = Some(now);
                    self.autoplay
                        .say(Doing::Fighting, format!("joining on {name}"));
                    return true;
                }
            }
        }
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
            .filter(|o| {
                !self
                    .autoplay
                    .given_up
                    .iter()
                    .any(|(g, t)| *g == o.guid && now.duration_since(*t) < GIVE_UP_FOR)
            })
            .filter_map(|o| {
                let p = o.world_pos()?;
                let d = p.distance(me);
                (d <= cfg.radius).then_some((d, o.guid, o.name.clone()))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((_, guid, name)) = target else {
            return false;
        };
        if self.autoplay_plan_hard(guid, &name, now) {
            return true;
        }
        self.remember_journey();
        self.arm_for(guid, stance);
        self.enter_combat();
        self.attack(guid);
        self.autoplay.last_attack = Some(now);
        self.autoplay.say(
            Doing::Fighting,
            if missile {
                format!("shooting {name}")
            } else {
                format!("attacking {name}")
            },
        );
        true
    }

    /// Fighting with spells: pick a target the same way, then throw the
    /// first spell in the list that can be cast right now.
    ///
    /// A swing sets `attack_target` and the server keeps swinging; a
    /// spell does not, so this keeps its own target and re-casts on the
    /// pace of a cast rather than of a frame. The target is selected
    /// first because that is what `try_cast` throws at.
    fn autoplay_fight_with_spells(&mut self, now: Instant, cfg: &Fight) -> bool {
        if cfg.spells.is_empty() && self.attack_spells_known().is_empty() {
            self.autoplay.say(Doing::Idle, "no attack spells known");
            return false;
        }
        if self.wielded_caster().is_none() {
            self.autoplay.say(Doing::Idle, "no caster wielded");
            return false;
        }
        let alive = |c: &Client, g: u32| {
            c.world
                .objects
                .get(&g)
                .is_some_and(|o| o.health.unwrap_or(1.0) > 0.0)
        };
        // Stay on the one already being fought while it lives, and is
        // taking damage.
        if let Some(g) = self.autoplay.casting_at {
            if self.stalled_on(g, now) {
                return false;
            }
        }
        let target = match self.autoplay.casting_at {
            Some(g) if alive(self, g) => Some(g),
            _ => {
                self.autoplay.casting_at = None;
                self.pick_target(cfg)
            }
        };
        let Some(guid) = target else {
            return false;
        };
        let name = self
            .world
            .objects
            .get(&guid)
            .map(|o| o.name.clone())
            .unwrap_or_default();
        self.autoplay.casting_at = Some(guid);
        if self
            .autoplay
            .last_cast
            .is_some_and(|t| now.duration_since(t) < CAST_EVERY)
        {
            self.autoplay
                .say(Doing::Fighting, format!("fighting {name}"));
            return true;
        }
        // Behind the throttle, so choosing a wand costs no more than one
        // look per cast. Wielding takes a moment, so this cast still
        // goes out with the old one and the next with the new.
        self.arm_for(guid, Stance::Magic);
        self.select(Some(guid));
        if self.autoplay_plan_hard(guid, &name, now) {
            return true;
        }
        // Something with a lot of health is worth softening first: one
        // vulnerability for the element it is weakest to, then throw
        // that element at it for the rest of the fight.
        if self.autoplay_make_vulnerable(guid, &name, now) {
            return true;
        }
        // Of the spells that could be thrown this moment, the one this
        // creature is hurt most by. An Ice Golem takes nothing at all
        // from cold and full damage from fire, so the difference between
        // choosing well and throwing the first spell on the list is the
        // difference between a fight and a stalemate.
        // The spells on offer: the ones named, or, with none named,
        // every attack spell in the book. The game is a closed system
        // and the book says what can be thrown.
        let table = self.assets.spell_table().ok();
        let offered: Vec<(u32, String)> = if cfg.spells.is_empty() {
            self.attack_spells_known()
                .into_iter()
                .map(|id| {
                    let name = table
                        .as_ref()
                        .and_then(|t| t.get(id).map(|s| s.name.clone()))
                        .unwrap_or_default();
                    (id, name)
                })
                .collect()
        } else {
            cfg.spells
                .iter()
                .filter_map(|n| self.spell_by_name(n).map(|id| (id, n.clone())))
                .collect()
        };
        let ready: Vec<(u32, String)> = offered
            .into_iter()
            .filter(|(id, _)| matches!(self.can_cast(*id), crate::magic::CastCheck::Ok))
            .collect();
        let ids: Vec<u32> = ready.iter().map(|(id, _)| *id).collect();
        let wcid = self
            .world
            .objects
            .get(&guid)
            .map(|o| o.weenie_class_id)
            .unwrap_or(0);
        if let Some((spell, worth)) = ac_world::elements::best_spell(wcid, &name, &ids) {
            let spell_name = ready
                .iter()
                .find(|(id, _)| *id == spell)
                .map(|(_, n)| n.clone())
                .unwrap_or_default();
            self.cast(spell);
            self.autoplay.last_cast = Some(now);
            let element = ac_world::elements::spell_element(spell)
                .map(|e| e.name())
                .unwrap_or("");
            let said = if element.is_empty() || (0.99..=1.01).contains(&worth) {
                format!("casting {spell_name} at {name}")
            } else {
                format!("casting {spell_name} at {name} ({element} x{worth:.2})")
            };
            self.autoplay.say(Doing::Fighting, said);
            return true;
        }
        // Nothing castable: out of mana, out of components, or the
        // spells are not learnt. Say which, for the first of them.
        let first: Option<(String, u32)> = if cfg.spells.is_empty() {
            self.attack_spells_known().first().map(|id| {
                let name = table
                    .as_ref()
                    .and_then(|t| t.get(*id).map(|s| s.name.clone()))
                    .unwrap_or_default();
                (name, *id)
            })
        } else {
            cfg.spells
                .iter()
                .filter_map(|n| self.spell_by_name(n).map(|id| (n.clone(), id)))
                .next()
        };
        let why = first
            .map(|(n, id)| format!("{n}: {}", cast_problem(&self.can_cast(id))))
            .unwrap_or_else(|| "none of the attack spells is known".into());
        self.autoplay
            .say(Doing::Fighting, format!("cannot cast at {name} ({why})"));
        true
    }

    /// The team's plan for a hard target. True when this tick was spent
    /// on it: either softening it, because that is this character's
    /// job, or holding fire while a teammate does. False when the fight
    /// may go ahead: an ordinary target, or a hard one already softened.
    fn autoplay_plan_hard(&mut self, guid: u32, name: &str, now: Instant) -> bool {
        if !self.is_hard_fight(guid) || self.softened_by_anyone(guid) {
            return false;
        }
        let me = self.world.stats.name.clone();
        match self.softener() {
            Some(who) if who == me => {
                // Ours to soften: a vulnerability for its weakest element
                // and an imperil, then it is marked and the others go.
                if self.autoplay_soften(guid, name, now) {
                    return true;
                }
                // Nothing castable right now: do not hold everyone up.
                if !self.autoplay.debuffed.contains(&guid) {
                    self.autoplay.debuffed.push(guid);
                }
                false
            }
            Some(who) => {
                if !self.autoplay.config.team.wait_for_debuff {
                    return false;
                }
                self.autoplay.say(
                    Doing::Helping,
                    format!("waiting for {who} to soften {name}"),
                );
                true
            }
            // Nobody can: fight it as it is.
            None => false,
        }
    }

    /// Land the vulnerability and the imperil on `guid`, one cast a
    /// tick, and mark it softened when both are on (or neither can be
    /// cast). True while there is still one to cast.
    fn autoplay_soften(&mut self, guid: u32, name: &str, now: Instant) -> bool {
        if self
            .autoplay
            .last_cast
            .is_some_and(|t| now.duration_since(t) < CAST_EVERY)
        {
            return true;
        }
        let stage = self
            .autoplay
            .softening
            .iter()
            .find(|(g, _)| *g == guid)
            .map(|(_, s)| *s)
            .unwrap_or(0);
        // Stage 0: the vulnerability. Stage 1: the imperil.
        let wcid = self
            .world
            .objects
            .get(&guid)
            .map(|o| o.weenie_class_id)
            .unwrap_or(0);
        let known = ac_world::elements::known(wcid, name);
        let candidates: Vec<u32> = if stage == 0 {
            known
                .and_then(|c| c.weakest_to())
                .map(ac_world::elements::vulnerabilities)
                .unwrap_or_default()
        } else {
            self.imperil_spells()
        };
        let spell = self.strongest_castable(&candidates);
        let advance = |this: &mut Self| {
            this.autoplay.softening.retain(|(g, _)| *g != guid);
            if stage == 0 {
                this.autoplay.softening.push((guid, 1));
            } else if !this.autoplay.debuffed.contains(&guid) {
                this.autoplay.debuffed.push(guid);
            }
        };
        match spell {
            Some(spell) => {
                if self.combat_stance() != Stance::Magic {
                    // A wand for the casting; the arming code sorts the
                    // hands out again when the fight proper begins.
                    self.wield_for(Stance::Magic);
                    return true;
                }
                self.select(Some(guid));
                self.cast(spell);
                self.autoplay.last_cast = Some(now);
                let what = if stage == 0 {
                    "vulnerability"
                } else {
                    "imperil"
                };
                self.autoplay
                    .say(Doing::Debuffing, format!("softening {name}: {what}"));
                if stage == 0 && !self.autoplay.vulned.contains(&guid) {
                    self.autoplay.vulned.push(guid);
                }
                advance(self);
                stage == 0
            }
            None => {
                // Nothing for this stage: on to the next, or done.
                advance(self);
                stage == 0 && !self.imperil_spells().is_empty()
            }
        }
    }

    /// Of `ids`, the strongest this character knows, can cast, and
    /// that takes a target. Levels come from power, never from names.
    fn strongest_castable(&self, ids: &[u32]) -> Option<u32> {
        let table = self.assets.spell_table().ok()?;
        ids.iter()
            .copied()
            .filter(|id| self.world.stats.spells.contains(id))
            .filter(|id| table.get(*id).is_some_and(|s| s.needs_target()))
            .filter(|id| {
                matches!(
                    self.can_cast(*id),
                    crate::magic::CastCheck::Ok | crate::magic::CastCheck::NoCaster
                )
            })
            .max_by_key(|id| table.get(*id).map(|s| s.power).unwrap_or(0))
    }

    /// Cast a vulnerability for the target's weakest element, once per
    /// target and only on something with health enough for the spell to
    /// pay for itself. True when a spell went out this tick.
    fn autoplay_make_vulnerable(&mut self, guid: u32, name: &str, now: Instant) -> bool {
        let least = self.autoplay.config.fight.vuln_above_health;
        if least == 0 || self.autoplay.vulned.contains(&guid) || self.softened_by_anyone(guid) {
            return false;
        }
        // Only the recent ones are worth remembering; a long session
        // should not keep every creature it has ever softened.
        if self.autoplay.vulned.len() > 64 {
            self.autoplay.vulned.drain(..32);
        }
        let known = self.creature_known(guid);
        let Some(element) = crate::weapons::vulnerability_for(known, least) else {
            // Nothing worth softening: do not ask again for this one.
            self.autoplay.vulned.push(guid);
            return false;
        };
        // The strongest one we know and can pay for. Levels are read
        // from the spell's own power, never from its name: level seven
        // has names of its own ("Curse of the Blades") and level eight
        // mixes the two.
        let table = self.assets.spell_table().ok();
        let mut known_spells: Vec<(u32, u32)> = crate::weapons::vulnerability_spells(element)
            .into_iter()
            .filter(|id| self.world.stats.spells.contains(id))
            .filter_map(|id| {
                let sp = table.as_ref()?.get(id)?;
                // Only the ones cast on someone else: the self versions
                // of these make us take more, not them.
                sp.needs_target().then_some((id, sp.level()))
            })
            .collect();
        known_spells.sort_by_key(|(_, level)| std::cmp::Reverse(*level));
        let castable = known_spells
            .into_iter()
            .find(|(id, _)| matches!(self.can_cast(*id), crate::magic::CastCheck::Ok));
        let Some((spell, level)) = castable else {
            // Cannot do it now; do not keep trying every cast.
            self.autoplay.vulned.push(guid);
            return false;
        };
        self.cast(spell);
        self.autoplay.vulned.push(guid);
        self.autoplay.last_vuln = Some(now);
        self.autoplay.last_cast = Some(now);
        let said = format!(
            "making {name} vulnerable to {} (level {level})",
            element.name()
        );
        self.autoplay.say(Doing::Debuffing, said);
        true
    }

    /// Every attack spell in the spellbook that is thrown at a target:
    /// the ones the element table knows deal an element, strongest
    /// first. Which of them to throw is decided against the target.
    fn attack_spells_known(&self) -> Vec<u32> {
        let Ok(table) = self.assets.spell_table() else {
            return Vec::new();
        };
        let mut ids: Vec<u32> = self
            .world
            .stats
            .spells
            .iter()
            .copied()
            .filter(|id| ac_world::elements::spell_element(*id).is_some())
            .filter(|id| table.get(*id).is_some_and(|s| s.needs_target()))
            .collect();
        ids.sort_by_key(|id| std::cmp::Reverse(table.get(*id).map(|s| s.power).unwrap_or(0)));
        ids
    }

    /// A swing cancels the journey (the move-to and the trip cannot both
    /// steer). Note where it was going, so it is taken up again once
    /// the fight is over.
    fn remember_journey(&mut self) {
        if self.traveling() {
            if let Some(goal) = self.travel_goal_xy() {
                self.autoplay.resume_trip = Some(goal);
            }
        }
    }

    /// Pick the journey up again after a fight, once there is nothing
    /// else to do.
    fn autoplay_resume_journey(&mut self) -> bool {
        let Some(goal) = self.autoplay.resume_trip else {
            return false;
        };
        if self.traveling() || self.attack_target.is_some() || self.autoplay.casting_at.is_some() {
            return false;
        }
        self.autoplay.resume_trip = None;
        if self.travel_to(goal) {
            self.autoplay.say(Doing::Idle, "back on the road");
            return true;
        }
        false
    }

    /// Note the target being worked on. True when it has taken no
    /// damage for too long and should be let go: it is out of reach,
    /// behind something, or not what it seems.
    fn stalled_on(&mut self, guid: u32, now: Instant) -> bool {
        let health = self
            .world
            .objects
            .get(&guid)
            .and_then(|o| o.health)
            .unwrap_or(1.0);
        match self.autoplay.engaged {
            Some((g, since, last)) if g == guid => {
                if health < last - 0.001 {
                    self.autoplay.engaged = Some((guid, now, health));
                    false
                } else if now.duration_since(since) > STALL_AFTER {
                    let name = self
                        .world
                        .objects
                        .get(&guid)
                        .map(|o| o.name.clone())
                        .unwrap_or_default();
                    self.autoplay
                        .note(format!("giving up on {name}: no damage in a while"), now);
                    self.autoplay
                        .given_up
                        .retain(|(_, t)| now.duration_since(*t) < GIVE_UP_FOR);
                    self.autoplay.given_up.push((guid, now));
                    self.autoplay.engaged = None;
                    self.attack_target = None;
                    self.autoplay.casting_at = None;
                    true
                } else {
                    false
                }
            }
            _ => {
                self.autoplay.engaged = Some((guid, now, health));
                false
            }
        }
    }

    /// The nearest creature the name rules allow, within the radius.
    fn pick_target(&self, cfg: &Fight) -> Option<u32> {
        let me = self.player.as_ref()?.world_position();
        let now = Instant::now();
        self.world
            .objects
            .values()
            .filter(|o| {
                !self
                    .autoplay
                    .given_up
                    .iter()
                    .any(|(g, t)| *g == o.guid && now.duration_since(*t) < GIVE_UP_FOR)
            })
            .filter(|o| {
                o.item_type & ac_world::item_type::CREATURE != 0
                    && o.object_desc_flags & ac_world::object_desc_flags::ATTACKABLE != 0
                    && o.object_desc_flags & ac_world::object_desc_flags::PLAYER == 0
                    && o.health.unwrap_or(1.0) > 0.0
                    && !o.is_player
            })
            .filter(|o| wanted_target(&o.name, cfg))
            .filter_map(|o| {
                let d = o.world_pos()?.distance(me);
                (d <= cfg.radius).then_some((d, o.guid))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, g)| g)
    }

    /// Note what this character is running short of, so the others can
    /// hand it over.
    fn autoplay_stock(&mut self) {
        let team = self.autoplay.config.team.clone();
        if !team.enabled {
            self.autoplay.wants.clear();
            return;
        }
        let mut wants = Vec::new();
        for (name, least) in &team.keep_stocked {
            if name.trim().is_empty() {
                continue;
            }
            let have: u32 = self
                .world
                .inventory()
                .filter(|o| o.name.to_lowercase().contains(&name.to_lowercase()))
                .map(|o| o.stack_size.max(1))
                .sum();
            if have < *least {
                wants.push(name.clone());
            }
        }
        self.autoplay.wants = wants;
    }

    /// A fellowship invitation from anyone is accepted while on a team:
    /// the leader sends them, and the leader is trusted.
    fn autoplay_accept_invites(&mut self) {
        const FELLOWSHIP: u32 = 4;
        let invites: Vec<(u32, u32)> = self
            .world
            .confirmations
            .iter()
            .filter(|c| c.kind == FELLOWSHIP)
            .map(|c| (c.kind, c.context))
            .collect();
        for (kind, context) in invites {
            self.confirm(kind, context, true);
        }
    }

    /// The leader brings the others into a fellowship, one invitation
    /// every few seconds. True when one went out.
    fn autoplay_fellowship(&mut self, now: Instant) -> bool {
        let team = self.autoplay.config.team.clone();
        if !team.enabled || !team.fellowship || !self.autoplay.team.leader {
            return false;
        }
        let Some(me) = self.player.as_ref().map(|p| p.world_position()) else {
            return false;
        };
        if self
            .autoplay
            .last_recruit
            .is_some_and(|t| now.duration_since(t) <= Duration::from_secs(5))
        {
            return false;
        }
        let mates: Vec<(u32, String)> = self
            .autoplay
            .team
            .mates
            .iter()
            .filter(|m| !m.in_fellowship && m.guid != 0 && m.world.distance(me) < 25.0)
            .map(|m| (m.guid, m.name.clone()))
            .collect();
        let Some((guid, name)) = mates.first().cloned() else {
            return false;
        };
        if self.world.fellowship.is_none() {
            let fname = team.fellowship_name.clone();
            self.fellowship_create(&fname, true);
        } else {
            self.fellowship_recruit(guid);
        }
        self.autoplay.last_recruit = Some(now);
        self.autoplay.say(
            Doing::Helping,
            format!("bringing {name} into the fellowship"),
        );
        true
    }

    /// Keep up with the leader: fly when it flies, walk straight after
    /// it while it is near, plan a journey after it when it has gone
    /// through a portal. With `urgent`, only a leader that has got well
    /// away counts (it is fetched before a fight); otherwise any leader
    /// further than the following distance. True while on the way.
    fn autoplay_follow(&mut self, now: Instant, urgent: bool) -> bool {
        let team = self.autoplay.config.team.clone();
        if !team.enabled || !team.follow || team.lead || self.autoplay.team.leader {
            return false;
        }
        let Some(leader) = self
            .autoplay
            .team
            .leader_mate()
            .filter(|m| m.leads)
            .cloned()
        else {
            return false;
        };
        let Some(me) = self.player.as_ref().map(|p| p.world_position()) else {
            return false;
        };
        let keep = team.follow_distance.max(1.5);
        // Fly when the leader flies, land when it lands.
        if leader.flying != self.noclip() {
            self.set_noclip(leader.flying);
        }
        let flat = glam::Vec2::new(leader.world.x - me.x, leader.world.y - me.y).length();
        let far = flat > FOLLOW_BREAK;
        if urgent && !far {
            return false;
        }
        let level = !leader.flying || (leader.world.z - me.z).abs() < 2.0;
        if flat <= keep && level {
            if self.follow.take().is_some() {
                self.steering.reset();
            }
            if self.autoplay.follow_trip.take().is_some() && self.traveling() {
                self.cancel_travel();
            }
            return false;
        }
        if far {
            // Whatever we were fighting is not worth losing the leader.
            self.autoplay.casting_at = None;
            self.attack_target = None;
        }
        if leader.flying || flat < FOLLOW_WALK {
            // Straight after it: the steering finds the way round
            // walls, and flight has nothing in the way.
            if self.autoplay.follow_trip.take().is_some() && self.traveling() {
                self.cancel_travel();
            }
            self.follow = Some(crate::Follow {
                target: leader.world,
                stop: keep,
            });
        } else {
            // Out of sight (through a portal, say): a journey there,
            // planned again once it has moved on. Not while it stands
            // somewhere a journey cannot end -- inside the Town Network
            // hub, or a dungeon -- in another landblock: it will come
            // out, and its last position outside is followed meanwhile.
            self.follow = None;
            let indoors = leader.cell & 0xFFFF >= 0x100;
            let my_block = self.player.as_ref().map(|p| p.landblock());
            if indoors && my_block != Some(leader.cell & 0xFFFF_0000) {
                self.autoplay
                    .say(Doing::Following, "waiting for the leader to come out");
                return false;
            }
            let goal = glam::Vec2::new(leader.world.x, leader.world.y);
            let stale = self
                .autoplay
                .follow_trip
                .is_none_or(|g| g.distance(goal) > 30.0);
            let due = self.autoplay.next_follow_plan.is_none_or(|t| now >= t);
            if (stale || !self.traveling()) && due {
                if self.travel_to(goal) {
                    self.autoplay.follow_trip = Some(goal);
                    self.autoplay.next_follow_plan = Some(now + Duration::from_secs(3));
                } else {
                    self.autoplay.follow_trip = None;
                    self.autoplay.next_follow_plan = Some(now + Duration::from_secs(10));
                }
            }
        }
        self.autoplay
            .say(Doing::Following, format!("following {}", leader.name));
        true
    }

    /// The things done for the team: land the debuffs on its target,
    /// recruit it into a fellowship, hand over what someone is short of,
    /// and heal whoever is worst hurt. True when it acted.
    fn autoplay_team(&mut self, now: Instant) -> bool {
        let team = self.autoplay.config.team.clone();
        if !team.enabled {
            return false;
        }
        let me = self.player.as_ref().map(|p| p.world_position());
        let Some(me) = me else { return false };

        if self.autoplay_fellowship(now) {
            return true;
        }

        // Hand over what a teammate is short of.
        if team.share_supplies
            && self
                .autoplay
                .last_give
                .is_none_or(|t| now.duration_since(t) > Duration::from_secs(3))
        {
            let mate = self.autoplay.team.wanting(me, 6.0).cloned();
            if let Some(mate) = mate {
                for want in &mate.wants {
                    let spare = self
                        .world
                        .inventory()
                        .filter(|o| o.name.to_lowercase().contains(&want.to_lowercase()))
                        .map(|o| (o.guid, o.name.clone()))
                        .next();
                    if let Some((item, name)) = spare {
                        self.give(mate.guid, item, None);
                        self.autoplay.last_give = Some(now);
                        self.autoplay
                            .say(Doing::Helping, format!("giving {name} to {}", mate.name));
                        return true;
                    }
                }
            }
        }

        // A healer looks after the others before it fights.
        if team.role == Role::Healer {
            let hurt = self
                .autoplay
                .team
                .worst_hurt()
                .filter(|m| m.health < self.autoplay.config.survive.heal_below)
                .cloned();
            if let Some(hurt) = hurt {
                let spell = self
                    .spell_by_name("Heal Other")
                    .filter(|s| matches!(self.can_cast(*s), crate::magic::CastCheck::Ok));
                if let Some(spell) = spell {
                    self.select(Some(hurt.guid));
                    self.cast(spell);
                    self.autoplay.last_heal = Some(now);
                    self.autoplay
                        .say(Doing::Healing, format!("healing {}", hurt.name));
                    return true;
                }
            }
        }

        // A debuffer softens the team's target before the others hit it.
        if team.role == Role::Debuffer && !team.debuffs.is_empty() {
            if self
                .autoplay
                .last_debuff
                .is_some_and(|t| now.duration_since(t) < BUFF_EVERY)
            {
                return false;
            }
            let target = self.autoplay.team.target().or_else(|| {
                self.attack_target
                    .map(|t| (t, self.last_target_name.clone()))
            });
            if let Some((guid, name)) = target {
                let alive = self
                    .world
                    .objects
                    .get(&guid)
                    .is_some_and(|o| o.health.unwrap_or(1.0) > 0.0);
                if alive && !self.autoplay.debuffed.contains(&guid) {
                    for spell_name in &team.debuffs {
                        let Some(spell) = self.spell_by_name(spell_name) else {
                            continue;
                        };
                        if !matches!(self.can_cast(spell), crate::magic::CastCheck::Ok) {
                            continue;
                        }
                        self.select(Some(guid));
                        self.cast(spell);
                        self.autoplay.last_debuff = Some(now);
                        let spell_name = spell_name.clone();
                        self.autoplay
                            .say(Doing::Debuffing, format!("casting {spell_name} on {name}"));
                        // One family per target: the rest of the team can
                        // stop waiting for us.
                        if team.debuffs.last() == Some(&spell_name) {
                            self.autoplay.debuffed.push(guid);
                        }
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Put a buff back up. True when it cast one.
    ///
    /// Two passes share this. The urgent one runs before anything else
    /// each tick and puts back whatever is under `never_below`, in a
    /// fight or out of one, swapping to a wand if the hands hold
    /// something else: a buff is never allowed to run out. The other
    /// runs last, in quiet moments, and tops up whatever is under the
    /// much wider `top_up_within`, a cast or two at a time, so the set
    /// is refreshed a little at every lull rather than all at once.
    fn autoplay_buff(&mut self, now: Instant, urgent: bool) -> bool {
        let cfg = self.autoplay.config.buffs.clone();
        if cfg.spells.is_empty() && !cfg.auto {
            return false;
        }
        let fighting = self.attack_target.is_some() || self.autoplay.casting_at.is_some();
        if !urgent && cfg.out_of_combat_only && fighting {
            return false;
        }
        // On a journey the top-ups wait: every cast roots the character
        // where it stands, and a character with a hundred buffs to put
        // back would never leave town. What is about to run out still
        // goes back up on the way.
        if !urgent && self.traveling() {
            return false;
        }
        if self
            .autoplay
            .last_buff
            .is_some_and(|t| now.duration_since(t) < BUFF_EVERY)
        {
            return false;
        }
        if urgent
            && self
                .autoplay
                .buffs_checked
                .is_some_and(|t| now.duration_since(t) < BUFF_CHECK_EVERY)
        {
            return false;
        }
        self.autoplay.buffs_checked = Some(now);
        let within = if urgent {
            cfg.never_below
        } else {
            cfg.top_up_within
        };
        // Find what is due before touching the hands: the urgent pass
        // runs every tick and must cost nothing when nothing is due.
        let Some((spell, target, category, name, lasts)) = self.due_buff(within, now) else {
            if !urgent {
                self.autoplay_explain_buffs(now);
            }
            return false;
        };
        // Mana is kept back for healing and fighting: a top-up waits
        // until there is that much to spare, and even an urgent recast
        // leaves half of it.
        let reserve = self.vital_max_of(2) as f32 * cfg.keep_mana * if urgent { 0.5 } else { 1.0 };
        let cost = self
            .assets
            .spell_table()
            .ok()
            .and_then(|t| t.get(spell).map(|s| self.mana_cost(s)))
            .unwrap_or(0) as f32;
        let have = self.world.stats.vitals[2].current as f32;
        if have - cost < reserve {
            self.autoplay
                .note(format!("holding off {name} to keep mana back"), now);
            return false;
        }
        // A wand is needed to cast. Out of a fight the arming code sorts
        // the hands out afterwards; in one, remember what was put down
        // so it is taken up again the moment the buffing is done.
        if self.combat_stance() != Stance::Magic {
            let held = self
                .world
                .wielded()
                .find(|o| {
                    o.item_type
                        & (ac_world::item_type::MELEE_WEAPON | ac_world::item_type::MISSILE_WEAPON)
                        != 0
                })
                .map(|o| o.guid);
            if !self.wield_for(Stance::Magic) {
                self.autoplay
                    .say(Doing::Buffing, format!("no wand to cast {name} with"));
                return false;
            }
            if fighting && self.autoplay.put_down.is_none() {
                self.autoplay.put_down = held;
            }
            // The wield takes a moment; cast next tick.
            self.autoplay.last_buff = Some(now);
            return true;
        }
        if !matches!(self.can_cast(spell), crate::magic::CastCheck::Ok) {
            return false;
        }
        use crate::buffs::Target;
        match target {
            Target::Me => {
                self.cast(spell);
                self.autoplay.say(Doing::Buffing, format!("casting {name}"));
            }
            Target::Item(g) => {
                self.cast_at(spell, g);
                self.autoplay
                    .item_buffs
                    .retain(|(i, c, _, _)| !(*i == g && *c == category));
                self.autoplay.item_buffs.push((g, category, now, lasts));
                let on = self
                    .world
                    .objects
                    .get(&g)
                    .map(|o| o.name.clone())
                    .unwrap_or_default();
                self.autoplay
                    .say(Doing::Buffing, format!("casting {name} on {on}"));
            }
        }
        self.autoplay.last_buff = Some(now);
        true
    }

    /// When buffs are wanted but none can be cast, say why for the
    /// first of them, so a character standing unbuffed is not a mystery.
    fn autoplay_explain_buffs(&mut self, now: Instant) {
        use crate::buffs::Target;
        use crate::magic::CastCheck;
        if !self.autoplay.config.buffs.auto {
            return;
        }
        let top_up = self.autoplay.config.buffs.top_up_within;
        for want in self.wanted_buffs() {
            let left = match want.target {
                Target::Me => self.category_left(want.category, want.power),
                Target::Item(g) => self.item_buff_left(g, want.category, now),
            };
            if left.is_some_and(|l| l > top_up) {
                continue;
            }
            let check = self.can_cast(want.spell);
            if matches!(check, CastCheck::Ok | CastCheck::NoCaster) {
                continue;
            }
            let why = cast_problem(&check);
            let name = self
                .assets
                .spell_table()
                .ok()
                .and_then(|t| t.get(want.spell).map(|s| s.name.clone()))
                .unwrap_or_default();
            self.autoplay
                .note(format!("cannot buff: {name}, {why}"), now);
            return;
        }
    }

    /// The maximum of a vital (0 health, 1 stamina, 2 mana).
    fn vital_max_of(&self, i: usize) -> u32 {
        self.world.stats.vital_max(i)
    }

    /// Take up again the weapon put down for an urgent buff, once no
    /// buff is due any more.
    fn autoplay_rearm(&mut self) {
        let Some(weapon) = self.autoplay.put_down else {
            return;
        };
        let never_below = self.autoplay.config.buffs.never_below;
        if self.due_buff(never_below, Instant::now()).is_some() {
            return;
        }
        self.autoplay.put_down = None;
        if self
            .world
            .objects
            .get(&weapon)
            .is_some_and(|o| o.container == self.world.player_guid)
        {
            tracing::info!("autoplay: taking the weapon up again after buffing");
            self.wield_guid(weapon);
        }
    }

    /// The buff with the least time left of those under `within`
    /// seconds, castable or not: the most pressing one is put back
    /// first. `(spell, target, category, name, seconds it lasts)`.
    fn due_buff(
        &self,
        within: f32,
        now: Instant,
    ) -> Option<(u32, crate::buffs::Target, u32, String, f32)> {
        use crate::buffs::Target;
        let cfg = &self.autoplay.config.buffs;
        let table = self.assets.spell_table().ok();
        let mut due: Option<(f32, u32, Target, u32, String, f32)> = None;
        let clock = self.session.server_time().is_some();
        let mut offer = |left: Option<f32>, spell: u32, target: Target, category: u32| {
            // Not up at all is due now; but until the server's clock is
            // known nothing can be told apart, so nothing is due.
            let left = match left {
                Some(l) => l,
                None if clock => 0.0,
                None => return,
            };
            if left > within {
                return;
            }
            // One that cannot be cast must not stand in front of the
            // rest: short of components or mana it is passed over. No
            // wand is different, since wielding one is the cure.
            match self.can_cast(spell) {
                crate::magic::CastCheck::Ok | crate::magic::CastCheck::NoCaster => {}
                _ => return,
            }
            if due.as_ref().is_some_and(|d| d.0 <= left) {
                return;
            }
            let sp = table.as_ref().and_then(|t| t.get(spell));
            let name = sp.map(|s| s.name.clone()).unwrap_or_default();
            let lasts = sp.and_then(|s| s.duration()).unwrap_or(1800.0) as f32;
            due = Some((left, spell, target, category, name, lasts));
        };
        if cfg.auto {
            for want in self.wanted_buffs() {
                let left = match want.target {
                    Target::Me => self.category_left(want.category, want.power),
                    Target::Item(g) => self.item_buff_left(g, want.category, now),
                };
                offer(left, want.spell, want.target, want.category);
            }
        }
        for name in &cfg.spells {
            let Some(spell) = self.spell_by_name(name) else {
                continue;
            };
            let category = table
                .as_ref()
                .and_then(|t| t.get(spell))
                .map(|s| s.category)
                .unwrap_or(0);
            offer(self.buff_left(spell), spell, Target::Me, category);
        }
        due.map(|(_, spell, target, category, name, lasts)| (spell, target, category, name, lasts))
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
