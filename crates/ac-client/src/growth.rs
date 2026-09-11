//! Keeping a character that plays on its own going for hours: the
//! rules of `crate::autoplay` fight, loot and buff, and these make sure
//! there is always something worth fighting, a pack to put the loot in,
//! and a character that grows with the experience it earns.
//!
//! Three rules, run when nothing more pressing is going on (see
//! [`Client::autoplay_grow`]):
//!
//! 1. **Spend experience.** The unassigned pool is spent a rank at a
//!    time, the way a player would: the skills it fights with first (the
//!    weapon in hand, the defences, the magic it casts), the attributes
//!    those skills are built from next, then the rest. Each candidate
//!    rank is priced from the XpTable and weighed by how much it matters,
//!    and the best value is bought; one rank at a time, no more often
//!    than the server can answer.
//! 2. **Go where the monsters are.** When nothing has been worth fighting
//!    for a while, a hunting ground that suits the character's level is
//!    picked from `ac_world::hunting` (the nearest, leaving out the one
//!    just hunted out and any that could not be reached) and travelled
//!    to. Fights on the way are fought; the journey is picked up again
//!    after each.
//! 3. **Run to town.** When the pack is full, or something the character
//!    lives on is short and cannot be made from what it carries
//!    (healing kits, arrows, spell components), it walks to the nearest
//!    vendor, sells the loot it has no use for, buys what it is short of,
//!    and goes back to its hunting ground. A run visits up to a few
//!    vendors of the same town, since no one shop buys everything or
//!    sells everything.
//!
//! What is sold is decided by searches in the inventory's own language,
//! the way loot is chosen (`Growth::sell`), with the things a character
//! cannot do without kept back whatever the searches say: what it
//! wears and wields, money, packs, components, ammunition, tools, the
//! items it keeps stocked, and weapons it could wield. A weapon it
//! cannot wield is loot.

use std::time::{Duration, Instant};

use glam::Vec2;
use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

use crate::autoplay::{name_matches, Doing, LootAction};
use crate::items::{ItemStats, Query};
use crate::logistics::{self, Stage, Supplies};
use crate::Client;
use ac_world::{equip, item_type, object_desc_flags};

/// One rank is bought at most this often.
const RAISE_EVERY: Duration = Duration::from_millis(700);
/// After a rank is bought, nothing more is spent until the server has
/// answered with the new pool, or this long has passed.
const RAISE_SETTLE: Duration = Duration::from_secs(3);
/// When nothing was affordable, the pool is looked at again this often
/// (or as soon as it changes).
const XP_CHECK_EVERY: Duration = Duration::from_secs(20);
/// Standing this close to a ground's middle counts as being there.
const GROUND_REACH: f32 = 30.0;
/// A ground the character has just left is not gone back to for this
/// long; one it could not reach, the same.
const SKIP_GROUND_FOR: Duration = Duration::from_secs(20 * 60);
/// Standing this close to a vendor is close enough to use it: the
/// server walks the character the last stretch itself.
const VENDOR_REACH: f32 = 35.0;
/// How close to stand before asking a counter to open. A vendor will
/// not trade with somebody across the room, and says so by telling the
/// character to walk there rather than by refusing.
pub(crate) const COUNTER_REACH: f32 = 3.0;
/// A vendor that does not answer a Use in this long is tried once more,
/// then left.
const VENDOR_OPEN_TIMEOUT: Duration = Duration::from_secs(12);
/// A rank the server would not sell (the pool did not move) is not
/// asked for again for this long.
const SULK_FOR: Duration = Duration::from_secs(10 * 60);
/// What the character is short of is worked out at most this often
/// while it waits for something to do.
const NEEDS_EVERY: Duration = Duration::from_secs(5);
/// At a ground with nothing in sight, the character walks this far to
/// look, this many times, before it moves on: the spawns of a block
/// are spread over it and the fight rules only look a short way.
const ROAM: f32 = 70.0;
const ROAMS: u32 = 3;
/// Sales go out a few at a time, this often.
const SELL_EVERY: Duration = Duration::from_millis(500);
/// How long to wait for the server to take the items sold, the
/// appraisals to come back, or the purchases to arrive.
const SETTLE: Duration = Duration::from_secs(5);
/// How long to leave it after a trip to town that bought and sold
/// nothing. Long enough that a character which cannot afford what it
/// needs goes back to earning instead of shuttling between counters.
const FUTILE_RUN_WAIT: Duration = Duration::from_secs(300);
/// How often a character that has stopped in town looks to see whether
/// its luck has changed.
const STOPPED_LOOK_EVERY: Duration = Duration::from_secs(5);
/// Least time between two town runs. A run that could sell nothing
/// leaves the pack as full as it found it, and the next is not until
/// this has passed.
const RUN_EVERY: Duration = Duration::from_secs(8 * 60);
/// Most vendors visited in one run.
const STOPS_PER_RUN: u32 = 3;
/// A walk to a vendor, or to a hunting ground, that has taken longer
/// than this is stuck somewhere: the place is given up on.
const WALK_TIMEOUT: Duration = Duration::from_secs(4 * 60);
/// After a journey that could not be planned, the next place is not
/// tried for this long.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// The next vendor of a run must be within this of the first: the same
/// town, not the next one.
const SAME_TOWN: f32 = 300.0;
/// The vendors' unlimited-stock marker.
const UNLIMITED_STACK: u32 = 0x00FF_FFFF;

/// A distance for a status line, in steps of fifty metres, so the line
/// changes (and is logged) now and then rather than every frame.
fn about(distance: f32) -> String {
    format!("{} m", ((distance / 50.0).ceil() * 50.0) as u32)
}

/// Growing the character and keeping it supplied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Growth {
    /// Spend unassigned experience on skills, attributes and vitals.
    pub auto_xp: bool,
    /// Go to a hunting ground that suits the level when nothing is about.
    pub hunt_grounds: bool,
    /// How many levels either side of the character's a ground may be
    /// (see `ac_world::hunting::Ground::suits`).
    pub level_margin: u32,
    /// Hunt this landblock and no other, 0 to pick whatever suits.
    /// A party follows its leader's choice.
    #[serde(default)]
    pub hunt_at: u32,
    /// How to hunt the ground once there.
    #[serde(default)]
    pub tactic: ac_world::hunting::Tactic,
    /// Seconds with nothing to fight before moving on.
    pub idle_before_move: f32,
    /// Walk to a vendor when the pack is full or supplies are short.
    pub town_runs: bool,
    /// Keep at least this many of each, by name, buying them in town
    /// ("Healing Kit", 2).
    pub keep_stocked: Vec<(String, u32)>,
    /// How much ammunition to carry for a bow or crossbow. A run to
    /// town is made when a quarter of this is left and none can be
    /// made from what is carried.
    pub ammo_keep: u32,
    /// How many Prismatic Tapers to carry, for a character that casts.
    /// Everything else in its formulas is scaled to this by how fast it
    /// burns (see [`Client::component_targets`]), because a taper is
    /// what a caster actually runs out of: with foci a cast burns about
    /// 0.4 of a taper and 0.003 of a scarab, so one number for both is
    /// wrong in both directions. A run is made when any is down to a
    /// quarter.
    #[serde(alias = "comps_keep")]
    pub tapers_keep: u32,
    /// Quest flags this character has earned, for the counters that
    /// ask for one (the Rossu Morta and Whispering Blade chapter
    /// houses, the Academy stores, and so on -- see
    /// `ac_world::shops::Gate`).
    ///
    /// The server never tells a client which quests it has done, so
    /// this is the player's word for it. Empty means those doors are
    /// treated as shut, which costs a walk to the next counter rather
    /// than a walk to a door that will not open. Society membership is
    /// not listed here: the character carries that itself.
    pub gates_open: Vec<String>,
    /// Sell what matches any of these searches (the inventory's
    /// language: `type:armor`, `value<50`).
    pub sell: Vec<String>,
    /// Never sell these, by name, whatever the searches say.
    pub keep: Vec<String>,
}

impl Default for Growth {
    fn default() -> Self {
        Growth {
            auto_xp: true,
            hunt_grounds: true,
            level_margin: 8,
            hunt_at: 0,
            tactic: ac_world::hunting::Tactic::default(),
            idle_before_move: 60.0,
            town_runs: true,
            keep_stocked: vec![("Healing Kit".into(), 2)],
            ammo_keep: 250,
            tapers_keep: 1000,
            gates_open: Vec::new(),
            // Vendor trash only. Gear worth keeping is left alone:
            // spelled armour and jewelry never match (see
            // `storage_worthy`), and what is left is capped by value so
            // that a good drop is carried home rather than sold for
            // pocket change. Peas and their like are what a run to town
            // is actually paid for with.
            sell: vec![
                "type:junk".into(),
                "type:misc".into(),
                "type:gem".into(),
                "type:food".into(),
                "type:armor value<2500".into(),
                "type:clothing value<2500".into(),
                "type:jewelry value<2500".into(),
                "type:weapon value<2500".into(),
                "type:missile value<2500".into(),
                "type:caster value<2500".into(),
            ],
            keep: Vec::new(),
        }
    }
}

/// One thing experience can be spent on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Raise {
    Skill(u32),
    /// Index into `advance::ATTRIBUTE_NAMES`.
    Attribute(usize),
    /// Index into `advance::VITAL_NAMES`.
    Vital(usize),
}

/// A rank on offer: what it is, what it costs, and how much it matters
/// (1 for the skill the character fights with, less for the rest).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Offer {
    pub raise: Raise,
    pub cost: u32,
    pub weight: f32,
}

/// The rank worth buying with `xp` to spend: of the affordable ones,
/// the cheapest for what it is worth. A cheap rank of a minor skill is
/// bought before a dear rank of the main one, and the main one catches
/// up as the minor ones get dear; nothing is bought when nothing can be
/// afforded.
pub fn choose_raise(offers: &[Offer], xp: i64) -> Option<Raise> {
    offers
        .iter()
        .filter(|o| o.weight > 0.0 && (o.cost as i64) <= xp)
        .min_by(|a, b| {
            (a.cost as f32 / a.weight)
                .total_cmp(&(b.cost as f32 / b.weight))
                .then(a.cost.cmp(&b.cost))
        })
        .map(|o| o.raise)
}

/// How much a skill matters to a character fighting with `weapon_skill`
/// (0 for a caster's wand), given the schools it casts with.
fn skill_weight(skill: u32, weapon_skill: Option<u32>, caster: bool) -> f32 {
    use ac_world::stats::skill;
    const CREATURE_ENCHANTMENT: u32 = 31;
    const ITEM_ENCHANTMENT: u32 = 32;
    const VOID_MAGIC: u32 = 43;
    if Some(skill) == weapon_skill {
        return 1.0;
    }
    match skill {
        skill::WAR_MAGIC | VOID_MAGIC => {
            if caster {
                1.0
            } else {
                0.15
            }
        }
        skill::LIFE_MAGIC => {
            if caster {
                0.9
            } else {
                0.6
            }
        }
        skill::MELEE_DEFENSE | skill::MISSILE_DEFENSE => 0.7,
        15 => 0.6, // Magic Defense
        skill::HEALING => 0.5,
        CREATURE_ENCHANTMENT | ITEM_ENCHANTMENT => 0.5,
        skill::MANA_CONVERSION => {
            if caster {
                0.6
            } else {
                0.2
            }
        }
        skill::FLETCHING => {
            if weapon_skill == Some(47) {
                0.5
            } else {
                0.1
            }
        }
        skill::RUN => 0.3,
        // Another weapon skill: not the one in hand.
        41 | 44..=50 => 0.1,
        _ => 0.15,
    }
}

/// Where a town run stands.
#[derive(Clone, Debug, PartialEq)]
enum Phase {
    /// Walking to the vendor.
    Going,
    /// The vendor was used; waiting for its stock.
    Opening { guid: u32, tries: u32 },
    /// The pack's items are being appraised before the sale.
    Appraising,
    /// Selling. `sent` is what has gone over the counter and not yet
    /// left the pack -- nothing else is remembered, because what there
    /// is to sell is asked of the pack afresh every turn.
    Selling { sent: Vec<u32> },
}

/// A run to town in progress.
#[derive(Clone, Debug, PartialEq)]
struct Run {
    vendor: String,
    at: Vec2,
    phase: Phase,
    /// When the phase began.
    since: Instant,
    last_sell: Option<Instant>,
    /// Where the run started, to keep its later stops in the same town.
    town: Vec2,
    stops: u32,
    /// How many items were sold at all the stops so far.
    sold: u32,
    /// Why the run was made.
    reason: String,
    /// The vendors already called on this run, by position.
    visited: Vec<Vec2>,
}

/// The running state of the growth rules.
#[derive(Default)]
pub struct State {
    /// What the party is doing, as this session last worked it out from
    /// the roster. Every session reaches the same answer, so this is a
    /// cache of a shared decision rather than a vote of its own.
    pub mode: crate::logistics::GroupMode,
    /// When the mode last changed, and why, for the log and the UI.
    pub mode_since: Option<Instant>,
    pub mode_because: String,
    /// This character has given the quartermaster its sale loot and its
    /// order. Cleared whenever the mode changes.
    pub handed_over: bool,
    /// Something was actually bought on the run in progress.
    pub bought_anything: bool,
    /// How many trips the quartermaster has made this time out.
    pub round: u32,
    /// The character came home from a counter too laden to be handed
    /// anything. Nothing at a shop changes that, so it does not go
    /// back until it has sold or used something.
    pub too_heavy: bool,
    /// Stacks the server would not join, so the tidying does not ask
    /// for ever. Prismatic Tapers at a hundred and two hundred should
    /// merge and the asking should stop when they do not.
    wont_merge: crate::did::Patience<(u32, u32)>,
    /// The shopping, as decided by `ac-vendor`. It holds what a
    /// snapshot cannot show: which phase the trip is in, what has been
    /// handed over and not yet answered for, and what has been refused.
    pub shop: ac_vendor::Run,
    /// The last thing the shopping rules said they were doing, for the
    /// log and the panel.
    pub last_saying: String,
    /// The character is underground. Worked out on the tick (it needs
    /// the block's collision, which is loaded lazily) and read by the
    /// rules that must not plan a walk out of a dungeon.
    pub in_dungeon: bool,
    /// The last run to town ended without buying or selling anything.
    /// Another one straight away would do the same, so it waits.
    pub run_was_futile: bool,
    /// The character has given up, in town. Out of money, with nothing
    /// left to sell and too short of supplies to go on hunting, it
    /// stops where the shops are -- which is where the money and the
    /// goods are, and where whoever is watching can put it right --
    /// rather than walk back to a hunting ground it cannot work.
    pub stopped_in_town: bool,
    /// When it last looked to see whether that had changed. Counting
    /// the pack is not free and it is standing still.
    stopped_looked: Option<Instant>,
    /// Why no run to town was started, as last logged. A run is decided
    /// once a frame and the answer is usually the same one; this keeps
    /// the log to the moments it changes.
    held_back: String,
    last_raise: Option<Instant>,
    /// The pool as it stood when the last rank was bought, and when.
    raise_pending: Option<(i64, Instant)>,
    /// The pool at which nothing was affordable, and when that was seen.
    nothing_at: Option<(i64, Instant)>,
    /// The rank last asked for, so one the server would not sell can be
    /// left alone.
    last_pick: Option<Raise>,
    /// Ranks the server would not sell, and when it was asked.
    sulking: Vec<(Raise, Instant)>,
    /// What the character was last found short of, and when.
    needs_seen: Option<(Instant, Vec<Need>)>,
    /// How many times the character has walked about the ground it is
    /// on looking for something to fight.
    roams: u32,
    /// Since when there has been nothing to do.
    idle_since: Option<Instant>,
    /// The ground being travelled to, and since when.
    bound: Option<(u32, Vec2, String)>,
    bound_since: Option<Instant>,
    /// The ground the character hunts on, once it has arrived.
    pub hunting_at: Option<u32>,
    /// Grounds not to go to for a while, and since when.
    skip: Vec<(u32, Instant)>,
    run: Option<Run>,
    last_run: Option<Instant>,
    /// Counters not to walk to for a while, keyed by where they stand
    /// rounded to the metre. A counter that could not be reached, or
    /// that came to nothing, is left alone and tried again later; each
    /// further disappointment doubles the wait.
    skip_vendors: crate::did::Patience<(i32, i32)>,
    /// Items no vendor will take. Refusals reach us after the item's
    /// kind and worth have already been checked against the counter,
    /// so what is left is the item saying no for itself -- the Academy
    /// bread, a quest token -- and no other counter will take it
    /// either. Remembered for the whole session, or the character
    /// offers the same loaf in every town.
    unsellable: crate::did::Patience<u32>,
    /// No run is started before this: a vendor that could not be
    /// reached is not tried again at once.
    next_run: Option<Instant>,
    /// No ground is chosen before this, for the same reason.
    next_hunt: Option<Instant>,
    /// Where the character last stood outdoors. A journey out of a
    /// building begins with a walk back to this spot, since the
    /// planner cannot see out of a shop.
    last_outdoors: Option<Vec2>,
    /// The journey to make once outside.
    after_out: Option<Vec2>,
    /// What the character was short of when the run began, for the
    /// stops after the first.
    needs: Vec<Need>,
}

/// Something the character is short of.
#[derive(Clone, Debug, PartialEq)]
struct Need {
    /// What it is, for the log.
    name: String,
    /// How many more to buy.
    want: u32,
    /// How many are carried, and how many the rules ask for. The
    /// shortfall alone does not say how close to empty a line is, and
    /// that is what decides whether the party stops hunting.
    have: u32,
    keep: u32,
    /// Whether it is short enough to be worth a run to town.
    urgent: bool,
    /// Whether enough of the world sells it to go and buy it. What is
    /// not is farmed instead: the Void components, and in practice the
    /// Diamond Scarab, whose one seller is a curiosity shop. A line
    /// like that must never drive a trip and must never be counted
    /// against the character's supplies, or a caster is out of stock
    /// for ever and the party restocks for ever.
    buyable: bool,
    kind: NeedKind,
}

#[derive(Clone, Debug, PartialEq)]
enum NeedKind {
    /// Stock whose name contains this.
    Named(String),
    /// Ammunition of this kind (`ac_world::fletching::ammo_type`).
    Ammo(u32),
    /// A spell component, by the weenie class of the item.
    Component(u32),
}

/// Which way out leaves the character nearest `at`, how far that is,
/// and what it is called.
fn nearest_way(ways: &[(Vec2, String)], at: Vec2) -> (Vec2, f32, String) {
    ways.iter()
        .map(|(w, name)| (*w, at.distance(*w), name.clone()))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((at, 0.0, "here".to_string()))
}

/// What a vendor's stock line is, for matching against needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Stock {
    pub guid: u32,
    pub name: String,
    pub wcid: u32,
    /// `ac_world::item_type` bits: what tells a trade note from the
    /// rest of the shelf.
    pub item_type: u32,
    /// What one costs at this vendor.
    pub price: u32,
    /// How many it has; `None` for unlimited.
    pub stack: Option<u32>,
    /// What one of them weighs. A purse is not the only thing a trip
    /// runs out of.
    pub burden: u32,
}

/// A few names, and how many more there are. A caster short of every
/// component in the book has thirty-one of them, and a line of the log
/// is not the place to read all thirty-one.
fn a_few(names: &[&str]) -> String {
    const MOST: usize = 4;
    if names.is_empty() {
        return "nothing".to_string();
    }
    if names.len() <= MOST {
        return names.join(", ");
    }
    format!(
        "{} and {} more",
        names[..MOST].join(", "),
        names.len() - MOST
    )
}

/// `haystack` contains `needle`, ignoring ASCII case; `needle` is
/// already lowercase.
///
/// Written out rather than lowercasing both sides, which allocates:
/// choosing which shop to walk to compares every need against every
/// ware of a thousand counters.
fn contains_fold(haystack: &str, needle: &str) -> bool {
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    if n.is_empty() || n.len() > h.len() {
        return false;
    }
    h.windows(n.len())
        .any(|w| w.iter().zip(n).all(|(a, b)| a.eq_ignore_ascii_case(b)))
}

/// Something in the pack the rules allow to be sold, before any one
/// counter's tastes are applied to it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Salable {
    pub guid: u32,
    /// `ac_world::item_type` bits.
    pub item_type: u32,
    /// What the whole stack is worth. The server counts a stack's value
    /// as the lot, not as one of them, and a counter's limit is on that
    /// figure -- which is why a hundred Pyreal Peas worth five million
    /// are refused by a counter that will not look at anything over a
    /// million.
    pub value: u32,
    pub stack: u32,
}

impl Salable {
    /// What one of them is worth.
    pub fn each(&self) -> u32 {
        self.value / self.stack.max(1)
    }

    /// How many of them a counter with this ceiling will take at once.
    /// Zero when it will not take even one.
    ///
    /// A stack worth more than the ceiling is not refused for good: it
    /// is split and sold a handful at a time, which is what a player
    /// does and what the counter's limit is for.
    pub fn at_once(&self, max_value: u32) -> u32 {
        if max_value == 0 {
            return self.stack.max(1);
        }
        let each = self.each().max(1);
        (max_value / each).min(self.stack.max(1))
    }

    /// Whether a counter that buys `item_types` between `min_value` and
    /// `max_value` (a maximum of 0 being no maximum) would take it,
    /// whole or in pieces.
    fn taken_by(&self, item_types: u32, min_value: u32, max_value: u32) -> bool {
        self.item_type & item_types != 0 && self.each() >= min_value && self.at_once(max_value) > 0
    }
}

/// What a trip to one shop is expected to achieve, worked out from the
/// shop lists before the character sets off.
///
/// A run to town costs minutes of hunting, so it is worth knowing in
/// advance whether the counter has what the character came for and
/// whether there is money enough to pay for it. Without it the party
/// does what it used to: walk to a shop, find nothing it can afford,
/// and walk to the next one, for ever.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Forecast {
    /// The shop, for the log.
    pub shop: String,
    /// Needs it has on the shelf, and needs it has not.
    pub stocks: Vec<String>,
    pub missing: Vec<String>,
    /// What the needs it stocks would cost in full at its prices.
    pub bill: u32,
    /// The least one of anything wanted costs here; 0 when it stocks
    /// nothing that is wanted.
    pub cheapest: u32,
    /// Coin and notes in hand.
    pub purse: u32,
    /// What it would pay for what the character means to sell, and how
    /// many things that is.
    pub takings: u32,
    pub selling: usize,
}

impl Forecast {
    /// What there is to spend once the selling is done.
    pub fn funds(&self) -> u32 {
        self.purse.saturating_add(self.takings)
    }

    /// Why this shop was the one chosen, in a few words.
    ///
    /// The choice is a ring search outwards, taking the best shop in
    /// the first ring with anything worth the walk, ordered by whether
    /// it fills the whole order, then by how many lines of it it
    /// stocks, then by how near it is. So the answer is always some
    /// mixture of those three, and worth saying out loud: "it went
    /// there" is a bug report, "it went there because it was the only
    /// one stocking quarrels" is an explanation.
    pub fn why_it_was_picked(&self) -> String {
        if self.stocks.is_empty() {
            return match self.selling {
                0 => "nothing wanted is sold here; nearest counter".to_string(),
                n => format!("nowhere sells what is wanted; {n} thing(s) to sell"),
            };
        }
        let what = a_few(&self.stocks.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        let covers = if self.covers_it() {
            "the whole order".to_string()
        } else if self.missing.is_empty() {
            format!("{what}, but short of coin")
        } else {
            format!("{what} (not {})", a_few(&self.missing.iter().map(|s| s.as_str()).collect::<Vec<_>>()))
        };
        match self.selling {
            0 => covers,
            n => format!("{covers}; {n} thing(s) to sell here"),
        }
    }

    /// Whether the trip fills the whole order.
    pub fn covers_it(&self) -> bool {
        self.missing.is_empty() && !self.stocks.is_empty() && self.funds() >= self.bill
    }

    /// Whether the trip achieves anything at all: something to sell, or
    /// one thing it wants that it can pay for. Buying part of an order
    /// counts -- a mage that can afford half its tapers is a mage that
    /// can keep casting -- but buying none of it does not.
    pub fn worth_going(&self) -> bool {
        self.selling > 0 || (self.cheapest > 0 && self.funds() >= self.cheapest)
    }

    /// Said aloud, so whoever is watching knows why the party did or
    /// did not set off.
    pub fn tell(&self) -> String {
        let asked = self.stocks.len() + self.missing.len();
        let mut out = format!("{} of {asked} on the shelf", self.stocks.len());
        if self.bill > 0 {
            out.push_str(&format!(", {} to buy", self.bill));
        }
        out.push_str(&format!(", {} in hand", self.purse));
        if self.selling > 0 {
            out.push_str(&format!(", {} for {} item(s)", self.takings, self.selling));
        }
        if !self.missing.is_empty() {
            let missing: Vec<&str> = self.missing.iter().map(String::as_str).collect();
            out.push_str(&format!("; no {}", a_few(&missing)));
        }
        out
    }
}

/// The cheapest ware on this shelf that answers the need, if any.
fn shop_ware<'a>(
    shop: &'a ac_world::shops::Shop,
    need: &Need,
    needle: &str,
) -> Option<&'a ac_world::shops::Ware> {
    shop.sells
        .iter()
        .filter(|w| match &need.kind {
            NeedKind::Named(_) => contains_fold(&w.name, needle),
            NeedKind::Ammo(kind) => ammo_stock(&w.name, *kind),
            NeedKind::Component(wcid) => w.wcid == *wcid,
        })
        .min_by_key(|w| w.value)
}

/// What a trip to `shop` would buy, cost and fetch. `wants` is the
/// needs paired with their names already folded, since this is asked of
/// every counter in range.
fn forecast(
    shop: &ac_world::shops::Shop,
    wants: &[(&Need, String)],
    purse: u32,
    salables: &[Salable],
) -> Forecast {
    let mut f = Forecast {
        shop: shop.name.clone(),
        purse,
        ..Default::default()
    };
    for (need, needle) in wants {
        match shop_ware(shop, need, needle) {
            Some(w) => {
                let unit = shop.charges_for(w).max(1);
                f.bill = f.bill.saturating_add(unit.saturating_mul(need.want));
                f.cheapest = if f.cheapest == 0 {
                    unit
                } else {
                    f.cheapest.min(unit)
                };
                f.stocks.push(need.name.clone());
            }
            None => f.missing.push(need.name.clone()),
        }
    }
    for it in salables
        .iter()
        .filter(|it| it.taken_by(shop.buys, shop.min_value, shop.max_value))
    {
        // A counter's limits are about one of a thing, and its payment
        // is for all of them. Judging a stack by its total refuses a
        // hundred Pyreal Peas outright -- five million against a
        // ceiling of one -- and then reckons the character has nothing
        // to sell, which is how it came to walk thirty kilometres past
        // a counter that would have bought them twenty at a time.
        if let Some(paid) = shop.pays_for(it.item_type, it.each()) {
            f.selling += 1;
            f.takings = f
                .takings
                .saturating_add(paid.saturating_mul(it.stack.max(1)));
        }
    }
    f
}

/// Where a counter stands, to the metre: what a skipped vendor is
/// remembered by. Two vendors a metre apart are the same counter for
/// this purpose, which is the same slack the old comparison allowed.
fn spot(at: Vec2) -> (i32, i32) {
    (at.x.round() as i32, at.y.round() as i32)
}

/// The next move in a sale.



/// Whether a stock line is plain ammunition of `kind`: an "Arrow", not
/// a "Bundle of Arrowheads" or a "Fire Arrow" (the plain kind is what
/// is cheap and always there).
fn ammo_stock(name: &str, kind: u32) -> bool {
    use ac_world::fletching::ammo_type;
    let plain = match kind {
        ammo_type::ARROW => "arrow",
        ammo_type::BOLT => "quarrel",
        ammo_type::ATLATL => "atlatl dart",
        _ => return false,
    };
    name.trim().to_lowercase() == plain
}

/// What the vendor charges for an item of `value` (ACE's SellPrice), at
/// least a pyreal.
/// Whether a name on the keep-stocked list is worth buying. A healing
/// kit does nothing for a character that has not trained Healing -- it
/// restores next to nothing untrained -- so buying one wastes the money
/// and the trip. Such a character heals with a spell instead.
fn worth_stocking(name: &str, heals_with_kits: bool) -> bool {
    if heals_with_kits {
        return true;
    }
    !name.to_lowercase().contains("healing kit")
}


/// How far to look for a shop before settling for a nearer one with
/// less on its shelves: the town we are in, the towns around it, then a
/// long walk, then anywhere at all.
const VENDOR_RINGS: [f32; 3] = [600.0, 3_000.0, 15_000.0];

/// The distances to search, widening, never past `within`.
///
/// The last one is `within` itself (or everything), so a character with
/// nothing nearby still finds a shop rather than standing still.
fn vendor_rings(within: Option<f32>) -> Vec<f32> {
    let cap = within.unwrap_or(f32::INFINITY);
    let mut out: Vec<f32> = VENDOR_RINGS.iter().copied().filter(|r| *r < cap).collect();
    out.push(cap);
    out
}

/// What a vendor charges for one of something worth `value`.
///
/// A trade note is the exception: the server ignores the shop's own
/// rate for those and charges a flat 1.15 times face value, so a shop
/// that marks everything else up by 1.7 still sells notes at 1.15.
pub fn buy_price(value: u32, sell_rate: f32, item_type: u32) -> u32 {
    if item_type & item_type::PROMISSORY_NOTE != 0 {
        return ac_world::shops::note_price(value);
    }
    ((value as f32 * sell_rate - 0.1).ceil().max(1.0)) as u32
}

/// The rules on what to part with.
pub struct SellRules<'a> {
    /// Searches an item must match one of.
    pub sell: &'a [String],
    /// Names never sold.
    pub keep: &'a [String],
    /// Whether the character could wield the item, when it is a
    /// weapon: `None` when that is not known (not appraised), in which
    /// case a weapon is kept.
    pub can_wield: Option<bool>,
    /// What the loot rules tagged items with when they were taken: a
    /// `Sell` tag sells whatever the searches say, a `Salvage` tag keeps
    /// the item for the salvager.
    pub tags: &'a BTreeMap<u32, LootAction>,
    /// The spell components this character actually casts with, by
    /// weenie class.
    ///
    /// Not every spell component is a spell component to the character
    /// holding it. A Pyreal Pea is one by item type and is worth fifty
    /// thousand at a counter: peas are what a run to town is paid for.
    /// A Prismatic Taper is one too and is what the character is going
    /// to town to buy. The difference is not in the type; it is whether
    /// this character's spells burn it.
    pub burns: &'a [u32],
}

/// Gear worth keeping rather than selling: armour, clothing or jewelry
/// that carries spells.
///
/// This is the difference between loot and stock. A Hauberk of Epic
/// Life Mastery is not worth its handful of pyreals at a vendor, it is
/// a piece of a suit that gets built over weeks, and a rule that sold
/// every armour above a value threshold would feed exactly that piece
/// to a shopkeeper. So gear with spells on it is never sold by a
/// blanket rule. The loot rules can still tag one `Sell` by hand, and
/// that tag wins.
pub fn storage_worthy(stats: &ItemStats) -> bool {
    let gear = item_type::ARMOR | item_type::CLOTHING | item_type::JEWELRY;
    stats.item_type & gear != 0 && !stats.spells.is_empty()
}

/// Whether an item is loot to sell. `ammo` says whether it goes in the
/// ammunition slot, which the stats do not carry.
///
/// The order matters, and it is: what cannot be sold, then what the
/// player said, then what the loot rules tagged, then the standing
/// searches. Anything the searches do not name is kept -- a character
/// that empties its pack into a vendor loses the suit it was building.
/// Things that never go over a counter, whatever any rule, profile or
/// tag says.
///
/// A rule is a policy and this is not: it is the difference between a
/// character that can be trusted with its own pack and one that has to
/// be watched. Tinkering is spent on an item and cannot be got back;
/// an inscription is somebody's writing; equipped gear is what the
/// character fights and lives in; and an item the server has flagged
/// unsellable will be refused anyway, so offering it is a wasted round
/// trip and, worse, a chance to get stuck on it.
pub fn never_sell(stats: &ItemStats) -> bool {
    stats.wielded || stats.tinks > 0 || stats.inscribed || stats.unsellable || stats.value == 0
}

/// The same, for an item the character is actually carrying: a pack
/// with anything in it is refused by the server and would take its
/// contents with it if it were not.
pub fn never_sell_carried(stats: &ItemStats, holds_anything: bool) -> bool {
    never_sell(stats) || (stats.item_type & item_type::CONTAINER != 0 && holds_anything)
}

/// Why an item is one of those, for the log.
pub fn never_sell_because(stats: &ItemStats) -> &'static str {
    if stats.wielded {
        "it is equipped"
    } else if stats.tinks > 0 {
        "it has been tinkered"
    } else if stats.inscribed {
        "it is inscribed"
    } else if stats.unsellable {
        "no vendor will take it"
    } else {
        "it is worth nothing"
    }
}

pub fn sellable(stats: &ItemStats, ammo: bool, rules: &SellRules) -> bool {
    // Not sellable at all.
    if never_sell(stats) || ammo {
        return false;
    }
    // The player's own word, before anything else.
    if name_matches(&stats.name, rules.keep) {
        return false;
    }
    // What the loot rules decided when the item was taken. A `Sell` tag
    // is a decision already made, so it beats every rule below.
    match rules.tags.get(&stats.guid) {
        Some(LootAction::Sell) => return true,
        Some(LootAction::Salvage | LootAction::Keep | LootAction::Skip) => return false,
        None => {}
    }
    // A Focus is equipment: it lives in a pack slot and halves the
    // components of its school. Vendors will not take one anyway, but
    // the point is not to walk to town meaning to sell it.
    if crate::magic::is_focus(stats.wcid) {
        return false;
    }
    // A spell component is kept when this character's spells burn it,
    // and sold when they do not. See `SellRules::burns`.
    if stats.item_type & item_type::SPELL_COMPONENTS != 0 && rules.burns.contains(&stats.wcid) {
        return false;
    }
    // Things a vendor should never be handed: money, the packs
    // themselves, what crafting is made of.
    let keep_types = item_type::MONEY
        | item_type::CONTAINER
        | item_type::PROMISSORY_NOTE
        | item_type::TINKERING_TOOL
        | item_type::KEY
        | item_type::MANA_STONE
        | item_type::CRAFT_FLETCHING_BASE
        | item_type::CRAFT_FLETCHING_INTERMEDIATE
        | item_type::CRAFT_ALCHEMY_BASE
        | item_type::CRAFT_ALCHEMY_INTERMEDIATE
        | item_type::CRAFT_COOKING_BASE;
    if stats.item_type & keep_types != 0 {
        return false;
    }
    if storage_worthy(stats) {
        return false;
    }
    let weapon = stats.item_type
        & (item_type::MELEE_WEAPON | item_type::MISSILE_WEAPON | item_type::CASTER)
        != 0;
    if weapon && rules.can_wield != Some(false) {
        return false;
    }
    rules.sell.iter().any(|f| {
        let q = Query::parse(f);
        !q.is_empty() && stats.matches(&q)
    })
}

impl State {
    /// Let go of any journey the growth rules were part-way through.
    /// Used when the player takes the character back (see
    /// `Client::stop_moving_by_itself`).
    pub fn let_go(&mut self) {
        self.bound = None;
        self.bound_since = None;
        self.after_out = None;
    }
}

impl Client {
    /// The growth rules: spend experience, find monsters, run to town.
    /// Run once a frame when nothing more pressing claimed it; true
    /// when it did something.
    pub fn autoplay_grow(&mut self, now: Instant) -> bool {
        let cfg = self.autoplay.config.growth.clone();
        if self.world.player_guid.is_none() {
            return false;
        }
        if let Some(pl) = self.player.as_ref() {
            if !pl.is_indoors() {
                let p = pl.world_position();
                self.autoplay.growth.last_outdoors = Some(Vec2::new(p.x, p.y));
            }
        }
        // Whether we are underground decides what a journey may be made
        // of. Knowing needs the block's collision, which loads lazily,
        // so it is settled here once a tick rather than asked for in
        // the middle of a decision.
        let assets = self.assets.clone();
        self.autoplay.growth.in_dungeon = self
            .player
            .as_mut()
            .map(|pl| pl.is_indoors() && pl.in_dungeon(&assets))
            .unwrap_or(false);
        let mode = self.grow_mode(now, &cfg);
        // A rank is one message and takes no time: it goes out even in
        // the middle of a walk to town.
        if cfg.auto_xp && self.grow_spend_xp(now, &cfg) {
            return true;
        }
        if cfg.town_runs && self.grow_town_run(now, &cfg) {
            return true;
        }
        // A party that has stopped to restock does not wander off to a
        // new hunting ground in the middle of it, and nor does one that
        // has given up and stopped in town.
        if cfg.hunt_grounds
            && mode.hunting()
            && !self.autoplay.growth.stopped_in_town
            && self.grow_hunt(now, &cfg)
        {
            return true;
        }
        false
    }

    // ---- experience -----------------------------------------------

    /// The weapon in hand (not the ammunition), if any.
    fn weapon_in_hand(&self) -> Option<u32> {
        self.world
            .wielded()
            .find(|o| {
                o.item_type & (item_type::MELEE_WEAPON | item_type::MISSILE_WEAPON) != 0
                    && o.valid_locations & equip::MISSILE_AMMO == 0
            })
            .map(|o| o.guid)
    }

    /// The weapon skill the character fights with, from its hands:
    /// `Some(0)` for a caster. An unappraised weapon says nothing about
    /// its skill, so the stance stands in: a bow is Missile Weapons,
    /// and a melee weapon whichever melee skill is trained highest.
    fn fighting_skill(&self) -> Option<u32> {
        use ac_world::stats::sac;
        let stance = self.combat_stance();
        if stance == crate::Stance::Magic {
            return Some(0);
        }
        let weapon = self.weapon_in_hand()?;
        if let Some(id) = self
            .stats_of(weapon)
            .map(|i| i.weapon_skill_id)
            .filter(|id| *id != 0)
        {
            return Some(id);
        }
        if stance == crate::Stance::Missile {
            return Some(47);
        }
        self.world
            .stats
            .skills
            .iter()
            .filter(|s| [41, 44, 45, 46].contains(&s.id) && s.advancement >= sac::TRAINED)
            .max_by_key(|s| (s.advancement, s.ranks))
            .map(|s| s.id)
    }

    /// Everything experience could go on right now, priced and weighed.
    pub fn raise_offers(&self) -> Vec<Offer> {
        use ac_world::stats::sac;
        let weapon_skill = self.fighting_skill();
        let caster = weapon_skill == Some(0);
        let table = self.assets.skill_table().ok();
        let mut offers = Vec::new();
        // The attributes the fighting skill is built from matter as
        // much as a defence; the others little, Endurance apart.
        let mut attr_weight = [0.15f32; 6];
        attr_weight[1] = 0.45;
        let feeds = |skill: u32| -> Vec<u32> {
            table
                .as_ref()
                .and_then(|t| t.get(skill))
                .map(|b| {
                    [b.formula.attr1, b.formula.attr2]
                        .into_iter()
                        .filter(|a| (1..=6).contains(a))
                        .collect()
                })
                .unwrap_or_default()
        };
        let main: Vec<u32> = match weapon_skill {
            Some(0) => vec![
                ac_world::stats::skill::WAR_MAGIC,
                ac_world::stats::skill::LIFE_MAGIC,
            ],
            Some(s) => vec![s],
            None => Vec::new(),
        };
        for s in &main {
            for a in feeds(*s) {
                attr_weight[a as usize - 1] = attr_weight[a as usize - 1].max(0.6);
            }
        }
        for sk in &self.world.stats.skills {
            if sk.advancement < sac::TRAINED {
                continue;
            }
            let weight = skill_weight(sk.id, weapon_skill, caster);
            if let Some(cost) = self.skill_raise_cost(sk.id).xp() {
                offers.push(Offer {
                    raise: Raise::Skill(sk.id),
                    cost,
                    weight,
                });
            }
        }
        for (i, w) in attr_weight.iter().enumerate() {
            if let Some(cost) = self.attribute_raise_cost(i).xp() {
                offers.push(Offer {
                    raise: Raise::Attribute(i),
                    cost,
                    weight: *w,
                });
            }
        }
        let vital_weight = [0.5, 0.2, if caster { 0.5 } else { 0.15 }];
        for (i, w) in vital_weight.iter().enumerate() {
            if let Some(cost) = self.vital_raise_cost(i).xp() {
                offers.push(Offer {
                    raise: Raise::Vital(i),
                    cost,
                    weight: *w,
                });
            }
        }
        offers
    }

    /// Buy one rank if one is worth buying. True when one was.
    fn grow_spend_xp(&mut self, now: Instant, _cfg: &Growth) -> bool {
        let xp = self.world.stats.available_xp;
        if xp <= 0 || self.world.stats.level <= 0 {
            return false;
        }
        let st = &mut self.autoplay.growth;
        if let Some((seen, when)) = st.raise_pending {
            if xp == seen && now.duration_since(when) < RAISE_SETTLE {
                return false;
            }
            // The pool did not move: the server would not sell that
            // rank. Leave it alone for a while.
            if xp == seen {
                if let Some(pick) = st.last_pick.take() {
                    tracing::info!("growth: the server did not take a raise of {pick:?}");
                    st.sulking.push((pick, now));
                }
            }
            st.raise_pending = None;
        }
        st.sulking
            .retain(|(_, t)| now.duration_since(*t) < SULK_FOR);
        let st = &self.autoplay.growth;
        if st
            .last_raise
            .is_some_and(|t| now.duration_since(t) < RAISE_EVERY)
        {
            return false;
        }
        if let Some((at, when)) = st.nothing_at {
            if at == xp && now.duration_since(when) < XP_CHECK_EVERY {
                return false;
            }
        }
        // The weapon's skill is read off its appraisal.
        if let Some(w) = self.weapon_in_hand() {
            if !self.appraisals.contains_key(&w) {
                self.appraise_many([w]);
            }
        }
        let sulking: Vec<Raise> = self
            .autoplay
            .growth
            .sulking
            .iter()
            .map(|(r, _)| *r)
            .collect();
        let offers: Vec<Offer> = self
            .raise_offers()
            .into_iter()
            .filter(|o| !sulking.contains(&o.raise))
            .collect();
        let Some(pick) = choose_raise(&offers, xp) else {
            self.autoplay.growth.nothing_at = Some((xp, now));
            return false;
        };
        let (sent, what) = match pick {
            Raise::Skill(id) => (
                self.raise_skill(id),
                ac_world::stats::skill_name(id).to_string(),
            ),
            Raise::Attribute(i) => (
                self.raise_attribute(i),
                crate::advance::ATTRIBUTE_NAMES[i.min(5)].to_string(),
            ),
            Raise::Vital(i) => (
                self.raise_vital(i),
                crate::advance::VITAL_NAMES[i.min(2)].to_string(),
            ),
        };
        if !sent {
            self.autoplay.growth.nothing_at = Some((xp, now));
            return false;
        }
        let st = &mut self.autoplay.growth;
        st.last_raise = Some(now);
        st.raise_pending = Some((xp, now));
        st.last_pick = Some(pick);
        st.nothing_at = None;
        // A note, not a status: a rank bought on the way to town must
        // not flip the status line back and forth with the walk.
        self.autoplay
            .note(format!("raising {what} ({xp} xp to spend)"), now);
        if matches!(self.autoplay.doing, Doing::Idle) {
            self.autoplay.say(Doing::Growing, "spending experience");
        }
        true
    }

    // ---- journeys -------------------------------------------------

    /// Set off for `goal`. From inside a building the planner sees no
    /// way out but a portal, so the walk begins with the spot the
    /// character last stood outdoors, and the journey proper follows
    /// (see [`Self::grow_travel_on`]). False when no way was found.
    fn grow_travel(&mut self, goal: Vec2, now: Instant) -> bool {
        self.autoplay.growth.after_out = None;
        let Some(pl) = self.player.as_ref() else {
            return false;
        };
        // The shortcut below is for buildings. A dungeon has no door to
        // step out of -- the way out is its exit portal or a recall --
        // and walking at the last outdoor spot from underground is
        // walking at rock.
        if pl.is_indoors() && !self.autoplay.growth.in_dungeon {
            let me = pl.world_position();
            let block = pl.cell & 0xFFFF_0000;
            let same_block =
                |p: Vec2| (((p.x / 192.0) as u32) << 24) | (((p.y / 192.0) as u32) << 16) == block;
            if let Some(out) = self.autoplay.growth.last_outdoors {
                if same_block(out)
                    && out.distance(Vec2::new(me.x, me.y)) < 150.0
                    && self.travel_to(out)
                {
                    self.autoplay.growth.after_out = Some(goal);
                    self.autoplay.note("stepping outside first", now);
                    return true;
                }
            }
        }
        self.travel_to(goal)
    }

    /// The second leg of a journey begun indoors: once the walk out has
    /// ended, the journey proper. True when one was started.
    fn grow_travel_on(&mut self) -> bool {
        if self.traveling() {
            return false;
        }
        let Some(goal) = self.autoplay.growth.after_out.take() else {
            return false;
        };
        if self.travel_to(goal) {
            return true;
        }
        tracing::info!("growth: no way on to {goal:?} from outside either");
        false
    }

    // ---- hunting grounds ------------------------------------------

    /// The hunting ground this character is on or heading for.
    pub fn hunting_ground(&self) -> Option<(u32, Vec2, String)> {
        let st = &self.autoplay.growth;
        st.bound.clone().or_else(|| {
            let lb = st.hunting_at?;
            let g = ac_world::hunting::all()
                .iter()
                .find(|g| g.landblock == lb)?;
            Some((lb, g.at, g.name.clone()))
        })
    }

    /// The ground the party is on, for a character that does not choose
    /// its own.
    ///
    /// A party that walks out of town each picking the nearest ground
    /// to whichever shop it finished at ends up in four different
    /// places. The leader's ground is the party's, so everyone comes
    /// back to the same one.
    fn party_ground(&self) -> Option<(u32, Vec2, String)> {
        let team = &self.autoplay.config.team;
        if !team.enabled || team.lead {
            return None;
        }
        self.autoplay
            .team
            .leader_mate()
            .and_then(|m| m.ground.clone())
    }

    /// Go somewhere with monsters when there have been none for a
    /// while. True while it is on the way.
    fn grow_hunt(&mut self, now: Instant, cfg: &Growth) -> bool {
        let Some((me, cell)) = self.player.as_ref().map(|p| (p.world_position(), p.cell)) else {
            return false;
        };
        let me = Vec2::new(me.x, me.y);
        let here = cell >> 16;
        // On the way: keep going, and notice arriving.
        if let Some((lb, at, name)) = self.autoplay.growth.bound.clone() {
            let too_long = self
                .autoplay
                .growth
                .bound_since
                .is_some_and(|t| now.duration_since(t) > WALK_TIMEOUT);
            if too_long {
                self.autoplay.note(
                    format!("the walk to the {name} ground is taking too long"),
                    now,
                );
                self.cancel_travel();
            } else if self.traveling() || self.grow_travel_on() {
                self.autoplay.say(
                    Doing::Traveling,
                    format!("going to hunt {name} ({})", about(at.distance(me))),
                );
                return true;
            }
            let st = &mut self.autoplay.growth;
            st.bound = None;
            if here == lb || at.distance(me) <= GROUND_REACH {
                st.hunting_at = Some(lb);
                st.idle_since = None;
                st.roams = 0;
                self.autoplay
                    .note(format!("at the hunting ground: {name}"), now);
            } else {
                st.skip.push((lb, now));
                self.autoplay
                    .note(format!("could not reach the {name} ground; another"), now);
            }
            return false;
        }
        let level = self.world.stats.level;
        if level <= 0 {
            return false;
        }
        // Idle is the engine having found nothing to do last frame
        // (spending experience counts as nothing to do).
        let idle = matches!(self.autoplay.doing, Doing::Idle | Doing::Growing)
            && self.attack_target.is_none()
            && self.autoplay.casting_at().is_none()
            && !self.traveling();
        if !idle {
            self.autoplay.growth.idle_since = None;
            return false;
        }
        let since = *self.autoplay.growth.idle_since.get_or_insert(now);
        if now.duration_since(since).as_secs_f32() < cfg.idle_before_move {
            return false;
        }
        if self.autoplay.growth.next_hunt.is_some_and(|t| now < t) {
            return false;
        }
        // What to do on a ground with nothing in sight depends on the
        // ground. A few spawn hard enough that standing still is never
        // idle and walking away only leaves the fight; most need
        // covering on foot; a thin one is worth leaving once it is
        // quiet.
        let tactic = ac_world::hunting::tactic_for(cfg.tactic, ac_world::hunting::at(here));
        if tactic == ac_world::hunting::Tactic::Camp
            && self.autoplay.growth.hunting_at == Some(here)
        {
            // Hold the spot. Saying so once is enough; repeating it
            // every frame would drown the log.
            if self.autoplay.doing != Doing::Idle {
                self.autoplay
                    .say(Doing::Idle, "holding the spot; they come to us");
            }
            self.autoplay.growth.idle_since = Some(now);
            return false;
        }
        // Patrol keeps walking the ground; only Sweep gives up on it.
        let roams_allowed = if tactic == ac_world::hunting::Tactic::Patrol {
            u32::MAX
        } else {
            ROAMS
        };
        if self.autoplay.growth.hunting_at == Some(here)
            && self.autoplay.growth.roams < roams_allowed
        {
            let n = self.autoplay.growth.roams % ROAMS;
            let angle = (n as f32 + 0.5) * std::f32::consts::TAU / ROAMS as f32;
            let origin = ac_world::landblock_origin(cell);
            let goal = (me + Vec2::new(angle.cos(), angle.sin()) * ROAM).clamp(
                Vec2::new(origin.x + 10.0, origin.y + 10.0),
                Vec2::new(origin.x + 182.0, origin.y + 182.0),
            );
            self.autoplay.growth.roams += 1;
            self.autoplay.growth.idle_since = Some(now);
            if self.travel_to(goal) {
                self.autoplay.say(
                    Doing::Traveling,
                    "nothing in sight; looking about the ground",
                );
                return true;
            }
        }
        let st = &mut self.autoplay.growth;
        st.skip
            .retain(|(_, t)| now.duration_since(*t) < SKIP_GROUND_FOR);
        let mut skip: Vec<u32> = st.skip.iter().map(|(g, _)| *g).collect();
        skip.push(here);
        if let Some(h) = st.hunting_at {
            skip.push(h);
        }
        // A ground the player named is where the party hunts. Not the
        // nearest one that suits its level: a place is hunted for its
        // loot, its money, its trophies, and none of that is the level
        // table's business. A follower takes its leader's ground the
        // same way, so naming one on the leader moves everybody.
        let pinned = (cfg.hunt_at != 0)
            .then(|| ac_world::hunting::at(cfg.hunt_at))
            .flatten()
            .map(|g| (g.landblock, g.at, g.name.clone()));
        let named = pinned.is_some();
        if let Some((lb, at, name)) = pinned.or_else(|| self.party_ground()) {
            if self.autoplay.growth.hunting_at != Some(lb) {
                if self.grow_travel(at, now) {
                    let st = &mut self.autoplay.growth;
                    st.bound = Some((lb, at, name.clone()));
                    st.bound_since = Some(now);
                    st.idle_since = None;
                    self.autoplay
                        .say(Doing::Traveling, format!("on the way to {name}"));
                    return true;
                }
                if named {
                    // Asked for somewhere it cannot plan a way to. Say
                    // so and try again later; picking somewhere else
                    // would be answering a question nobody asked.
                    self.autoplay.note(
                        format!("cannot find a way to {name} from here; will try again"),
                        now,
                    );
                    self.autoplay.growth.idle_since = Some(now);
                    self.autoplay.growth.next_hunt = Some(now + RETRY_AFTER);
                    return false;
                }
            } else if named {
                // Standing on the named ground: this is where we hunt,
                // and nothing below gets to move us on.
                self.autoplay.growth.hunting_at = Some(lb);
            }
        }
        if named {
            // The player named this ground. Whatever the tactic makes
            // of a quiet spell, it does not get to go somewhere else.
            return false;
        }
        let Some(g) = ac_world::hunting::nearest_for(level as u32, cfg.level_margin, me, &skip)
        else {
            self.autoplay.note(
                format!(
                    "no hunting ground suits level {level} within {} levels",
                    cfg.level_margin
                ),
                now,
            );
            self.autoplay.growth.idle_since = Some(now);
            return false;
        };
        let (lb, at, name) = (g.landblock, g.at, g.name.clone());
        let (glo, ghi) = (g.min_level, g.max_level);
        if self.grow_travel(at, now) {
            let st = &mut self.autoplay.growth;
            st.bound = Some((lb, at, name.clone()));
            st.bound_since = Some(now);
            st.idle_since = None;
            if let Some(h) = st.hunting_at.take() {
                st.skip.push((h, now));
            }
            self.autoplay.say(
                Doing::Traveling,
                format!(
                    "nothing about; going to hunt {name} (levels {glo}-{ghi}, {})",
                    about(at.distance(me))
                ),
            );
            true
        } else {
            let st = &mut self.autoplay.growth;
            st.skip.push((lb, now));
            st.idle_since = None;
            st.next_hunt = Some(now + RETRY_AFTER);
            self.autoplay
                .note(format!("no way to the {name} ground from here"), now);
            false
        }
    }

    // ---- town runs ------------------------------------------------

    /// Ammunition carried, wielded and in the packs, for a bow or
    /// crossbow in hand: `(kind, count)`.
    fn ammo_carried(&self) -> Option<(u32, u32)> {
        let launcher = self
            .wielded_missile_weapon()
            .and_then(|g| self.stats_of(g))
            .filter(crate::weapons::is_launcher)?;
        if launcher.ammo_type == 0 {
            return None;
        }
        let me = self.world.player_guid;
        let count: u32 = self
            .world
            .objects
            .values()
            .filter(|o| o.valid_locations & equip::MISSILE_AMMO != 0)
            .filter(|o| o.wielder == me || self.world.is_carried(o.guid))
            .map(|o| o.stack_size.max(1))
            .sum();
        Some((launcher.ammo_type, count))
    }

    /// Whether more ammunition of `kind` could be made from what is
    /// carried (see `autoplay::Fight::craft_ammo`).
    fn can_craft_ammo(&self, kind: u32) -> bool {
        if !self.autoplay.config.fight.craft_ammo {
            return false;
        }
        let carried: Vec<u32> = self.world.inventory().map(|o| o.weenie_class_id).collect();
        ac_world::fletching::making(kind)
            .any(|r| carried.contains(&r.source) && carried.contains(&r.target))
    }

    /// How many of a named thing are carried (name contains, as the
    /// team's `keep_stocked` counts).
    fn carried_named(&self, name: &str) -> u32 {
        let want = name.trim().to_lowercase();
        self.world
            .inventory()
            .filter(|o| o.name.to_lowercase().contains(&want))
            .map(|o| o.stack_size.max(1))
            .sum()
    }

    /// What the character is short of.
    fn grow_needs(&self, cfg: &Growth) -> Vec<Need> {
        let mut needs = Vec::new();
        let mut named: Vec<(String, u32)> = cfg.keep_stocked.clone();
        if self.autoplay.config.team.enabled {
            named.extend(self.autoplay.config.team.keep_stocked.iter().cloned());
        }
        for (name, least) in named {
            if name.trim().is_empty() || least == 0 {
                continue;
            }
            if !worth_stocking(&name, self.heals_with_kits()) {
                continue;
            }
            let have = self.carried_named(&name);
            if have < least
                && !needs
                    .iter()
                    .any(|n: &Need| n.kind == NeedKind::Named(name.clone()))
            {
                needs.push(Need {
                    name: name.clone(),
                    want: least - have,
                    have,
                    keep: least,
                    urgent: true,
                    buyable: true,
                    kind: NeedKind::Named(name),
                });
            }
        }
        if let Some((kind, have)) = self.ammo_carried() {
            if have < cfg.ammo_keep {
                needs.push(Need {
                    name: ac_world::fletching::ammo_type::name(kind).to_string(),
                    want: cfg.ammo_keep - have,
                    have,
                    keep: cfg.ammo_keep,
                    urgent: have < cfg.ammo_keep / 4 && !self.can_craft_ammo(kind),
                    buyable: true,
                    kind: NeedKind::Ammo(kind),
                });
            }
        }
        // Components: for a character with spells and a wand. Those it
        // carries, and those its buffs ask for that it has run out of.
        if cfg.tapers_keep > 0 && !self.world.stats.spells.is_empty() {
            let has_wand = self.wielded_caster().is_some()
                || self
                    .world
                    .inventory()
                    .any(|o| o.item_type & item_type::CASTER != 0);
            if has_wand {
                if let Ok(mapper) = self.assets.spell_component_ids() {
                    let targets = self.component_targets(cfg.tapers_keep);
                    // Every component the spells this character casts
                    // will burn, and every one it happens to carry.
                    //
                    // The targets are what matters. Asking only what is
                    // in the pack, as this once did, leaves a caster
                    // that has run right out of something unable to
                    // notice: with none of it carried there is nothing
                    // to count, so nothing is short, so it never goes
                    // to town for more.
                    let carried = self.components();
                    let mut ids: Vec<u32> = targets.keys().copied().collect();
                    ids.extend(carried.iter().map(|c| c.component_id));
                    ids.sort_unstable();
                    ids.dedup();
                    for id in ids {
                        let Some(wcid) = mapper.component_wcid(id) else {
                            continue;
                        };
                        // What this one burns at, not what a taper does.
                        let keep = targets.get(&id).copied().unwrap_or(cfg.tapers_keep);
                        let c = carried.iter().find(|c| c.component_id == id);
                        let have = c.map_or(0, |c| c.count);
                        let buyable = ac_world::shops::sold_anywhere(wcid);
                        if have < keep {
                            // What it is called in the client's own
                            // component table, which is what a vendor
                            // and a person both call it. The enum name
                            // is the last resort: "LeadScarab" is a
                            // symbol, not a thing you can ask for.
                            let name = c
                                .map(|c| c.name.clone())
                                .or_else(|| {
                                    self.assets
                                        .spell_components()
                                        .ok()
                                        .and_then(|t| t.get(id).map(|c| c.name.clone()))
                                })
                                .or_else(|| mapper.name_of(id).map(str::to_string))
                                .unwrap_or_else(|| format!("component {id}"));
                            needs.push(Need {
                                name,
                                want: keep - have,
                                have,
                                keep,
                                urgent: have < keep / 4 && buyable,
                                buyable,
                                kind: NeedKind::Component(wcid),
                            });
                        }
                    }
                }
            }
        }
        needs
    }

    /// Pyreals carried.
    pub(crate) fn purse(&self) -> u32 {
        self.world
            .inventory()
            .filter(|o| o.item_type & item_type::MONEY != 0)
            .map(|o| o.stack_size.max(1))
            .sum()
    }

    /// Work out what the party is doing and remember it.
    ///
    /// Every session runs this on the same roster and reaches the same
    /// answer, so there is nothing to agree on: the party changes mode
    /// together without a message being sent about it. A character
    /// playing alone, or one whose team rules are off, is always
    /// hunting -- its own supplies still send it to town, by the older
    /// rule that fires on an urgent shortfall.
    fn grow_mode(&mut self, now: Instant, cfg: &Growth) -> crate::logistics::GroupMode {
        use crate::logistics::{decide, GroupMode};
        let team = &self.autoplay.config.team;
        if !team.enabled || !team.restock.together {
            self.autoplay.growth.mode = GroupMode::Hunting;
            return GroupMode::Hunting;
        }
        let policy = team.restock.clone();
        let mut party: Vec<Supplies> = self
            .autoplay
            .team
            .mates
            .iter()
            .map(|m| m.supplies.clone())
            .collect();
        party.push(self.supplies(cfg));
        let was = self.autoplay.growth.mode;
        // A trip that has dragged on has failed at something no rule
        // here can see: a vendor out of tapers, a purse that ran dry, a
        // character that died on the way. Waiting at the hunting ground
        // for ever is worse than hunting undersupplied, so the party
        // gives up and goes back to it.
        let stalled = !was.hunting()
            && policy.give_up_after > 0.0
            && self
                .autoplay
                .growth
                .mode_since
                .is_some_and(|t| now.duration_since(t).as_secs_f32() > policy.give_up_after);
        if stalled {
            let st = &mut self.autoplay.growth;
            st.mode = GroupMode::Hunting;
            st.mode_since = Some(now);
            st.mode_because = "the trip to town took too long".to_string();
            st.handed_over = false;
            st.round = 0;
            self.autoplay.note(
                "giving up on the trip to town and going back to hunting",
                now,
            );
            return GroupMode::Hunting;
        }
        if let Some(switch) = decide(was, &party, &policy, self.autoplay.growth.round) {
            let st = &mut self.autoplay.growth;
            // Back to the start of a trip is another round; going home
            // starts the count again.
            st.round = match switch.mode {
                GroupMode::Hunting => 0,
                GroupMode::Restocking(Stage::HandOver) if !was.hunting() => st.round + 1,
                GroupMode::Restocking(_) => st.round,
            };
            st.mode = switch.mode;
            st.mode_since = Some(now);
            st.mode_because = switch.because.clone();
            // A new stage is a new set of errands; nothing carries over.
            st.handed_over = false;
            self.autoplay.note(
                format!("the party is {}: {}", switch.mode, switch.because),
                now,
            );
        }
        self.autoplay.growth.mode
    }

    /// Free item slots, side packs counted: what decides who can carry
    /// the party's shopping.
    pub fn free_space(&self) -> u32 {
        let (used, capacity) = self.item_slots();
        capacity.saturating_sub(used)
    }

    /// What the character can spend. Coin and trade notes both: a note
    /// is money in a lighter form, and a vendor takes either.
    pub fn spendable(&self) -> u32 {
        let notes: u32 = self
            .world
            .inventory()
            .filter(|o| o.item_type & item_type::PROMISSORY_NOTE != 0)
            .map(|o| o.value.saturating_mul(o.stack_size.max(1)))
            .sum();
        self.purse().saturating_add(notes)
    }

    /// What this character says about itself for the party to decide
    /// with: how close to empty it is, what it still has to buy, and
    /// what that will cost.
    ///
    /// The level is the worst supply line, not the average. A mage with
    /// a full load of scarabs and no tapers cannot cast, and averaging
    /// the two would hide that.
    pub fn supplies(&self, cfg: &Growth) -> Supplies {
        let needs = self.grow_needs(cfg);
        // The worst line the party can do anything about. A Void mage
        // is permanently out of Nightshade -- no counter in Dereth
        // sells it -- and counting that would hold the level at nought
        // for ever, which reads as "always restocking, never stocked".
        let level = needs
            .iter()
            .filter(|n| n.buyable)
            .map(|n| logistics::line_level(n.have, n.keep))
            .fold(1.0f32, f32::min);
        let policy = self.autoplay.config.team.restock.sane();
        Supplies {
            name: self
                .world
                .player()
                .map(|p| p.name.clone())
                .unwrap_or_default(),
            level,
            pack_full: self.pack_full(),
            // Ready to go back: stocked up, and not still mid-errand.
            stocked: level >= policy.full_at && self.autoplay.growth.run.is_none(),
            handed_over: self.autoplay.growth.handed_over,
            holding_orders: self.holding_orders(),
            bill: needs.iter().map(|n| self.rough_cost(n)).sum(),
            purse: self.spendable(),
            // Short of something the trip cannot supply: either nothing
            // left to pay with, or a trip that came back with nothing,
            // which says the same thing about this town. Either way the
            // character is done shopping and goes back to earning.
            broke: !needs.is_empty()
                && (self.spendable() == 0 || self.autoplay.growth.run_was_futile),
            free_space: self.free_space(),
            order: needs
                .iter()
                .filter(|n| n.want > 0 && n.buyable)
                .map(|n| (n.name.clone(), n.want))
                .collect(),
        }
    }

    /// Roughly what filling a need will cost, for working out who has
    /// to be handed money before the party can shop. A vendor sells
    /// above an item's own value, so this is an underestimate rather
    /// than a promise; it is only ever compared against a purse.
    fn rough_cost(&self, need: &Need) -> u32 {
        let unit = self
            .world
            .inventory()
            .find(|o| o.name.eq_ignore_ascii_case(&need.name) && o.value > 0)
            .map(|o| o.value)
            .unwrap_or(0);
        unit.saturating_mul(need.want)
    }

    /// Whether this character is carrying something another character
    /// asked for. On a quartermaster run that is how the party knows
    /// the runner still has goods to hand out; on any other it is
    /// simply false, since nobody has asked it for anything.
    ///
    /// Worked out from the others' orders alone, never from who the
    /// runner is: the runner is chosen from these reports, so asking
    /// would be circular.
    fn holding_orders(&self) -> bool {
        let me = self.world.stats.name.as_str();
        let wanted: Vec<&str> = self
            .autoplay
            .team
            .mates
            .iter()
            .filter(|m| m.name != me)
            .flat_map(|m| m.supplies.order.iter())
            .filter(|(_, count)| *count > 0)
            .map(|(name, _)| name.as_str())
            .collect();
        if wanted.is_empty() {
            return false;
        }
        self.world
            .inventory()
            .any(|o| wanted.iter().any(|w| o.name.eq_ignore_ascii_case(w)))
    }

    /// How many of each spell component to carry, keyed by component id.
    ///
    /// A taper is the yardstick: it is what a caster runs out of, and
    /// it is the one number a player sets. Everything else is scaled to
    /// it by how fast it burns relative to a taper, which the client
    /// can work out rather than guess, because both halves are in its
    /// own tables. ACE rolls each component of a formula separately
    /// (`Spell.TryBurnComponents`):
    ///
    /// ```text
    /// burn = spell.ComponentLoss * component.CDM * min(1, power / skill)
    /// ```
    ///
    /// The skill term cancels when one component is divided by another
    /// of the same spell, so the ratio is just the loss and the CDMs,
    /// and a component used by several spells is stocked for the one
    /// that burns it fastest. With foci a top-level cast burns about
    /// 0.4 of a taper against 0.003 of a scarab, so a thousand tapers
    /// comes out at a handful of scarabs rather than a thousand.
    pub fn component_targets(&self, tapers_keep: u32) -> BTreeMap<u32, u32> {
        let mut out: BTreeMap<u32, u32> = BTreeMap::new();
        if tapers_keep == 0 {
            return out;
        }
        let (Ok(table), Ok(comps)) = (self.assets.spell_table(), self.assets.spell_components())
        else {
            return out;
        };
        // The fastest rate each component burns at, across the spells
        // this character actually casts.
        let mut fastest: BTreeMap<u32, f32> = BTreeMap::new();
        let spells: Vec<u32> = self
            .wanted_buffs()
            .iter()
            .map(|w| w.spell)
            .chain(self.attack_spells_known())
            .collect();
        for spell in spells {
            let Some(sp) = table.get(spell) else { continue };
            for id in self.current_formula(spell) {
                let Some(c) = comps.get(id) else { continue };
                let rate = sp.component_loss * c.cdm;
                let e = fastest.entry(id).or_insert(0.0);
                if rate > *e {
                    *e = rate;
                }
            }
        }
        let taper = fastest
            .get(&crate::magic::PRISMATIC_TAPER)
            .copied()
            .filter(|r| *r > 0.0);
        // A floor under everything: twenty to the thousand. The burn
        // rates say a scarab would only need eight, and that is cutting
        // it far too fine for something bought once a trip -- being
        // over-provisioned on the cheap, light things costs a slot and
        // saves a walk. Anything that genuinely burns faster than the
        // floor keeps its own number, so a caster without foci still
        // stocks its herbs properly.
        let floor = (tapers_keep / 50).max(1);
        for (id, rate) in &fastest {
            let want = match taper {
                // Scaled to the taper by how fast it burns.
                Some(t) => ((tapers_keep as f32) * rate / t).round() as u32,
                // No taper in any formula (no foci, or an odd build):
                // the configured number stands for everything.
                None => tapers_keep,
            };
            out.insert(*id, want.max(floor));
        }
        out
    }

    /// The party as everyone has last described itself, this character
    /// included. What every shared decision is worked out from.
    pub fn party_supplies(&self, cfg: &Growth) -> Vec<Supplies> {
        let mut party: Vec<Supplies> = self
            .autoplay
            .team
            .mates
            .iter()
            .map(|m| m.supplies.clone())
            .collect();
        party.push(self.supplies(cfg));
        party
    }

    /// Who is doing the party's shopping, when it sends one character
    /// rather than all going.
    pub fn quartermaster_name(&self, cfg: &Growth) -> Option<String> {
        if self.autoplay.config.team.restock.plan != crate::logistics::Plan::Quartermaster {
            return None;
        }
        crate::logistics::quartermaster(&self.party_supplies(cfg)).map(|m| m.name.clone())
    }

    /// Whether this character is the one doing the shopping.
    pub fn is_quartermaster(&self, cfg: &Growth) -> bool {
        let me = self.world.stats.name.as_str();
        !me.is_empty() && self.quartermaster_name(cfg).as_deref() == Some(me)
    }


    /// Everything the character is carrying that the rules would sell,
    /// whether or not a vendor is open. What the party hands its
    /// quartermaster before it leaves.
    pub fn loot_for_sale(&self, cfg: &Growth) -> Vec<u32> {
        let wielder = self.wielder();
        let keep = self.keep_names(cfg);
        let tags = self.autoplay.tags().clone();
        let burns = self.burns(cfg);
        self.world
            .inventory()
            .filter_map(|o| {
                let stats = self.stats_of(o.guid)?;
                let ammo = o.valid_locations & equip::MISSILE_AMMO != 0;
                let rules = SellRules {
                    sell: &cfg.sell,
                    keep: &keep,
                    can_wield: stats.appraised.then(|| wielder.can_wield(&stats)),
                    tags: &tags,
                    burns: &burns,
                };
                sellable(&stats, ammo, &rules).then_some(o.guid)
            })
            .collect()
    }

    /// Every name that is never sold: the player's own list, whatever
    /// is kept stocked, and whatever the loot rules always keep.
    fn keep_names(&self, cfg: &Growth) -> Vec<String> {
        let mut keep: Vec<String> = cfg.keep.clone();
        keep.extend(cfg.keep_stocked.iter().map(|(n, _)| n.clone()));
        keep.extend(
            self.autoplay
                .config
                .team
                .keep_stocked
                .iter()
                .map(|(n, _)| n.clone()),
        );
        keep.extend(self.autoplay.config.loot.always.iter().cloned());
        keep
    }

    /// The pack items to sell to the open vendor, which takes only some
    /// kinds of thing. Weapons are only sold once appraised and found
    /// beyond the character.
    fn sale_list(&self, cfg: &Growth) -> Vec<u32> {
        let Some(v) = self.world.open_vendor.as_ref() else {
            return Vec::new();
        };
        // The vendor buys some kinds of thing, within a range of values
        // (a range of 0 is no range at all).
        self.salables(cfg)
            .into_iter()
            .filter(|it| it.taken_by(v.item_types, v.min_value, v.max_value))
            .map(|it| it.guid)
            .collect()
    }

    /// Everything in the pack the selling rules allow to go, before any
    /// one counter's tastes are applied to it.
    ///
    /// [`Self::sale_list`] narrows this to the vendor standing in front
    /// of the character; a forecast narrows it to a shop the character
    /// has not walked to yet, which is how a trip is judged before it is
    /// started.
    fn salables(&self, cfg: &Growth) -> Vec<Salable> {
        let wielder = self.wielder();
        let keep = self.keep_names(cfg);
        let tags = self.autoplay.tags().clone();
        let burns = self.burns(cfg);
        let rules_for = |stats: &ItemStats| SellRules {
            sell: &cfg.sell,
            keep: &keep,
            can_wield: stats.appraised.then(|| wielder.can_wield(stats)),
            tags: &tags,
            burns: &burns,
        };
        // What goes to a counter is a profile's decision, not a list of
        // searches buried in the code.
        //
        // The vendor profile when there is one, and the loot profile
        // when there is not: a rule that says "take this to sell" has
        // already said where it goes, and saying it twice is how the
        // two drift apart. The old searches are the last resort, for a
        // character nobody has given a profile at all.
        let loot = &self.autoplay.config.loot;
        let vendor = self
            .profiles
            .get(&loot.vendor_profile)
            .or_else(|| self.profiles.get(&loot.profile));
        let my_name = self.world.stats.name.clone();
        let unsellable = &self.autoplay.growth.unsellable;
        let now = Instant::now();
        self.world
            .inventory()
            .filter(|o| !unsellable.held(&o.guid, now))
            .filter_map(|o| {
                let stats = self.stats_of(o.guid)?;
                let ammo = o.valid_locations & equip::MISSILE_AMMO != 0;
                // A pack with things in it is not loot to be sold; the
                // server refuses it, and it holds the character's
                // belongings.
                let holds_anything = self
                    .world
                    .objects
                    .values()
                    .any(|it| it.container == Some(o.guid));
                if never_sell_carried(&stats, holds_anything) {
                    return None;
                }
                let sells = match &vendor {
                    Some(p) => {
                        // A profile does not get past the things that
                        // never go over a counter (see `never_sell`).
                        !never_sell(&stats)
                            && !ammo
                            && matches!(
                                p.judge(
                                    &stats,
                                    self.appraisals.get(&o.guid),
                                    &wielder,
                                    &my_name,
                                    0
                                ),
                                crate::profile::Verdict::Decided(LootAction::Sell, _)
                            )
                    }
                    None => sellable(&stats, ammo, &rules_for(&stats)),
                };
                sells.then_some(Salable {
                    guid: o.guid,
                    item_type: o.item_type,
                    value: o.value,
                    stack: o.stack_size.max(1),
                })
            })
            .collect()
    }

    /// Everything the counter in front of the character will do, as
    /// one plan: what to sell, what to cash, what to buy and what to
    /// carry home as notes.
    ///
    /// The pieces were all here already and were used one at a time, in
    /// an order written out by hand at each call site. Gathering them
    /// makes the order one thing, and makes it the same order whether
    /// the trip is being made or merely weighed up (see
    /// `crate::errand`).
    pub fn errand(&self, cfg: &Growth) -> crate::errand::Plan {
        use crate::errand::{ForSale, Means, Note, Wanted};
        let (carrying, capacity) = self.burden();
        let means = Means {
            coin: self.purse(),
            notes: self
                .world
                .inventory()
                .filter(|o| o.item_type & item_type::PROMISSORY_NOTE != 0)
                .map(|o| o.value.saturating_mul(o.stack_size.max(1)))
                .sum(),
            room: capacity.saturating_mul(3).saturating_sub(carrying),
            slots: self.free_space(),
        };
        let vendor = self.world.open_vendor.as_ref();
        let sale: Vec<ForSale> = self
            .sale_list(cfg)
            .into_iter()
            .filter_map(|guid| {
                let o = self.world.objects.get(&guid)?;
                let stack = o.stack_size.max(1);
                // `value` is already the whole stack's worth, so the
                // payment is that times the rate -- not that times the
                // stack a second time, which valued a hundred peas at
                // five hundred million.
                let pays = vendor
                    .map(|v| (o.value as f32 * v.buy_rate).round() as u32)
                    .unwrap_or(0);
                Some(ForSale {
                    guid,
                    pays,
                    weighs: o.burden.saturating_mul(stack),
                })
            })
            .collect();
        let notes: Vec<Note> = self
            .world
            .inventory()
            .filter(|o| o.item_type & item_type::PROMISSORY_NOTE != 0)
            .map(|o| Note {
                guid: o.guid,
                face: o.value.saturating_mul(o.stack_size.max(1)),
            })
            .collect();
        let shelf = self.stock();
        let wanted: Vec<Wanted> = self
            .grow_needs(cfg)
            .into_iter()
            .filter(|n| n.want > 0 && n.buyable)
            .filter_map(|need| {
                let line = shelf
                    .iter()
                    .filter(|s| match &need.kind {
                        NeedKind::Named(t) => contains_fold(&s.name, &t.to_lowercase()),
                        NeedKind::Ammo(kind) => ammo_stock(&s.name, *kind),
                        NeedKind::Component(wcid) => s.wcid == *wcid,
                    })
                    .filter(|s| s.price > 0)
                    .min_by_key(|s| s.price)?;
                Some(Wanted {
                    line: line.guid,
                    name: need.name.clone(),
                    want: need.want,
                    each: line.price,
                    weighs: line.burden,
                    stock: line.stack,
                })
            })
            .collect();
        // The Mayoi note, if this counter makes one. Only that one: no
        // smaller note is worth making, since every note costs fifteen
        // per cent of its face whatever it carries.
        let note_face = shelf
            .iter()
            .filter(|s| s.wcid == ac_world::shops::MMD && s.price > 0)
            .find_map(|s| ac_world::shops::note_face(s.wcid));
        crate::errand::plan(
            means,
            &sale,
            &notes,
            &wanted,
            self.autoplay.config.team.restock.float,
            note_face,
        )
    }




    /// The open vendor's stock, priced.
    fn stock(&self) -> Vec<Stock> {
        let Some(v) = self.world.open_vendor.as_ref() else {
            return Vec::new();
        };
        v.items
            .iter()
            .map(|it| Stock {
                guid: it.guid,
                name: it.desc.name.clone(),
                wcid: it.desc.weenie_class_id,
                item_type: it.desc.item_type,
                price: buy_price(it.desc.value, v.sell_rate, it.desc.item_type),
                stack: (it.stack < UNLIMITED_STACK).then_some(it.stack),
                burden: it.desc.burden,
            })
            .collect()
    }

    /// The best vendor to make for, given every way the character has
    /// of being somewhere else: not one being avoided, not one in
    /// `visited`, within `within` metres of a way out when given, and
    /// -- this is the point of it -- one the trip is known in advance
    /// to achieve something at.
    ///
    /// `ways` is where the character can cheaply be (see
    /// [`Self::ways_out`]); a shop is judged by its distance to the
    /// nearest of them.
    ///
    /// Comes back with the forecast it was chosen on, so the caller can
    /// decide whether to set off at all and can say why.
    fn pick_vendor(
        &self,
        cfg: &Growth,
        needs: &[Need],
        ways: &[(Vec2, String)],
        within: Option<f32>,
        visited: &[Vec2],
        now: Instant,
    ) -> Option<(String, Vec2, Forecast)> {
        // How far a shop is: from the nearest way out, not from the
        // feet.
        let reach = |at: Vec2| {
            ways.iter()
                .map(|(w, _)| at.distance(*w))
                .fold(f32::MAX, f32::min)
        };
        // Counters this character can actually trade at. A society's
        // archmage is a very good archmage and no use at all to
        // somebody else's society, and a chapter house behind a quest
        // portal is a journey to a door that will not open.
        let society = self.society();
        let quests = cfg.gates_open.clone();
        let skip = &self.autoplay.growth.skip_vendors;
        let allowed = |at: Vec2| {
            within.is_none_or(|w| reach(at) <= w)
                && !visited.iter().any(|p| p.distance(at) < 1.0)
                && !skip.held(&spot(at), now)
        };
        // Worked out once for the whole search rather than per shop:
        // this walks every counter in range.
        let wants: Vec<(&Need, String)> = needs
            .iter()
            .filter(|n| n.want > 0 && n.buyable)
            .map(|n| {
                let needle = match &n.kind {
                    NeedKind::Named(t) => t.trim().to_lowercase(),
                    _ => String::new(),
                };
                (n, needle)
            })
            .collect();
        let purse = self.spendable();
        let salables = self.salables(cfg);

        // A shop that has what the character came for is worth a longer
        // walk than one that does not: an archer out of quarrels is not
        // helped by the archmage next door. But only so much longer.
        // Ranking on what a shop stocks alone sent a character twenty-
        // five kilometres to a counter with one more line on the shelf,
        // so the search widens in rings and takes the best shop in the
        // first ring that has anything.
        for ring in vendor_rings(within) {
            let best = ac_world::shops::all()
                .iter()
                .filter(|s| allowed(s.xy()) && reach(s.xy()) <= ring)
                .filter(|s| s.open_to(society, &quests))
                .map(|s| (s, forecast(s, &wants, purse, &salables)))
                .filter(|(_, f)| f.worth_going())
                .min_by(|(a, fa), (b, fb)| {
                    // The whole order in one stop beats part of it, more
                    // of the order beats less, and the nearer counter
                    // settles a tie.
                    fb.covers_it()
                        .cmp(&fa.covers_it())
                        .then_with(|| fb.stocks.len().cmp(&fa.stocks.len()))
                        .then_with(|| reach(a.xy()).total_cmp(&reach(b.xy())))
                })
                .map(|(s, f)| (s.name.clone(), s.xy(), f));
            if best.is_some() {
                return best;
            }
        }
        // Nothing sells what is wanted, nothing is wanted at all, or
        // there is no money for any of it: any counter will do, which is
        // the case when the trip is to empty a full pack rather than to
        // buy something. The forecast comes back saying as much, and the
        // caller decides whether that is reason enough to walk.
        ac_world::landmarks::all()
            .iter()
            .filter(|l| l.kind == ac_world::landmarks::Kind::Vendor)
            .filter(|l| allowed(l.xy()))
            .min_by(|a, b| reach(a.xy()).total_cmp(&reach(b.xy())))
            .map(|l| {
                let look = Forecast {
                    shop: l.name.clone(),
                    missing: wants.iter().map(|(n, _)| n.name.clone()).collect(),
                    purse,
                    ..Default::default()
                };
                (l.name.clone(), l.xy(), look)
            })
    }

    /// What the character is short of, worked out afresh at most once
    /// every [`NEEDS_EVERY`]. Counting the pack is not free and the
    /// answer does not change between frames.
    fn needs_now(&mut self, now: Instant, cfg: &Growth) -> Vec<Need> {
        if let Some(n) = self
            .autoplay
            .growth
            .needs_seen
            .as_ref()
            .filter(|(t, _)| now.duration_since(*t) < NEEDS_EVERY)
            .map(|(_, n)| n.clone())
        {
            return n;
        }
        let n = self.grow_needs(cfg);
        self.autoplay.growth.needs_seen = Some((now, n.clone()));
        n
    }

    /// The spell components this character's spells burn, by weenie
    /// class: what it goes to town to buy, as against what it goes to
    /// town to sell.
    pub fn burns(&self, cfg: &Growth) -> Vec<u32> {
        let Ok(mapper) = self.assets.spell_component_ids() else {
            return Vec::new();
        };
        self.component_targets(cfg.tapers_keep)
            .keys()
            .filter_map(|id| mapper.component_wcid(*id))
            .collect()
    }

    /// Pour one loose stack into another, and say what came of it.
    ///
    /// `Did::Done` means the pack is as tight as it goes. `Blocked`
    /// means nothing can be poured until the character is lighter:
    /// the server weighs a pour as though the source were being picked
    /// up for the first time, so a character near its ceiling is
    /// refused a move that changes its burden by nothing at all. The
    /// refusal carries no message and no code, which is how a tidier
    /// that could not read it spent whole afternoons asking.
    pub(crate) fn compress(&mut self, now: Instant) -> crate::did::Did {
        use crate::did::{Because, Did};
        let may_carry = self.burden_room();
        let stacks = self.pack_stacks();
        let held = |from: u32, to: u32| {
            self.autoplay
                .growth
                .wont_merge
                .held(&(from, to), now)
        };
        let Some(m) = crate::pack::next_merge_unless(&stacks, may_carry, held) else {
            // Nothing to pour -- or nothing light enough. The two are
            // worth telling apart: one is a tidy pack, the other is a
            // character that must sell something first.
            return if crate::pack::next_merge(&stacks).is_some() {
                Did::Blocked(Because::ours("too laden to put stacks together"))
            } else {
                Did::Done
            };
        };
        // What the server made of the last ask. It answers a pour it
        // will not make with the source's guid and, half the time, no
        // reason whatever.
        if let Some((code, _)) = self.move_refused.remove(&m.from) {
            let did = Did::Blocked(match code {
                0 => Because::ours("the server would not put those two together"),
                code => Because::server(code),
            });
            self.autoplay.growth.wont_merge.note((m.from, m.to), &did, now);
            return did;
        }
        if self
            .autoplay
            .last_merge
            .is_some_and(|t| now.duration_since(t) < crate::autoplay::MERGE_EVERY)
        {
            return Did::waiting("the last pour has not landed yet");
        }
        self.autoplay.last_merge = Some(now);
        if !self.merge_stacks(m.from, m.to, Some(m.amount)) {
            // Our own rules turned it down: not both carried, not the
            // same weenie, or the target does not stack at all.
            let did = Did::refused("those two will never join");
            self.autoplay.growth.wont_merge.note((m.from, m.to), &did, now);
            return did;
        }
        self.autoplay.say(
            Doing::Tidying,
            format!("putting {} {} with the rest", m.amount, m.name),
        );
        Did::Acting
    }

    /// What the character is short of, as the shopping rules want it:
    /// a weenie class, a name and how many more to buy.
    ///
    /// Only lines that can actually be bought. What no counter stocks
    /// is farmed instead, and a want nobody can fill would hold a trip
    /// open for ever.
    pub(crate) fn vendor_shortfall(&self, cfg: &Growth) -> Vec<ac_vendor::counter::Want> {
        self.grow_needs(cfg)
            .into_iter()
            .filter(|n| n.buyable && n.want > 0)
            .filter_map(|n| {
                let wcid = match n.kind {
                    NeedKind::Component(wcid) => wcid,
                    // Ammunition and named stock are matched on the
                    // shelf by name rather than by class, so they are
                    // left to the older path for now.
                    _ => return None,
                };
                Some(ac_vendor::counter::Want {
                    wcid,
                    name: n.name,
                    short: n.want,
                    urgent: n.urgent,
                })
            })
            .collect()
    }

    /// Everywhere the character can get to cheaply, and what takes it
    /// there.
    ///
    /// A journey is not measured from the feet. A character carries
    /// ways of being somewhere else -- a lifestone recall, the two
    /// portal recalls, whatever gems are in the pack -- and each lands
    /// it at a known spot for the price of one cast. The counter worth
    /// going to is the one nearest *any* of those, not the one nearest
    /// where it happens to be standing.
    ///
    /// It matters most underground, where the feet are the one place
    /// that leads nowhere: a dungeon lies under the landblock it
    /// belongs to, so the nearest counter to a character in Holtburg
    /// Dungeon is one in Holtburg, a hundred metres up through rock.
    /// But it is just as true on the surface -- a recall to Arwic beats
    /// a two-kilometre walk to the shop over the hill.
    fn ways_out(&self, me: Vec2) -> Vec<(Vec2, String)> {
        let mut out: Vec<(Vec2, String)> = Vec::new();
        // Where it stands, unless where it stands leads nowhere.
        if !self.autoplay.growth.in_dungeon {
            out.push((me, "on foot".to_string()));
        }
        let world_xy = |p: ac_world::Position| {
            let o = ac_world::landblock_origin(p.cell);
            Vec2::new(o.x + p.local.x, o.y + p.local.y)
        };
        for r in self.castable_recalls() {
            if let Some(p) = self.recall_destination(r.spell) {
                out.push((world_xy(p), r.name.clone()));
            }
        }
        // A gem needs no skill and no components: carrying one is the
        // whole requirement, which often makes it the cheapest way out
        // a character has.
        for g in self.carried_gems() {
            out.push((g.exit, g.name.clone()));
        }
        // And the dungeon's own door. Every portal's mouth and its
        // landing are known, so a character underground can say which
        // dungeon it is in and where the way out comes up -- and be
        // judged on the shops near *that*, which is what walking out
        // would actually achieve.
        if self.autoplay.growth.in_dungeon {
            if let Some(pl) = self.player.as_ref() {
                let block = pl.cell & 0xFFFF_0000;
                for p in ac_world::portals::all() {
                    if p.from_cell & 0xFFFF_0000 != block || !p.works() {
                        continue;
                    }
                    if !p.exit_outdoors() {
                        continue;
                    }
                    out.push((p.to_xy(), format!("{} (the way out)", p.name)));
                }
            }
        }
        // Nothing to hand: the feet are all there is, wherever they are.
        if out.is_empty() {
            out.push((me, "on foot".to_string()));
        }
        out
    }

    /// What the character is carrying, in burden units, and the most
    /// it may carry.
    ///
    /// The capacity is a hundred and fifty times Strength, plus thirty
    /// more per rank of the carrying-capacity augmentation; the server
    /// refuses to hand over anything that would take the character past
    /// three times that. The comfortable place to work is well under
    /// it: a character at twice its capacity is slow and a character at
    /// three times it cannot pick up what it kills.
    pub fn burden(&self) -> (u32, u32) {
        const ENCUMBRANCE_VAL: u32 = 5;
        const CARRY_AUGMENTATION: u32 = 230;
        let int_of = |k: u32| {
            self.world
                .stats
                .ints
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| (*v).max(0) as u32)
        };
        // What the server says we are carrying, or the pack counted up
        // when it has not said.
        //
        // Counted the way the server counts it, which is not the
        // obvious way. A stack's weight is the whole stack's already,
        // so multiplying by its size counts it twice over; and a side
        // pack's weight is its own plus everything inside it, so adding
        // the contents as well counts those twice too. Only what hangs
        // directly off the character is added: the main pack's items,
        // side packs included as the single lumps they weigh, and
        // whatever is being worn or held.
        let now = int_of(ENCUMBRANCE_VAL).unwrap_or_else(|| {
            self.world
                .main_pack()
                .chain(self.world.wielded())
                .map(|o| o.burden)
                .sum()
        });
        let strength = self.wielder().attributes_current[0];
        let augmented = 150 + 30 * int_of(CARRY_AUGMENTATION).unwrap_or(0);
        (now, strength.saturating_mul(augmented))
    }

    /// How much more the character may be handed before the server
    /// starts refusing: three times its capacity, less what it carries.
    pub fn burden_room(&self) -> u32 {
        let (now, capacity) = self.burden();
        capacity.saturating_mul(3).saturating_sub(now)
    }

    /// How much more the character will take on before it stops
    /// hunting and goes to sell: `loot.carry_up_to` times its capacity,
    /// less what it carries. Zero means it has had enough.
    ///
    /// This is the working limit, not the server's. [`burden_room`] is
    /// the wall -- what the server will still accept -- and a character
    /// that hunts up to the wall cannot loot, cannot merge stacks and
    /// can barely walk.
    pub fn carry_room(&self) -> u32 {
        let (now, capacity) = self.burden();
        let up_to = self.autoplay.config.loot.carry_up_to.max(0.0);
        let limit = (capacity as f32 * up_to) as u32;
        // Never claim more room than the server would actually allow.
        limit.min(capacity.saturating_mul(3)).saturating_sub(now)
    }

    /// Which society this character belongs to, as `Faction1Bits`: 1
    /// the Celestial Hand, 2 the Eldrytch Web, 4 the Radiant Blood, 0
    /// none. The server sends it with the rest of the character's
    /// properties, so this is knowledge rather than the player's word
    /// for it -- and it is what the societies' own counters ask for.
    pub fn society(&self) -> u32 {
        const FACTION1_BITS: u32 = 281;
        self.world
            .stats
            .ints
            .iter()
            .find(|(k, _)| *k == FACTION1_BITS)
            .map(|(_, v)| (*v).max(0) as u32)
            .unwrap_or(0)
    }

    /// Whether the character is stuck: short of something it needs to
    /// go on hunting, with no money to buy it and nothing left to sell.
    ///
    /// This is the one thing it gives up over, and giving up means
    /// stopping in town rather than going back to the hunt: without
    /// supplies there is nothing to earn out there, and the shops,
    /// the party and the player are all here.
    fn stranded(&self, needs: &[Need], cfg: &Growth) -> bool {
        needs.iter().any(|n| n.urgent) && self.spendable() == 0 && self.salables(cfg).is_empty()
    }

    /// Say, once, why no run to town is being started, and answer that
    /// none is. There is nothing to see when a character stands about
    /// doing nothing, so this is how it explains itself.
    fn held_back(&mut self, why: impl Into<String>) -> bool {
        let why = why.into();
        if self.autoplay.growth.held_back != why {
            tracing::debug!("no town run: {why}");
            self.autoplay.growth.held_back = why;
        }
        false
    }

    /// Start a run when one is due, or carry the current one on. True
    /// while a run is in progress.
    fn grow_town_run(&mut self, now: Instant, cfg: &Growth) -> bool {
        if self.autoplay.growth.run.is_some() {
            return self.grow_run_step(now, cfg);
        }
        // Stopped in town for want of money: it stays put. Something
        // has to change from outside -- coin or goods handed over, a
        // quartermaster's delivery -- and when it does it picks the
        // shopping straight back up.
        if self.autoplay.growth.stopped_in_town {
            if self
                .autoplay
                .growth
                .stopped_looked
                .is_some_and(|t| now.duration_since(t) < STOPPED_LOOK_EVERY)
            {
                return false;
            }
            self.autoplay.growth.stopped_looked = Some(now);
            let needs = self.needs_now(now, cfg);
            if self.stranded(&needs, cfg) {
                return false;
            }
            let st = &mut self.autoplay.growth;
            st.stopped_in_town = false;
            st.stopped_looked = None;
            st.run_was_futile = false;
            st.last_run = None;
            self.autoplay
                .note("something to spend again: shopping", now);
        }
        // Too laden to be handed anything: a counter cannot help, so
        // it does not go to one. Selling, using or handing something
        // over is what lifts this, and all three happen elsewhere.
        if self.autoplay.growth.too_heavy {
            let needs = self.needs_now(now, cfg);
            let room = self.burden_room();
            // Room enough for the lightest thing it still wants is
            // room enough to be worth the walk.
            if room == 0 && !needs.is_empty() {
                let (carrying, capacity) = self.burden();
                return self.held_back(format!(
                    "too heavy to buy anything ({carrying} of {capacity}, three times over)"
                ));
            }
            self.autoplay.growth.too_heavy = false;
        }
        // Not while anything else is going on.
        if self.attack_target.is_some()
            || self.autoplay.casting_at().is_some()
            || self.traveling()
            || self.autoplay.growth.bound.is_some()
        {
            return self.held_back("busy with something else");
        }
        // The party has already decided to shop, so the throttle that
        // stops a lone character wearing a path to the vendor does not
        // apply: it would leave the rest waiting at the hunting ground
        // for nothing. The retry delay still holds, since it means a
        // vendor could not be reached at all.
        // The party having agreed to shop lifts the throttle that stops
        // a lone character wearing a path to the vendor -- but not when
        // the last trip came back with nothing. Without that a party
        // that cannot buy what it needs walks between counters for ever.
        let party_restocking = !self.autoplay.growth.mode.hunting();
        let wait = if party_restocking {
            if self.autoplay.growth.run_was_futile {
                FUTILE_RUN_WAIT
            } else {
                Duration::ZERO
            }
        } else {
            RUN_EVERY
        };
        if let Some(t) = self
            .autoplay
            .growth
            .last_run
            .filter(|t| now.duration_since(*t) < wait)
        {
            let left = wait.saturating_sub(now.duration_since(t));
            return self.held_back(format!("{} s to wait since the last run", left.as_secs()));
        }
        if self.autoplay.growth.next_run.is_some_and(|t| now < t) {
            return self.held_back("waiting to try a vendor again");
        }
        let full = self.pack_full();
        // A pack runs out of room two ways, and weight is the one that
        // creeps up unnoticed: slots stay free while the character
        // grows too heavy to lift anything, tidy anything, or move at
        // any speed. It is as good a reason to go and sell as a pack
        // with no slots left.
        let laden = self.carry_room() == 0;
        let needs = self.needs_now(now, cfg);
        // On a team that restocks together, the party's mode decides:
        // one character does not walk off to a vendor while the rest
        // are fighting, and none of them stays behind when the party
        // has agreed to go. Alone, the older rule stands -- something
        // urgent, or a pack with no room left.
        let party_mode = self.autoplay.growth.mode;
        let together =
            self.autoplay.config.team.enabled && self.autoplay.config.team.restock.together;
        let urgent: Vec<&Need> = needs.iter().filter(|n| n.urgent).collect();
        let reason = if together {
            match party_mode.stage() {
                None => return self.held_back("the party is hunting"),
                // Only the runner walks to town; the rest hold their
                // place at the hunting ground and wait for it.
                Some(Stage::HandOver | Stage::Away | Stage::HandOut)
                    if self.autoplay.config.team.restock.plan
                        == crate::logistics::Plan::Quartermaster
                        && !self.is_quartermaster(cfg) =>
                {
                    return self.held_back("the quartermaster is doing the shopping")
                }
                Some(_) => {
                    let because = self.autoplay.growth.mode_because.clone();
                    if because.is_empty() {
                        "the party is restocking".to_string()
                    } else {
                        because
                    }
                }
            }
        } else if full {
            "the pack is full".to_string()
        } else if laden {
            "carrying as much as it means to".to_string()
        } else if urgent.is_empty() {
            let short: Vec<&str> = needs.iter().map(|n| n.name.as_str()).collect();
            return self.held_back(format!("nothing urgent (short of {})", a_few(&short)));
        } else {
            let short: Vec<&str> = urgent
                .iter()
                .filter(|n| n.buyable)
                .map(|n| n.name.as_str())
                .collect();
            format!("short of {}", a_few(&short))
        };
        let Some(me) = self.player.as_ref().map(|p| p.world_position()) else {
            return self.held_back("not placed in the world yet");
        };
        let me = Vec2::new(me.x, me.y);
        // Underground, "nearest" is measured from where the character
        // will come up, not from where it is standing.
        //
        // A dungeon lies under the landblock it belongs to, so the
        // counter nearest a character in Holtburg Dungeon is one in
        // Holtburg -- a hundred metres away and a hundred metres of
        // rock in between. The way out is a recall, and a recall lands
        // somewhere particular; the shops worth considering are the
        // ones near *that*.
        let ways = self.ways_out(me);
        let Some((vendor, at, look)) = self.pick_vendor(cfg, &needs, &ways, None, &[], now) else {
            self.autoplay.note("no vendor to run to", now);
            self.autoplay.growth.last_run = Some(now);
            return false;
        };
        // Why this counter and not another, in the log. The choice is a
        // ring search outwards from `from`, taking the best shop in the
        // first ring with anything worth the walk, so the answer is
        // always some mixture of what it stocks and how far off it is.
        self.autoplay.note(
            format!(
                "{vendor} it is: {}, {:.0} m by {}",
                look.why_it_was_picked(),
                nearest_way(&ways, at).1,
                nearest_way(&ways, at).2
            ),
            now,
        );
        // Know before setting off whether the trip can achieve anything.
        // A counter with nothing the character needs, or nothing it can
        // pay for, is a walk to town and back for its own sake -- and
        // repeated, it is the party pacing between vendors for ever.
        //
        // Two reasons override it. A full pack is its own errand: that
        // trip is to empty it, not to buy. And a character with nothing
        // to spend and nothing to sell goes anyway, because town is
        // where it stops: see [`Self::stranded`].
        if !full && !look.worth_going() && !self.stranded(&needs, cfg) {
            self.autoplay.note(
                format!("not worth a trip to {vendor}: {}", look.tell()),
                now,
            );
            let st = &mut self.autoplay.growth;
            st.last_run = Some(now);
            // Nothing to buy and nothing to sell is exactly what a
            // futile run comes home with, and the party reads it the
            // same way: this one is broke, carry on without it.
            st.run_was_futile = true;
            return false;
        }
        if !self.grow_travel(at, now) {
            // Not the next one straight away: a character somewhere no
            // journey can start from would try every vendor in the
            // world, one a frame.
            let st = &mut self.autoplay.growth;
            st.skip_vendors.note(
                spot(at),
                &crate::did::Did::blocked("no way there from here"),
                now,
            );
            st.next_run = Some(now + RETRY_AFTER);
            self.autoplay
                .note(format!("no way to {vendor} from here"), now);
            return false;
        }
        let st = &mut self.autoplay.growth;
        st.needs = needs;
        st.run = Some(Run {
            vendor: vendor.clone(),
            at,
            phase: Phase::Going,
            since: now,
            last_sell: None,
            town: at,
            stops: 1,
            sold: 0,
            reason: reason.clone(),
            visited: vec![at],
        });
        self.autoplay.say(
            Doing::Shopping,
            format!(
                "{reason}: going to {vendor} ({}) -- {}",
                about(at.distance(me)),
                look.tell()
            ),
        );
        true
    }

    /// The vendor object nearest `at`, by name when one matches.
    fn vendor_object(&self, name: &str, at: Vec2) -> Option<u32> {
        let vendors: Vec<(f32, bool, u32)> = self
            .world
            .objects
            .values()
            .filter(|o| {
                o.object_desc_flags & object_desc_flags::VENDOR != 0
                    || (o.item_type & item_type::CREATURE != 0
                        && o.object_desc_flags & object_desc_flags::ATTACKABLE == 0
                        && !o.is_player
                        && o.name == name)
            })
            .filter_map(|o| {
                let p = o.world_pos()?;
                let d = Vec2::new(p.x, p.y).distance(at);
                (d <= VENDOR_REACH + 10.0).then_some((d, o.name == name, o.guid))
            })
            .collect();
        vendors
            .iter()
            .filter(|(_, named, _)| *named)
            .chain(vendors.iter())
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, _, g)| *g)
    }

    /// One step of the run in progress.
    fn grow_run_step(&mut self, now: Instant, cfg: &Growth) -> bool {
        let Some(mut run) = self.autoplay.growth.run.take() else {
            return false;
        };
        let Some(me) = self.player.as_ref().map(|p| p.world_position()) else {
            self.autoplay.growth.run = Some(run);
            return true;
        };
        let me = Vec2::new(me.x, me.y);
        let elapsed = now.duration_since(run.since);
        match run.phase.clone() {
            Phase::Going => {
                if elapsed > WALK_TIMEOUT {
                    self.autoplay.note(
                        format!("the walk to {} is taking too long", run.vendor),
                        now,
                    );
                    self.cancel_travel();
                    return self.grow_run_next(run, now, cfg, true);
                }
                if self.traveling() || self.grow_travel_on() {
                    self.autoplay.say(
                        Doing::Shopping,
                        format!(
                            "{}: going to {} ({})",
                            run.reason,
                            run.vendor,
                            about(run.at.distance(me))
                        ),
                    );
                    self.autoplay.growth.run = Some(run);
                    return true;
                }
                if run.at.distance(me) > VENDOR_REACH {
                    self.autoplay.note(
                        format!(
                            "could not get to {} ({:.0} m short)",
                            run.vendor,
                            run.at.distance(me)
                        ),
                        now,
                    );
                    return self.grow_run_next(run, now, cfg, true);
                }
                match self.vendor_object(&run.vendor, run.at) {
                    Some(guid) => {
                        // Stand at the counter before asking it to
                        // open. The journey gets the character within
                        // thirty-five metres of where the vendor is
                        // listed, and a vendor will not trade with
                        // somebody across the room: the server answers
                        // by telling us to walk there, and a client
                        // that does not walk waits for a window that
                        // never opens. The same thing that left
                        // characters standing under corpses.
                        let where_it_is = self.world.objects.get(&guid).and_then(|o| o.world_pos());
                        if let Some(spot) = where_it_is {
                            if Vec2::new(spot.x, spot.y).distance(me) > COUNTER_REACH {
                                self.follow = Some(crate::Follow {
                                    target: spot,
                                    stop: COUNTER_REACH / 2.0,
                                });
                                self.autoplay
                                    .say(Doing::Shopping, format!("walking up to {}", run.vendor));
                                self.autoplay.growth.run = Some(run);
                                return true;
                            }
                        }
                        if self.follow.take().is_some() {
                            self.steering.reset();
                        }
                        self.use_object(guid);
                        run.phase = Phase::Opening { guid, tries: 1 };
                        run.since = now;
                        self.autoplay
                            .say(Doing::Shopping, format!("talking to {}", run.vendor));
                        self.autoplay.growth.run = Some(run);
                        true
                    }
                    None => {
                        self.autoplay.note(
                            format!("{} is not here (indoors, or gone)", run.vendor),
                            now,
                        );
                        self.grow_run_next(run, now, cfg, true)
                    }
                }
            }
            Phase::Opening { guid, tries } => {
                let open = self
                    .world
                    .open_vendor
                    .as_ref()
                    .is_some_and(|v| v.vendor == guid);
                if open {
                    // Appraise what might be sold, so weapons can be
                    // judged.
                    let candidates: Vec<u32> = self
                        .world
                        .inventory()
                        .filter(|o| !o.wielder.is_some())
                        .filter(|o| o.value > 0)
                        .filter(|o| !self.appraisals.contains_key(&o.guid))
                        .map(|o| o.guid)
                        .collect();
                    let n = self.appraise_many(candidates);
                    run.phase = Phase::Appraising;
                    run.since = now;
                    self.autoplay.say(
                        Doing::Shopping,
                        format!("at {}; looking over {n} item(s)", run.vendor),
                    );
                    self.autoplay.growth.run = Some(run);
                    return true;
                }
                if elapsed > VENDOR_OPEN_TIMEOUT {
                    if tries < 2 {
                        self.use_object(guid);
                        run.phase = Phase::Opening {
                            guid,
                            tries: tries + 1,
                        };
                        run.since = now;
                        self.autoplay.growth.run = Some(run);
                        return true;
                    }
                    self.autoplay
                        .note(format!("{} would not trade", run.vendor), now);
                    return self.grow_run_next(run, now, cfg, true);
                }
                self.autoplay.growth.run = Some(run);
                true
            }
            Phase::Appraising => {
                let waiting = self
                    .world
                    .inventory()
                    .filter(|o| o.value > 0)
                    .any(|o| !self.appraisals.contains_key(&o.guid));
                if waiting && elapsed < SETTLE * 2 {
                    self.autoplay.growth.run = Some(run);
                    return true;
                }
                // Compress the pack before selling any of it.
                //
                // Loose change and part stacks waste slots, and slots
                // are what a sale runs out of. Doing it here rather
                // than during the selling matters: a merge makes one
                // stack vanish into another, and doing that while a
                // sale is in flight pulls the ground from under it.
                //
                // It stays here until there is nothing left to pour.
                // Falling through after a single pour left the pack
                // almost as loose as it started.
                // Acting or waiting on a pour keeps the tick. Done
                // means the pack is as tight as it goes; blocked means
                // it cannot be tightened until something is sold, and
                // selling is the next thing either way.
                if matches!(
                    self.compress(now),
                    crate::did::Did::Acting | crate::did::Did::Waiting(_)
                ) {
                    self.autoplay.growth.run = Some(run);
                    return true;
                }
                let n = self.sale_list(cfg).len();
                run.phase = Phase::Selling { sent: Vec::new() };
                run.since = now;
                self.autoplay.say(
                    Doing::Shopping,
                    format!("selling {n} item(s) to {}", run.vendor),
                );
                self.autoplay.growth.run = Some(run);
                true
            }
            Phase::Selling { .. } => {
                // The shopping itself is decided in `ac-vendor`, which
                // has no idea what a socket is: it is handed a plain
                // description of the pack, the purse and the counter
                // and answers with one thing to do. This side reads the
                // world into that description and carries the answer
                // out. The same rules run in the panel and in
                // `cargo run -p ac-vendor --example trip`, so a trip
                // can be argued about without a server being awake.
                if run
                    .last_sell
                    .is_some_and(|t| now.duration_since(t) < SELL_EVERY)
                {
                    self.autoplay.growth.run = Some(run);
                    return true;
                }
                let snap = self.vendor_snapshot(cfg);
                let next = self.autoplay.growth.shop.step(&snap, now);
                self.autoplay.growth.last_saying = next.saying.clone();
                match next.act {
                    Some(ac_vendor::Act::Close) | None => {
                        run.sold = self.autoplay.growth.shop.sold;
                        self.close_vendor();
                        self.autoplay.growth.shop = ac_vendor::Run::new();
                        return self.grow_run_next(run, now, cfg, false);
                    }
                    Some(act) => {
                        // A refusal on this side is an answer too: the
                        // rules are told, so that they stop asking
                        // rather than spend the afternoon on it.
                        if !self.do_vendor_act(&act, &next.saying) {
                            self.autoplay
                                .note(format!("could not {}: {act:?}", next.saying), now);
                        }
                        run.last_sell = Some(now);
                        run.since = now;
                        run.phase = Phase::Selling { sent: Vec::new() };
                        self.autoplay.growth.run = Some(run);
                        return true;
                    }
                }
            }
        }
    }

    /// The run is done with this vendor: on to the next of the town
    /// when something is still wanted, else home. `failed` says the
    /// vendor was no use and is to be avoided for a while. True while
    /// the run goes on.
    fn grow_run_next(&mut self, run: Run, now: Instant, cfg: &Growth, failed: bool) -> bool {
        if self.world.open_vendor.is_some() {
            self.close_vendor();
        }
        if failed {
            self.autoplay.growth.skip_vendors.note(
                spot(run.at),
                &crate::did::Did::blocked("that counter was no use"),
                now,
            );
        }
        let needs = self.grow_needs(cfg);
        let still_full = self.pack_full();
        let wanting = needs.iter().any(|n| n.urgent) || still_full;
        if wanting && run.stops < STOPS_PER_RUN {
            // The next counter in the same town is chosen by what is
            // still on the list, not by which is closest -- and only
            // when there is reason to think it can help. A full pack is
            // reason enough on its own: that stop is to empty it.
            if let Some((vendor, at, look)) =
                self.pick_vendor(
                    cfg,
                    &needs,
                    &[(run.town, "in town".to_string())],
                    Some(SAME_TOWN),
                    &run.visited,
                    now,
                )
            {
                if (still_full || look.worth_going()) && self.grow_travel(at, now) {
                    let what = if still_full {
                        "the rest of the loot"
                    } else {
                        "the rest"
                    };
                    self.autoplay.say(
                        Doing::Shopping,
                        format!("on to {vendor} for {what} -- {}", look.tell()),
                    );
                    let mut visited = run.visited;
                    visited.push(at);
                    self.autoplay.growth.run = Some(Run {
                        vendor,
                        at,
                        phase: Phase::Going,
                        since: now,
                        last_sell: None,
                        town: run.town,
                        stops: run.stops + 1,
                        sold: run.sold,
                        reason: run.reason,
                        visited,
                    });
                    return true;
                }
                // Either no journey could be planned to it or it was
                // judged no help; either way, leave it alone for a
                // while rather than choose it again next frame.
                self.autoplay
                    .note(format!("skipping {vendor}: {}", look.tell()), now);
                self.autoplay.growth.skip_vendors.note(
                    spot(at),
                    &crate::did::Did::blocked("could not get there"),
                    now,
                );
            }
        }
        // A trip that neither bought nor sold anything achieved
        // nothing, and starting another at once achieves nothing again:
        // that is the running back and forth between vendors for ever.
        let futile = run.sold == 0 && !self.autoplay.growth.bought_anything;
        let st = &mut self.autoplay.growth;
        st.run_was_futile = futile;
        st.bought_anything = false;
        st.last_run = Some(now);
        st.run = None;
        st.needs.clear();
        st.skip_vendors.tidy(now);
        let sold = run.sold;
        let full = if still_full { ", pack still full" } else { "" };
        self.autoplay
            .note(format!("town run done: sold {sold} item(s){full}"), now);
        // The one thing worth giving up over, and this is what giving
        // up looks like: stay in town. Walking back to the hunting
        // ground without the supplies to work it would only mean
        // walking straight back again.
        if self.stranded(&needs, cfg) {
            let short: Vec<&str> = needs
                .iter()
                .filter(|n| n.urgent)
                .map(|n| n.name.as_str())
                .collect();
            let short = a_few(&short);
            self.autoplay.growth.stopped_in_town = true;
            self.autoplay.growth.stopped_looked = Some(now);
            self.autoplay.growth.bound = None;
            self.autoplay.growth.bound_since = None;
            self.autoplay.say(
                Doing::Shopping,
                format!("out of money and short of {short} -- stopping in town"),
            );
            return false;
        }
        // Back to the hunting ground.
        if let Some(lb) = self.autoplay.growth.hunting_at {
            if let Some(g) = ac_world::hunting::at(lb) {
                let (at, name) = (g.at, g.name.clone());
                if self.grow_travel(at, now) {
                    self.autoplay.growth.bound = Some((lb, at, name.clone()));
                    self.autoplay.growth.bound_since = Some(now);
                    self.autoplay
                        .say(Doing::Traveling, format!("back to hunt {name}"));
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(raise: Raise, cost: u32, weight: f32) -> Offer {
        Offer {
            raise,
            cost,
            weight,
        }
    }

    #[test]
    fn the_best_value_rank_is_bought_and_none_beyond_the_pool() {
        let offers = [
            offer(Raise::Skill(47), 1000, 1.0),
            offer(Raise::Skill(24), 300, 0.3),
            offer(Raise::Attribute(3), 500, 0.6),
            offer(Raise::Vital(0), 400, 0.5),
        ];
        // Attribute: 500/0.6 = 833 beats 1000, 1000 and 800? No: health
        // 400/0.5 = 800 is the best value.
        assert_eq!(choose_raise(&offers, 5000), Some(Raise::Vital(0)));
        // With less to spend, only what is affordable.
        assert_eq!(choose_raise(&offers, 350), Some(Raise::Skill(24)));
        assert_eq!(choose_raise(&offers, 100), None);
        assert_eq!(choose_raise(&[], 100), None);
        // The main skill wins over a minor one at the same price.
        let tie = [
            offer(Raise::Skill(24), 300, 0.3),
            offer(Raise::Skill(47), 300, 1.0),
        ];
        assert_eq!(choose_raise(&tie, 300), Some(Raise::Skill(47)));
        // A zero weight is never bought.
        assert_eq!(choose_raise(&[offer(Raise::Skill(1), 1, 0.0)], 10), None);
    }

    #[test]
    fn skills_are_weighed_by_how_the_character_fights() {
        // An archer: bows first, fletching worth something, war magic not.
        assert_eq!(skill_weight(47, Some(47), false), 1.0);
        assert!(skill_weight(37, Some(47), false) > skill_weight(37, Some(44), false));
        assert!(skill_weight(34, Some(47), false) < 0.5);
        // A caster: war magic first, life magic close behind.
        assert_eq!(skill_weight(34, Some(0), true), 1.0);
        assert!(skill_weight(33, Some(0), true) > 0.8);
        // Another weapon skill is nearly worthless.
        assert!(skill_weight(44, Some(47), false) < 0.2);
    }

    fn item(name: &str, item_type: u32, value: u32) -> ItemStats {
        ItemStats {
            name: name.into(),
            item_type,
            kind: crate::items::kind_name(item_type),
            value,
            ..Default::default()
        }
    }

    #[test]
    fn a_stack_worth_more_than_the_counter_allows_is_sold_in_pieces() {
        // A hundred Pyreal Peas: fifty thousand each and five million
        // the stack, and the server reckons a stack's value as the lot.
        // A counter that will not look at anything over a million
        // refuses all hundred -- so it takes twenty at a time.
        let peas = Salable {
            guid: 1,
            item_type: item_type::SPELL_COMPONENTS,
            value: 5_000_000,
            stack: 100,
        };
        assert_eq!(peas.each(), 50_000);
        assert_eq!(peas.at_once(1_000_000), 20);
        assert!(
            peas.taken_by(item_type::SPELL_COMPONENTS, 0, 1_000_000),
            "in pieces, but taken"
        );

        // A counter with no ceiling takes the lot in one go.
        assert_eq!(peas.at_once(0), 100);

        // One too dear even singly is not taken at all, and saying so
        // is better than splitting a stack down to nothing.
        let jewel = Salable {
            guid: 2,
            item_type: item_type::JEWELRY,
            value: 4_000_000,
            stack: 1,
        };
        assert_eq!(jewel.at_once(1_000_000), 0);
        assert!(!jewel.taken_by(item_type::JEWELRY, 0, 1_000_000));

        // And the counter's floor is about one of them, not the pile:
        // a stack of cheap things is not made sellable by being big.
        let chaff = Salable {
            guid: 3,
            item_type: item_type::MISC,
            value: 10_000,
            stack: 1_000,
        };
        assert_eq!(chaff.each(), 10);
        assert!(
            !chaff.taken_by(item_type::MISC, 100, 0),
            "ten is under the floor"
        );
    }

    #[test]
    fn a_pea_is_sold_and_a_taper_is_kept() {
        // Both are spell components by item type. One is worth fifty
        // thousand at a counter and is what a run to town is paid for;
        // the other is what the character is going to town to buy. The
        // type does not tell them apart -- whether this character's
        // spells burn it does.
        const TAPER: u32 = 691;
        const PEA: u32 = 8330;
        let sell = vec!["value>0".to_string()];
        let casts = [TAPER];
        let rules = SellRules {
            sell: &sell,
            keep: &[],
            can_wield: None,
            tags: &BTreeMap::new(),
            burns: &casts,
        };

        let mut pea = item("Pyreal Pea", item_type::SPELL_COMPONENTS, 50_000);
        pea.wcid = PEA;
        assert!(sellable(&pea, false, &rules), "peas pay for the trip");

        let mut taper = item("Prismatic Taper", item_type::SPELL_COMPONENTS, 5);
        taper.wcid = TAPER;
        assert!(!sellable(&taper, false, &rules), "and tapers are why");

        // A caster that does not use tapers -- an archer, say -- would
        // sell them like any other loot.
        let archer = SellRules {
            sell: &sell,
            keep: &[],
            can_wield: None,
            tags: &BTreeMap::new(),
            burns: &[],
        };
        assert!(sellable(&taper, false, &archer));
    }

    #[test]
    fn what_is_never_sold_is_never_sold() {
        let cfg = Growth::default();
        let sell_everything = vec!["value>0".to_string()];
        let rules = SellRules {
            sell: &sell_everything,
            keep: &[],
            can_wield: Some(false),
            tags: &BTreeMap::new(),
            burns: &[],
        };
        // A rule that says "sell anything" still does not sell these.
        let plain = item("Copper Pea", item_type::MISC, 40);
        assert!(sellable(&plain, false, &rules), "the control");

        let mut tinkered = plain.clone();
        tinkered.tinks = 1;
        assert!(!sellable(&tinkered, false, &rules));
        assert_eq!(never_sell_because(&tinkered), "it has been tinkered");

        let mut inscribed = plain.clone();
        inscribed.inscribed = true;
        assert!(!sellable(&inscribed, false, &rules));
        assert_eq!(never_sell_because(&inscribed), "it is inscribed");

        let mut worn = plain.clone();
        worn.wielded = true;
        assert!(!sellable(&worn, false, &rules));
        assert_eq!(never_sell_because(&worn), "it is equipped");

        // The server's own word: offering one of these is a refusal
        // waiting to happen, and a refusal is what gets a character
        // stuck at a counter.
        let mut refused = plain.clone();
        refused.unsellable = true;
        assert!(!sellable(&refused, false, &rules));
        assert_eq!(never_sell_because(&refused), "no vendor will take it");

        // And a tag put on by hand does not get past them either: the
        // loot rules' "sell this" is a policy, and these are not.
        let mut tags = BTreeMap::new();
        tags.insert(tinkered.guid, LootAction::Sell);
        let tagged = SellRules {
            sell: &cfg.sell,
            keep: &[],
            can_wield: Some(false),
            tags: &tags,
            burns: &[],
        };
        assert!(!sellable(&tinkered, false, &tagged));

        // A pack is only loot when it is empty. One with anything in
        // it takes the character's belongings with it, and the server
        // refuses it besides.
        let sack = item("Sack", item_type::CONTAINER, 65);
        assert!(never_sell_carried(&sack, true), "a full sack stays");
        assert!(!never_sell_carried(&sack, false), "an empty one may go");
        // The rule is about packs, not about everything carried.
        assert!(!never_sell_carried(&plain, true));
    }

    #[test]
    fn loot_is_sold_and_what_the_character_lives_on_is_kept() {
        let cfg = Growth::default();
        let keep = vec!["Healing Kit".to_string()];
        let rules = SellRules {
            sell: &cfg.sell,
            keep: &keep,
            can_wield: None,
            tags: &BTreeMap::new(),
            burns: &[],
        };
        assert!(sellable(
            &item("Leather Cap", item_type::ARMOR, 120),
            false,
            &rules
        ));
        assert!(sellable(
            &item("Ornate Ring", item_type::JEWELRY, 900),
            false,
            &rules
        ));
        assert!(sellable(
            &item("Old Boot", item_type::MISC, 5),
            false,
            &rules
        ));
        // Money, packs, components, tools and ammunition stay.
        assert!(!sellable(
            &item("Pyreal", item_type::MONEY, 1),
            false,
            &rules
        ));
        assert!(!sellable(
            &item("Sack", item_type::CONTAINER, 50),
            false,
            &rules
        ));
        assert!(!sellable(
            &item("Lead Scarab", item_type::SPELL_COMPONENTS, 10),
            false,
            &rules
        ));
        assert!(!sellable(
            &item("Ust", item_type::TINKERING_TOOL, 100),
            false,
            &rules
        ));
        // Food is what a run to town is paid for: peas and their like
        // are picked up to be sold. Food worth keeping goes on the
        // keep list by name, like the healing kits below.
        assert!(sellable(&item("Peas", item_type::FOOD, 40), false, &rules));
        assert!(!sellable(
            &item("Arrow", item_type::MISSILE_WEAPON, 1),
            true,
            &rules
        ));
        assert!(!sellable(
            &item("Bundle of Arrowheads", item_type::CRAFT_FLETCHING_BASE, 30),
            false,
            &rules
        ));
        // The stocked names, whatever their type.
        assert!(!sellable(
            &item("Handy Healing Kit", item_type::MISC, 400),
            false,
            &rules
        ));
        // Worthless things are not offered; worn things never.
        assert!(!sellable(
            &item("Pathwarden Token", item_type::MISC, 0),
            false,
            &rules
        ));
        let mut worn = item("Leather Cap", item_type::ARMOR, 120);
        worn.wielded = true;
        assert!(!sellable(&worn, false, &rules));
    }

    #[test]
    fn the_suit_being_built_is_not_sold_for_pocket_change() {
        let cfg = Growth::default();
        let keep: Vec<String> = Vec::new();
        let rules = SellRules {
            sell: &cfg.sell,
            keep: &keep,
            can_wield: None,
            tags: &BTreeMap::new(),
            burns: &[],
        };
        // Plain armour off a drudge is trash and goes.
        assert!(sellable(
            &item("Leather Cap", item_type::ARMOR, 120),
            false,
            &rules
        ));
        // The same piece with spells on it is a piece of a suit.
        let mut hauberk = item("Hauberk", item_type::ARMOR, 900);
        hauberk.spells = vec!["Epic Life Magic Aptitude".into()];
        assert!(storage_worthy(&hauberk));
        assert!(!sellable(&hauberk, false, &rules));
        let mut ring = item("Ring", item_type::JEWELRY, 400);
        ring.spells = vec!["Epic Endurance".into(), "Epic Focus".into()];
        assert!(!sellable(&ring, false, &rules));
        // Value alone keeps a good drop out of a vendor's hands too.
        assert!(!sellable(
            &item("Olthoi Koujia", item_type::ARMOR, 9_000),
            false,
            &rules
        ));
    }

    #[test]
    fn what_the_loot_rules_tagged_beats_every_other_rule() {
        let cfg = Growth::default();
        let keep: Vec<String> = Vec::new();
        let mut hauberk = item("Hauberk", item_type::ARMOR, 9_000);
        hauberk.guid = 7;
        hauberk.spells = vec!["Epic Life Magic Aptitude".into()];

        // Tagged for sale by hand: it goes, storage rule or not.
        let mut tags = BTreeMap::new();
        tags.insert(7u32, LootAction::Sell);
        let sell_it = SellRules {
            sell: &cfg.sell,
            keep: &keep,
            can_wield: None,
            tags: &tags,
            burns: &[],
        };
        assert!(sellable(&hauberk, false, &sell_it));

        // Tagged to keep or salvage: it stays, search or not.
        for action in [LootAction::Keep, LootAction::Salvage, LootAction::Skip] {
            let mut tags = BTreeMap::new();
            tags.insert(7u32, action);
            let rules = SellRules {
                sell: &cfg.sell,
                keep: &keep,
                can_wield: None,
                tags: &tags,
                burns: &[],
            };
            let mut cap = item("Leather Cap", item_type::ARMOR, 120);
            cap.guid = 7;
            assert!(!sellable(&cap, false, &rules), "{action:?}");
        }

        // The player's own keep list beats even a Sell tag.
        let keep = vec!["Hauberk".to_string()];
        let mut tags = BTreeMap::new();
        tags.insert(7u32, LootAction::Sell);
        let kept = SellRules {
            sell: &cfg.sell,
            keep: &keep,
            can_wield: None,
            tags: &tags,
            burns: &[],
        };
        assert!(!sellable(&hauberk, false, &kept));
    }

    #[test]
    fn weapons_go_only_when_they_are_beyond_the_character() {
        let cfg = Growth::default();
        let keep: Vec<String> = Vec::new();
        let sword = item("Broad Sword", item_type::MELEE_WEAPON, 800);
        let unknown = SellRules {
            sell: &cfg.sell,
            keep: &keep,
            can_wield: None,
            tags: &BTreeMap::new(),
            burns: &[],
        };
        let usable = SellRules {
            sell: &cfg.sell,
            keep: &keep,
            can_wield: Some(true),
            tags: &BTreeMap::new(),
            burns: &[],
        };
        let beyond = SellRules {
            sell: &cfg.sell,
            keep: &keep,
            can_wield: Some(false),
            tags: &BTreeMap::new(),
            burns: &[],
        };
        assert!(!sellable(&sword, false, &unknown), "not appraised: kept");
        assert!(!sellable(&sword, false, &usable));
        assert!(sellable(&sword, false, &beyond));
        let wand = item("Wand", item_type::CASTER, 300);
        assert!(!sellable(&wand, false, &usable));
        assert!(sellable(&wand, false, &beyond));
        // A search list without weapons keeps them regardless.
        let armour_only = vec!["type:armor".to_string()];
        let rules = SellRules {
            sell: &armour_only,
            keep: &keep,
            can_wield: Some(false),
            tags: &BTreeMap::new(),
            burns: &[],
        };
        assert!(!sellable(&sword, false, &rules));
    }

    #[test]
    fn loot_tags_decide_selling() {
        let cfg = Growth::default();
        let keep: Vec<String> = Vec::new();
        let cheap = item("Trinket", item_type::JEWELRY, 5);
        let mut tags = BTreeMap::new();
        tags.insert(cheap.guid, LootAction::Sell);
        let rules = SellRules {
            sell: &cfg.sell,
            keep: &keep,
            can_wield: None,
            tags: &tags,
            burns: &[],
        };
        assert!(sellable(&cheap, false, &rules), "a Sell tag sells");
        let rich = item("Ornate Ring", item_type::JEWELRY, 900);
        let mut tags = BTreeMap::new();
        tags.insert(rich.guid, LootAction::Salvage);
        let rules = SellRules {
            sell: &cfg.sell,
            keep: &keep,
            can_wield: None,
            tags: &tags,
            burns: &[],
        };
        assert!(
            !sellable(&rich, false, &rules),
            "a Salvage tag keeps it for the salvager"
        );
    }

    fn need(kind: NeedKind, want: u32) -> Need {
        Need {
            name: "x".into(),
            want,
            have: 0,
            keep: want,
            urgent: true,
            buyable: true,
            kind,
        }
    }


    #[test]
    fn a_long_list_is_cut_short() {
        assert_eq!(a_few(&[]), "nothing");
        assert_eq!(a_few(&["Myrrh"]), "Myrrh");
        assert_eq!(a_few(&["a", "b", "c", "d"]), "a, b, c, d");
        assert_eq!(a_few(&["a", "b", "c", "d", "e"]), "a, b, c, d and 1 more");
    }

    #[test]
    fn a_focus_is_equipment_and_never_sold() {
        let sell = vec!["type:gem".to_string()];
        let rules = SellRules {
            sell: &sell,
            keep: &[],
            can_wield: None,
            tags: &BTreeMap::new(),
            burns: &[],
        };
        let mut focus = item("Foci of Strife", item_type::GEM, 500);
        focus.wcid = 15271;
        assert!(!sellable(&focus, false, &rules));
        // The rule is the weenie, not the name: a keepsake called after
        // one is still just a keepsake.
        focus.wcid = 999;
        assert!(sellable(&focus, false, &rules));
    }

    fn ware(wcid: u32, name: &str, value: u32) -> ac_world::shops::Ware {
        ac_world::shops::Ware {
            wcid,
            name: name.into(),
            item_type: ac_world::item_type::SPELL_COMPONENTS,
            value,
        }
    }

    fn shop(name: &str, sells: Vec<ac_world::shops::Ware>) -> ac_world::shops::Shop {
        ac_world::shops::Shop {
            wcid: 1,
            name: name.into(),
            cell: 0x0001_0001,
            at: glam::Vec3::ZERO,
            sells,
            buys: ac_world::item_type::GEM,
            min_value: 10,
            max_value: 0,
            buy_rate: 0.5,
            sell_rate: 2.0,
            gate: None,
        }
    }

    fn salable(guid: u32, item_type: u32, value: u32, stack: u32) -> Salable {
        Salable {
            guid,
            item_type,
            value,
            stack,
        }
    }

    #[test]
    fn a_counter_behind_a_door_is_not_planned_around() {
        use ac_world::shops::Gate;
        let mut hall = shop(
            "Vermilia the Archmage",
            vec![ware(37155, "Mana Scarab", 15_000)],
        );
        hall.gate = Some(Gate::Society(4));
        // A very good archmage, and no use at all to anyone else's
        // society or to nobody's.
        assert!(hall.open_to(4, &[]));
        assert!(!hall.open_to(2, &[]));
        assert!(!hall.open_to(0, &[]));
        let mut chapter = shop("Rossu Morta Quartermaster", vec![]);
        chapter.gate = Some(Gate::Quest("RossuMortaChapterhouse_Flag".into()));
        assert!(!chapter.open_to(4, &[]), "no society opens a quest door");
        assert!(chapter.open_to(0, &["rossumortachapterhouse_flag".to_string()]));
    }

    #[test]
    fn case_folds_without_allocating() {
        assert!(contains_fold("Prismatic Taper", "taper"));
        assert!(contains_fold("PRISMATIC TAPER", "prismatic"));
        assert!(!contains_fold("Lead Scarab", "taper"));
        assert!(!contains_fold("Tap", "taper"));
        assert!(!contains_fold("anything", ""));
    }

    #[test]
    fn a_trip_is_judged_before_it_is_walked() {
        let mut tapers = need(NeedKind::Component(691), 100);
        tapers.name = "Prismatic Taper".into();
        let mut scarabs = need(NeedKind::Component(690), 20);
        scarabs.name = "Lead Scarab".into();
        let needs = [tapers, scarabs];
        let wants: Vec<(&Need, String)> = needs.iter().map(|n| (n, String::new())).collect();

        let mage = shop(
            "Archmage",
            vec![
                ware(691, "Prismatic Taper", 5),
                ware(690, "Lead Scarab", 30),
            ],
        );
        let smith = shop("Blacksmith", vec![ware(20630, "Trade Note", 250)]);

        // Money in hand, everything on the shelf: the whole order in one
        // stop, and the trip is worth walking.
        let rich = forecast(&mage, &wants, 10_000, &[]);
        assert_eq!(rich.bill, 100 * 10 + 20 * 60);
        assert_eq!(rich.cheapest, 10);
        assert!(rich.missing.is_empty());
        assert!(rich.covers_it());
        assert!(rich.worth_going());

        // Enough for some of it is still worth walking: a mage that can
        // afford half its tapers is a mage that can keep casting.
        let thin = forecast(&mage, &wants, 50, &[]);
        assert!(!thin.covers_it());
        assert!(thin.worth_going());

        // Not a copper, and nothing to sell: the trip buys nothing and
        // is not made.
        let broke = forecast(&mage, &wants, 0, &[]);
        assert_eq!(broke.funds(), 0);
        assert!(!broke.worth_going());

        // Unless there is something to sell, which pays for the rest.
        //
        // Three gems, twelve hundred the lot: a stack's `value` is the
        // whole stack's, which is how the server reckons it and what a
        // counter's limits are measured against. Four hundred each, the
        // shop pays half, so six hundred for the three.
        let loot = [salable(9, ac_world::item_type::GEM, 1_200, 3)];
        let selling = forecast(&mage, &wants, 0, &loot);
        assert_eq!(selling.takings, 200 * 3);
        assert_eq!(selling.selling, 1);
        assert!(selling.worth_going());
        // What it will not buy does not count towards the trip.
        let armour = [salable(9, ac_world::item_type::ARMOR, 400, 1)];
        assert_eq!(forecast(&mage, &wants, 0, &armour).takings, 0);
        // Nor does what is beneath its notice -- and "beneath" is
        // about one of them, not the pile.
        let trinket = [salable(9, ac_world::item_type::GEM, 5, 1)];
        assert_eq!(forecast(&mage, &wants, 0, &trinket).takings, 0);

        // A counter that stocks none of it is no help however rich the
        // character is.
        let wrong = forecast(&smith, &wants, 100_000, &[]);
        assert_eq!(wrong.cheapest, 0);
        assert_eq!(wrong.missing.len(), 2);
        assert!(!wrong.covers_it());
        assert!(!wrong.worth_going());
    }





    #[test]
    fn a_named_ground_is_not_judged_on_level() {
        // Hunting a place is about its loot, its money, its trophies.
        // A ground the player named must not be refused because the
        // level table disapproves, and `suits` is the only thing that
        // would refuse it.
        let ground = ac_world::hunting::all()
            .iter()
            .find(|g| g.max_level < 50)
            .expect("a low-level ground");
        // Far too low for a level 275 character by the usual rule...
        assert!(!ground.suits(275, 8));
        // ...but naming it is a landblock id, and looking one up asks
        // nothing about levels.
        assert_eq!(
            ac_world::hunting::at(ground.landblock).map(|g| g.landblock),
            Some(ground.landblock)
        );
    }

    #[test]
    fn the_search_widens_rather_than_crossing_the_world() {
        // The bug: ranking on what a shop stocks alone sent a character
        // twenty-five kilometres to a counter with one more line on the
        // shelf. The rings mean a good enough shop in this town wins.
        let r = vendor_rings(None);
        assert_eq!(r.first().copied(), Some(600.0), "the town first");
        assert!(r.windows(2).all(|w| w[0] < w[1]), "{r:?} does not widen");
        assert_eq!(r.last().copied(), Some(f32::INFINITY), "and then anywhere");
    }

    #[test]
    fn a_capped_search_never_looks_past_the_cap() {
        // The next stop of a run stays in the same town.
        let r = vendor_rings(Some(500.0));
        assert_eq!(r, vec![500.0]);
        assert!(r.iter().all(|x| *x <= 500.0));
        // A cap between rings keeps the ones below it.
        let mid = vendor_rings(Some(1_000.0));
        assert_eq!(mid, vec![600.0, 1_000.0]);
    }

    #[test]
    fn there_is_always_a_last_ring_to_fall_back_on() {
        // Whatever the cap, the search ends somewhere rather than
        // leaving the character with nowhere to go.
        for cap in [1.0f32, 600.0, 3_000.0, 15_000.0, 100_000.0] {
            let r = vendor_rings(Some(cap));
            assert!(!r.is_empty(), "cap {cap}");
            assert_eq!(r.last().copied(), Some(cap));
        }
    }

    #[test]
    fn a_healing_kit_is_not_bought_for_someone_who_cannot_use_one() {
        // Untrained Healing: a kit restores next to nothing, so it is
        // not worth the money or the trip to a vendor.
        assert!(!worth_stocking("Healing Kit", false));
        assert!(!worth_stocking("Excellent Healing Kit", false));
        // Trained, it is worth having again.
        assert!(worth_stocking("Healing Kit", true));
        // Everything else is judged on its own, either way.
        assert!(worth_stocking("Prismatic Taper", false));
        assert!(worth_stocking("Mana Stone", false));
    }



    #[test]
    fn ammunition_is_the_plain_kind() {
        use ac_world::fletching::ammo_type;
        assert!(ammo_stock("Arrow", ammo_type::ARROW));
        assert!(ammo_stock("arrow ", ammo_type::ARROW));
        assert!(!ammo_stock("Fire Arrow", ammo_type::ARROW));
        assert!(!ammo_stock("Arrow", ammo_type::BOLT));
        assert!(ammo_stock("Quarrel", ammo_type::BOLT));
        assert!(ammo_stock("Atlatl Dart", ammo_type::ATLATL));
        assert_eq!(buy_price(100, 1.0, 0), 100);
        assert_eq!(buy_price(0, 1.0, 1), 1);
        // A shop that marks everything up by 1.7 still sells notes at
        // the server's flat 1.15.
        assert_eq!(buy_price(100, 1.7, 0), 170);
        assert_eq!(buy_price(100, 1.7, item_type::PROMISSORY_NOTE), 115);
    }

    #[test]
    fn growth_config_has_defaults_and_round_trips() {
        let g: Growth = serde_json::from_str("{}").unwrap();
        assert_eq!(g, Growth::default());
        assert!(g.auto_xp && g.hunt_grounds && g.town_runs);
        let text = serde_json::to_string(&g).unwrap();
        let back: Growth = serde_json::from_str(&text).unwrap();
        assert_eq!(back, g);
        let partial: Growth =
            serde_json::from_str(r#"{"auto_xp":false,"level_margin":3}"#).unwrap();
        assert!(!partial.auto_xp);
        assert_eq!(partial.level_margin, 3);
        assert!(partial.town_runs);
    }
}
