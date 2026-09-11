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
//! 2. **Urgent buffs**: one about to run out goes back up, fight or no
//!    fight. Fighting with spells, the buffs take turns with the attack
//!    spells rather than crowding them out (`buff_yields_to_attack`).
//! 3. **Loot**: a corpse of something we killed is opened, the items
//!    the rules say to take are taken, and it is closed again.
//! 4. **Salvage**: items the rules tagged for salvage are salvaged by
//!    the team's best salvager, and carried to it by everyone else.
//! 5. **Fight**: pick the nearest creature that passes the name rules
//!    and attack it, with the weapon that suits it best.
//! 6. **Top up buffs**: in a quiet moment, recast anything that has
//!    run out or will soon.
//!
//! Loot is judged by ordered [`LootRule`]s in the inventory's own search
//! language (`crate::items::Query`): `value>500` keep, `slot:ring
//! epics>=2` keep, `ws<6 -epics>0` salvage. The first rule that matches
//! decides ([`loot_action`]); items are appraised first when a rule
//! needs numbers. Everything a rule keeps, salvages or sells is picked
//! up, and its guid tagged with the action (`Autoplay::loot_action`) for
//! the salvage pass, the UI and scripts.
//!
//! Nothing here talks to the UI: the panel edits a [`Config`] and reads
//! [`Autoplay::status`].

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::items::Query;
use crate::{Client, Stance};

/// How long to wait for a corpse to open before asking again, when it
/// is right under our feet. A corpse further off is given time for the
/// walk as well (see [`loot_wait`]).
///
/// Short, because the character now walks to the corpse itself (see
/// [`CORPSE_REACH`]) and asks from on top of it. The first ask is
/// nonetheless often ignored -- the server still has us moving -- so
/// what matters is how quickly the second one follows.
const LOOT_TIMEOUT: Duration = Duration::from_millis(2500);
/// How many times to ask before leaving a corpse alone. Asking is
/// cheap now that it is asked from arm's length.
const LOOT_TRIES: u32 = 3;

/// A wait that has doubled this far means the stack has been asked to
/// come apart half a dozen times and has not. The party gets on
/// without that hand-over.
const WILL_NOT_SPLIT: Duration = Duration::from_secs(8);
/// Coming this much closer (metres) counts as getting somewhere.
const REACH_PROGRESS: f32 = 1.0;
/// Walking towards something for this long without getting closer is
/// not walking towards it any more.
const REACH_GIVE_UP: Duration = Duration::from_secs(20);
/// How long a corpse may stay open before the character gives up on
/// it. Emptying one takes a second or two; anything past this is an
/// item the server will not hand over, and standing there asking for
/// it again every four hundred milliseconds is how a character spends
/// an afternoon over one drudge.
const LOOT_GIVE_UP: Duration = Duration::from_secs(45);
/// A pessimistic walking speed for pricing that walk, metres a second:
/// the way round a dungeon corner is longer than the line to it.
const LOOT_WALK: f32 = 2.5;

/// How long to allow a corpse `away` metres off to open.
///
/// Opening one asks the server to walk us there, and that walk is not
/// instant: a flat six seconds was enough for a corpse at our feet and
/// not for one across a room, so the far ones were written off unopened.
fn loot_wait(away: f32) -> Duration {
    LOOT_TIMEOUT + Duration::from_secs_f32((away.max(0.0) / LOOT_WALK).min(30.0))
}
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
/// How long dropping to peace mode takes on the server, which will not
/// craft in any other stance.
const STANCE_CHANGE: Duration = Duration::from_millis(1000);
/// The Fletching skill.
const FLETCHING: u32 = 37;
/// The same note is not logged again within this.
const NOTE_EVERY: Duration = Duration::from_secs(5);
/// Least time between two salvage batches, and between two hand-offs.
const SALVAGE_EVERY: Duration = Duration::from_secs(3);
/// How long a salvage batch or a hand-off is given to take effect (the
/// items leaving the pack) before it counts as refused.
const SALVAGE_TIMEOUT: Duration = Duration::from_secs(5);
/// A batch or an item refused this many times is left alone.
const SALVAGE_TRIES: u8 = 3;
/// The salvager is walked to when within this; further off, the salvage
/// waits for the team to come together.
const HAND_OFF_RANGE: f32 = 30.0;
/// Close enough to hand something over (ACE's use radius, with room).
const GIVE_REACH: f32 = 2.0;
/// How long an item that arrived in the pack waits for its appraisal
/// before the rules judge it as it is.
const TAG_TIMEOUT: Duration = Duration::from_secs(15);
/// The Salvaging skill.
const SALVAGING: u32 = 40;
/// How often the buffs are gone through to see what is due.
const BUFF_CHECK_EVERY: Duration = Duration::from_millis(1000);
/// Least time between two attack orders.
const ATTACK_EVERY: Duration = Duration::from_millis(1200);
/// How long to wait on a cast the server never answers for. Casting is
/// paced by its answer, not by a clock; this only stops a character
/// waiting for ever on one that went astray.
const CAST_LOST: Duration = Duration::from_secs(6);

/// Staying alive.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Survive {
    /// Heal when health falls below this fraction of its maximum.
    pub heal_below: f32,
    /// Stop fighting below this fraction (0 to keep fighting).
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
    /// After a death, go back for the corpse and take the gear off it
    /// (see `crate::recovery`).
    pub recover_corpse: bool,
    /// Give up on the corpse when it has not been reached in this many
    /// minutes.
    pub corpse_minutes: f32,
    /// While the vitae penalty is at least `vitae_above`, leave the
    /// hard fights and whatever killed us alone.
    pub vitae_wait: bool,
    /// The vitae penalty, as a fraction, from which the fights are
    /// picked with care: 0.25 is five deaths' worth.
    pub vitae_above: f32,
    /// Step out of the way of spells flying at us instead of standing
    /// in them (see `crate::dodge`).
    pub dodge: bool,
}

impl Default for Survive {
    fn default() -> Self {
        Survive {
            heal_below: 0.6,
            use_kits: true,
            heal_spell: String::new(),
            manage_mana: true,
            mana_below: 0.4,
            stamina_below: 0.3,
            recover_corpse: true,
            corpse_minutes: 10.0,
            vitae_wait: true,
            vitae_above: 0.25,
            dodge: true,
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

/// What to do with an item a loot rule matches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LootAction {
    /// Take it and keep it.
    #[default]
    Keep,
    /// Take it and salvage it (or carry it to whoever salvages).
    Salvage,
    /// Take it to sell on the next run to town.
    Sell,
    /// Leave it on the corpse.
    Skip,
}

impl LootAction {
    pub const ALL: [LootAction; 4] = [
        LootAction::Keep,
        LootAction::Salvage,
        LootAction::Sell,
        LootAction::Skip,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LootAction::Keep => "keep",
            LootAction::Salvage => "salvage",
            LootAction::Sell => "sell",
            LootAction::Skip => "skip",
        }
    }

    /// The action a word names ("keep", "salvage", "sell", "skip").
    pub fn parse(word: &str) -> Option<LootAction> {
        let w = word.trim().to_lowercase();
        LootAction::ALL.into_iter().find(|a| a.label() == w)
    }

    /// Whether an item the rule matches is picked up at all.
    pub fn takes(self) -> bool {
        self != LootAction::Skip
    }
}

/// One loot rule: a search in the inventory's language and what to do
/// with an item it matches. Rules are tried in order and the first
/// match decides.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LootRule {
    pub query: String,
    pub action: LootAction,
}

impl LootRule {
    pub fn new(query: impl Into<String>, action: LootAction) -> Self {
        LootRule {
            query: query.into(),
            action,
        }
    }
}

/// What loot is worth taking, and what to do with it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Loot {
    pub enabled: bool,
    /// The rules, first match wins (see [`loot_action`]). Missing from
    /// an old config, whose `filters` become keep rules.
    #[serde(default)]
    pub rules: Vec<LootRule>,
    /// The old flat list: searches whose matches were taken. Read for
    /// compatibility, folded into `rules` by [`Loot::rules`] and
    /// [`Loot::migrate`], never written again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<String>,
    /// Always take these, whatever the rules say (by name).
    pub always: Vec<String>,
    /// Never take these (by name), even when a rule matches.
    pub never: Vec<String>,
    /// Ask the server about the corpse's items before deciding, so that
    /// rules on damage, armour and spells can be judged.
    pub appraise: bool,
    /// Salvage what the rules tagged, when this character is the team's
    /// best salvager.
    pub salvage: bool,
    /// Carry what the rules tagged to the team's best salvager, when
    /// that is someone else.
    pub hand_off: bool,
    /// Pour loose stacks of the same thing together, so that slots are
    /// not wasted on the change left by buying and looting.
    #[serde(default = "yes")]
    pub tidy_pack: bool,
    /// The loot profile this character reads, by name (see
    /// `crate::profile`). Empty falls back to `rules` below, which is
    /// what a character had before profiles existed.
    ///
    /// Defaulted by name rather than by `Default::default`, because a
    /// settings file that mentions `loot` at all and leaves this out
    /// would otherwise get an empty string -- serde fills a missing
    /// field from its type, not from the struct's own default -- and a
    /// character would quietly stop reading its profile.
    #[serde(default = "starter")]
    pub profile: String,
    /// The profile that decides what goes to a vendor. Empty falls back
    /// to the older `growth.sell` searches.
    #[serde(default)]
    pub vendor_profile: String,
    /// Finish what you kill: while a body the character made is still
    /// unlooted and within reach, another fight waits.
    ///
    /// Off, a body only outranks the next fight once it has aged
    /// enough to be in danger of rotting, which in a busy place means
    /// the floor fills up and the oldest are lost. On, nothing is left
    /// behind at all -- slower, and everything gets picked up.
    #[serde(default = "yes")]
    pub after_every_fight: bool,
    /// How laden a character is willing to get while hunting, in
    /// multiples of its carrying capacity (150 x Strength).
    ///
    /// The game's own landmarks: at one capacity a character is
    /// comfortable, at two it is slowed, and at three the server stops
    /// letting it pick anything up. Stopping only at that last one is
    /// how a level twelve character came to be carrying twenty four
    /// thousand of a twenty four thousand three hundred ceiling, with
    /// seventeen units to spare -- unable to loot, barely able to
    /// move, and unable to put two stacks together, because the server
    /// weighs a merge as though the source were being lifted afresh.
    ///
    /// So the rules stop well before the wall and go and sell instead.
    #[serde(default = "carry_up_to")]
    pub carry_up_to: f32,
}

/// Serde's default for a switch that is on unless it was turned off.
fn yes() -> bool {
    true
}

/// Laden, but a long way from the wall: half again the comfortable
/// load, which leaves room under the server's ceiling for any stack a
/// character is likely to be carrying to still be poured into another.
fn carry_up_to() -> f32 {
    1.5
}

/// The profile the shelf seeds itself with, which is what a character
/// reads when nobody has said otherwise.
fn starter() -> String {
    "Starter".to_string()
}

impl Default for Loot {
    fn default() -> Self {
        Loot {
            enabled: true,
            rules: vec![LootRule::new("value>250", LootAction::Keep)],
            filters: Vec::new(),
            always: vec!["Pyreal".into()],
            never: Vec::new(),
            appraise: true,
            salvage: true,
            hand_off: true,
            tidy_pack: true,
            // The profile the shelf seeds itself with, so a character
            // nobody has configured still reads its rules from data
            // rather than from a list in the code.
            profile: "Starter".into(),
            vendor_profile: String::new(),
            after_every_fight: true,
            carry_up_to: carry_up_to(),
        }
    }
}

impl Loot {
    /// The rules in force: `rules`, then any old `filters` as keep rules
    /// (blank and repeated ones dropped).
    pub fn rules(&self) -> Vec<LootRule> {
        let mut out = self.rules.clone();
        for f in &self.filters {
            let f = f.trim();
            if f.is_empty() || out.iter().any(|r| r.query.trim() == f) {
                continue;
            }
            out.push(LootRule::new(f, LootAction::Keep));
        }
        out
    }

    /// Fold the old `filters` into `rules` for good.
    pub fn migrate(&mut self) {
        if !self.filters.is_empty() {
            self.rules = self.rules();
            self.filters.clear();
        }
    }

    /// Whether any rule needs the items appraised first.
    pub fn needs_appraisal(&self) -> bool {
        self.rules()
            .iter()
            .any(|r| Query::parse(&r.query).needs_appraisal())
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
    /// A follower fights only what stands within this of its leader,
    /// metres: further off, a monster would draw it away.
    pub fight_radius: f32,
    /// When the party stops hunting to restock, how it makes the trip,
    /// and what it does about money. How much of each thing to carry
    /// lives in `growth::Growth`.
    pub restock: crate::logistics::Restock,
}

/// A leader further off than twice the following distance (and at least
/// this) is followed before anything else, a fight included; nearer,
/// the fight comes first. Following is the follower's job.
const FOLLOW_BREAK: f32 = 10.0;

/// How often one character hands something to another. The server
/// takes one give at a time and answers in its own time.
const GIVE_EVERY: Duration = Duration::from_millis(700);

/// How often a stack is poured into another. The server takes one merge
/// at a time and answers in its own time.
pub(crate) const MERGE_EVERY: Duration = Duration::from_millis(600);
/// How near a body has to be to count as one the character made and
/// should finish with before starting another fight.
const CORPSE_IS_MINE: f32 = 25.0;

/// How often one item is taken from a corpse. The server moves one at a
/// time and refuses the rest as "you're too busy", so they go one by
/// one rather than all at once.
const TAKE_EVERY: Duration = Duration::from_millis(400);
/// A wait that has doubled this far means the item has been asked for
/// several times and has not moved -- three asks, starting a quarter
/// of a second apart and doubling. It is not coming, so the queue goes
/// on without it rather than standing behind it.
const NOT_COMING: Duration = Duration::from_millis(900);

/// How long a monster's corpse lasts before it rots away. ACE gives an
/// unlooted corpse no timer at all until its first heartbeat, when it
/// takes the default of five minutes and counts down from there
/// (`WorldObject_Decay`), so this is the whole window there is.
pub(crate) const CORPSE_LIFE: Duration = Duration::from_secs(300);

/// How close to rotting a corpse has to be before it is worth breaking
/// off for. Inside this there is no second chance.
pub(crate) const CORPSE_URGENT: Duration = Duration::from_secs(75);

/// How close the character has to stand before a corpse will open.
///
/// The server will not hand over a container we are not standing at: a
/// use from across the room is answered by being told to walk there,
/// and it waits for us to arrive. A client that never walks waits for
/// ever, which is what every corpse that "did not open" turned out to
/// be. Two and a half metres is inside the use radius of everything
/// that leaves a body.
const CORPSE_REACH: f32 = 2.5;

/// How far from its leader a follower keeping `keep` metres may stray
/// before following comes before everything else.
pub fn follow_break(keep: f32) -> f32 {
    (2.0 * keep).max(FOLLOW_BREAK)
}
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
            fellowship_name: "acswarm".into(),
            debuffs: Vec::new(),
            share_supplies: true,
            keep_stocked: vec![("Healing Kit".into(), 1)],
            hard_fight_health: 400,
            wait_for_debuff: false,
            lead: false,
            follow: true,
            follow_distance: 4.0,
            fight_radius: 25.0,
            restock: crate::logistics::Restock::default(),
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
    /// Level and experience, total and unspent, as the sheet has them
    /// (0 before it arrives). The fleet view works XP an hour out of
    /// the total over time.
    pub level: i32,
    pub total_xp: i64,
    pub available_xp: i64,
    /// Stamina and mana as fractions of their maximum, like `health`.
    pub stamina: f32,
    pub mana: f32,
    /// Its rules are on: it plays on its own.
    pub autoplay: bool,
    /// It follows the leader about (`Team::follow`, and not leading).
    pub following: bool,
    /// Its Salvaging as it stands, buffs counted, and whether it carries
    /// an Ust: what decides who salvages for the team.
    pub salvaging: u32,
    pub has_ust: bool,
    /// How close to empty it is, what it still has to buy and what that
    /// will cost: what the party decides hunting and restocking from.
    pub supplies: crate::logistics::Supplies,
    /// The hunting ground it is on or heading for: landblock, where,
    /// and what it is called. What the party goes back to together
    /// after a trip to town.
    pub ground: Option<(u32, glam::Vec2, String)>,
}

/// The larger of a health boost and a stamina transfer, as
/// `(spell, points restored)`. Either may be missing; a tie goes to the
/// boost, which does not spend a bar the character may need to run.
fn bigger_heal(boost: Option<(u32, u32)>, transfer: Option<(u32, u32)>) -> Option<(u32, u32)> {
    match (boost, transfer) {
        (Some(b), Some(t)) => Some(if t.1 > b.1 { t } else { b }),
        (b, t) => b.or(t),
    }
}

/// Who salvages for the team, out of `mates` (the caller includes
/// itself): the highest Salvaging among those with an Ust, ties to the
/// name that sorts first. `None` when nobody carries an Ust.
pub fn best_salvager<'a>(mates: impl Iterator<Item = &'a Mate>) -> Option<(String, u32)> {
    mates
        .filter(|m| m.has_ust && m.guid != 0)
        .max_by(|a, b| {
            a.salvaging
                .cmp(&b.salvaging)
                .then_with(|| b.name.cmp(&a.name))
        })
        .map(|m| (m.name.clone(), m.guid))
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
    pub fn wanting(&self, me: glam::Vec3, radius: f32) -> Option<&Mate> {
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
    /// Growing and keeping supplied over the hours (see `crate::growth`).
    pub growth: crate::growth::Growth,
    /// Getting a new character through the Training Academy (see
    /// `crate::academy`).
    pub academy: crate::academy::Academy,
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
/// The ammunition to make for a launcher that takes `fits` (see
/// `ac_world::fletching::ammo_type`), from what is carried as `(wcid,
/// guid)` pairs, within a Fletching of `fletching`: `(recipe, heads,
/// shafts)`. The element `weakest` (the target's weakest, when known)
/// comes first, then whatever is hardest to make, which is the better
/// arrow. `None` when no pair of bundles carried makes anything the
/// launcher shoots.
pub fn choose_recipe(
    fits: u32,
    fletching: u32,
    carried: &[(u32, u32)],
    weakest: Option<ac_world::elements::Element>,
) -> Option<(&'static ac_world::fletching::Recipe, u32, u32)> {
    let held = |wcid: u32| carried.iter().find(|(w, _)| *w == wcid).map(|(_, g)| *g);
    ac_world::fletching::making(fits)
        .filter(|r| r.difficulty <= fletching)
        .filter_map(|r| Some((r, held(r.source)?, held(r.target)?)))
        .max_by_key(|(r, _, _)| {
            let hits = weakest.is_some_and(|w| r.element() == Some(w));
            (hits, r.difficulty)
        })
}

pub fn wanted_loot(stats: &crate::items::ItemStats, l: &Loot) -> bool {
    loot_action(stats, l).takes()
}

/// What the rules say to do with an item: `never` names are skipped,
/// `always` names kept, then the first rule whose search matches
/// decides; nothing matching is skipped. A blank rule matches nothing.
pub fn loot_action(stats: &crate::items::ItemStats, l: &Loot) -> LootAction {
    if name_matches(&stats.name, &l.never) {
        return LootAction::Skip;
    }
    if name_matches(&stats.name, &l.always) {
        return LootAction::Keep;
    }
    l.rules()
        .iter()
        .find(|r| {
            let q = Query::parse(&r.query);
            !q.is_empty() && stats.matches(&q)
        })
        .map(|r| r.action)
        .unwrap_or(LootAction::Skip)
}

/// What the loot rules make of an item, and whether they can say yet.
///
/// The profile named in the config decides when there is one; a
/// character with no profile falls back to the searches it had before
/// profiles existed, which never ask for an appraisal they have not
/// already been given.
pub fn judge_loot(
    stats: &crate::items::ItemStats,
    id: Option<&ac_net::messages::Appraisal>,
    l: &Loot,
    library: &crate::profile::Library,
    me: &crate::weapons::Wielder,
    my_name: &str,
    held: u32,
) -> crate::profile::Verdict {
    use crate::profile::Verdict;
    // The player's own word comes first, whatever any rule says.
    if name_matches(&stats.name, &l.never) {
        return Verdict::Decided(LootAction::Skip, "never take these".into());
    }
    if name_matches(&stats.name, &l.always) {
        return Verdict::Decided(LootAction::Keep, "always take these".into());
    }
    match library.get(&l.profile) {
        Some(p) => p.judge(stats, id, me, my_name, held),
        None => Verdict::Decided(loot_action(stats, l), "the loot rules".into()),
    }
}

/// Whether an item that turned up in the pack (given, bought, made)
/// should be tagged, and with what: the rules' salvage and sell
/// verdicts, since keep and skip mean nothing for something already
/// carried. A salvage bag is only tagged when a rule names it.
pub fn arrival_tag(stats: &crate::items::ItemStats, l: &Loot) -> Option<LootAction> {
    match loot_action(stats, l) {
        a @ (LootAction::Salvage | LootAction::Sell) => Some(a),
        _ => None,
    }
}

/// What the character is doing on its own right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Doing {
    #[default]
    Idle,
    Healing,
    Fighting,
    Looting,
    /// Salvaging, or carrying salvage to whoever does.
    Salvaging,
    Buffing,
    Debuffing,
    Helping,
    Following,
    /// Dead, or coming back from it (see `crate::recovery`).
    Recovering,
    /// Spending experience (see `crate::growth`).
    Growing,
    /// On the way to a hunting ground.
    Traveling,
    /// On a run to town.
    Shopping,
    /// Stepping out of a spell's way (see `crate::dodge`).
    Dodging,
    /// Doing the Training Academy tutorial (see `crate::academy`).
    Training,
    /// Pouring loose stacks together (see `crate::pack`).
    Tidying,
}

impl Doing {
    pub fn label(self) -> &'static str {
        match self {
            Doing::Tidying => "tidying the pack",
            Doing::Idle => "waiting",
            Doing::Healing => "healing",
            Doing::Fighting => "fighting",
            Doing::Looting => "looting",
            Doing::Salvaging => "salvaging",
            Doing::Buffing => "buffing",
            Doing::Debuffing => "debuffing",
            Doing::Helping => "helping the team",
            Doing::Following => "following the leader",
            Doing::Recovering => "recovering from death",
            Doing::Growing => "spending experience",
            Doing::Traveling => "travelling",
            Doing::Shopping => "in town",
            Doing::Dodging => "dodging",
            Doing::Training => "in the Training Academy",
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
    /// `Event::Autoplay`s not yet handed out: one per change of `doing`
    /// or `status`, taken by `Client::drain_events`.
    pub announced: Vec<crate::Event>,
    last_heal: Option<Instant>,
    last_attack: Option<Instant>,
    /// When a spell of any kind last went out (attacks, debuffs and
    /// buffs alike, since the server takes them one at a time), and
    /// what the attack spells are being thrown at: a spell keeps no
    /// `attack_target` of its own the way a swing does, so the engine
    /// remembers what it is working on.
    last_cast: Option<Instant>,
    /// When a cast was sent and the server has not yet said it is done.
    /// Cleared by its answer (`UseDone`), which is what paces the next
    /// one.
    pub(crate) cast_sent: Option<Instant>,
    casting_at: Option<u32>,
    /// The target the weapon in hand was chosen for, so it is chosen
    /// once a fight and not once a frame.
    pub(crate) armed_for: Option<u32>,
    /// Coming back from a death (see `crate::recovery`).
    pub recovery: crate::recovery::Recovery,
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
    /// Notes said lately and when, so that two alternating notes are
    /// each said once per `NOTE_EVERY` rather than every frame.
    noted: Vec<(String, Instant)>,
    /// When the buffs were last gone through. The urgent pass runs
    /// every tick, and working out what is due walks the whole
    /// spellbook, so it is only done once a second.
    buffs_checked: Option<Instant>,
    /// The weapon put down to cast an urgent buff mid-fight, to be taken
    /// up again the moment the buffing is done.
    put_down: Option<u32>,
    /// The ammunition chosen for the target, to be wielded with the bow.
    wanted_ammo: Option<u32>,
    /// The shield to put on once a one-handed weapon is in hand.
    wanted_shield: Option<u32>,
    /// When the hands were last asked to change weapon.
    last_rewield: Option<Instant>,
    /// A weapon to wield as soon as the hands are empty.
    pending_wield: Option<u32>,
    /// A journey put down for a fight, to be picked up again after it.
    pub(crate) resume_trip: Option<glam::Vec2>,
    /// The target being worked on, since when, and its health when
    /// last seen to drop: a target that takes no damage for a while is
    /// out of reach, and is let go.
    engaged: Option<(u32, Instant, f32)>,
    /// Targets let go, and when, so they are left alone for a while.
    given_up: Vec<(u32, Instant)>,
    /// When ammunition was last made.
    last_craft: Option<Instant>,
    /// Bundles waiting to be used on each other once the character has
    /// dropped to peace mode: `(heads, shafts, when peace was asked)`.
    crafting: Option<(u32, u32, Instant)>,
    /// When stamina was last poured into mana or Revitalize cast.
    last_vital: Option<Instant>,
    /// The corpse being looted and when we started.
    /// The corpse being opened: which, since when, how long to allow
    /// (the server walks us to it, so a far one is slower), and how
    /// many times we have asked.
    corpse: Option<(u32, Instant, Duration, u32)>,
    /// Which step claimed this tick, for the log and the panel.
    pub(crate) step: Option<&'static str>,
    /// The last "stood aside" said, so the same one is not said twice.
    aside_said: Option<String>,
    /// Corpses already emptied.
    pub(crate) looted: Vec<u32>,
    /// Corpses that would not open. A corpse is locked to the group
    /// that killed it until it has rotted a while, so this is a
    /// "later", not a "never", and the wait grows if it keeps saying no.
    pub(crate) shelved: crate::did::Patience<u32>,
    /// Weenie classes the server has refused to hand over because they
    /// can only be had so often. See `Client::loot_refused`.
    pub(crate) refused_kinds: crate::did::Patience<u32>,
    /// The corpse being walked to, if the looting set the walk going.
    /// A follower is walking after its leader with the same machinery,
    /// and that walk is not ours to cancel.
    pub(crate) walking_to: Option<u32>,
    /// Corpse items we asked the server about.
    appraising: bool,
    /// What the rules said about each carried item taken as loot (or
    /// found in the pack afterwards), by guid: the salvage pass, the UI
    /// and scripts read it.
    tags: std::collections::BTreeMap<u32, LootAction>,
    /// Carried items already looked at by the salvage pass, so an item
    /// is judged once, when it arrives. Empty until the first pass,
    /// which takes what is carried then as the baseline (nothing owned
    /// before the rules ran is salvaged behind the player's back).
    seen: std::collections::BTreeSet<u32>,
    baselined: bool,
    /// Arrivals waiting for their appraisal before the rules judge
    /// them, and since when.
    pending_tags: Vec<(u32, Instant)>,
    /// The salvage batch sent, and when; refused batches and hand-offs
    /// are counted per item so a stubborn one is given up on.
    salvaging: Option<(Vec<u32>, Instant)>,
    handing: Option<(u32, Instant)>,
    refused: std::collections::BTreeMap<u32, u8>,
    last_salvage: Option<Instant>,
    /// The other characters being played, as the host last saw them.
    pub team: TeamView,
    /// Targets this character has landed its debuffs on.
    pub debuffed: Vec<u32>,
    /// What this character is short of, for the others to hand over.
    pub wants: Vec<String>,
    last_debuff: Option<Instant>,
    last_give: Option<Instant>,
    /// Hand-overs that have not gone through, by what was being handed
    /// over. Counting the money out into its own stack takes a moment
    /// and the server answers in its own time; this is what stops the
    /// asking from running away.
    give_tries: crate::did::Patience<u32>,
    /// The spot being walked to, how close it has been got to, and when
    /// that last improved. See `Client::reaching_too_long`.
    reaching: Option<(glam::Vec3, f32, Instant)>,
    pub(crate) last_merge: Option<Instant>,
    /// Items decided on but not yet taken from the open corpse, and
    /// when the last one was asked for. The server takes one at a time.
    take_queue: Vec<u32>,
    last_take: Option<Instant>,
    /// Items asked for and not moved. The server can refuse -- a full
    /// pack, a chest that will not give the thing up -- and it refuses
    /// in chat, not in a reply we can wait on, so the only way to hear
    /// "no" is to notice the item has not moved.
    take_tries: crate::did::Patience<u32>,
    /// When each corpse was first seen, so the ones about to rot can be
    /// emptied first. A corpse we never saw appear is taken as fresh.
    pub(crate) corpse_seen: Vec<(u32, Instant)>,
    last_recruit: Option<Instant>,
    /// Where the journey after a far-off leader was bound, to plan
    /// again once it has moved on.
    follow_trip: Option<glam::Vec2>,
    /// No journey after the leader is planned before this: planning
    /// costs a search, and one that found no way is not tried again for
    /// a while.
    next_follow_plan: Option<Instant>,
    /// The growth rules' own state (see `crate::growth`).
    pub growth: crate::growth::State,
    /// The academy rule's own state (see `crate::academy`).
    pub academy: crate::academy::State,
    /// The corpse the academy rule is emptying, and since when.
    pub(crate) academy_corpse: Option<(u32, Instant)>,
    /// Doors the academy rule opened lately, and when.
    pub(crate) academy_doors: Vec<(u32, Instant)>,
    /// When the academy rule last asked for a weapon to be wielded.
    pub(crate) academy_armed: Option<Instant>,
}

impl Autoplay {
    /// The creature spells are being thrown at, if any: the magic
    /// fighter's counterpart to `Client::attack_target`.
    pub fn casting_at(&self) -> Option<u32> {
        self.casting_at
    }

    /// What is to be done with an item: its tag if it was tagged when
    /// taken (or when it arrived), else what the rules say of it now.
    pub fn loot_action(&self, stats: &crate::items::ItemStats) -> LootAction {
        self.tags
            .get(&stats.guid)
            .copied()
            .unwrap_or_else(|| loot_action(stats, &self.config.loot))
    }

    /// Tag an item by hand (a script, the inventory panel): what the
    /// salvage pass does with it from now on.
    pub fn tag(&mut self, guid: u32, action: LootAction) {
        self.tags.insert(guid, action);
        self.seen.insert(guid);
    }

    /// Every tag, by guid.
    pub fn tags(&self) -> &std::collections::BTreeMap<u32, LootAction> {
        &self.tags
    }

    /// Let go of whatever is being fought: the spells' target and the
    /// engagement (the fleet view's "regroup" and "stop"; the caller
    /// clears `Client::attack_target` itself).
    pub fn drop_target(&mut self) {
        self.casting_at = None;
        self.engaged = None;
    }

    /// A spell of any kind went out less than a cast ago, so another
    /// sent now would queue behind it or be dropped.
    pub(crate) fn cast_in_flight(&self, now: Instant) -> bool {
        // The server says when a cast is finished, so that is what is
        // waited on -- not a guess at how long spells take. A heal sent
        // the moment the last one lands is the difference between
        // living and dying, and no fixed interval can be both quick
        // enough for that and slow enough never to have the next spell
        // dropped for arriving early.
        //
        // The clock that remains is a backstop, not the pacing: if the
        // server never answers at all, the character must not wait for
        // ever.
        match self.cast_sent {
            Some(t) => now.duration_since(t) < CAST_LOST,
            None => false,
        }
    }

    /// Whether an urgent buff should stand aside for the attack this
    /// moment. In a fight with spells, buffs and attacks take turns:
    /// after a buff goes out, the next cast slot belongs to the attack,
    /// and the buffs resume once it has been used, or has gone unused
    /// for a whole slot (nothing castable, say). Without this a long
    /// list of buffs coming due at once had the character "fighting"
    /// something for twenty seconds without throwing a thing at it.
    /// Outside a spell fight there is no attack cast to wait for.
    fn buff_yields_to_attack(&self, now: Instant) -> bool {
        if self.casting_at.is_none() {
            return false;
        }
        if self.cast_in_flight(now) {
            return true;
        }
        let buff_was_last = match (self.last_buff, self.last_cast) {
            (Some(b), Some(c)) => b >= c,
            _ => false,
        };
        buff_was_last
            && self
                .last_buff
                .is_some_and(|t| now.duration_since(t) < CAST_LOST)
    }

    /// Something worth knowing that is not what the character is doing:
    /// logged, at most every few seconds for the same words, and the
    /// status left as it was. Said every tick it would drown the log
    /// and flip the status back and forth with whatever else is going on.
    pub(crate) fn note(&mut self, text: impl Into<String>, now: Instant) {
        let text = text.into();
        self.noted
            .retain(|(_, when)| now.duration_since(*when) < NOTE_EVERY);
        if !self.noted.iter().any(|(t, _)| *t == text) {
            tracing::info!("autoplay: {text}");
            self.noted.push((text, now));
        }
    }

    pub(crate) fn say(&mut self, doing: Doing, status: impl Into<String>) {
        let status = status.into();
        if self.doing != doing || self.status != status {
            tracing::info!("autoplay: {status}");
            self.announced.push(crate::Event::Autoplay {
                doing: format!("{doing:?}").to_lowercase(),
                text: status.clone(),
            });
        }
        self.doing = doing;
        self.status = status;
    }
}

impl Client {
    /// Health as a fraction of its maximum, 1.0 when unknown.
    pub fn health_fraction(&self) -> f32 {
        let stats = &self.world.stats;
        let max = stats.vital_max_current(0);
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

    /// The biggest heal the character can land right now, and how much
    /// health it would restore.
    ///
    /// A mage has two ways out of an emergency and they are not the
    /// same size. Heal Self restores a fixed number of points however
    /// hurt it is; Stamina to Health takes half the stamina bar, which
    /// on a character with a full bar is far more, and is why a caster
    /// in trouble reaches for it rather than a kit. Which is larger
    /// depends on the moment, so both are worked out and the larger
    /// wins. A transfer from a bar that is nearly empty scores near
    /// nothing and loses on its own merits, so no floor is needed.
    fn best_emergency_heal(&self) -> Option<(u32, u32)> {
        use ac_world::vitals::vital;
        let table = self.assets.spell_table().ok()?;
        let usable = |spell: u32| {
            self.world.stats.spells.contains(&spell)
                && table.get(spell).is_some_and(|s| s.is_self_targeted())
                && matches!(self.can_cast(spell), crate::magic::CastCheck::Ok)
        };
        let boost = ac_world::vitals::boosts_of(vital::HEALTH)
            .into_iter()
            .filter(|b| usable(b.spell))
            .map(|b| (b.spell, ((b.low + b.high) / 2).max(0) as u32))
            .max_by_key(|(_, gain)| *gain);
        let stamina = self.world.stats.vitals[1].current;
        let transfer = ac_world::vitals::transfers_between(vital::STAMINA, vital::HEALTH)
            .into_iter()
            .filter(|t| usable(t.spell))
            .map(|t| (t.spell, t.gain(stamina)))
            .max_by_key(|(_, gain)| *gain);
        bigger_heal(boost, transfer)
    }

    /// Keep mana and stamina up the way a caster does: stamina poured
    /// into mana when mana runs low, Revitalize when stamina does. True
    /// when a spell went out.
    pub(crate) fn autoplay_vitals(&mut self, now: Instant) -> bool {
        use ac_world::vitals::vital;
        let cfg = self.autoplay.config.survive.clone();
        if !cfg.manage_mana {
            return false;
        }
        // Same again: the next draught or cast waits on the server
        // answering for the last, not on a clock.
        if self.autoplay.cast_in_flight(now) {
            return false;
        }
        let frac = |i: usize| {
            let max = self.world.stats.vital_max_current(i).max(1) as f32;
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
        // A spell on its way to us is stepped out of before anything
        // else, healing included (see `crate::dodge`).
        if self.autoplay_dodge(now) {
            return;
        }
        if self.autoplay_survive(now) {
            return;
        }
        // Dead, or on the way back from it: nothing else until the
        // corpse is dealt with (see `crate::recovery`).
        if self.autoplay_recover(now) {
            return;
        }
        // A new character in the Training Academy does the tutorial
        // before anything else (see `crate::academy`).
        if self.autoplay_academy(now) {
            return;
        }
        // Everything from here is a table rather than a chain, so the
        // order can be read, logged and tested rather than only obeyed
        // (see `crate::steps` and `docs/agent.md`).
        for chore in crate::steps::HOUSEKEEPING {
            chore.run(self, now);
        }
        // The reflexes run in their order, always. Nothing is weighed
        // against a spell already in the air.
        for step in crate::steps::reflexes() {
            let did = step.run(self, now);
            if did.acting() {
                self.autoplay.step = Some(step.name);
                return;
            }
            self.aside(step.name, &did, now);
        }
        // The goals are weighed. Most say nothing and take their place
        // in the table, which is the order they had; the ones with an
        // opinion can say that a corpse about to rot is worth more than
        // the next fight (see `crate::steps`).
        let mut goals = crate::steps::weigh(self, now);
        while let Some((step, _)) = goals.next(self, now) {
            let did = step.run(self, now);
            if did.acting() {
                self.autoplay.step = Some(step.name);
                return;
            }
            self.aside(step.name, &did, now);
        }
        self.autoplay.step = None;
        let doing = self.autoplay.doing;
        if doing != Doing::Idle {
            self.autoplay.say(Doing::Idle, "waiting");
        }
    }

    /// Every stack in the packs, as the compactor sees them.
    ///
    /// What the character is holding is here too: a quiver of arrows
    /// can be topped up from the pack, and is worth topping up, but is
    /// never the stack poured away.
    pub fn pack_stacks(&self) -> Vec<crate::pack::Stack> {
        let me = self.world.player_guid;
        let describe = |o: &ac_world::WorldObject, wielded: bool| crate::pack::Stack {
            guid: o.guid,
            wcid: o.weenie_class_id,
            name: o.name.clone(),
            count: o.stack_size.max(1),
            max: o.max_stack_size,
            wielded,
            burden: o.burden,
        };
        self.world
            .inventory()
            .map(|o| describe(o, false))
            .chain(
                self.world
                    .objects
                    .values()
                    .filter(|o| me.is_some() && o.wielder == me)
                    .map(|o| describe(o, true)),
            )
            .collect()
    }

    /// Pour loose stacks together. True when a merge went out.
    ///
    /// Slots are the scarce thing, not weight, and nothing warns a
    /// player that a purchase landed beside a pile of the same. This
    /// runs before the rules that decide the pack is full, so that a
    /// pack full of change does not send the character to town.
    pub(crate) fn autoplay_tidy(&mut self, now: Instant) -> bool {
        if !self.autoplay.config.loot.tidy_pack {
            return false;
        }
        // Not with a counter open. A run to town holds a list of what
        // it has sent the vendor, by guid, and a merge makes one of
        // those guids vanish mid-sale. Whatever was bought is tidied
        // the moment the window closes, which is soon enough.
        if self.world.open_vendor.is_some() {
            return false;
        }
        // Nor in the middle of loading or unloading the quartermaster.
        // Money counted out for the runner is a stack of its own, and
        // tidying poured it straight back into the pile it came from --
        // count out, merge back, count out again, a hundred and twenty
        // six times in one watched run.
        if matches!(
            self.autoplay.growth.mode.stage(),
            Some(crate::logistics::Stage::HandOver | crate::logistics::Stage::HandOut)
        ) {
            return false;
        }
        if self
            .autoplay
            .last_merge
            .is_some_and(|t| now.duration_since(t) < MERGE_EVERY)
        {
            return false;
        }
        let Some(m) = crate::pack::next_merge(&self.pack_stacks()) else {
            return false;
        };
        if !self.merge_stacks(m.from, m.to, Some(m.amount)) {
            // The server would refuse it; do not ask again at once.
            self.autoplay.last_merge = Some(now);
            return false;
        }
        self.autoplay.last_merge = Some(now);
        self.autoplay.say(
            Doing::Tidying,
            format!("putting {} {} with the rest", m.amount, m.name),
        );
        true
    }

    /// Heal, and break off a losing fight. True when it acted.
    pub(crate) fn autoplay_survive(&mut self, now: Instant) -> bool {
        let cfg = self.autoplay.config.survive.clone();
        let health = self.health_fraction();
        if health >= cfg.heal_below || health <= 0.0 {
            return false;
        }
        // Paced by the server, not by a clock: the next heal goes out
        // as soon as the last one is answered for. A fixed interval is
        // either too slow to save a character or quick enough to have
        // the spell dropped for arriving over the last.
        if self.autoplay.cast_in_flight(now) {
            // Waiting for the next heal is not a reason to stand
            // still. Everything below this in the list -- looting,
            // walking, tidying -- carries on; it is only the fighting
            // that must not go first, and the fight rule sees to that
            // itself (`too_hurt_to_fight`). Holding the tick here
            // instead left the character idle between heals.
            return false;
        }
        // A kit is quicker and cheaper than a spell -- but only to
        // someone who has trained Healing. Untrained it restores next to
        // nothing, so a caster is far better off with Heal Self, and
        // reaching for a kit only wastes the moment it takes.
        if cfg.use_kits && self.heals_with_kits() {
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
            self.best_emergency_heal().map(|(spell, _)| spell)
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

    /// Whether a healing kit is worth using: Healing has to be trained
    /// or specialised for one to restore anything much. A character
    /// without it heals with a spell instead, and does not want kits
    /// bought for it either (see `grow_needs`).
    pub fn heals_with_kits(&self) -> bool {
        use ac_world::stats::{sac, skill};
        self.world
            .stats
            .skill(skill::HEALING)
            .is_some_and(|s| s.advancement >= sac::TRAINED)
    }

    /// How many of the same weenie are already carried, for the rules
    /// that stop at a number.
    fn already_carried(&self, wcid: u32) -> u32 {
        self.world
            .inventory()
            .filter(|o| o.weenie_class_id == wcid)
            .map(|o| o.stack_size.max(1))
            .sum()
    }

    /// What the rules say to do with an item, now.
    fn loot_verdict(&self, stats: &crate::items::ItemStats, cfg: &Loot) -> LootAction {
        use crate::profile::Verdict;
        let me = self.wielder();
        let name = self.world.stats.name.clone();
        let held = self.already_carried(stats.wcid);
        match judge_loot(
            stats,
            self.appraisals.get(&stats.guid),
            cfg,
            &self.profiles,
            &me,
            &name,
            held,
        ) {
            Verdict::Decided(action, _) => action,
            // Still cannot say, because the server was never asked (the
            // appraising is switched off, or it would not answer). A
            // rule that cannot be judged has not claimed anything.
            Verdict::NeedsId(_) | Verdict::None => LootAction::Skip,
        }
    }

    /// Which of a corpse's items have to be identified before the rules
    /// can say anything about them. Everything else is already decided.
    fn loot_to_look_over(&self, items: &[u32], cfg: &Loot, now: Instant) -> Vec<u32> {
        use crate::profile::Verdict;
        let me = self.wielder();
        let name = self.world.stats.name.clone();
        items
            .iter()
            .copied()
            .filter(|g| !self.appraisals.contains_key(g))
            .filter(|g| {
                let Some(stats) = self.stats_of(*g) else {
                    return false;
                };
                if self.refused_lately(stats.wcid, now) {
                    return false;
                }
                let held = self.already_carried(stats.wcid);
                matches!(
                    judge_loot(&stats, None, cfg, &self.profiles, &me, &name, held),
                    Verdict::NeedsId(_)
                )
            })
            .collect()
    }

    /// Say why a step stood aside, when it is worth saying.
    ///
    /// A character doing nothing is the hardest thing to account for
    /// from outside: it stands there and nobody can see what it is
    /// waiting on. `Waiting` is not worth a word -- it is the ordinary
    /// business of a tick -- but a step that is blocked or has given up
    /// is something the player wants to know, and once is enough: the
    /// reason is the same on every tick until it changes.
    fn aside(&mut self, step: &'static str, did: &crate::did::Did, now: Instant) {
        use crate::did::Did;
        let because = match did {
            Did::Blocked(b) | Did::Refused(b) => b,
            Did::Acting | Did::Done | Did::Waiting(_) => return,
        };
        let said = format!("{step}: {because}");
        if self.autoplay.aside_said.as_deref() == Some(said.as_str()) {
            return;
        }
        self.autoplay.aside_said = Some(said.clone());
        self.autoplay.note(said, now);
    }

    /// The server has refused something with `code`. When the reason is
    /// one that will not change today -- a thing that can only be had
    /// so many times a day -- the *kind* of thing is remembered, not
    /// the one on this corpse: the next corpse's copy would be refused
    /// for the same reason, and asking again is a round trip spent to
    /// be told no twice.
    pub(crate) fn loot_refused(&mut self, code: u32) {
        // YouHaveSolvedThisQuestTooRecently / TooManyTimes: what gates
        // the drops that can only be had so often.
        // YouHaveSolvedThisQuestTooRecently / TooManyTimes: what gates
        // the drops that can only be had so often. Both are waits --
        // the first will certainly lift, and a solve cap can be raised
        // -- so neither is a `Refused`, which would mean never.
        if !matches!(code, 0x043E | 0x043F) {
            return;
        }
        let Some(&guid) = self.autoplay.take_queue.first() else {
            return;
        };
        let Some(o) = self.world.objects.get(&guid) else {
            return;
        };
        let (wcid, name) = (o.weenie_class_id, o.name.clone());
        if wcid != 0 {
            let now = Instant::now();
            // The wait doubles with each refusal, so a cooldown of an
            // hour is picked up within the hour and a daily one costs a
            // handful of wasted asks a day -- nothing against missing
            // the thing for a day.
            self.autoplay.refused_kinds.note(
                wcid,
                &crate::did::Did::Blocked(crate::did::Because::server(code)),
                now,
            );
            tracing::info!("autoplay: {name} cannot be had yet; leaving its kind for a while");
        }
        self.autoplay.take_queue.retain(|g| *g != guid);
    }

    /// Whether this kind of thing is still inside the wait a refusal
    /// put on it.
    fn refused_lately(&self, wcid: u32, now: Instant) -> bool {
        self.autoplay.refused_kinds.held(&wcid, now)
    }

    /// Let go of a walk toward a corpse, wherever that corpse has been
    /// let go of. Nothing else is steering here, so leaving the walk
    /// running would carry the character off to a body it has already
    /// finished with.
    fn stop_walking_to_loot(&mut self) {
        if self.autoplay.walking_to.take().is_some() && self.follow.take().is_some() {
            self.steering.reset();
        }
    }

    /// A weapon waiting for empty hands is taken up as soon as they
    /// are: a bow cannot be drawn with a shield up, and a two-handed
    /// weapon needs both. Runs every tick and never claims one.
    pub(crate) fn autoplay_pending_wield(&mut self) {
        if let Some(g) = self.autoplay.pending_wield {
            // A shield in the off hand counts as a full hand for a
            // weapon that cannot be held with one.
            let offhand_matters = self
                .stats_of(g)
                .is_some_and(|i| crate::weapons::needs_free_offhand(&i));
            let hands_full = self.world.wielded().any(|o| {
                (o.item_type
                    & (ac_world::item_type::MELEE_WEAPON
                        | ac_world::item_type::MISSILE_WEAPON
                        | ac_world::item_type::CASTER)
                    != 0
                    && o.valid_locations & ac_world::equip::MISSILE_AMMO == 0)
                    || (offhand_matters && o.valid_locations & ac_world::equip::SHIELD != 0)
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
    }

    /// Open the corpse of something we killed and take what is worth
    /// taking. True while looting.
    pub(crate) fn autoplay_loot(&mut self, now: Instant) -> bool {
        if !self.autoplay.config.loot.enabled {
            return false;
        }
        // Already at one: wait for its contents, then empty it.
        if let Some((guid, since, allow, tries)) = self.autoplay.corpse {
            // The clock is on the opening, not on the emptying. Once
            // the corpse is open the character is taking from it an
            // item at a time, and asking the server to open it again
            // in the middle of that pulls the container out from under
            // the take in flight -- which is what "Source item not
            // found!" is.
            let opened = self
                .world
                .open_container
                .as_ref()
                .is_some_and(|(g, _)| *g == guid);
            if opened && now.duration_since(since) > LOOT_GIVE_UP {
                tracing::info!("autoplay: giving up on corpse {guid:#010x}; it will not empty");
                self.close_container();
                self.autoplay.looted.push(guid);
                self.autoplay.corpse = None;
                self.stop_walking_to_loot();
                self.autoplay.appraising = false;
                self.autoplay.take_queue.clear();
                self.autoplay.take_tries.clear();
                self.autoplay.last_take = None;
                return false;
            }
            if !opened && now.duration_since(since) > allow {
                self.autoplay.corpse = None;
                self.stop_walking_to_loot();
                self.autoplay.appraising = false;
                // Opening a corpse asks the server to walk us to it,
                // and indoors that walk goes round corners. Giving up
                // once and never asking again left loot on the floor,
                // so ask again before writing it off.
                if tries < LOOT_TRIES {
                    tracing::info!(
                        "autoplay: corpse {guid:#010x} did not open; asking again ({tries})"
                    );
                    self.interact(guid);
                    self.autoplay.corpse = Some((guid, now, allow, tries + 1));
                    return true;
                }
                // Not "done with": set aside. A corpse belongs to
                // whoever killed it until it has rotted a while, so one
                // that will not open now may well open later, and
                // writing it off for good leaves a boss's loot on the
                // floor. It is tried again while it is still there.
                tracing::info!(
                    "autoplay: corpse {guid:#010x} will not open yet; trying again later"
                );
                self.autoplay.shelved.note(
                    guid,
                    &crate::did::Did::blocked("it will not open yet"),
                    now,
                );
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
            // Ask the server about the ones -- and only the ones -- whose
            // fate cannot be settled without it.
            //
            // An identify is a round trip each, and on a corpse of eight
            // that is eight of them before anything is picked up. So
            // every item is judged cheaply first and only those that
            // came back "cannot say yet" are asked about. A profile
            // whose early rules ask about name, kind and worth empties
            // a corpse without a single one.
            if cfg.appraise && !self.autoplay.appraising {
                let missing = self.loot_to_look_over(&items, &cfg, now);
                if !missing.is_empty() {
                    let n = missing.len();
                    self.appraise_many(missing);
                    self.autoplay.appraising = true;
                    self.autoplay.say(
                        Doing::Looting,
                        format!("looking over {n} of {} item(s)", items.len()),
                    );
                    return true;
                }
            }
            if self.autoplay.appraising
                && items.iter().any(|g| !self.appraisals.contains_key(g))
                && now.duration_since(since) < allow
            {
                return true;
            }
            // A full pack takes nothing, and asking the server anyway
            // only fills the log with refusals.
            // A pack runs out of two things and this only ever counted
            // one. Eighty-four refusals in one watched run were a
            // character with slots to spare and nothing left to lift
            // with, asking anyway and being told no each time.
            // Not the server's wall but our own limit, well short of
            // it: a character that loots until the server refuses has
            // nothing left to lift with and cannot even tidy its pack.
            let laden = self.carry_room() == 0;
            if self.pack_full() || laden {
                let why = if laden {
                    "carrying as much as it means to, leaving the loot"
                } else {
                    "pack full, leaving the loot"
                };
                self.autoplay.note(why, now);
                self.autoplay.looted.push(guid);
                self.autoplay.corpse = None;
                self.stop_walking_to_loot();
                self.autoplay.appraising = false;
                return false;
            }
            // Decide once, then take them one at a time. The server
            // does one move at a time and refuses the rest as "you're
            // too busy", queueing at most one; asking for eight at once
            // got one item and seven refusals, and closing the corpse
            // straight afterwards left the rest with nowhere to be
            // found. So the wanted items become a queue.
            if self.autoplay.take_queue.is_empty() && self.autoplay.last_take.is_none() {
                let own_corpse = self
                    .world
                    .open_container
                    .as_ref()
                    .and_then(|c| self.world.objects.get(&c.0))
                    .is_some_and(|o| {
                        let me = self.world.stats.name.to_lowercase();
                        !me.is_empty() && o.name.to_lowercase() == format!("corpse of {me}")
                    });
                let mut queue = Vec::new();
                for g in &items {
                    let Some(stats) = self.stats_of(*g) else {
                        continue;
                    };
                    // A kind the server has lately said cannot be had
                    // yet is left alone for a while (see
                    // `loot_refused`).
                    if self.refused_lately(stats.wcid, now) {
                        continue;
                    }
                    let action = if own_corpse {
                        // Our own body: everything on it is ours, and
                        // the wand and the components on it are what
                        // the character needs to fight again.
                        LootAction::Keep
                    } else {
                        self.loot_verdict(&stats, &cfg)
                    };
                    if action.takes() {
                        tracing::info!("autoplay: taking {} ({})", stats.name, action.label());
                        self.autoplay.tag(*g, action);
                        queue.push(*g);
                    }
                }
                if queue.is_empty() {
                    self.close_container();
                    self.autoplay.looted.push(guid);
                    self.autoplay.corpse = None;
                    self.stop_walking_to_loot();
                    self.autoplay.appraising = false;
                    self.autoplay.say(Doing::Looting, "nothing worth taking");
                    return true;
                }
                self.autoplay.take_queue = queue;
            }
            // Whatever has left the corpse is done with, and whatever
            // has been asked for often enough and stayed put is not
            // coming: the pack is full, or the thing will not be moved.
            // Asking again every four hundred milliseconds until the
            // corpse rots is not persistence, it is a stuck character.
            let still: Vec<u32> = self.autoplay.take_queue.clone();
            let asked = &self.autoplay.take_tries;
            self.autoplay.take_queue = still
                .into_iter()
                .filter(|g| items.contains(g) && !self.world.is_carried(*g))
                // Asked for three times and never moved: the pack is
                // full, or the thing will not be given up. Leave it
                // rather than let the queue stand behind it for ever.
                .filter(|g| asked.waited(g).is_none_or(|w| w <= NOT_COMING))
                .collect();
            if let Some(next) = self.autoplay.take_queue.first().copied() {
                // One at a time, in order, and the order does not
                // change while one is outstanding. Skipping over the
                // item at the front because its wait has not run out
                // would start the next take before this one had
                // finished, and asking for two at once is what
                // "Source item not found!" is.
                let ready = self
                    .autoplay
                    .last_take
                    .is_none_or(|t| now.duration_since(t) >= TAKE_EVERY)
                    && !self.autoplay.take_tries.held(&next, now);
                if ready {
                    // The server answers a take by moving the item, or
                    // refuses it in chat -- never in a reply we can wait
                    // on. So the ask itself is recorded as waiting, and
                    // an item that keeps not moving is asked for less
                    // and less often: a quarter second, then a half,
                    // then a second. A take that works takes the item
                    // out of the corpse and so out of the queue, which
                    // is what ends it.
                    //
                    // Waiting rather than blocked because a slow answer
                    // is the common case and must not be mistaken for a
                    // refusal.
                    self.autoplay.take_tries.note(
                        next,
                        &crate::did::Did::waiting("it has not come out yet"),
                        now,
                    );
                    self.take(next);
                    self.autoplay.last_take = Some(now);
                }
                return true;
            }
            let what = self
                .world
                .objects
                .get(&guid)
                .map(|o| o.name.clone())
                .unwrap_or_else(|| "the corpse".to_string());
            self.close_container();
            self.autoplay.looted.push(guid);
            self.autoplay.corpse = None;
            self.stop_walking_to_loot();
            self.autoplay.appraising = false;
            self.autoplay.last_take = None;
            self.autoplay.take_tries.clear();
            self.autoplay.say(Doing::Looting, format!("emptied {what}"));
            return true;
        }
        // Look for one nearby that we have not emptied.
        //
        // Mid-fight the loot waits: a corpse keeps for five minutes and
        // the thing hitting us does not. The exception is a corpse
        // about to rot, which is worth breaking off for because there
        // is no second chance at it.
        //
        // A cast in progress is different from a fight in progress: a
        // spell half thrown is wasted, and the corpse keeps for the
        // second it takes to finish. Whether the fight itself is worth
        // breaking off is not decided here any more -- the worth of
        // looting says that, and it rises as bodies age and pile up
        // (see `crate::steps`).
        let pressed = self.autoplay.casting_at().is_some();
        let me = self.player.as_ref().map(|p| p.world_position());
        let Some(me) = me else { return false };
        let looted = self.autoplay.looted.clone();
        // A corpse set aside for being locked is tried again once its
        // wait is up; waits that have run out stop being remembered.
        self.autoplay.shelved.tidy(now);
        // Note when each corpse turned up, so the ones running out can
        // be emptied first. Forgotten once emptied, so the list stays
        // the size of what is on the ground.
        for o in self.world.objects.values() {
            if o.object_desc_flags & ac_world::object_desc_flags::CORPSE != 0
                && !self.autoplay.corpse_seen.iter().any(|(g, _)| *g == o.guid)
            {
                self.autoplay.corpse_seen.push((o.guid, now));
            }
        }
        self.autoplay
            .corpse_seen
            .retain(|(g, t)| now.duration_since(*t) < CORPSE_LIFE * 2 && !looted.contains(g));
        let seen_at: std::collections::BTreeMap<u32, Instant> =
            self.autoplay.corpse_seen.iter().copied().collect();
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
            .filter(|o| !looted.contains(&o.guid) && !self.autoplay.shelved.held(&o.guid, now))
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
            .map(|(d, guid, name)| {
                let seen = seen_at.get(&guid).copied().unwrap_or(now);
                let left = CORPSE_LIFE.saturating_sub(now.duration_since(seen));
                (left, d, guid, name)
            })
            .min_by(|a, b| {
                // The one closest to rotting first, and among the ones
                // in no danger, the nearest.
                let urgent = |l: Duration| l <= CORPSE_URGENT;
                urgent(b.0)
                    .cmp(&urgent(a.0))
                    .then_with(|| a.1.total_cmp(&b.1))
            });
        let Some((left, away, guid, name)) = corpse else {
            return false;
        };
        // Never mid-cast, unless the body will not be there when the
        // spell lands.
        if pressed && left > CORPSE_URGENT {
            return false;
        }
        // Breaking off a fight means breaking it off. The server marks
        // a character swinging or shooting as busy and refuses to open
        // anything for it, so letting the attack run while walking to a
        // corpse buys "You're too busy" and nothing else -- the target
        // goes first, then the combat stance.
        self.attack_target = None;
        self.autoplay.casting_at = None;
        self.autoplay.armed_for = None;
        if self.combat {
            self.toggle_combat();
        }
        // Stand over it first (see [`CORPSE_REACH`]).
        if away > CORPSE_REACH {
            if let Some(at) = self.world.objects.get(&guid).and_then(|o| o.world_pos()) {
                // Well inside the radius rather than on its edge: the
                // last metre of a walk wanders, and stopping on the
                // line means stepping back off it again.
                self.head_for(at, CORPSE_REACH / 2.0, "the corpse");
                // Said once for the walk, not once a frame: the
                // distance changes every tick and the log is not a
                // tape measure.
                if self.autoplay.walking_to != Some(guid) {
                    self.autoplay.walking_to = Some(guid);
                    self.autoplay.say(
                        Doing::Looting,
                        format!("walking to {name} ({} m)", away.round()),
                    );
                }
                return true;
            }
        }
        self.stop_walking_to_loot();
        // Opening a corpse is "using something", which ends a journey.
        self.remember_journey();
        self.interact(guid);
        self.autoplay.corpse = Some((guid, now, loot_wait(away), 0));
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
    fn fighting_stance_as(&mut self, style: Style) -> Stance {
        let want = match style {
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
    fn arm_for(&mut self, target: u32, stance: Stance, cfg: &Fight) {
        if !cfg.pick_weapon {
            return;
        }
        if self.autoplay.armed_for == Some(target) {
            return;
        }
        self.autoplay.armed_for = Some(target);
        let Some(known) = self.creature_known(target) else {
            tracing::debug!("arm: nothing known about {target:#010x}, keeping what is held");
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
        let free_choice = cfg.style == Style::Auto;
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
            tracing::debug!("arm: nothing to pick from for {}", known.name);
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
        // Swapping costs nothing worth counting, so take the best there
        // is: anything better than what is in hand wins. Equal keeps
        // what is held, so a tie cannot set it swapping back and forth.
        let now_worth = held
            .map(|i| crate::weapons::score(i, Some(known), &wielder))
            .unwrap_or(0.0);
        tracing::debug!(
            "arm: {} vs {} -> best {} {:.3} (held {:.3})",
            known.name,
            held.map(|i| i.name.as_str()).unwrap_or("nothing"),
            pick.name,
            pick.score,
            now_worth
        );
        if held.map(|i| i.guid) == Some(pick.guid) || pick.score <= now_worth {
            return;
        }
        tracing::info!(
            "autoplay: wielding {} against {} ({})",
            pick.name,
            known.name,
            pick.why
        );
        let picked_stats = carried.iter().find(|i| i.guid == pick.guid);
        // A one-handed melee weapon leaves the off hand for a shield,
        // which is put on once the weapon is in hand (see
        // `autoplay_shield`); anything else wants that hand empty.
        let free_offhand = picked_stats.is_some_and(crate::weapons::needs_free_offhand);
        self.autoplay.wanted_shield = if free_offhand {
            None
        } else {
            crate::weapons::best_shield(&carried, &wielder).map(|s| s.guid)
        };
        // The server will not put a second weapon in full hands: the
        // old one goes back in the pack first, the shield too when the
        // new weapon cannot be held with one, and the new one is
        // wielded once the hands are empty.
        let mut sent = self.put_weapons_away();
        if free_offhand {
            let me = self.world.player_guid;
            let shield = self.wielded_shield();
            if let (Some(me), Some(shield)) = (me, shield) {
                sent |= self.put_in_container(shield, me);
            }
        }
        if sent {
            self.autoplay.pending_wield = Some(pick.guid);
        } else {
            self.wield_guid(pick.guid);
        }
    }

    /// The shield on the off hand, if any.
    fn wielded_shield(&self) -> Option<u32> {
        self.world
            .wielded()
            .find(|o| o.valid_locations & ac_world::equip::SHIELD != 0)
            .map(|o| o.guid)
    }

    /// Put the shield chosen with a one-handed weapon on, once that
    /// weapon is in hand and the off hand is free.
    pub(crate) fn autoplay_shield(&mut self, now: Instant) {
        let Some(shield) = self.autoplay.wanted_shield else {
            return;
        };
        if self.autoplay.pending_wield.is_some() {
            return;
        }
        if !self.world.is_carried(shield) || self.wielded_shield().is_some() {
            self.autoplay.wanted_shield = None;
            return;
        }
        // Only with a one-handed melee weapon actually in hand: the
        // weapon may still be on its way, or have turned out to be
        // something a shield cannot go with.
        let held: Vec<crate::items::ItemStats> = self
            .world
            .wielded()
            .filter(|o| crate::weapons::stance_of(&crate::items::ItemStats::of(o, None)).is_some())
            .map(|o| {
                self.stats_of(o.guid)
                    .unwrap_or_else(|| crate::items::ItemStats::of(o, None))
            })
            .collect();
        let Some(weapon) = held.first() else {
            return;
        };
        if crate::weapons::needs_free_offhand(weapon) {
            self.autoplay.wanted_shield = None;
            return;
        }
        if self
            .autoplay
            .last_rewield
            .is_some_and(|t| now.duration_since(t) < REWIELD_EVERY)
        {
            return;
        }
        self.autoplay.last_rewield = Some(now);
        self.autoplay.wanted_shield = None;
        tracing::info!("autoplay: putting the shield on with the weapon");
        self.wield_guid(shield);
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
        self.skill_now(33)
    }

    /// This character's Salvaging as it stands, buffs counted.
    pub fn salvaging(&self) -> u32 {
        self.skill_now(SALVAGING)
    }

    /// A skill as it stands right now, 0 when the sheet lacks it.
    fn skill_now(&self, id: u32) -> u32 {
        let stats = &self.world.stats;
        let Some(sk) = stats.skill(id) else {
            return 0;
        };
        let table = self.assets.skill_table().ok();
        stats.skill_current(sk, table.as_ref().and_then(|t| t.get(id)))
    }

    /// Who salvages for the team, this character included: the highest
    /// Salvaging among those carrying an Ust (see [`best_salvager`]).
    /// Off the team it is this character, if it has an Ust.
    pub fn best_salvager(&self) -> Option<(String, u32)> {
        let me = Mate {
            name: self.world.stats.name.clone(),
            guid: self.world.player_guid.unwrap_or(0),
            salvaging: self.salvaging(),
            has_ust: self.salvage_tool().is_some(),
            ..Default::default()
        };
        best_salvager(std::iter::once(&me).chain(self.autoplay.team.mates.iter()))
    }

    /// Look at what has turned up in the pack since last time and tag
    /// what the rules would salvage or sell (see [`arrival_tag`]): the
    /// salvage a teammate handed over, mostly. The first pass only
    /// notes what is carried.
    fn autoplay_tag_arrivals(&mut self, now: Instant, cfg: &Loot) {
        let carried: Vec<u32> = self
            .world
            .inventory()
            .chain(self.world.wielded())
            .map(|o| o.guid)
            .collect();
        let ap = &mut self.autoplay;
        if !ap.baselined {
            ap.seen = carried.iter().copied().collect();
            ap.baselined = true;
            return;
        }
        ap.seen.retain(|g| carried.contains(g));
        ap.tags.retain(|g, _| carried.contains(g));
        ap.refused.retain(|g, _| carried.contains(g));
        ap.pending_tags.retain(|(g, _)| carried.contains(g));
        for g in &carried {
            if ap.seen.insert(*g) && !ap.tags.contains_key(g) {
                ap.pending_tags.push((*g, now));
            }
        }
        let needs = cfg.appraise && cfg.needs_appraisal();
        let pending = std::mem::take(&mut self.autoplay.pending_tags);
        for (g, since) in pending {
            let Some(stats) = self.stats_of(g) else {
                continue;
            };
            if needs && !stats.appraised && now.duration_since(since) < TAG_TIMEOUT {
                self.appraise_many([g]);
                self.autoplay.pending_tags.push((g, since));
                continue;
            }
            if let Some(action) = arrival_tag(&stats, cfg) {
                tracing::info!(
                    "autoplay: {} arrived, tagged {}",
                    stats.name,
                    action.label()
                );
                self.autoplay.tags.insert(g, action);
            }
        }
    }

    /// Carried items tagged for salvage that can go: not worn, not
    /// wanted by a blank tag. `bags` says whether salvage bags count
    /// (they are handed on, never salvaged again).
    fn salvage_tagged(&self, bags: bool) -> Vec<(u32, String)> {
        let me = self.world.player_guid;
        let mut items: Vec<(u32, String)> = self
            .autoplay
            .tags
            .iter()
            .filter(|(_, a)| **a == LootAction::Salvage)
            .filter_map(|(g, _)| self.world.objects.get(g))
            .filter(|o| o.wielder != me && self.world.is_carried(o.guid))
            .filter(|o| {
                let bag = o.name.starts_with("Salvaged ");
                if bag {
                    bags
                } else {
                    o.material != 0 && o.workmanship > 0.0
                }
            })
            .filter(|o| {
                self.autoplay
                    .refused
                    .get(&o.guid)
                    .is_none_or(|n| *n < SALVAGE_TRIES)
            })
            .map(|o| (o.guid, o.name.clone()))
            .collect();
        items.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        items
    }

    /// Salvage what the rules tagged, or carry it to whoever salvages
    /// for the team. Runs between fights. True while busy with it.
    pub(crate) fn autoplay_salvage(&mut self, now: Instant) -> bool {
        let cfg = self.autoplay.config.loot.clone();
        if self.world.player_guid.is_none() {
            return false;
        }
        self.autoplay_tag_arrivals(now, &cfg);
        if !cfg.enabled || self.attack_target.is_some() || self.autoplay.corpse.is_some() {
            return false;
        }
        // A batch on its way: wait for the items to go, and count a
        // refusal against each when they do not.
        if let Some((items, since)) = self.autoplay.salvaging.clone() {
            let left: Vec<u32> = items
                .iter()
                .copied()
                .filter(|g| self.world.is_carried(*g))
                .collect();
            if left.is_empty() {
                self.autoplay.salvaging = None;
                self.autoplay.say(
                    Doing::Salvaging,
                    format!("salvaged {} item(s)", items.len()),
                );
            } else if now.duration_since(since) < SALVAGE_TIMEOUT {
                return true;
            } else {
                self.autoplay.salvaging = None;
                for g in left {
                    let n = self.autoplay.refused.entry(g).or_default();
                    *n += 1;
                    if *n >= SALVAGE_TRIES {
                        let name = self.world.objects.get(&g).map(|o| o.name.clone());
                        self.autoplay.tags.insert(g, LootAction::Keep);
                        self.autoplay.note(
                            format!("could not salvage {}, keeping it", name.unwrap_or_default()),
                            now,
                        );
                    }
                }
            }
        }
        if let Some((item, since)) = self.autoplay.handing {
            if !self.world.is_carried(item) {
                self.autoplay.handing = None;
            } else if now.duration_since(since) < SALVAGE_TIMEOUT {
                return true;
            } else {
                self.autoplay.handing = None;
                let n = self.autoplay.refused.entry(item).or_default();
                *n += 1;
                if *n >= SALVAGE_TRIES {
                    let name = self
                        .world
                        .objects
                        .get(&item)
                        .map(|o| o.name.clone())
                        .unwrap_or_default();
                    self.autoplay.tags.insert(item, LootAction::Keep);
                    self.autoplay
                        .note(format!("{name} was not taken, keeping it"), now);
                }
            }
        }
        let Some((who, guid)) = self.best_salvager() else {
            if !self.salvage_tagged(false).is_empty() {
                self.autoplay
                    .note("salvage waiting: nobody on the team carries an Ust", now);
            }
            return false;
        };
        let rate_ok = self
            .autoplay
            .last_salvage
            .is_none_or(|t| now.duration_since(t) >= SALVAGE_EVERY);
        if Some(guid) == self.world.player_guid {
            if !cfg.salvage {
                return false;
            }
            let items = self.salvage_tagged(false);
            if items.is_empty() || !rate_ok {
                return false;
            }
            // The server salvages in peace mode only.
            if self.combat {
                self.toggle_combat();
                return true;
            }
            let guids: Vec<u32> = items.iter().map(|(g, _)| *g).collect();
            if !self.salvage(&guids) {
                return false;
            }
            self.autoplay.salvaging = Some((guids.clone(), now));
            self.autoplay.last_salvage = Some(now);
            self.autoplay.say(
                Doing::Salvaging,
                format!("salvaging {} item(s)", guids.len()),
            );
            return true;
        }
        if !cfg.hand_off {
            return false;
        }
        let items = self.salvage_tagged(true);
        if items.is_empty() {
            return false;
        }
        let Some(mate) = self
            .autoplay
            .team
            .mates
            .iter()
            .find(|m| m.guid == guid)
            .cloned()
        else {
            return false;
        };
        let Some(me) = self.player.as_ref().map(|p| p.world_position()) else {
            return false;
        };
        // Not while it is fighting, and not from across the map.
        if mate.target.is_some() {
            return false;
        }
        let distance = mate.world.distance(me);
        if distance > HAND_OFF_RANGE {
            self.autoplay.note(
                format!(
                    "salvage waiting: {who} is {distance:.0} m off (salvaging {})",
                    mate.salvaging
                ),
                now,
            );
            return false;
        }
        if distance > GIVE_REACH {
            if self
                .follow
                .is_none_or(|f| f.target.distance(mate.world) > 1.0)
            {
                self.interrupt_travel("taking salvage to the salvager");
                self.steering.reset();
            }
            self.head_for(mate.world, GIVE_REACH * 0.8, &who);
            self.autoplay
                .say(Doing::Salvaging, format!("taking salvage to {who}"));
            return true;
        }
        if self.follow.take().is_some() {
            self.steering.reset();
        }
        if !rate_ok {
            return true;
        }
        let (item, name) = items[0].clone();
        if !self.give(guid, item, None) {
            return false;
        }
        self.autoplay.handing = Some((item, now));
        self.autoplay.last_salvage = Some(now);
        self.autoplay.say(
            Doing::Salvaging,
            format!(
                "giving {name} to {who} to salvage ({} left)",
                items.len() - 1
            ),
        );
        true
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
    /// True when this tick went on making some.
    ///
    /// The server's side of it (ACE `RecipeManager::UseObjectOnTarget`):
    /// using the heads on the shafts is refused outright in any combat
    /// stance, and by a character not trained in Fletching, so the
    /// character drops to peace first and the bundles are used once the
    /// stance change has had its moment. The server may then ask, as a
    /// yes/no confirmation, whether the chance of success is good
    /// enough; it is answered yes, and the arrows land in the pack a
    /// clap of the hands later, where the bow's arming picks them up.
    fn autoplay_craft_ammo(&mut self, now: Instant) -> bool {
        if !self.autoplay.config.fight.craft_ammo {
            return false;
        }
        // The chance-of-success question, if the character has that
        // option on: the answer is always yes, the bundles being for
        // nothing else.
        const CRAFT: u32 = 5;
        let asked: Vec<u32> = self
            .world
            .confirmations
            .iter()
            .filter(|c| c.kind == CRAFT)
            .map(|c| c.context)
            .collect();
        if !asked.is_empty() && self.autoplay.last_craft.is_some() {
            for context in asked {
                self.confirm(CRAFT, context, true);
            }
            return true;
        }
        // Waiting for peace mode before the bundles are used.
        if let Some((source, target, since)) = self.autoplay.crafting {
            if now.duration_since(since) < STANCE_CHANGE {
                return true;
            }
            self.autoplay.crafting = None;
            self.autoplay.last_craft = Some(now);
            return self.use_on(source, target);
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
        // Fletching as it stands, and only if trained: an untrained
        // skill has a number too, but the server will not craft with it.
        let fletching = {
            let stats = &self.world.stats;
            let table = self.assets.skill_table().ok();
            stats
                .skill(FLETCHING)
                .filter(|sk| sk.advancement >= ac_world::stats::sac::TRAINED)
                .map(|sk| stats.skill_current(sk, table.as_ref().and_then(|t| t.get(FLETCHING))))
                .unwrap_or(0)
        };
        let carried: Vec<(u32, u32)> = self
            .world
            .objects
            .values()
            .filter(|o| self.world.is_carried(o.guid))
            .map(|o| (o.weenie_class_id, o.guid))
            .collect();
        let weakest = self
            .autoplay
            .casting_at
            .or(self.attack_target)
            .and_then(|g| self.creature_known(g))
            .and_then(|c| c.weakest_to());
        let Some((recipe, source, target)) =
            choose_recipe(launcher.ammo_type, fletching, &carried, weakest)
        else {
            self.autoplay.note(
                format!(
                    "out of {} and nothing to make more from",
                    ac_world::fletching::ammo_type::name(launcher.ammo_type)
                ),
                now,
            );
            return false;
        };
        self.autoplay.say(
            Doing::Looting,
            format!("making {} from {}", recipe.result_name, recipe.source_name),
        );
        if self.combat || self.magic {
            // Peace first; the use goes out once the stance has changed.
            self.leave_combat();
            self.autoplay.crafting = Some((source, target, now));
            return true;
        }
        self.autoplay.last_craft = Some(now);
        self.use_on(source, target)
    }

    /// Item slots the character has, and how many are in use.
    ///
    /// Side packs count. A pack brings its own item slots with it, and
    /// the server fills them by itself once the main pack is full, so
    /// judging fullness by the main pack alone declares a character out
    /// of room while it is carrying six empty bags.
    ///
    /// Pack slots are a separate count and are left out of both
    /// numbers: a pack, and each of the five Foci, sits in one of those
    /// (see `ac_world::pack_slot`).
    pub fn item_slots(&self) -> (u32, u32) {
        let mine = self
            .world
            .player()
            .map(|p| p.items_capacity)
            .filter(|c| *c > 0)
            .unwrap_or(102);
        let me = self.world.player_guid;
        let mut capacity = mine;
        let mut used = 0;
        for o in self.world.inventory() {
            let is_pack = o.item_type & ac_world::item_type::CONTAINER != 0;
            if ac_world::pack_slot::used_by(o.weenie_class_id, is_pack) {
                // A pack in a pack slot: its own slots are added, and it
                // does not spend one of the character's own.
                if o.container == me {
                    capacity = capacity.saturating_add(o.items_capacity);
                    continue;
                }
            }
            used += 1;
        }
        (used, capacity)
    }

    /// There is no room for another item.
    /// A body within reach that has not been emptied yet.
    ///
    /// What "within reach" means matters: a corpse across the dungeon
    /// is not something the character owes anything to, and waiting on
    /// it would stop the fighting altogether. This is about the one at
    /// its feet that it just made.
    pub fn owes_a_corpse(&self) -> bool {
        let Some(me) = self.player.as_ref().map(|p| p.world_position()) else {
            return false;
        };
        self.world
            .objects
            .values()
            .filter(|o| o.object_desc_flags & ac_world::object_desc_flags::CORPSE != 0)
            .filter(|o| !self.autoplay.looted.contains(&o.guid))
            .filter_map(|o| o.world_pos())
            .any(|at| at.distance(me) <= CORPSE_IS_MINE)
    }

    pub fn pack_full(&self) -> bool {
        let (used, capacity) = self.item_slots();
        used >= capacity
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
    pub(crate) fn autoplay_fight(&mut self, now: Instant) -> bool {
        let cfg = self.autoplay.config.fight.clone();
        self.autoplay_fight_as(now, &cfg)
    }

    /// The fight rule with the rules given (the academy points it at the
    /// creatures a task names).
    pub(crate) fn autoplay_fight_as(&mut self, now: Instant, cfg: &Fight) -> bool {
        let cfg = cfg.clone();
        if !cfg.enabled {
            return false;
        }
        let stance = self.fighting_stance_as(cfg.style);
        if stance == Stance::Magic {
            return self.autoplay_fight_with_spells(now, &cfg);
        }
        // Already on one that is still alive.
        if let Some(t) = self.attack_target {
            if self.stalled_on(t, now) {
                return false;
            }
            // And still here. A target is chosen from within the fight
            // radius, but nothing checked it was still within it
            // afterwards -- so a creature the character walked away
            // from, or left behind in the Academy, stayed its target
            // for ever while it planned a journey to the other side of
            // the world to swing at it.
            let gone = self
                .world
                .objects
                .get(&t)
                .and_then(|o| o.world_pos())
                .zip(self.player.as_ref().map(|p| p.world_position()))
                .is_some_and(|(at, me)| at.distance(me) > crate::travel::WALKABLE);
            if gone {
                self.attack_target = None;
                self.autoplay.casting_at = None;
            }
            if let Some(o) = self.world.objects.get(&t).filter(|_| !gone) {
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
                    self.arm_for(guid, stance, &cfg);
                    if missile {
                        let range = self.attack_range(crate::dodge::How::Missile);
                        if self.autoplay_approach(guid, &name, range) {
                            return true;
                        }
                    }
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
            // With the vitae high, the hard ones and the killer wait.
            .filter(|o| !self.shy_of(o))
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
        self.arm_for(guid, stance, &cfg);
        // A bow refused for range shoots nothing: close in first (see
        // `crate::dodge`). A swing from too far the server walks us
        // in for.
        if missile {
            let range = self.attack_range(crate::dodge::How::Missile);
            if self.autoplay_approach(guid, &name, range) {
                return true;
            }
        }
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
        // Paced to the casting, buffs included: the server queues one
        // spell sent over another and drops the next, so an attack
        // thrown on the heels of a buff would be the one dropped.
        if self.autoplay.cast_in_flight(now) {
            self.autoplay
                .say(Doing::Fighting, format!("fighting {name}"));
            return true;
        }
        // Behind the throttle, so choosing a wand costs no more than one
        // look per cast. Wielding takes a moment, so this cast still
        // goes out with the old one and the next with the new.
        self.arm_for(guid, Stance::Magic, cfg);
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
            // From too far off the server refuses the cast outright:
            // close in first (see `crate::dodge`).
            let range = self.attack_range(crate::dodge::How::Spell(spell));
            if self.autoplay_approach(guid, &name, range) {
                return true;
            }
            self.cast(spell);
            self.autoplay.last_cast = Some(now);
        self.autoplay.cast_sent = Some(now);
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
        // Waiting on the server's answer, not on a clock.
        if self.autoplay.cast_in_flight(now) {
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
        self.autoplay.cast_sent = Some(now);
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
        self.autoplay.cast_sent = Some(now);
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
    pub(crate) fn attack_spells_known(&self) -> Vec<u32> {
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
    pub(crate) fn autoplay_resume_journey(&mut self) -> bool {
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
    fn pick_target(&mut self, cfg: &Fight) -> Option<u32> {
        let me = self.player.as_ref()?.world_position();
        let now = Instant::now();
        // A follower fights beside its leader, not wherever a monster
        // happens to be.
        let leader_at = self.followed_leader().map(|m| m.world);
        let fight_radius = self.autoplay.config.team.fight_radius.max(1.0);
        let candidates: Vec<(u32, glam::Vec3)> = self
            .world
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
            // With the vitae high, the hard ones and the killer wait.
            .filter(|o| !self.shy_of(o))
            .filter_map(|o| {
                let at = o.world_pos()?;
                let near_leader = leader_at.is_none_or(|l| at.distance(l) <= fight_radius);
                (at.distance(me) <= cfg.radius && near_leader).then_some((o.guid, at))
            })
            .collect();
        // The nearest one we can actually hit: one behind a wall is
        // taken only when nothing is in sight, and then the fight rules
        // walk round to it.
        candidates
            .into_iter()
            .map(|(guid, at)| {
                let seen = self.can_see_at(at);
                ((!seen) as u8, at.distance(me), guid)
            })
            .min_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)))
            .map(|(_, _, g)| g)
    }

    /// The leader this character follows, when it follows one.
    pub(crate) fn followed_leader(&self) -> Option<&Mate> {
        let team = &self.autoplay.config.team;
        if !team.enabled || !team.follow || team.lead || self.autoplay.team.leader {
            return None;
        }
        self.autoplay.team.leader_mate().filter(|m| m.leads)
    }

    /// Whether a spell or arrow from here would reach something standing
    /// at `at` without hitting a wall first (see `Player::sees`).
    pub(crate) fn can_see_at(&mut self, at: glam::Vec3) -> bool {
        let assets = self.assets.clone();
        match self.player.as_mut() {
            Some(pl) => {
                let me = pl.world_position();
                pl.sees(&assets, me, at)
            }
            None => true,
        }
    }

    /// Note what this character is running short of, so the others can
    /// hand it over.
    pub(crate) fn autoplay_stock(&mut self) {
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
        // The server asks the character before recruiting it only when
        // its options allow: with "accept fellowship requests" off the
        // leader's invitation is refused outright, and with "automatically
        // accept" on it never has to be answered. A teammate keeps both on,
        // and lets the others give it items: that is how salvage reaches
        // whoever salvages, and the server refuses a gift to anyone with
        // the option off (ACE `CharacterOptions1.AllowGive`).
        for name in [
            "accept fellowship",
            "automatically accept fellowship",
            "let other players give you items",
        ] {
            if let Some(o) = crate::options::option_by_name(name) {
                if !self.option_enabled(o) {
                    self.set_option(o, true);
                }
            }
        }
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
    pub(crate) fn autoplay_follow(&mut self, now: Instant, urgent: bool) -> bool {
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
        let far = flat > follow_break(keep);
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
            self.head_for(leader.world, keep, "the leader");
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

    /// How close two characters must stand to hand something over.
    const REACH: f32 = 5.0;

    /// Loading the quartermaster and unloading it again.
    ///
    /// On a quartermaster run one character carries the party's sale
    /// loot to town and its shopping home, so there are two moments
    /// where items change hands: everyone gives it their loot before it
    /// leaves, and it gives everyone their order when it gets back.
    /// Both are the same shape -- walk into reach, hand one thing over,
    /// come back next frame for the next -- because the server takes
    /// one give at a time.
    ///
    /// True when it acted, which stops the rest of the rules for this
    /// frame: nothing else matters while the party is being loaded.
    fn autoplay_quartermaster(&mut self, now: Instant) -> bool {
        use crate::logistics::{Plan, Stage};
        let team = self.autoplay.config.team.clone();
        if !team.enabled || !team.restock.together || team.restock.plan != Plan::Quartermaster {
            return false;
        }
        let Some(stage) = self.autoplay.growth.mode.stage() else {
            return false;
        };
        // Waiting on the last give. The frame is still this errand's:
        // let something else have it and the tidying merges away the
        // money that was just counted out.
        if self
            .autoplay
            .last_give
            .is_some_and(|t| now.duration_since(t) < GIVE_EVERY)
        {
            return matches!(stage, Stage::HandOver | Stage::HandOut);
        }
        let growth = self.autoplay.config.growth.clone();
        let Some(runner) = self.quartermaster_name(&growth) else {
            return false;
        };
        let am_runner = runner == self.world.stats.name;
        match stage {
            Stage::HandOver if !am_runner => self.load_the_quartermaster(&runner, &growth, now),
            Stage::HandOut if am_runner => self.unload_the_quartermaster(&growth, now),
            _ => false,
        }
    }

    /// Give the runner this character's sale loot, then say so.
    fn load_the_quartermaster(
        &mut self,
        runner: &str,
        growth: &crate::growth::Growth,
        now: Instant,
    ) -> bool {
        if self.autoplay.growth.handed_over {
            return false;
        }
        let Some(mate) = self
            .autoplay
            .team
            .mates
            .iter()
            .find(|m| m.name == runner)
            .cloned()
        else {
            return false;
        };
        let loot = self.loot_for_sale(growth);
        // Money travels with the loot. The runner is the one standing
        // at the counter, so it is the one that has to be able to pay,
        // and coin changes hands for nothing: it is trade notes that
        // cost to make (see `spare_coin`).
        let coin = self
            .autoplay
            .config
            .team
            .restock
            .share_money
            .then(|| self.spare_coin())
            .flatten();
        if loot.is_empty() && coin.is_none() {
            // Nothing to hand over: this character is loaded already.
            self.autoplay.growth.handed_over = true;
            return false;
        }
        if !self.step_into_reach(mate.world) {
            if self.reaching_too_long(mate.world, now) {
                // Cannot get to them -- a wall, a different building,
                // a floor above. The party is not held up over it.
                self.autoplay.growth.handed_over = true;
                self.autoplay
                    .note(format!("cannot get to {runner} to hand over"), now);
                return false;
            }
            self.autoplay
                .say(Doing::Helping, format!("taking the loot to {runner}"));
            return true;
        }
        if let Some((purse, amount)) = coin {
            // Part of a stack cannot be handed over as it stands: the
            // money is counted out first (see `hand_stack`).
            match self.hand_stack(mate.guid, purse, amount, now) {
                Some(true) => {
                    self.autoplay.say(
                        Doing::Helping,
                        format!("giving {runner} {amount} pyreals to shop with"),
                    );
                    return true;
                }
                None => {
                    self.autoplay.say(
                        Doing::Helping,
                        format!("counting out {amount} pyreals for {runner}"),
                    );
                    return true;
                }
                // The money will not come apart. The loot still can go,
                // and the runner may have enough of its own.
                Some(false) => {}
            }
        }
        let Some(&item) = loot.first() else {
            self.autoplay.growth.handed_over = true;
            return false;
        };
        let name = self
            .world
            .objects
            .get(&item)
            .map(|o| o.name.clone())
            .unwrap_or_default();
        if !self.give(mate.guid, item, None) {
            // The server would not take it; do not jam on this item.
            self.autoplay.growth.handed_over = true;
            return false;
        }
        self.autoplay.last_give = Some(now);
        self.autoplay
            .give_tries
            .note(item, &crate::did::Did::Done, now);
        self.autoplay
            .say(Doing::Helping, format!("giving {name} to {runner} to sell"));
        // Loaded once the last piece has gone.
        self.autoplay.growth.handed_over = loot.len() == 1;
        true
    }

    /// A carried stack of this weenie holding exactly this many, which
    /// is what a split leaves behind: the piece counted out to hand
    /// over. `None` for the weenie matches anything of the right size.
    fn piece_of(&self, wcid: Option<u32>, amount: u32) -> Option<u32> {
        self.world
            .inventory()
            .find(|o| {
                o.stack_size == amount
                    && match wcid {
                        Some(w) => o.weenie_class_id == w,
                        None => o.item_type & ac_world::item_type::MONEY != 0,
                    }
            })
            .map(|o| o.guid)
    }

    /// Hand `amount` out of a carried `stack` to a teammate.
    ///
    /// The server takes whole objects: a give of part of a stack goes
    /// into the void unanswered. So anything short of the whole stack
    /// is counted out into a stack of its own first, and that is what
    /// changes hands. `None` while the counting-out is still going on,
    /// `Some(true)` when a give went out, `Some(false)` when the thing
    /// will not come apart and the party should get on without it.
    fn hand_stack(&mut self, to: u32, stack: u32, amount: u32, now: Instant) -> Option<bool> {
        let (whole, wcid) = self
            .world
            .objects
            .get(&stack)
            .map(|o| (o.stack_size.max(1), o.weenie_class_id))
            .unwrap_or((1, 0));
        let piece = if amount >= whole {
            Some(stack)
        } else {
            self.piece_of(Some(wcid), amount)
        };
        if let Some(g) = piece {
            if self.give(to, g, None) {
                self.autoplay.last_give = Some(now);
                self.autoplay
                    .give_tries
                    .note(stack, &crate::did::Did::Done, now);
                return Some(true);
            }
            return Some(false);
        }
        // The counting-out has been asked for and the piece has not
        // appeared. Waiting rather than blocked: a split is one message
        // and the server answers in its own time, so the first few asks
        // are a quarter of a second apart and double from there.
        if self
            .autoplay
            .give_tries
            .waited(&stack)
            .is_some_and(|w| w > WILL_NOT_SPLIT)
        {
            return Some(false);
        }
        self.autoplay.give_tries.note(
            stack,
            &crate::did::Did::waiting("the money has not come apart yet"),
            now,
        );
        self.split_stack(stack, None, amount);
        self.autoplay.last_give = Some(now);
        None
    }

    /// Give everyone what they ordered.
    fn unload_the_quartermaster(&mut self, growth: &crate::growth::Growth, now: Instant) -> bool {
        let party = self.party_supplies(growth);
        // Work the whole party's split out for each thing carried, so
        // that a short run is shared rather than filling the first
        // order and leaving the last character with nothing.
        for (item, _) in crate::logistics::merged_order(&party) {
            let carried: Vec<(u32, u32, String)> = self
                .world
                .inventory()
                .filter(|o| o.name.eq_ignore_ascii_case(&item))
                .map(|o| (o.guid, o.stack_size.max(1), o.name.clone()))
                .collect();
            let brought: u32 = carried.iter().map(|(_, n, _)| n).sum();
            if brought == 0 {
                continue;
            }
            for (who, share) in crate::logistics::hand_out(&party, &item, brought) {
                if who == self.world.stats.name || share == 0 {
                    continue;
                }
                let Some(mate) = self
                    .autoplay
                    .team
                    .mates
                    .iter()
                    .find(|m| m.name == who)
                    .cloned()
                else {
                    continue;
                };
                if !self.step_into_reach(mate.world) {
                    if self.reaching_too_long(mate.world, now) {
                        self.autoplay
                            .note(format!("cannot get to {who} to hand out"), now);
                        continue;
                    }
                    self.autoplay
                        .say(Doing::Helping, format!("taking {who} their supplies"));
                    return true;
                }
                let (guid, stack, name) = carried[0].clone();
                let share = share.min(stack);
                match self.hand_stack(mate.guid, guid, share, now) {
                    Some(true) => {
                        self.autoplay
                            .say(Doing::Helping, format!("giving {share} {name} to {who}"));
                        return true;
                    }
                    None => {
                        self.autoplay.say(
                            Doing::Helping,
                            format!("counting out {share} {name} for {who}"),
                        );
                        return true;
                    }
                    Some(false) => continue,
                }
            }
        }
        false
    }

    /// The coin this character can hand over, as `(stack, amount)`:
    /// what it carries less the float it keeps for itself.
    ///
    /// Coin, not trade notes. A note is the lighter way to carry a
    /// fortune, but the server charges 1.15 times a note's face value
    /// to make one and pays only face value to cash it back, so turning
    /// a purse into notes and back costs the party thirteen percent of
    /// it. Handing over pyreals costs nothing and buys exactly as much.
    fn spare_coin(&self) -> Option<(u32, u32)> {
        let float = self.autoplay.config.team.restock.float;
        let purse = self
            .world
            .inventory()
            .filter(|o| o.item_type & ac_world::item_type::MONEY != 0)
            .map(|o| (o.guid, o.stack_size.max(1)))
            .max_by_key(|(_, n)| *n)?;
        let spare = purse.1.checked_sub(float)?;
        (spare > 0).then_some((purse.0, spare))
    }

    /// Walk towards a spot until close enough to hand something over.
    /// True once in reach.
    fn step_into_reach(&mut self, spot: glam::Vec3) -> bool {
        let Some(me) = self.player.as_ref().map(|p| p.world_position()) else {
            return false;
        };
        if glam::Vec2::new(spot.x - me.x, spot.y - me.y).length() <= Self::REACH {
            self.autoplay.reaching = None;
            if self.follow.take().is_some() {
                self.steering.reset();
            }
            return true;
        }
        self.head_for(spot, Self::REACH * 0.6, "the spot");
        false
    }

    /// Whether walking to `spot` has gone on too long to be walking any
    /// more. Indoors a counter can stand behind a wall the steering
    /// cannot get round, and a teammate can be in the next building;
    /// without this the character presses towards it for ever.
    ///
    /// The clock starts when a new spot is aimed at and is reset by any
    /// real progress towards it, so a long walk is fine and a stopped
    /// one is not.
    fn reaching_too_long(&mut self, spot: glam::Vec3, now: Instant) -> bool {
        let Some(me) = self.player.as_ref().map(|p| p.world_position()) else {
            return false;
        };
        let away = glam::Vec2::new(spot.x - me.x, spot.y - me.y).length();
        match self.autoplay.reaching {
            Some((at, best, since)) if at.distance(spot) < 1.0 => {
                if away < best - REACH_PROGRESS {
                    self.autoplay.reaching = Some((spot, away, now));
                    return false;
                }
                if now.duration_since(since) > REACH_GIVE_UP {
                    self.autoplay.reaching = None;
                    return true;
                }
                false
            }
            _ => {
                self.autoplay.reaching = Some((spot, away, now));
                false
            }
        }
    }

    /// The things done for the team: land the debuffs on its target,
    /// recruit it into a fellowship, hand over what someone is short of,
    /// and heal whoever is worst hurt. True when it acted.
    pub(crate) fn autoplay_team(&mut self, now: Instant) -> bool {
        let team = self.autoplay.config.team.clone();
        if !team.enabled {
            return false;
        }
        let me = self.player.as_ref().map(|p| p.world_position());
        let Some(me) = me else { return false };

        if self.autoplay_fellowship(now) {
            return true;
        }

        // Loading and unloading the quartermaster comes before the rest:
        // nothing else matters while the party is changing hands.
        if self.autoplay_quartermaster(now) {
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
    pub(crate) fn autoplay_buff(&mut self, now: Instant, urgent: bool) -> bool {
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
        // Fighting with spells, the buffs take turns with the attack
        // rather than starving it (see `buff_yields_to_attack`).
        if urgent && self.autoplay.buff_yields_to_attack(now) {
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
        // Casting a low level of something we know a better version of
        // is worth saying once: it is nearly always components for the
        // higher formula, and a character quietly buffing at level one
        // looks like a bug rather than an empty pack.
        if let Some((better, why)) = self.better_buff_blocked(spell) {
            self.autoplay
                .note(format!("buffing with a weaker spell: {better} {why}"), now);
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
        // A buff is a cast like any other as far as the pacing goes:
        // an attack thrown over it would be dropped.
        self.autoplay.last_buff = Some(now);
        self.autoplay.last_cast = Some(now);
        self.autoplay.cast_sent = Some(now);
        true
    }

    /// A stronger spell than `spell` that we know, do the same thing
    /// with, and cannot cast: its name and why. `None` when the one we
    /// are about to cast is already the best we know.
    fn better_buff_blocked(&self, spell: u32) -> Option<(String, String)> {
        use crate::magic::CastCheck;
        let table = self.assets.spell_table().ok()?;
        let mine = table.get(spell)?;
        let mut best: Option<(u32, String, String)> = None;
        for &id in &self.world.stats.spells {
            let Some(sp) = table.get(id) else { continue };
            // The same buff, only stronger: the category is the family
            // and the power is the level within it.
            if sp.category != mine.category || sp.power <= mine.power {
                continue;
            }
            let check = self.can_cast(id);
            if matches!(check, CastCheck::Ok) {
                continue;
            }
            let why = cast_problem(&check);
            if best.as_ref().is_none_or(|b| sp.power > b.0) {
                best = Some((sp.power, sp.name.clone(), why));
            }
        }
        best.map(|(_, name, why)| (name, why))
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
        self.world.stats.vital_max_current(i)
    }

    /// Take up again the weapon put down for an urgent buff, once no
    /// buff is due any more.
    pub(crate) fn autoplay_rearm(&mut self) {
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
    pub(crate) fn due_buff(
        &self,
        within: f32,
        now: Instant,
    ) -> Option<(u32, crate::buffs::Target, u32, String, f32)> {
        use crate::buffs::Target;
        let cfg = &self.autoplay.config.buffs;
        let table = self.assets.spell_table().ok();
        // (rank, left, ...): creature magic goes first. Its buffs raise
        // the skills and attributes the other schools cast from, so a
        // character that buffs them first can land higher levels of
        // everything after.
        let mut due: Option<(u8, f32, u32, Target, u32, String, f32)> = None;
        let clock = self.session.server_time().is_some();
        let creature_first = |spell: u32| -> u8 {
            let school = table.as_ref().and_then(|t| t.get(spell)).map(|s| s.school);
            u8::from(school != Some(ac_formats::spell_table::school::CREATURE))
        };
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
            let rank = creature_first(spell);
            if due.as_ref().is_some_and(|d| (d.0, d.1) <= (rank, left)) {
                return;
            }
            let sp = table.as_ref().and_then(|t| t.get(spell));
            let name = sp.map(|s| s.name.clone()).unwrap_or_default();
            let lasts = sp.and_then(|s| s.duration()).unwrap_or(1800.0) as f32;
            due = Some((rank, left, spell, target, category, name, lasts));
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
        due.map(|(_, _, spell, target, category, name, lasts)| {
            (spell, target, category, name, lasts)
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_take_queue_waits_on_its_front_item_and_steps_over_a_dead_one() {
        use crate::did::{Did, Patience};
        let t0 = Instant::now();
        let mut asked: Patience<u32> = Patience::new();
        let outstanding = Did::waiting("it has not come out yet");

        // Asked once: held briefly. The queue waits rather than
        // starting a second take, because two at once is what
        // "Source item not found!" is.
        asked.note(1, &outstanding, t0);
        assert!(asked.held(&1, t0));
        assert!(
            asked.waited(&1).is_some_and(|w| w <= NOT_COMING),
            "one ask is not a dead item"
        );

        // Three asks, a quarter of a second apart and doubling, and it
        // still has not moved: the queue goes on without it.
        let mut at = t0;
        for _ in 0..3 {
            at += asked.waited(&1).unwrap_or_default();
            asked.note(1, &outstanding, at);
        }
        assert!(
            asked.waited(&1).is_some_and(|w| w > NOT_COMING),
            "asked enough times to call it not coming"
        );

        // And an item that arrives clears everything remembered about
        // it, so a later corpse's copy starts afresh.
        asked.note(1, &Did::Done, at);
        assert!(asked.waited(&1).is_none());
    }

    #[test]
    fn a_daily_limit_is_a_wait_and_not_a_grudge() {
        use crate::did::{Because, Did, Patience};
        let t0 = Instant::now();
        let mut kinds: Patience<u32> = Patience::new();

        // YouHaveSolvedThisQuestTooRecently is what gates a once-a-day
        // drop. It is a wait: the thing comes back, and a session can
        // run for days.
        let too_recently = Did::Blocked(Because::server(0x043E));
        assert_eq!(too_recently.because().and_then(|b| b.code), Some(0x043E));
        kinds.note(7299, &too_recently, t0);
        assert!(kinds.held(&7299, t0), "left alone for now");
        // Not for ever, though: a day later it is asked about again.
        assert!(
            !kinds.held(&7299, t0 + Duration::from_secs(24 * 60 * 60)),
            "a day later it is worth another ask"
        );
        // Nor is "too many times", which a raised cap can lift.
        kinds.note(7299, &Did::Blocked(Because::server(0x043F)), t0);
        assert!(!kinds.held(&7299, t0 + Duration::from_secs(24 * 60 * 60)));
        // Only a thing no counter will ever take is for ever, and that
        // is a different answer entirely.
        kinds.note(1, &Did::refused("no vendor will take it"), t0);
        assert!(kinds.held(&1, t0 + Duration::from_secs(24 * 60 * 60)));
    }

    use super::*;
    use crate::items::ItemStats;

    #[test]
    fn ammunition_is_made_for_the_bow_and_the_targets_weakness() {
        use ac_world::elements::Element;
        use ac_world::fletching::ammo_type;
        // Plain arrowheads (4586), fire arrowheads (5341), arrowshafts
        // (4585) and quarrel shafts (5339), by guid.
        let carried = [(4586, 1), (5341, 2), (4585, 3), (5339, 4)];
        // Fletching enough for fire arrows, against something weak to fire.
        let (r, heads, shafts) =
            choose_recipe(ammo_type::ARROW, 100, &carried, Some(Element::Fire)).expect("fire");
        assert_eq!(
            (r.result_name.as_str(), heads, shafts),
            ("Fire Arrow", 2, 3)
        );
        // Weak to cold and no cold heads carried: the hardest recipe
        // that can be made, which is still the fire one.
        let (r, _, _) =
            choose_recipe(ammo_type::ARROW, 100, &carried, Some(Element::Cold)).expect("any");
        assert_eq!(r.result_name, "Fire Arrow");
        // Not skilled enough for fire arrows: plain ones.
        let (r, heads, shafts) =
            choose_recipe(ammo_type::ARROW, 10, &carried, Some(Element::Fire)).expect("plain");
        assert_eq!((r.result_name.as_str(), heads, shafts), ("Arrow", 1, 3));
        // A crossbow wants quarrels, made on the quarrel shafts.
        let (r, _, shafts) = choose_recipe(ammo_type::BOLT, 100, &carried, None).expect("quarrels");
        assert_eq!((r.result_name.as_str(), shafts), ("Fire Quarrel", 4));
        // No dart shafts: nothing for an atlatl.
        assert!(choose_recipe(ammo_type::ATLATL, 100, &carried, None).is_none());
        // Untrained (0): nothing at all.
        assert!(choose_recipe(ammo_type::ARROW, 0, &carried, None).is_none());
    }

    #[test]
    fn a_follower_strays_no_further_than_twice_its_distance() {
        assert_eq!(follow_break(4.0), 10.0);
        assert_eq!(follow_break(8.0), 16.0);
    }

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
        // A stat rule.
        l.rules = vec![LootRule::new("type:armor al>=200", LootAction::Keep)];
        assert!(wanted_loot(&item("Platemail", 100, 240), &l));
        assert!(!wanted_loot(&item("Platemail", 100, 120), &l));
        // Never wins over always and the rules.
        l.never = vec!["platemail".into()];
        assert!(!wanted_loot(&item("Platemail", 100, 240), &l));
        // A blank rule matches nothing.
        l.rules = vec![LootRule::new("", LootAction::Keep)];
        l.never = Vec::new();
        assert!(!wanted_loot(&item("Ornate Ring", 900, 0), &l));
        assert!(wanted_loot(&item("Pyreal", 12, 0), &l), "always still wins");
    }

    #[test]
    fn the_first_matching_rule_decides() {
        let mut l = Loot {
            rules: vec![
                LootRule::new("slot:ring epics>=2", LootAction::Keep),
                LootRule::new("ring", LootAction::Salvage),
                LootRule::new("value>250", LootAction::Keep),
                LootRule::new("armor", LootAction::Sell),
            ],
            ..Default::default()
        };
        let ring = |spells: &[&str]| ItemStats {
            name: "Gold Ring".into(),
            kind: "jewelry",
            valid_locations: crate::items::slot::FINGER,
            appraised: true,
            value: 900,
            material: "Gold",
            workmanship: 5.0,
            spells: spells.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let two = ring(&["Epic Strength", "Epic Focus"]);
        let one = ring(&["Epic Strength"]);
        assert_eq!(loot_action(&two, &l), LootAction::Keep);
        // The salvage rule comes before the value rule.
        assert_eq!(loot_action(&one, &l), LootAction::Salvage);
        assert!(wanted_loot(&one, &l), "salvage is still picked up");
        assert_eq!(
            loot_action(&item("Platemail", 100, 240), &l),
            LootAction::Sell
        );
        assert_eq!(loot_action(&item("Rusty Nail", 3, 0), &l), LootAction::Skip);
        assert!(!LootAction::Skip.takes());
        // Never and always still cut across the rules.
        l.never = vec!["gold".into()];
        assert_eq!(loot_action(&two, &l), LootAction::Skip);
        l.never.clear();
        l.always = vec!["gold".into()];
        assert_eq!(loot_action(&one, &l), LootAction::Keep);
        // What arrives in the pack is only tagged for salvage or sale.
        l.always.clear();
        assert_eq!(arrival_tag(&one, &l), Some(LootAction::Salvage));
        assert_eq!(arrival_tag(&two, &l), None);
        assert_eq!(arrival_tag(&item("Rusty Nail", 3, 0), &l), None);
        assert_eq!(
            arrival_tag(&item("Platemail", 100, 240), &l),
            Some(LootAction::Sell)
        );
        assert_eq!(LootAction::parse("Salvage"), Some(LootAction::Salvage));
        assert_eq!(LootAction::parse("burn"), None);
        assert!(l.needs_appraisal());
        l.rules = vec![LootRule::new("value>250", LootAction::Keep)];
        assert!(!l.needs_appraisal());
    }

    #[test]
    fn old_filters_become_keep_rules() {
        let old = r#"{"enabled":true,"filters":["value>500","type:armor al>=200"],"always":["Pyreal"],"never":[],"appraise":true}"#;
        let mut l: Loot = serde_json::from_str(old).unwrap();
        assert!(l.rules.is_empty());
        assert_eq!(
            l.rules(),
            vec![
                LootRule::new("value>500", LootAction::Keep),
                LootRule::new("type:armor al>=200", LootAction::Keep),
            ]
        );
        // The old list is in force before anything is migrated.
        assert!(wanted_loot(&item("Ornate Ring", 900, 0), &l));
        assert!(!wanted_loot(&item("Ornate Ring", 300, 0), &l));
        assert!(l.salvage && l.hand_off, "new switches take their defaults");
        l.migrate();
        assert!(l.filters.is_empty());
        assert_eq!(l.rules.len(), 2);
        // Written back, the filters are gone and the rules stay.
        let text = serde_json::to_string(&l).unwrap();
        assert!(!text.contains("filters"));
        assert!(text.contains(r#""action":"keep""#));
        let back: Loot = serde_json::from_str(&text).unwrap();
        assert_eq!(back, l);
        // A rule mentioned both ways is one rule.
        let both =
            r#"{"rules":[{"query":"value>500","action":"salvage"}],"filters":["value>500"," "]}"#;
        let l: Loot = serde_json::from_str(both).unwrap();
        assert_eq!(
            l.rules(),
            vec![LootRule::new("value>500", LootAction::Salvage)]
        );
    }

    #[test]
    fn the_best_salvager_has_an_ust_and_the_highest_skill() {
        let mate = |name: &str, guid: u32, salvaging: u32, has_ust: bool| Mate {
            name: name.into(),
            guid,
            salvaging,
            has_ust,
            ..Default::default()
        };
        let team = [
            mate("Zed", 1, 300, true),
            mate("Amy", 2, 300, true),
            mate("Bob", 3, 400, false),
            mate("Cal", 4, 200, true),
        ];
        // Bob's skill is highest but he has no Ust; Amy and Zed tie and
        // the name that sorts first wins.
        assert_eq!(best_salvager(team.iter()), Some(("Amy".into(), 2)));
        assert_eq!(best_salvager(team[3..].iter()), Some(("Cal".into(), 4)));
        assert_eq!(best_salvager(team[2..3].iter()), None);
        assert_eq!(best_salvager(std::iter::empty()), None);
        // Someone not yet in the world (guid 0) cannot be handed anything.
        assert_eq!(best_salvager([mate("Nobody", 0, 999, true)].iter()), None);
    }

    #[test]
    fn tags_answer_before_the_rules_do() {
        let mut ap = Autoplay::default();
        ap.config.loot.rules = vec![LootRule::new("value>250", LootAction::Keep)];
        let ring = item("Ornate Ring", 900, 0);
        assert_eq!(ap.loot_action(&ring), LootAction::Keep);
        ap.tag(ring.guid, LootAction::Salvage);
        assert_eq!(ap.loot_action(&ring), LootAction::Salvage);
        assert_eq!(ap.tags().get(&ring.guid), Some(&LootAction::Salvage));
        assert_eq!(Doing::Salvaging.label(), "salvaging");
    }

    #[test]
    fn config_round_trips_through_json() {
        let mut c = Config {
            enabled: true,
            ..Config::default()
        };
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
#[cfg(test)]
mod heal_choice_tests {
    use super::bigger_heal;
    use ac_world::vitals::{transfers_between, vital, Transfer};

    #[test]
    fn the_bigger_heal_wins() {
        // Heal Self restores a fixed amount; the transfer takes half a
        // bar. On a full stamina bar the transfer is much the larger.
        assert_eq!(bigger_heal(Some((1, 100)), Some((2, 260))), Some((2, 260)));
        // Nearly out of stamina, it is worth almost nothing and loses.
        assert_eq!(bigger_heal(Some((1, 100)), Some((2, 12))), Some((1, 100)));
    }

    #[test]
    fn a_tie_goes_to_the_spell_that_costs_no_stamina() {
        assert_eq!(bigger_heal(Some((1, 100)), Some((2, 100))), Some((1, 100)));
    }

    #[test]
    fn either_may_be_missing() {
        assert_eq!(bigger_heal(Some((1, 40)), None), Some((1, 40)));
        assert_eq!(bigger_heal(None, Some((2, 40))), Some((2, 40)));
        assert_eq!(bigger_heal(None, None), None);
    }

    #[test]
    fn a_caster_really_does_know_stamina_to_health() {
        // The choice is worth nothing if the table has no such spell.
        let found: Vec<Transfer> = transfers_between(vital::STAMINA, vital::HEALTH);
        assert!(!found.is_empty(), "no stamina to health transfers");
        // The strongest moves half a bar, and on a big bar that beats
        // any fixed heal.
        let best = found.iter().map(|t| t.gain(700)).max().unwrap_or(0);
        assert!(best > 200, "the top transfer only returned {best}");
    }
}
#[cfg(test)]
mod loot_wait_tests {
    use super::{loot_wait, LOOT_TIMEOUT};

    #[test]
    fn a_corpse_underfoot_gets_the_plain_wait() {
        assert_eq!(loot_wait(0.0), LOOT_TIMEOUT);
        // A negative distance cannot happen, but must not panic or
        // shorten the wait.
        assert_eq!(loot_wait(-5.0), LOOT_TIMEOUT);
    }

    #[test]
    fn a_corpse_across_a_room_is_given_time_to_walk_to() {
        // The bug: a flat six seconds covered a corpse at our feet and
        // not one twenty metres off, so the far ones were written off
        // unopened.
        let near = loot_wait(2.0);
        let far = loot_wait(20.0);
        assert!(far > near, "{far:?} is not longer than {near:?}");
        assert!(far > LOOT_TIMEOUT * 2, "twenty metres barely added time");
    }

    #[test]
    fn the_wait_does_not_run_away_with_itself() {
        // Whatever distance arrives, the character does not sit on a
        // corpse for ever.
        let silly = loot_wait(100_000.0);
        assert!(silly <= LOOT_TIMEOUT + std::time::Duration::from_secs(30));
    }
}

#[cfg(test)]
mod loot_timing_tests {
    use super::{CORPSE_LIFE, CORPSE_URGENT};
    use std::time::Duration;

    #[test]
    fn a_corpse_lasts_five_minutes() {
        // ACE gives an unlooted monster corpse no timer until its first
        // heartbeat, when it takes the default of five minutes. That is
        // the whole window, so it is what the rules plan against.
        assert_eq!(CORPSE_LIFE, Duration::from_secs(300));
    }

    #[test]
    fn breaking_off_a_fight_is_reserved_for_a_corpse_about_to_go() {
        // Loot keeps for minutes; the thing hitting you does not. The
        // urgency window has to be small enough that a fight is not
        // interrupted for a corpse with plenty of time left, and big
        // enough to actually reach one.
        assert!(CORPSE_URGENT < CORPSE_LIFE / 3, "too eager to break off");
        assert!(
            CORPSE_URGENT >= Duration::from_secs(30),
            "no time to get there"
        );
    }
}
