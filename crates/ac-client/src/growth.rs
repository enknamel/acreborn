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
/// A vendor that could not be reached or would not open is left alone
/// for this long.
const SKIP_VENDOR_FOR: Duration = Duration::from_secs(30 * 60);
/// Sales go out a few at a time, this often.
const SELL_EVERY: Duration = Duration::from_millis(500);
const SELL_BATCH: usize = 4;
/// How long to wait for the server to take the items sold, the
/// appraisals to come back, or the purchases to arrive.
const SETTLE: Duration = Duration::from_secs(5);
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
    /// How many of each spell component to carry, for a character that
    /// casts. A run is made when any is down to a quarter.
    pub comps_keep: u32,
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
            idle_before_move: 60.0,
            town_runs: true,
            keep_stocked: vec![("Healing Kit".into(), 2)],
            ammo_keep: 250,
            comps_keep: 40,
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
    /// Sales going out, a batch at a time.
    Selling { queue: Vec<u32>, sent: Vec<u32> },
    /// Purchases sent; waiting for them to arrive.
    Buying,
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
    /// How many trips the quartermaster has made this time out.
    pub round: u32,
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
    /// Vendors not to go to for a while (by position), and since when.
    skip_vendors: Vec<(Vec2, Instant)>,
    /// Items a vendor would not take: `(vendor, item)`. Another vendor
    /// may.
    unsellable: Vec<(u32, u32)>,
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

/// What a vendor's stock line is, for matching against needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Stock {
    pub guid: u32,
    pub name: String,
    pub wcid: u32,
    /// What one costs at this vendor.
    pub price: u32,
    /// How many it has; `None` for unlimited.
    pub stack: Option<u32>,
}

/// What the character wants from the stock of a vendor, and can pay
/// for: `(stock guid, amount)` per need, cheapest match first, the
/// purse running down as it goes.
fn orders(needs: &[Need], stock: &[Stock], mut purse: u32) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for need in needs {
        let want = |s: &Stock| match &need.kind {
            NeedKind::Named(n) => s.name.to_lowercase().contains(&n.to_lowercase()),
            NeedKind::Ammo(kind) => ammo_stock(&s.name, *kind),
            NeedKind::Component(wcid) => s.wcid == *wcid,
        };
        let Some(line) = stock
            .iter()
            .filter(|s| want(s) && s.price > 0)
            .min_by_key(|s| s.price)
        else {
            continue;
        };
        let mut amount = need.want.min(purse / line.price);
        if let Some(have) = line.stack {
            amount = amount.min(have);
        }
        if amount == 0 {
            continue;
        }
        purse -= amount * line.price;
        match out.iter_mut().find(|(g, _)| *g == line.guid) {
            Some(o) => o.1 += amount,
            None => out.push((line.guid, amount)),
        }
    }
    out
}

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

pub fn buy_price(value: u32, sell_rate: f32) -> u32 {
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
pub fn sellable(stats: &ItemStats, ammo: bool, rules: &SellRules) -> bool {
    // Not sellable at all.
    if stats.wielded || stats.value == 0 || ammo {
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
    // Things a vendor should never be handed: money, the packs
    // themselves, what spells and crafting are made of.
    let keep_types = item_type::MONEY
        | item_type::CONTAINER
        | item_type::SPELL_COMPONENTS
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
        // new hunting ground in the middle of it.
        if cfg.hunt_grounds && mode.hunting() && self.grow_hunt(now, &cfg) {
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
        if pl.is_indoors() {
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
        // On a ground with nothing in sight, look about first.
        if self.autoplay.growth.hunting_at == Some(here) && self.autoplay.growth.roams < ROAMS {
            let n = self.autoplay.growth.roams;
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
            if have < least {
                if !needs
                    .iter()
                    .any(|n: &Need| n.kind == NeedKind::Named(name.clone()))
                {
                    needs.push(Need {
                        name: name.clone(),
                        want: least - have,
                        have,
                        keep: least,
                        urgent: true,
                        kind: NeedKind::Named(name),
                    });
                }
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
                    kind: NeedKind::Ammo(kind),
                });
            }
        }
        // Components: for a character with spells and a wand. Those it
        // carries, and those its buffs ask for that it has run out of.
        if cfg.comps_keep > 0 && !self.world.stats.spells.is_empty() {
            let has_wand = self.wielded_caster().is_some()
                || self
                    .world
                    .inventory()
                    .any(|o| o.item_type & item_type::CASTER != 0);
            if has_wand {
                if let Ok(mapper) = self.assets.spell_component_ids() {
                    let mut ids: Vec<u32> = self
                        .wanted_buffs()
                        .iter()
                        .flat_map(|w| self.current_formula(w.spell))
                        .collect();
                    let carried = self.components();
                    ids.extend(carried.iter().map(|c| c.component_id));
                    ids.sort_unstable();
                    ids.dedup();
                    for id in ids {
                        let Some(wcid) = mapper.component_wcid(id) else {
                            continue;
                        };
                        let c = carried.iter().find(|c| c.component_id == id);
                        let have = c.map_or(0, |c| c.count);
                        if have < cfg.comps_keep {
                            let name = c
                                .map(|c| c.name.clone())
                                .or_else(|| mapper.name_of(id).map(str::to_string))
                                .unwrap_or_else(|| format!("component {id}"));
                            needs.push(Need {
                                name,
                                want: cfg.comps_keep - have,
                                have,
                                keep: cfg.comps_keep,
                                urgent: have < cfg.comps_keep / 4,
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
    fn purse(&self) -> u32 {
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

    /// Free slots in the main pack: what decides who can carry the
    /// party's shopping.
    pub fn free_space(&self) -> u32 {
        let capacity = self
            .world
            .player()
            .map(|p| p.items_capacity)
            .filter(|c| *c > 0)
            .unwrap_or(102);
        capacity.saturating_sub(self.world.main_pack().count() as u32)
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
        let level = needs
            .iter()
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
            free_space: self.free_space(),
            order: needs
                .iter()
                .filter(|n| n.want > 0)
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
        let wielder = self.wielder();
        let keep = self.keep_names(cfg);
        let tags = self.autoplay.tags().clone();
        let rules_for = |stats: &ItemStats| SellRules {
            sell: &cfg.sell,
            keep: &keep,
            can_wield: stats.appraised.then(|| wielder.can_wield(stats)),
            tags: &tags,
        };
        let unsellable = &self.autoplay.growth.unsellable;
        let vendor = v.vendor;
        // The vendor buys some kinds of thing, within a range of values
        // (a range of 0 is no range at all).
        let in_range =
            |value: u32| value >= v.min_value && (v.max_value == 0 || value <= v.max_value);
        self.world
            .inventory()
            .filter(|o| o.item_type & v.item_types != 0 && in_range(o.value))
            .filter(|o| !unsellable.contains(&(vendor, o.guid)))
            .filter_map(|o| {
                let stats = self.stats_of(o.guid)?;
                let ammo = o.valid_locations & equip::MISSILE_AMMO != 0;
                sellable(&stats, ammo, &rules_for(&stats)).then_some(o.guid)
            })
            .collect()
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
                price: buy_price(it.desc.value, v.sell_rate),
                stack: (it.stack < UNLIMITED_STACK).then_some(it.stack),
            })
            .collect()
    }

    /// The nearest vendor to `from` that is not being avoided and not
    /// in `visited`, within `within` metres when given.
    fn pick_vendor(
        &self,
        from: Vec2,
        within: Option<f32>,
        visited: &[Vec2],
        now: Instant,
        wanted: &[String],
    ) -> Option<(String, Vec2)> {
        let skip = &self.autoplay.growth.skip_vendors;
        let allowed = |at: Vec2| {
            within.is_none_or(|w| at.distance(from) <= w)
                && !visited.iter().any(|p| p.distance(at) < 1.0)
                && !skip
                    .iter()
                    .any(|(p, t)| p.distance(at) < 1.0 && now.duration_since(*t) < SKIP_VENDOR_FOR)
        };
        // A shop that has what the character came for is worth a longer
        // walk than one that does not. An archer out of quarrels is not
        // helped by the archmage next door, which is what made these
        // runs look aimless: the nearest vendor was the only thing that
        // decided them.
        let stocked = ac_world::shops::all()
            .iter()
            .filter(|s| allowed(s.xy()))
            .map(|s| {
                let has = wanted.iter().filter(|w| s.stocks(w).is_some()).count();
                (s, has)
            })
            .filter(|(_, has)| *has > 0)
            .min_by(|(a, ha), (b, hb)| {
                hb.cmp(ha)
                    .then_with(|| a.xy().distance(from).total_cmp(&b.xy().distance(from)))
            })
            .map(|(s, _)| (s.name.clone(), s.xy()));
        if stocked.is_some() {
            return stocked;
        }
        // Nothing sells what is wanted, or nothing is wanted at all:
        // any counter will do, which is the case when the trip is to
        // empty a full pack rather than to buy something.
        ac_world::landmarks::all()
            .iter()
            .filter(|l| l.kind == ac_world::landmarks::Kind::Vendor)
            .filter(|l| allowed(l.xy()))
            .min_by(|a, b| a.xy().distance(from).total_cmp(&b.xy().distance(from)))
            .map(|l| (l.name.clone(), l.xy()))
    }

    /// Start a run when one is due, or carry the current one on. True
    /// while a run is in progress.
    fn grow_town_run(&mut self, now: Instant, cfg: &Growth) -> bool {
        if self.autoplay.growth.run.is_some() {
            return self.grow_run_step(now, cfg);
        }
        // Not while anything else is going on.
        if self.attack_target.is_some()
            || self.autoplay.casting_at().is_some()
            || self.traveling()
            || self.autoplay.growth.bound.is_some()
        {
            return false;
        }
        // The party has already decided to shop, so the throttle that
        // stops a lone character wearing a path to the vendor does not
        // apply: it would leave the rest waiting at the hunting ground
        // for nothing. The retry delay still holds, since it means a
        // vendor could not be reached at all.
        let party_restocking = !self.autoplay.growth.mode.hunting();
        if !party_restocking
            && self
                .autoplay
                .growth
                .last_run
                .is_some_and(|t| now.duration_since(t) < RUN_EVERY)
        {
            return false;
        }
        if self.autoplay.growth.next_run.is_some_and(|t| now < t) {
            return false;
        }
        let full = self.pack_full();
        let fresh = self
            .autoplay
            .growth
            .needs_seen
            .as_ref()
            .filter(|(t, _)| now.duration_since(*t) < NEEDS_EVERY)
            .map(|(_, n)| n.clone());
        let needs = match fresh {
            Some(n) => n,
            None => {
                let n = self.grow_needs(cfg);
                self.autoplay.growth.needs_seen = Some((now, n.clone()));
                n
            }
        };
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
                None => return false,
                // Only the runner walks to town; the rest hold their
                // place at the hunting ground and wait for it.
                Some(Stage::HandOver | Stage::Away | Stage::HandOut)
                    if self.autoplay.config.team.restock.plan
                        == crate::logistics::Plan::Quartermaster
                        && !self.is_quartermaster(cfg) =>
                {
                    return false
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
        } else if urgent.is_empty() {
            return false;
        } else {
            format!(
                "short of {}",
                urgent
                    .iter()
                    .map(|n| n.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let Some(me) = self.player.as_ref().map(|p| p.world_position()) else {
            return false;
        };
        let me = Vec2::new(me.x, me.y);
        let shopping: Vec<String> = needs.iter().map(|n| n.name.clone()).collect();
        let Some((vendor, at)) = self.pick_vendor(me, None, &[], now, &shopping) else {
            self.autoplay.note("no vendor to run to", now);
            self.autoplay.growth.last_run = Some(now);
            return false;
        };
        if !self.grow_travel(at, now) {
            // Not the next one straight away: a character somewhere no
            // journey can start from would try every vendor in the
            // world, one a frame.
            let st = &mut self.autoplay.growth;
            st.skip_vendors.push((at, now));
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
            format!("{reason}: going to {vendor} ({})", about(at.distance(me))),
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
                let queue = self.sale_list(cfg);
                let n = queue.len();
                run.phase = Phase::Selling {
                    queue,
                    sent: Vec::new(),
                };
                run.since = now;
                self.autoplay.say(
                    Doing::Shopping,
                    format!("selling {n} item(s) to {}", run.vendor),
                );
                self.autoplay.growth.run = Some(run);
                true
            }
            Phase::Selling {
                mut queue,
                mut sent,
            } => {
                if self.world.open_vendor.is_none() {
                    self.autoplay.note("the vendor closed on us", now);
                    return self.grow_run_next(run, now, cfg, false);
                }
                if !queue.is_empty() {
                    if run
                        .last_sell
                        .is_none_or(|t| now.duration_since(t) >= SELL_EVERY)
                    {
                        for _ in 0..SELL_BATCH {
                            let Some(g) = queue.pop() else { break };
                            if self.world.is_carried(g) {
                                self.sell(g);
                                sent.push(g);
                            }
                        }
                        run.last_sell = Some(now);
                        run.since = now;
                    }
                    run.phase = Phase::Selling { queue, sent };
                    self.autoplay.growth.run = Some(run);
                    return true;
                }
                // Everything sent: wait for the server to take it.
                let still: Vec<u32> = sent
                    .iter()
                    .copied()
                    .filter(|g| self.world.is_carried(*g))
                    .collect();
                if !still.is_empty() && elapsed < SETTLE {
                    run.phase = Phase::Selling { queue, sent };
                    self.autoplay.growth.run = Some(run);
                    return true;
                }
                run.sold += (sent.len() - still.len()) as u32;
                if !still.is_empty() {
                    tracing::info!("growth: {} item(s) the vendor would not take", still.len());
                    let vendor = self.world.open_vendor.as_ref().map_or(0, |v| v.vendor);
                    self.autoplay
                        .growth
                        .unsellable
                        .extend(still.into_iter().map(|g| (vendor, g)));
                }
                // Now buy what is short.
                let needs = self.grow_needs(cfg);
                let stock = self.stock();
                let purse = self.purse();
                let orders = orders(&needs, &stock, purse);
                let mut bought = Vec::new();
                for (guid, amount) in &orders {
                    self.buy_amount(*guid, *amount);
                    let name = stock
                        .iter()
                        .find(|s| s.guid == *guid)
                        .map(|s| s.name.clone())
                        .unwrap_or_default();
                    bought.push(format!("{amount} {name}"));
                }
                self.autoplay.growth.needs = needs;
                run.phase = Phase::Buying;
                run.since = now;
                let sold = run.sold;
                self.autoplay.say(
                    Doing::Shopping,
                    if bought.is_empty() {
                        format!("sold {sold} item(s); nothing to buy here")
                    } else {
                        format!("sold {sold} item(s); buying {}", bought.join(", "))
                    },
                );
                self.autoplay.growth.run = Some(run);
                true
            }
            Phase::Buying => {
                if elapsed < Duration::from_secs(2) {
                    self.autoplay.growth.run = Some(run);
                    return true;
                }
                self.close_vendor();
                self.grow_run_next(run, now, cfg, false)
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
            self.autoplay.growth.skip_vendors.push((run.at, now));
        }
        let needs = self.grow_needs(cfg);
        let still_full = self.pack_full();
        let wanting = needs.iter().any(|n| n.urgent) || still_full;
        if wanting && run.stops < STOPS_PER_RUN {
            // The next counter in the same town is chosen by what is
            // still on the list, not by which is closest.
            let left: Vec<String> = needs.iter().map(|n| n.name.clone()).collect();
            if let Some((vendor, at)) =
                self.pick_vendor(run.town, Some(SAME_TOWN), &run.visited, now, &left)
            {
                if self.grow_travel(at, now) {
                    let what = if still_full {
                        "the rest of the loot"
                    } else {
                        "the rest"
                    };
                    self.autoplay
                        .say(Doing::Shopping, format!("on to {vendor} for {what}"));
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
                self.autoplay.growth.skip_vendors.push((at, now));
            }
        }
        let st = &mut self.autoplay.growth;
        st.last_run = Some(now);
        st.run = None;
        st.needs.clear();
        st.skip_vendors
            .retain(|(_, t)| now.duration_since(*t) < SKIP_VENDOR_FOR);
        let sold = run.sold;
        let full = if still_full { ", pack still full" } else { "" };
        self.autoplay
            .note(format!("town run done: sold {sold} item(s){full}"), now);
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
    fn loot_is_sold_and_what_the_character_lives_on_is_kept() {
        let cfg = Growth::default();
        let keep = vec!["Healing Kit".to_string()];
        let rules = SellRules {
            sell: &cfg.sell,
            keep: &keep,
            can_wield: None,
            tags: &BTreeMap::new(),
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
        };
        let usable = SellRules {
            sell: &cfg.sell,
            keep: &keep,
            can_wield: Some(true),
            tags: &BTreeMap::new(),
        };
        let beyond = SellRules {
            sell: &cfg.sell,
            keep: &keep,
            can_wield: Some(false),
            tags: &BTreeMap::new(),
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
            kind,
        }
    }

    fn stock(guid: u32, name: &str, wcid: u32, price: u32, stack: Option<u32>) -> Stock {
        Stock {
            guid,
            name: name.into(),
            wcid,
            price,
            stack,
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
    fn orders_match_needs_to_stock_within_the_purse() {
        let needs = vec![
            need(NeedKind::Named("Healing Kit".into()), 2),
            need(NeedKind::Ammo(ac_world::fletching::ammo_type::ARROW), 200),
            need(NeedKind::Component(691), 30),
        ];
        let shelf = vec![
            stock(1, "Handy Healing Kit", 100, 40, None),
            stock(2, "Excellent Healing Kit", 101, 300, None),
            stock(3, "Fire Arrow", 200, 5, None),
            stock(4, "Arrow", 201, 1, None),
            stock(5, "Bundle of Arrowheads", 202, 20, None),
            stock(6, "Lead Scarab", 691, 1, Some(10)),
        ];
        // Cheapest kit, plain arrows, the scarab by weenie; the scarab
        // stock is only ten deep.
        assert_eq!(
            orders(&needs, &shelf, 10_000),
            vec![(1, 2), (4, 200), (6, 10)]
        );
        // A thin purse buys what it can, in order of need.
        assert_eq!(orders(&needs, &shelf, 100), vec![(1, 2), (4, 20)]);
        assert_eq!(orders(&needs, &shelf, 0), vec![]);
        // Nothing wanted from a stock that has none of it.
        assert!(orders(&needs, &[stock(9, "Sword", 5, 10, None)], 1000).is_empty());
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
        assert_eq!(buy_price(100, 1.0), 100);
        assert_eq!(buy_price(0, 1.0), 1);
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
