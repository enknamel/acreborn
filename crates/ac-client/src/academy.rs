//! Getting a new character through the Training Academy.
//!
//! Every character ACE creates starts in the academy (landblock 0x8602)
//! and its exit portal wants the Sentry's quest done, so a character
//! played by the rules -- a follower made from the fleet panel, say --
//! would stand in the first room for ever. This rule runs whenever the
//! character is in that landblock: it walks the tutorial's steps
//! (`ac_world::academy`), talking to the agents, fighting what they ask
//! for, handing over what they want and walking into the portals, and
//! stops the moment the character is anywhere else.
//!
//! The decisions live in [`State`], a step machine fed a [`View`] of the
//! world each tick and answering with one [`Action`]; it touches no
//! client state, so the transitions are tested without a server. The
//! [`Client`] side gathers the view and carries the action out with the
//! ordinary means: a walk goal (`Client::follow`), `use_object`, `give`,
//! `pick_up`, and the fight rule pointed at the creatures a step names.
//!
//! What the agents say is the only word on the quests (the server keeps
//! the stamps to itself), so every talk listens: an agent's "already
//! done" line skips the rest of its quest, which is how a character
//! coming back from a death, or a restarted client, picks up where it
//! was. A portal that works is proof of the quest before it too.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use glam::Vec3;
use serde::{Deserialize, Serialize};

pub use ac_world::academy::{skip_steps, steps, Kind, Step, LANDBLOCK};

use crate::autoplay::Doing;
use crate::Client;

/// The settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Academy {
    /// Get out of the academy when the character is in it.
    pub enabled: bool,
    /// Take Jonathan's Exit Token, the way out the game itself offers:
    /// the same rewards in under a minute. Off, the tasks are done one
    /// by one. Either way the tasks are the fallback when Jonathan is
    /// not to be found.
    pub skip: bool,
}

impl Default for Academy {
    fn default() -> Self {
        Academy {
            enabled: true,
            skip: true,
        }
    }
}

/// Close enough to an agent, a door or a portal to use it.
const REACH: f32 = 3.5;
/// Close enough to an item on the floor to pick it up.
const PICKUP_REACH: f32 = 2.0;
/// A portal's landing spot counts as reached within this.
const LANDED: f32 = 12.0;
/// A jump of this much between two ticks, outside a portal, is a
/// teleport: a death took the character back to the start.
const TELEPORTED: f32 = 30.0;
/// An agent gets this long to answer a use or a gift.
const ANSWER_WAIT: Duration = Duration::from_secs(6);
/// An item an agent hands over is given this long to turn up.
const ITEM_WAIT: Duration = Duration::from_secs(5);
/// An agent still speaking refuses gifts ("AI refuse item during
/// emote"): a gift waits until its last line is this old.
const SPEECH_SETTLE: Duration = Duration::from_millis(1500);
/// After the Exit Token is handed back, the Free Ride takes a moment.
const RIDE_WAIT: Duration = Duration::from_secs(15);
/// After an agent's first line, its later lines are given this long.
const MORE_LINES: Duration = Duration::from_secs(3);
/// No progress on a walk for this long counts as stuck.
const STUCK_AFTER: Duration = Duration::from_secs(15);
/// Movement below this is not progress.
const PROGRESS: f32 = 0.5;
/// How often a portal is stepped into again.
const PORTAL_RETRY: Duration = Duration::from_secs(5);
/// A door is opened at most this often.
const DOOR_EVERY: Duration = Duration::from_secs(8);
/// Doors this near are opened on the way.
const DOOR_NEAR: f32 = 4.5;
/// Corpses this near are emptied while hunting.
const LOOT_RANGE: f32 = 20.0;
/// A corpse that does not open in this long is left.
const LOOT_TIMEOUT: Duration = Duration::from_secs(6);
/// The creatures a hunt goes after are looked for within this.
const HUNT_RADIUS: f32 = 45.0;
/// Asking, giving, picking up: tries before the step is let go.
const MAX_TRIES: u32 = 3;
/// Runs through the table before giving up.
const MAX_ROUNDS: u32 = 3;
/// A quest whose portal refuses is done again at most this often.
const MAX_QUEST_RETRIES: u32 = 2;

/// How long a step of each kind may take before it is let go.
fn limit(kind: Kind) -> Duration {
    Duration::from_secs(match kind {
        Kind::Talk | Kind::Give | Kind::Unlock => 45,
        Kind::Pickup => 30,
        Kind::Wear => 8,
        Kind::Hunt => 15 * 60,
        Kind::Portal => 90,
        Kind::Walk => 90,
    })
}

/// A line heard from an agent (or the server, with an empty sender).
#[derive(Clone, Debug)]
pub struct Heard {
    pub sender: String,
    pub text: String,
    pub when: Instant,
}

/// A world object as the machine sees it.
#[derive(Clone, Debug, Default)]
pub struct Seen {
    pub guid: u32,
    pub name: String,
    pub wcid: u32,
    /// World position.
    pub pos: Vec3,
    pub creature: bool,
    pub alive: bool,
    pub door: bool,
    pub portal: bool,
    pub corpse: bool,
}

/// A carried item as the machine sees it.
#[derive(Clone, Copy, Debug)]
pub struct Carried {
    pub guid: u32,
    pub wcid: u32,
    pub wielded: bool,
}

/// What the machine looks at each tick.
pub struct View<'a> {
    pub now: Instant,
    /// The character stands in the academy landblock.
    pub in_academy: bool,
    /// The character's world position.
    pub pos: Vec3,
    /// The academy landblock's origin (the table is local to it).
    pub origin: Vec3,
    pub dead: bool,
    /// Everything in the world nearby, carried things excepted.
    pub objects: &'a [Seen],
    pub carried: &'a [Carried],
}

/// What the client should do this tick.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Not in the academy (or the rule is off): the other rules run.
    Nothing,
    /// The character has just come out: the other rules take over.
    Left,
    /// Nothing to do this moment but wait, saying why.
    Wait(String),
    /// Walk to `target` and stop within `stop` of it.
    Approach {
        target: Vec3,
        stop: f32,
        status: String,
    },
    Use(u32),
    Give {
        npc: u32,
        item: u32,
    },
    PickUp(u32),
    Wield(u32),
    UseOn {
        item: u32,
        target: u32,
    },
    /// Fight creatures called `name` around `centre` and empty their
    /// corpses.
    Hunt {
        name: String,
        centre: Vec3,
    },
    /// The tutorial could not be finished; the status says so.
    GaveUp,
}

/// The step machine's state.
#[derive(Debug, Default)]
pub struct State {
    /// The tutorial is being walked.
    pub active: bool,
    /// The step being done.
    pub step: usize,
    /// Runs through the table so far.
    pub round: u32,
    /// Quests the agents said were done (or whose portal worked).
    pub done_quests: Vec<String>,
    /// Nothing more is tried.
    pub gave_up: bool,
    /// When the character entered the academy rule.
    pub since: Option<Instant>,
    /// The lines heard lately.
    heard: VecDeque<Heard>,
    /// When the current step began.
    started: Option<Instant>,
    /// When the step last acted (asked, gave, picked up...).
    acted: Option<Instant>,
    /// When the agent's first answer to the last ask came.
    first_line: Option<Instant>,
    tries: u32,
    /// A key was used on the door; the door is opened next.
    unlocked: bool,
    /// Where the walk last made progress, and when.
    progress: Option<(Vec3, Instant)>,
    /// The walk is not getting anywhere: act from here.
    stuck: bool,
    /// Where the character was last tick, for the teleport check.
    last_pos: Option<Vec3>,
    /// Retries of a quest whose portal refused, by quest name.
    quest_retries: Vec<(String, u32)>,
    /// The route being walked: false the full tutorial, true Jonathan's
    /// way out.
    skipping: bool,
}

impl State {
    /// Remember a line (called for every chat line; cheap).
    pub fn hear(&mut self, sender: &str, text: &str, now: Instant) {
        if !self.active {
            return;
        }
        self.heard.push_back(Heard {
            sender: sender.to_string(),
            text: text.to_string(),
            when: now,
        });
        while self.heard.len() > 40 {
            self.heard.pop_front();
        }
    }

    /// The steps of the route being walked.
    fn table(&self) -> &'static [Step] {
        if self.skipping {
            skip_steps()
        } else {
            steps()
        }
    }

    /// The step being done, if any.
    pub fn current(&self) -> Option<&'static Step> {
        self.table().get(self.step)
    }

    /// A few words on where the tutorial has got to, for the panel.
    pub fn progress(&self) -> String {
        match self.current() {
            Some(s) if self.active => {
                format!(
                    "step {}/{}: {:?} {}",
                    self.step + 1,
                    self.table().len(),
                    s.kind,
                    s.target
                )
            }
            _ => String::new(),
        }
    }

    fn reset_step(&mut self) {
        self.started = None;
        self.acted = None;
        self.first_line = None;
        self.tries = 0;
        self.unlocked = false;
        self.progress = None;
        self.stuck = false;
    }

    fn advance(&mut self) {
        self.step += 1;
        self.reset_step();
    }

    /// Start (or start over) at the first step; what is known done
    /// stays known.
    fn restart(&mut self) {
        self.step = 0;
        self.reset_step();
    }

    fn quest_done(&mut self, quest: &str) {
        if !self.done_quests.iter().any(|q| q == quest) {
            tracing::info!("academy: quest {quest} is done");
            self.done_quests.push(quest.to_string());
        }
    }

    fn is_done(&self, quest: &str) -> bool {
        self.done_quests.iter().any(|q| q == quest)
    }

    /// Lines from `who` (case-insensitively; "" for the server's own)
    /// heard since `since`.
    fn lines_from<'a>(
        &'a self,
        who: &'a str,
        since: Instant,
    ) -> impl Iterator<Item = &'a Heard> + 'a {
        self.heard
            .iter()
            .filter(move |h| h.when >= since && h.sender.eq_ignore_ascii_case(who))
    }

    fn heard_phrase(&self, who: &str, phrase: &str, since: Instant) -> bool {
        !phrase.is_empty()
            && self
                .lines_from(who, since)
                .any(|h| h.text.to_lowercase().contains(&phrase.to_lowercase()))
    }

    /// Walk toward `target`: `None` once within `stop` of it (or stuck
    /// and told to act from here), else the approach to make.
    fn approach(&mut self, v: &View, target: Vec3, stop: f32, status: String) -> Option<Action> {
        let flat = glam::Vec2::new(target.x - v.pos.x, target.y - v.pos.y).length();
        if flat <= stop || self.stuck {
            return None;
        }
        match self.progress {
            Some((p, since)) if p.distance(v.pos) < PROGRESS => {
                if v.now.duration_since(since) >= STUCK_AFTER {
                    tracing::info!(
                        "academy: stuck {flat:.0} m short of the goal, acting from here"
                    );
                    self.stuck = true;
                    return None;
                }
            }
            _ => self.progress = Some((v.pos, v.now)),
        }
        Some(Action::Approach {
            target,
            stop,
            status,
        })
    }

    /// Run the machine for this tick.
    pub fn step(&mut self, v: &View, skip: bool) -> Action {
        if !v.in_academy {
            if self.active {
                tracing::info!("academy: out of the academy after {} step(s)", self.step);
                *self = State::default();
                return Action::Left;
            }
            return Action::Nothing;
        }
        if !self.active {
            self.active = true;
            self.skipping = skip;
            self.since = Some(v.now);
            self.restart();
            tracing::info!(
                "academy: in the Training Academy, {}",
                if skip {
                    "taking the way out"
                } else {
                    "doing the tutorial"
                }
            );
        }
        if self.gave_up {
            return Action::GaveUp;
        }
        if v.dead {
            return Action::Wait("dead".into());
        }
        // A jump across the dungeon that no portal explains is a death
        // (the lifestone is the start room): begin again from there.
        let in_portal = self.current().is_some_and(|s| s.kind == Kind::Portal);
        if let Some(p) = self.last_pos {
            if p.distance(v.pos) > TELEPORTED && !in_portal {
                tracing::info!(
                    "academy: moved {:.0} m at once; starting over",
                    p.distance(v.pos)
                );
                self.restart();
            }
        }
        self.last_pos = Some(v.pos);
        // Find the step to do: a done quest's steps are skipped, its
        // portal excepted (it is the way on, and it works).
        loop {
            let Some(s) = self.current() else {
                if self.skipping {
                    // The token is handed back; the Free Ride follows.
                    let ended = *self.started.get_or_insert(v.now);
                    if v.now.duration_since(ended) < RIDE_WAIT {
                        return Action::Wait("waiting for the Free Ride".into());
                    }
                    tracing::info!("academy: Jonathan's way out did not work; doing the tasks");
                    self.skipping = false;
                    self.restart();
                    continue;
                }
                self.round += 1;
                if self.round >= MAX_ROUNDS {
                    tracing::warn!(
                        "academy: still inside after {} round(s); giving up",
                        self.round
                    );
                    self.gave_up = true;
                    return Action::GaveUp;
                }
                tracing::info!("academy: still inside; round {}", self.round + 1);
                self.restart();
                continue;
            };
            if self.is_done(&s.quest) && s.kind != Kind::Portal {
                self.advance();
                continue;
            }
            break;
        }
        let s = self.current().expect("a step");
        let started = *self.started.get_or_insert(v.now);
        if v.now.duration_since(started) > limit(s.kind) {
            tracing::info!(
                "academy: {:?} {} took too long; moving on",
                s.kind,
                s.target
            );
            self.advance();
            return Action::Wait("moving on".into());
        }
        match s.kind {
            Kind::Talk => self.talk(v, s),
            Kind::Give => self.give(v, s),
            Kind::Pickup => self.pickup(v, s),
            Kind::Wear => self.wear(v, s),
            Kind::Hunt => self.hunt(v, s),
            Kind::Unlock => self.unlock(v, s),
            Kind::Portal => self.portal(v, s),
            Kind::Walk => self.walk(v, s),
        }
    }

    /// The nearest object called `name` (creatures for agents).
    fn find<'a>(v: &'a View, name: &str, pick: impl Fn(&Seen) -> bool) -> Option<&'a Seen> {
        v.objects
            .iter()
            .filter(|o| o.name.eq_ignore_ascii_case(name) && pick(o))
            .min_by(|a, b| a.pos.distance(v.pos).total_cmp(&b.pos.distance(v.pos)))
    }

    fn carried(v: &View, wcid: u32) -> Option<Carried> {
        v.carried.iter().copied().find(|c| c.wcid == wcid)
    }

    /// Ask again, or let the step go after enough tries.
    fn retry(&mut self, what: &str) -> Action {
        self.tries += 1;
        if self.tries >= MAX_TRIES {
            tracing::info!(
                "academy: no luck {what} after {} tries; moving on",
                self.tries
            );
            self.advance();
        } else {
            self.acted = None;
            self.first_line = None;
        }
        Action::Wait(format!("trying {what} again"))
    }

    fn talk(&mut self, v: &View, s: &Step) -> Action {
        let Some(npc) = Self::find(v, &s.target, |o| o.creature) else {
            // Out of view: go where the table says it stands.
            if let Some(at) = s.at {
                if let Some(a) =
                    self.approach(v, v.origin + at, REACH, format!("going to {}", s.target))
                {
                    return a;
                }
            }
            if v.now.duration_since(self.started.unwrap_or(v.now)) > Duration::from_secs(10) {
                if self.skipping {
                    tracing::info!("academy: no {} in sight; doing the tasks instead", s.target);
                    self.skipping = false;
                    self.restart();
                } else {
                    tracing::info!("academy: no {} in sight; moving on", s.target);
                    self.advance();
                }
            }
            return Action::Wait(format!("looking for {}", s.target));
        };
        let Some(asked) = self.acted else {
            if let Some(a) = self.approach(v, npc.pos, REACH, format!("going to {}", s.target)) {
                return a;
            }
            self.acted = Some(v.now);
            return Action::Use(npc.guid);
        };
        if self.heard_phrase(&s.target, &s.done, asked) {
            let quest = s.quest.clone();
            self.quest_done(&quest);
            self.advance();
            return Action::Wait(format!("{} says the {quest} task is done", s.target));
        }
        let first = self.lines_from(&s.target, asked).map(|h| h.when).min();
        match first {
            Some(t) => {
                self.first_line.get_or_insert(t);
                // The open line is enough; otherwise give the rest of
                // what it has to say a moment, then take it as open.
                if self.heard_phrase(&s.target, &s.open, asked)
                    || v.now.duration_since(t) >= MORE_LINES
                {
                    self.advance();
                    return Action::Wait(format!("{} gave the task", s.target));
                }
                Action::Wait(format!("listening to {}", s.target))
            }
            None if v.now.duration_since(asked) > ANSWER_WAIT => {
                self.retry(&format!("talking to {}", s.target))
            }
            None => Action::Wait(format!("waiting for {} to answer", s.target)),
        }
    }

    fn give(&mut self, v: &View, s: &Step) -> Action {
        let Some(item) = Self::carried(v, s.wcid) else {
            if self.acted.is_some() {
                // Gone from the pack: taken.
                self.advance();
                return Action::Wait(format!("{} took it", s.target));
            }
            // An agent hands things over a moment after it speaks.
            if v.now.duration_since(self.started.unwrap_or(v.now)) < ITEM_WAIT {
                return Action::Wait(format!("waiting for the {} to turn up", s.target));
            }
            tracing::info!("academy: nothing to give {} (wcid {})", s.target, s.wcid);
            self.advance();
            return Action::Wait("nothing to give".into());
        };
        let Some(npc) = Self::find(v, &s.target, |o| o.creature) else {
            if let Some(at) = s.at {
                if let Some(a) =
                    self.approach(v, v.origin + at, REACH, format!("going to {}", s.target))
                {
                    return a;
                }
            }
            if v.now.duration_since(self.started.unwrap_or(v.now)) > Duration::from_secs(10) {
                self.advance();
            }
            return Action::Wait(format!("looking for {}", s.target));
        };
        let Some(gave) = self.acted else {
            if let Some(a) =
                self.approach(v, npc.pos, REACH, format!("bringing it to {}", s.target))
            {
                return a;
            }
            let lately = v.now.checked_sub(SPEECH_SETTLE).unwrap_or(v.now);
            let speaking = self.lines_from(&s.target, lately).next().is_some();
            if speaking {
                return Action::Wait(format!("letting {} finish", s.target));
            }
            self.acted = Some(v.now);
            return Action::Give {
                npc: npc.guid,
                item: item.guid,
            };
        };
        if self.heard_phrase(&s.target, &s.done, gave) {
            self.advance();
            return Action::Wait(format!("{} took it", s.target));
        }
        if v.now.duration_since(gave) > ANSWER_WAIT {
            return self.retry(&format!("giving to {}", s.target));
        }
        Action::Wait(format!("handing over to {}", s.target))
    }

    fn pickup(&mut self, v: &View, s: &Step) -> Action {
        if Self::carried(v, s.wcid).is_some() {
            self.advance();
            return Action::Wait(format!("picked up {}", s.target));
        }
        let item = v
            .objects
            .iter()
            .filter(|o| o.wcid == s.wcid)
            .min_by(|a, b| a.pos.distance(v.pos).total_cmp(&b.pos.distance(v.pos)));
        let Some(item) = item else {
            if v.now.duration_since(self.started.unwrap_or(v.now)) > Duration::from_secs(8) {
                tracing::info!("academy: no {} lying about; moving on", s.target);
                self.advance();
            }
            return Action::Wait(format!("looking for {}", s.target));
        };
        match self.acted {
            None => {
                if let Some(a) =
                    self.approach(v, item.pos, PICKUP_REACH, format!("fetching {}", s.target))
                {
                    return a;
                }
                self.acted = Some(v.now);
                Action::PickUp(item.guid)
            }
            Some(t) if v.now.duration_since(t) > Duration::from_secs(5) => {
                self.retry(&format!("picking up {}", s.target))
            }
            Some(_) => Action::Wait(format!("picking up {}", s.target)),
        }
    }

    fn wear(&mut self, v: &View, s: &Step) -> Action {
        let Some(item) = Self::carried(v, s.wcid) else {
            self.advance();
            return Action::Wait(format!("no {} to wear", s.target));
        };
        if item.wielded {
            self.advance();
            return Action::Wait(format!("wearing {}", s.target));
        }
        match self.acted {
            None => {
                self.acted = Some(v.now);
                Action::Wield(item.guid)
            }
            Some(t) if v.now.duration_since(t) > Duration::from_secs(4) => {
                self.retry(&format!("wearing {}", s.target))
            }
            Some(_) => Action::Wait(format!("putting on {}", s.target)),
        }
    }

    fn hunt(&mut self, v: &View, s: &Step) -> Action {
        if Self::carried(v, s.wcid).is_some() {
            tracing::info!("academy: got what the {} hunt was for", s.target);
            self.advance();
            return Action::Wait("got it".into());
        }
        Action::Hunt {
            name: s.target.clone(),
            centre: v.origin + s.at.unwrap_or_default(),
        }
    }

    fn unlock(&mut self, v: &View, s: &Step) -> Action {
        let spot = v.origin + s.at.unwrap_or_default();
        let door = Self::find(v, &s.target, |o| o.door).or_else(|| {
            v.objects
                .iter()
                .filter(|o| o.door && o.pos.distance(spot) < 6.0)
                .min_by(|a, b| a.pos.distance(spot).total_cmp(&b.pos.distance(spot)))
        });
        let Some(door) = door else {
            if let Some(a) = self.approach(v, spot, REACH, format!("going to the {}", s.target)) {
                return a;
            }
            tracing::info!("academy: no door {} here; moving on", s.target);
            self.advance();
            return Action::Wait("no door".into());
        };
        let Some(key) = Self::carried(v, s.wcid) else {
            tracing::info!("academy: no key for {}; trying the door as it is", s.target);
            self.unlocked = true;
            return self.open_door(v, door, s);
        };
        if let Some(a) = self.approach(v, door.pos, REACH, format!("unlocking the {}", s.target)) {
            return a;
        }
        if !self.unlocked {
            self.unlocked = true;
            self.acted = Some(v.now);
            return Action::UseOn {
                item: key.guid,
                target: door.guid,
            };
        }
        self.open_door(v, door, s)
    }

    fn open_door(&mut self, v: &View, door: &Seen, s: &Step) -> Action {
        // A moment after the key, the door; a moment after that, on.
        match self.acted {
            Some(t) if v.now.duration_since(t) < Duration::from_millis(1500) => {
                Action::Wait(format!("opening the {}", s.target))
            }
            _ if self.tries == 0 => {
                self.tries = 1;
                self.acted = Some(v.now);
                Action::Use(door.guid)
            }
            _ => {
                self.advance();
                Action::Wait(format!("through the {}", s.target))
            }
        }
    }

    fn portal(&mut self, v: &View, s: &Step) -> Action {
        let spot = v.origin + s.at.unwrap_or_default();
        // Landed on the far side: the quest before it is proven done.
        if let Some(l) = s.lands {
            if v.pos.distance(v.origin + l) < LANDED && self.acted.is_some() {
                if s.quest != "exit" {
                    let q = s.quest.clone();
                    self.quest_done(&q);
                }
                self.advance();
                return Action::Wait(format!("through the {} portal", s.target));
            }
        }
        // Refused: the quest is not done after all. Do it again.
        if let Some(t) = self.acted {
            if self.heard_phrase("", "complete quest to use portal", t)
                || self.heard_phrase("", "You must complete", t)
            {
                return self.quest_again(s);
            }
        }
        let portal = Self::find(v, &s.target, |o| o.portal).or_else(|| {
            v.objects
                .iter()
                .filter(|o| o.portal && o.pos.distance(spot) < 4.0)
                .min_by(|a, b| a.pos.distance(spot).total_cmp(&b.pos.distance(spot)))
        });
        let target = portal.map_or(spot, |p| p.pos);
        if let Some(a) = self.approach(
            v,
            target,
            1.5,
            format!("walking to the {} portal", s.target),
        ) {
            return a;
        }
        match (portal, self.acted) {
            (Some(p), None) => {
                self.acted = Some(v.now);
                Action::Use(p.guid)
            }
            (Some(p), Some(t)) if v.now.duration_since(t) > PORTAL_RETRY => {
                self.acted = Some(v.now);
                self.tries += 1;
                if self.tries > 6 {
                    return self.quest_again(s);
                }
                Action::Use(p.guid)
            }
            (None, _) => {
                self.acted.get_or_insert(v.now);
                Action::Wait(format!("standing in the {} portal", s.target))
            }
            _ => Action::Wait(format!("in the {} portal", s.target)),
        }
    }

    /// The portal refused: back to the first step of its quest, a
    /// couple of times.
    fn quest_again(&mut self, s: &Step) -> Action {
        let quest = s.quest.clone();
        let n = match self.quest_retries.iter_mut().find(|(q, _)| *q == quest) {
            Some((_, n)) => {
                *n += 1;
                *n
            }
            None => {
                self.quest_retries.push((quest.clone(), 1));
                1
            }
        };
        if n > MAX_QUEST_RETRIES {
            tracing::warn!("academy: the {} portal keeps refusing; giving up", s.target);
            self.gave_up = true;
            return Action::GaveUp;
        }
        tracing::info!(
            "academy: the {} portal refused; doing the {quest} tasks again",
            s.target
        );
        self.done_quests.retain(|q| *q != quest);
        let first = self
            .table()
            .iter()
            .position(|t| t.quest == quest)
            .unwrap_or(0);
        self.step = first;
        self.reset_step();
        Action::Wait(format!("doing the {quest} tasks again"))
    }

    fn walk(&mut self, v: &View, s: &Step) -> Action {
        let spot = v.origin + s.at.unwrap_or_default();
        if let Some(a) = self.approach(v, spot, PICKUP_REACH, format!("walking to {}", s.target)) {
            return a;
        }
        self.advance();
        Action::Wait(format!("at {}", s.target))
    }
}

impl Client {
    /// The academy rule: true while it has the character. Runs before
    /// following, fighting and growing, and only in the academy.
    pub(crate) fn autoplay_academy(&mut self, now: Instant) -> bool {
        let cfg = self.autoplay.config.academy.clone();
        if !cfg.enabled {
            return false;
        }
        let Some(pl) = self.player.as_ref() else {
            return false;
        };
        let in_academy = pl.landblock() == LANDBLOCK;
        if !in_academy && !self.autoplay.academy.active {
            return false;
        }
        let pos = pl.world_position();
        let me = self.world.player_guid;
        let objects: Vec<Seen> = self
            .world
            .objects
            .values()
            .filter(|o| Some(o.guid) != me && o.container.is_none() && o.wielder.is_none())
            .filter_map(|o| {
                use ac_world::{item_type, object_desc_flags as f};
                Some(Seen {
                    guid: o.guid,
                    name: o.name.clone(),
                    wcid: o.weenie_class_id,
                    pos: o.world_pos()?,
                    creature: o.item_type & item_type::CREATURE != 0 && !o.is_player,
                    alive: o.health.unwrap_or(1.0) > 0.0,
                    door: o.object_desc_flags & f::DOOR != 0,
                    portal: o.object_desc_flags & f::PORTAL != 0,
                    corpse: o.object_desc_flags & f::CORPSE != 0,
                })
            })
            .collect();
        let carried: Vec<Carried> = self
            .world
            .inventory()
            .chain(self.world.wielded())
            .map(|o| Carried {
                guid: o.guid,
                wcid: o.weenie_class_id,
                wielded: o.wielder.is_some(),
            })
            .collect();
        let view = View {
            now,
            in_academy,
            pos,
            origin: ac_world::landblock_origin(LANDBLOCK),
            dead: self.is_dead(),
            objects: &objects,
            carried: &carried,
        };
        let action = self.autoplay.academy.step(&view, cfg.skip);
        // A walk goal belongs to the step that set it.
        if !matches!(action, Action::Approach { .. } | Action::Hunt { .. })
            && self.follow.take().is_some()
        {
            self.steering.reset();
        }
        let progress = self.autoplay.academy.progress();
        match action {
            Action::Nothing => false,
            Action::Left => {
                self.attack_target = None;
                self.autoplay.drop_target();
                self.autoplay
                    .say(Doing::Training, "out of the Training Academy");
                false
            }
            Action::GaveUp => {
                self.autoplay
                    .say(Doing::Training, "could not finish the Training Academy");
                true
            }
            Action::Wait(why) => {
                self.autoplay
                    .say(Doing::Training, format!("{progress}: {why}"));
                true
            }
            Action::Approach {
                target,
                stop,
                status,
            } => {
                self.academy_leave_fight();
                self.academy_open_doors(&objects, pos, now);
                self.head_for(target, stop, "the way on");
                self.autoplay
                    .say(Doing::Training, format!("{progress}: {status}"));
                true
            }
            Action::Use(guid) => {
                self.academy_leave_fight();
                self.academy_peace();
                self.use_object(guid);
                self.autoplay
                    .say(Doing::Training, format!("{progress}: using it"));
                true
            }
            Action::Give { npc, item } => {
                self.academy_leave_fight();
                self.academy_peace();
                if !self.give(npc, item, None) {
                    tracing::info!("academy: could not give {item:#010x} to {npc:#010x}");
                }
                self.autoplay
                    .say(Doing::Training, format!("{progress}: giving"));
                true
            }
            Action::PickUp(guid) => {
                self.academy_peace();
                self.pick_up(guid);
                self.autoplay
                    .say(Doing::Training, format!("{progress}: picking up"));
                true
            }
            Action::Wield(guid) => {
                self.wield_guid(guid);
                self.autoplay
                    .say(Doing::Training, format!("{progress}: wearing"));
                true
            }
            Action::UseOn { item, target } => {
                self.academy_peace();
                self.use_on(item, target);
                self.autoplay
                    .say(Doing::Training, format!("{progress}: unlocking"));
                true
            }
            Action::Hunt { name, centre } => {
                self.academy_hunt(&name, centre, &objects, pos, now, &progress);
                true
            }
        }
    }

    /// How a new character fights: with the bow when Missile Weapons is
    /// trained and a bow is carried, with spells when War Magic is and
    /// an attack spell is known, else hand to hand.
    fn academy_style(&self) -> crate::autoplay::Style {
        use crate::autoplay::Style;
        use ac_world::stats::{sac, skill};
        let trained = |id: u32| {
            self.world
                .stats
                .skill(id)
                .is_some_and(|s| s.advancement >= sac::TRAINED)
        };
        // In the pack or already in hand: a wielded bow is not in the
        // pack, and forgetting it swapped the bow for the knife and back
        // every few seconds.
        let carries = |mask: u32| {
            self.world
                .inventory()
                .chain(self.world.wielded())
                .any(|o| o.item_type & mask != 0)
        };
        if trained(skill::MISSILE_WEAPONS) && carries(ac_world::item_type::MISSILE_WEAPON) {
            Style::Missile
        } else if trained(skill::WAR_MAGIC)
            && carries(ac_world::item_type::CASTER)
            && !self.attack_spells_known().is_empty()
        {
            Style::Magic
        } else {
            Style::Melee
        }
    }

    /// Put the weapon for `style` in hand (and arrows on the bow),
    /// dropping to peace mode to do it. True while that is under way.
    fn academy_arm(&mut self, style: crate::autoplay::Style, now: Instant) -> bool {
        use crate::autoplay::Style;
        use crate::Stance;
        let want = match style {
            Style::Missile => Stance::Missile,
            Style::Magic => Stance::Magic,
            Style::Melee | Style::Auto => Stance::Melee,
        };
        let mask = match want {
            Stance::Magic => ac_world::item_type::CASTER,
            Stance::Missile => ac_world::item_type::MISSILE_WEAPON,
            Stance::Melee => ac_world::item_type::MELEE_WEAPON,
        };
        let carried = self.world.inventory().chain(self.world.wielded()).any(|o| {
            o.item_type & mask != 0 && o.valid_locations & ac_world::equip::MISSILE_AMMO == 0
        });
        // A wield takes the server a couple of seconds (the animation
        // plays first) and it queues what comes in meanwhile: a second
        // ask sent too soon put the bow straight back in the pack.
        let due = self
            .autoplay
            .academy_armed
            .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(4));
        if self.combat_stance() != want && carried {
            if due {
                let hands: Vec<String> = self
                    .world
                    .wielded()
                    .map(|o| {
                        format!(
                            "{} type {:#x} at {:#x} of {:#x}",
                            o.name, o.item_type, o.wielded_location, o.valid_locations
                        )
                    })
                    .collect();
                tracing::debug!(
                    "academy: hands give {:?}, want {:?}: {}",
                    self.combat_stance(),
                    want,
                    hands.join(", ")
                );
                self.autoplay.academy_armed = Some(now);
                if self.combat || self.magic {
                    self.leave_combat();
                }
                self.wield_for(want);
            }
            return true;
        }
        if want == Stance::Missile && self.wielded_ammo().is_none() {
            if due {
                self.autoplay.academy_armed = Some(now);
                if !self.wield_ammo() {
                    return false;
                }
            }
            return true;
        }
        false
    }

    /// Let go of whatever the fight rule was on.
    fn academy_leave_fight(&mut self) {
        if self.attack_target.take().is_some() {
            tracing::info!("academy: leaving the fight");
        }
        self.autoplay.drop_target();
    }

    /// Out of combat mode, to use, give and pick up.
    fn academy_peace(&mut self) {
        if self.combat {
            self.toggle_combat();
        }
    }

    /// Open any door close by on the way (once in a while each).
    fn academy_open_doors(&mut self, objects: &[Seen], pos: Vec3, now: Instant) {
        let doors: Vec<u32> = objects
            .iter()
            .filter(|o| o.door && o.pos.distance(pos) < DOOR_NEAR)
            .map(|o| o.guid)
            .collect();
        for guid in doors {
            let recently = self
                .autoplay
                .academy_doors
                .iter()
                .any(|(g, t)| *g == guid && now.duration_since(*t) < DOOR_EVERY);
            if !recently {
                self.autoplay
                    .academy_doors
                    .retain(|(_, t)| now.duration_since(*t) < DOOR_EVERY);
                self.autoplay.academy_doors.push((guid, now));
                self.use_object(guid);
            }
        }
    }

    /// Fight the creatures called `name` around `centre` and empty their
    /// corpses: the fight rule pointed at them, a small loot of its own
    /// (the tutorial drops are one item each), and a walk to the spot
    /// when nothing is in sight.
    fn academy_hunt(
        &mut self,
        name: &str,
        centre: Vec3,
        objects: &[Seen],
        pos: Vec3,
        now: Instant,
        progress: &str,
    ) {
        // A corpse being emptied.
        if let Some((guid, since)) = self.autoplay.academy_corpse {
            if now.duration_since(since) > LOOT_TIMEOUT {
                tracing::info!("academy: corpse {guid:#010x} did not open");
                self.autoplay.academy_corpse = None;
                self.autoplay.looted.push(guid);
            } else {
                match self.world.open_container.clone() {
                    Some((open, items)) if open == guid => {
                        // Everything on it is taken, one at a time (the
                        // server allows one pickup in flight), and the
                        // corpse stays open until the last has landed:
                        // closed early, the server answers "Source
                        // item not found".
                        let waiting: Vec<u32> = items
                            .iter()
                            .copied()
                            .filter(|g| {
                                self.world
                                    .objects
                                    .get(g)
                                    .is_some_and(|o| o.container == Some(open))
                            })
                            .collect();
                        if waiting.is_empty() {
                            self.close_container();
                            self.autoplay.looted.push(guid);
                            self.autoplay.academy_corpse = None;
                            self.autoplay
                                .say(Doing::Training, format!("{progress}: emptied a corpse"));
                        } else {
                            for g in waiting {
                                self.take(g);
                            }
                            self.autoplay.say(
                                Doing::Training,
                                format!("{progress}: taking what is on a corpse"),
                            );
                        }
                    }
                    _ => {
                        self.autoplay
                            .say(Doing::Training, format!("{progress}: opening a corpse"));
                    }
                }
                return;
            }
        }
        // A corpse of one of them nearby, not yet emptied.
        let fighting = self
            .attack_target
            .and_then(|g| self.world.objects.get(&g))
            .is_some_and(|o| o.health.unwrap_or(1.0) > 0.0);
        if !fighting {
            let corpse = objects
                .iter()
                .filter(|o| o.corpse && !self.autoplay.looted.contains(&o.guid))
                .filter(|o| o.name.to_lowercase().contains(&name.to_lowercase()))
                .filter(|o| o.pos.distance(pos) <= LOOT_RANGE)
                .min_by(|a, b| a.pos.distance(pos).total_cmp(&b.pos.distance(pos)));
            if let Some(c) = corpse {
                let (guid, cname) = (c.guid, c.name.clone());
                self.academy_peace();
                self.autoplay.drop_target();
                if self.follow.take().is_some() {
                    self.steering.reset();
                }
                self.interact(guid);
                self.autoplay.academy_corpse = Some((guid, now));
                self.autoplay
                    .say(Doing::Training, format!("{progress}: looting {cname}"));
                return;
            }
        }
        // The weapon the character was made for goes in hand first,
        // in peace mode: the general weapon picker judges by appraisals
        // a new character has not made (it reached for the Training
        // Wand of a Bow Hunter), and a wield asked for mid-swing is
        // refused.
        let style = self.academy_style();
        if self.academy_arm(style, now) {
            self.autoplay
                .say(Doing::Training, format!("{progress}: arming"));
            return;
        }
        let mut fight = self.autoplay.config.fight.clone();
        fight.enabled = true;
        fight.only = vec![name.to_string()];
        fight.avoid.clear();
        fight.radius = HUNT_RADIUS;
        fight.pick_weapon = false;
        fight.style = style;
        if self.autoplay_fight_as(now, &fight) {
            return;
        }
        // Nothing to fight: to the spot, then wait for them to appear.
        let flat = glam::Vec2::new(centre.x - pos.x, centre.y - pos.y).length();
        if flat > REACH {
            self.academy_open_doors(objects, pos, now);
            self.head_for(centre, REACH, name);
            self.autoplay
                .say(Doing::Training, format!("{progress}: going after {name}"));
        } else {
            if self.follow.take().is_some() {
                self.steering.reset();
            }
            self.autoplay.say(
                Doing::Training,
                format!("{progress}: waiting for {name} to appear"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> Vec3 {
        ac_world::landblock_origin(LANDBLOCK)
    }

    fn npc(name: &str, at: Vec3) -> Seen {
        Seen {
            guid: 0x8000_0001 + name.len() as u32,
            name: name.into(),
            creature: true,
            alive: true,
            pos: origin() + at,
            ..Default::default()
        }
    }

    fn view<'a>(
        now: Instant,
        pos_local: Vec3,
        objects: &'a [Seen],
        carried: &'a [Carried],
    ) -> View<'a> {
        View {
            now,
            in_academy: true,
            pos: origin() + pos_local,
            origin: origin(),
            dead: false,
            objects,
            carried,
        }
    }

    fn step_named(st: &State, target: &str, kind: Kind) -> bool {
        st.current()
            .is_some_and(|s| s.target == target && s.kind == kind)
    }

    #[test]
    fn outside_the_academy_nothing_happens() {
        let mut st = State::default();
        let v = View {
            now: Instant::now(),
            in_academy: false,
            pos: Vec3::ZERO,
            origin: origin(),
            dead: false,
            objects: &[],
            carried: &[],
        };
        assert_eq!(st.step(&v, false), Action::Nothing);
        assert!(!st.active);
    }

    #[test]
    fn a_gift_with_nothing_carried_is_skipped_and_a_talk_walks_up() {
        let t0 = Instant::now();
        let mut st = State::default();
        let samuel = npc("Samuel", Vec3::new(21.9, -30.6, 0.0));
        let objs = vec![samuel.clone()];
        // First step: give the Calling Stone; not carried -> skipped
        // once it has had a moment to turn up.
        let v = view(t0, Vec3::new(12.3, -28.5, 0.0), &objs, &[]);
        let a = st.step(&v, false);
        assert!(matches!(a, Action::Wait(_)), "{a:?}");
        let t0 = t0 + ITEM_WAIT + Duration::from_secs(1);
        let v = view(t0, Vec3::new(12.3, -28.5, 0.0), &objs, &[]);
        st.step(&v, false);
        assert!(step_named(&st, "Samuel", Kind::Talk), "{:?}", st.current());
        // Samuel is 10 m off: walk there.
        let a = st.step(&v, false);
        match a {
            Action::Approach { target, stop, .. } => {
                assert_eq!(target, samuel.pos);
                assert_eq!(stop, REACH);
            }
            other => panic!("{other:?}"),
        }
        // Within reach: use him.
        let v = view(t0, Vec3::new(20.0, -30.0, 0.0), &objs, &[]);
        assert_eq!(st.step(&v, false), Action::Use(samuel.guid));
        // He says the task is open: on to the armour.
        st.hear(
            "Samuel",
            "Looks like you need some armor! There are 3 different pieces...",
            t0 + Duration::from_millis(300),
        );
        let v = view(
            t0 + Duration::from_millis(400),
            Vec3::new(20.0, -30.0, 0.0),
            &objs,
            &[],
        );
        let a = st.step(&v, false);
        assert!(matches!(a, Action::Wait(_)), "{a:?}");
        assert!(
            step_named(&st, "Leather Gauntlets", Kind::Pickup),
            "{:?}",
            st.current()
        );
        assert!(!st.is_done("armor"));
    }

    #[test]
    fn an_agents_done_line_skips_its_quest() {
        let t0 = Instant::now();
        let mut st = State::default();
        let samuel = npc("Samuel", Vec3::new(21.9, -30.6, 0.0));
        let objs = vec![samuel.clone()];
        let here = Vec3::new(20.0, -30.0, 0.0);
        let v = view(t0, here, &objs, &[]);
        st.step(&v, false);
        let t0 = t0 + ITEM_WAIT + Duration::from_secs(1);
        let v = view(t0, here, &objs, &[]);
        st.step(&v, false); // give skipped
        assert_eq!(st.step(&v, false), Action::Use(samuel.guid));
        st.hear(
            "Samuel",
            "You found some armor! ...",
            t0 + Duration::from_millis(200),
        );
        st.hear(
            "Samuel",
            "You may now proceed to the Training Area.",
            t0 + Duration::from_millis(400),
        );
        let v = view(t0 + Duration::from_millis(500), here, &objs, &[]);
        st.step(&v, false);
        assert!(st.is_done("armor"));
        // Straight on to the Training Master, past the armour steps.
        st.step(&v, false);
        assert!(
            step_named(&st, "Training Master", Kind::Talk),
            "{:?}",
            st.current()
        );
    }

    #[test]
    fn a_hunt_ends_when_the_item_is_carried_and_the_gift_goes_out() {
        let t0 = Instant::now();
        // Jump to the golem hunt.
        let mut st = State {
            active: true,
            since: Some(t0),
            step: steps().iter().position(|s| s.kind == Kind::Hunt).unwrap(),
            ..Default::default()
        };
        let master = npc("Training Master", Vec3::new(56.1, -20.1, 0.0));
        let objs = vec![master.clone()];
        let here = Vec3::new(58.0, -20.0, 0.0);
        let v = view(t0, here, &objs, &[]);
        match st.step(&v, false) {
            Action::Hunt { name, centre } => {
                assert_eq!(name, "Sparring Golem");
                assert_eq!(centre, origin() + Vec3::new(78.0, -20.0, 0.0));
            }
            other => panic!("{other:?}"),
        }
        // The token turns up in the pack: hunt over, give it.
        let token = [Carried {
            guid: 0x9000_0001,
            wcid: 12709,
            wielded: false,
        }];
        let v = view(t0 + Duration::from_secs(1), here, &objs, &token);
        st.step(&v, false);
        assert!(step_named(&st, "Training Master", Kind::Give));
        assert_eq!(
            st.step(&v, false),
            Action::Give {
                npc: master.guid,
                item: 0x9000_0001
            }
        );
        // Taken: the token is gone from the pack.
        let v = view(t0 + Duration::from_secs(2), here, &objs, &[]);
        st.step(&v, false);
        assert!(
            step_named(&st, "Central Courtyard", Kind::Portal),
            "{:?}",
            st.current()
        );
    }

    #[test]
    fn a_portal_that_lands_proves_its_quest_and_a_refusal_redoes_it() {
        let t0 = Instant::now();
        let idx = steps()
            .iter()
            .position(|s| s.kind == Kind::Portal && s.target == "Central Courtyard")
            .unwrap();
        let mut st = State {
            active: true,
            since: Some(t0),
            step: idx,
            ..Default::default()
        };
        let portal = Seen {
            guid: 0x7000_0001,
            name: "Central Courtyard".into(),
            portal: true,
            pos: origin() + Vec3::new(70.0, -40.0, -0.1),
            ..Default::default()
        };
        let objs = vec![portal.clone()];
        let at_portal = Vec3::new(70.0, -40.5, 0.0);
        let v = view(t0, at_portal, &objs, &[]);
        assert_eq!(st.step(&v, false), Action::Use(portal.guid));
        st.last_pos = Some(v.pos);
        // Landed by the linkspot: the token quest is proven, on to the Foreman.
        let v = view(
            t0 + Duration::from_secs(2),
            Vec3::new(50.0, -54.0, 1.0),
            &objs,
            &[],
        );
        st.step(&v, false);
        assert!(st.is_done("token"));
        assert!(
            step_named(&st, "Academy Foreman", Kind::Talk),
            "{:?}",
            st.current()
        );

        // The same portal refusing sends the machine back to the quest's start.
        let mut st = State {
            active: true,
            since: Some(t0),
            step: idx,
            done_quests: vec!["token".into()],
            ..Default::default()
        };
        let v = view(t0, at_portal, &objs, &[]);
        assert_eq!(st.step(&v, false), Action::Use(portal.guid));
        st.hear(
            "",
            "You must complete quest to use portal",
            t0 + Duration::from_millis(500),
        );
        let v = view(t0 + Duration::from_secs(1), at_portal, &objs, &[]);
        st.step(&v, false);
        assert!(!st.is_done("token"));
        assert!(
            step_named(&st, "Training Master", Kind::Talk),
            "{:?}",
            st.current()
        );
    }

    #[test]
    fn a_teleport_starts_over_but_keeps_what_is_known() {
        let t0 = Instant::now();
        let mut st = State {
            active: true,
            since: Some(t0),
            done_quests: vec!["armor".into(), "token".into(), "wasp".into()],
            step: steps()
                .iter()
                .position(|s| s.kind == Kind::Hunt && s.target == "Olthoi")
                .unwrap(),
            ..Default::default()
        };
        let v = view(t0, Vec3::new(150.0, -230.0, -12.0), &[], &[]);
        assert!(matches!(st.step(&v, false), Action::Hunt { .. }));
        // Dead, then back in the first room.
        let mut v = view(
            t0 + Duration::from_secs(5),
            Vec3::new(150.0, -230.0, -12.0),
            &[],
            &[],
        );
        v.dead = true;
        assert_eq!(st.step(&v, false), Action::Wait("dead".into()));
        for i in 0..6 {
            let t = t0 + Duration::from_secs(20 + 6 * i);
            let v = view(t, Vec3::new(12.3, -28.5, 0.0), &[], &[]);
            st.step(&v, false);
        }
        // The done quests are skipped, their portal excepted: it is the
        // way back to the courtyard.
        assert!(st.is_done("armor") && st.is_done("token"));
        assert!(
            step_named(&st, "Central Courtyard", Kind::Portal),
            "{:?}",
            st.current()
        );
    }

    #[test]
    fn leaving_the_landblock_finishes() {
        let t0 = Instant::now();
        let mut st = State::default();
        let v = view(t0, Vec3::new(12.3, -28.5, 0.0), &[], &[]);
        st.step(&v, false);
        assert!(st.active);
        let mut v = view(
            t0 + Duration::from_secs(1),
            Vec3::new(12.3, -28.5, 0.0),
            &[],
            &[],
        );
        v.in_academy = false;
        assert_eq!(st.step(&v, false), Action::Left);
        assert!(!st.active);
        assert_eq!(st.step(&v, false), Action::Nothing);
    }

    #[test]
    fn the_skip_route_talks_to_jonathan_and_gives_the_token_back() {
        let t0 = Instant::now();
        let mut st = State::default();
        let jonathan = npc("Jonathan", Vec3::new(22.1, -19.1, 0.0));
        let objs = vec![jonathan.clone()];
        let here = Vec3::new(21.0, -20.0, 0.0);
        let v = view(t0, here, &objs, &[]);
        assert_eq!(st.step(&v, true), Action::Use(jonathan.guid));
        st.hear("Jonathan", "If you want to skip your training and leave the Academy early, give this token back to me.", t0 + Duration::from_millis(300));
        let token = [Carried {
            guid: 0x9000_0002,
            wcid: 29335,
            wielded: false,
        }];
        let v = view(t0 + Duration::from_millis(600), here, &objs, &token);
        st.step(&v, true);
        // He is still speaking: the gift waits for him to finish.
        assert!(matches!(st.step(&v, true), Action::Wait(_)));
        let v = view(t0 + Duration::from_secs(3), here, &objs, &token);
        assert_eq!(
            st.step(&v, true),
            Action::Give {
                npc: jonathan.guid,
                item: 0x9000_0002
            }
        );
        // Taken: the ride is waited for before the tasks are fallen back on.
        let v = view(t0 + Duration::from_secs(4), here, &objs, &[]);
        st.step(&v, true);
        assert_eq!(
            st.step(&v, true),
            Action::Wait("waiting for the Free Ride".into())
        );
        let v = view(
            t0 + Duration::from_secs(4) + RIDE_WAIT + Duration::from_secs(1),
            here,
            &objs,
            &[],
        );
        st.step(&v, true);
        assert!(!st.skipping);
        assert_eq!(st.step, 0);
        // Out of the landblock: done.
        let mut v = view(t0 + Duration::from_secs(30), here, &objs, &[]);
        v.in_academy = false;
        assert_eq!(st.step(&v, true), Action::Left);
    }

    #[test]
    fn without_jonathan_in_view_the_walk_goes_to_his_room_first() {
        let t0 = Instant::now();
        let mut st = State::default();
        // Standing down the corridor, nobody in view: walk to where the
        // table puts him.
        let far = Vec3::new(45.0, -20.0, 0.0);
        let v = view(t0, far, &[], &[]);
        match st.step(&v, true) {
            Action::Approach { target, .. } => {
                assert_eq!(target, origin() + Vec3::new(22.1, -19.1, 0.0));
            }
            other => panic!("{other:?}"),
        }
        // There, and still nobody after a while: the tasks instead.
        let v = view(
            t0 + Duration::from_secs(11),
            Vec3::new(21.0, -20.0, 0.0),
            &[],
            &[],
        );
        st.step(&v, true);
        assert!(!st.skipping);
        assert!(
            step_named(&st, "Society Greeter", Kind::Give),
            "{:?}",
            st.current()
        );
    }

    #[test]
    fn a_stuck_walk_acts_from_where_it_is() {
        let t0 = Instant::now();
        let mut st = State::default();
        let samuel = npc("Samuel", Vec3::new(21.9, -30.6, 0.0));
        let objs = vec![samuel.clone()];
        let here = Vec3::new(5.0, -30.0, 0.0);
        let v = view(t0, here, &objs, &[]);
        st.step(&v, false);
        let t0 = t0 + ITEM_WAIT + Duration::from_secs(1);
        let v = view(t0, here, &objs, &[]);
        st.step(&v, false); // give skipped
        assert!(matches!(st.step(&v, false), Action::Approach { .. }));
        // Not a step of progress for a good while: use him from here.
        let v = view(t0 + STUCK_AFTER + Duration::from_secs(1), here, &objs, &[]);
        assert_eq!(st.step(&v, false), Action::Use(samuel.guid));
    }
}
