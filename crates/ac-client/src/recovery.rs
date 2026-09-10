//! Coming back from a death.
//!
//! A character that dies wakes at its lifestone with its buffs purged, a
//! vitae penalty, and its best gear lying on a corpse where it fell.
//! Left to the other rules it would stand there, or worse, walk off to
//! hunt with nothing in its hands. This module is the rule that runs
//! first after "stay alive": it notices the death, waits out the
//! respawn, puts the buffs back, walks to the corpse, empties it, wields
//! what was wielded, and hands the character back to whatever it was
//! doing.
//!
//! The decisions live in [`Recovery`], a state machine fed a [`View`] of
//! the world each tick and answering with one [`Action`]; it touches no
//! client state, so the transitions are tested without a server. The
//! [`Client`] side gathers the view and carries out the action.
//!
//! Two things are given up on rather than waited for forever: a corpse
//! that cannot be reached within `Survive::corpse_minutes`, and one that
//! has killed the character again on the way back. Vitae is not waited
//! out (it wears off with experience, not time); instead, while it is
//! over `Survive::vitae_above` the fight rules leave the hard targets
//! and the killer alone (see [`Client::shy_of`]).

use std::time::{Duration, Instant};

use glam::Vec2;

use crate::autoplay::Doing;
use crate::Client;

/// A lifestone further than this from the death spot means the
/// teleport has happened.
const TELEPORTED: f32 = 30.0;
/// Alive again for this long counts as landed even without a move: a
/// lifestone can stand within sight of where the character died.
const ALIVE_SETTLE: Duration = Duration::from_secs(8);
/// Time given the sheet, the purge and the vitae to arrive after landing.
const LANDED_SETTLE: Duration = Duration::from_secs(3);
/// Buffing before the walk back is given at most this long.
const REBUFF_CAP: Duration = Duration::from_secs(120);
/// Journeys back are planned at most this often.
const PLAN_EVERY: Duration = Duration::from_secs(3);
/// Standing this near the death spot counts as arrived.
const NEAR: f32 = 12.0;
/// A corpse within this can be opened (the server walks us the rest).
const OPEN_RANGE: f32 = 20.0;
/// The corpse is asked to open at most this often.
const OPEN_EVERY: Duration = Duration::from_secs(4);
/// Standing at the death spot without the corpse in sight for this long
/// means it is gone.
const CORPSE_WAIT: Duration = Duration::from_secs(15);
/// A corpse that was in view and has vanished since is gone for good
/// (ACE destroys an emptied player corpse on the spot): this is all
/// the grace it gets.
const CORPSE_GONE: Duration = Duration::from_secs(3);
/// The hands are given this long to take the gear back up.
const REWIELD_CAP: Duration = Duration::from_secs(20);
/// One wield order at a time, this far apart.
const WIELD_EVERY: Duration = Duration::from_millis(1000);
/// Journeys back that end short of the spot before giving up.
const MAX_REPLANS: u32 = 4;
/// Dying this many times over one corpse means it is not worth it.
const MAX_DEATHS: u32 = 2;

/// Where the recovery has got to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// Alive and not recovering: the other rules run.
    #[default]
    None,
    /// Health is at zero; waiting for the lifestone.
    Dead,
    /// Alive again, letting the respawn settle.
    Landed,
    /// Putting the buffs back before setting off.
    Rebuff,
    /// On the way back to the death spot.
    Returning,
    /// At the spot, opening and emptying the corpse.
    Looting,
    /// Taking the gear up again.
    Rewielding,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::None => "alive",
            Phase::Dead => "dead",
            Phase::Landed => "back at the lifestone",
            Phase::Rebuff => "putting the buffs back",
            Phase::Returning => "going back for the corpse",
            Phase::Looting => "emptying the corpse",
            Phase::Rewielding => "taking the gear up again",
        }
    }
}

/// What the world looks like this tick, as far as the recovery cares.
#[derive(Clone, Debug)]
pub struct View {
    pub now: Instant,
    /// Health at zero, with a character sheet to say so.
    pub dead: bool,
    /// Where the character stands (world xy) and its landblock.
    pub xy: Vec2,
    pub landblock: u32,
    /// Our own corpse in view: its guid and how far off it is.
    pub corpse: Option<(u32, f32)>,
    /// A journey is under way.
    pub traveling: bool,
    /// The container open right now, if any.
    pub open: Option<u32>,
    /// Of the items that were wielded at death, the ones now carried in
    /// a pack rather than wielded.
    pub carried: Vec<u32>,
    /// Everything wielded right now, for remembering at death.
    pub wielded: Vec<u32>,
    /// Where a journey was bound, for picking it up again.
    pub trip: Option<Vec2>,
    /// What we were fighting, by name.
    pub fighting: Option<String>,
    /// Whether the corpse is to be gone back for at all.
    pub recover: bool,
    /// How long the walk back may take before giving up.
    pub limit: Duration,
}

/// The one thing to do this tick.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Not recovering: the rest of the rules run.
    Nothing,
    /// Hold the other rules and wait.
    Wait,
    /// Run the buff rule (the driver reports back when nothing is due).
    Buff,
    /// Plan and start a journey to the spot.
    Travel(Vec2),
    /// Open this corpse.
    Open(u32),
    /// Take everything out of this open corpse and close it.
    Empty(u32),
    /// Wield this item.
    Wield(u32),
    /// Done: hand back to the rules, resuming this journey if any.
    Finish { trip: Option<Vec2> },
}

/// The recovery in progress.
#[derive(Clone, Debug)]
pub struct Recovery {
    pub phase: Phase,
    /// When the phase began.
    pub since: Instant,
    /// Where the character died (world xy) and in which landblock.
    pub death_xy: Option<Vec2>,
    pub death_block: u32,
    /// The corpse being gone back for, once seen.
    pub corpse: Option<u32>,
    /// What was in the hands at death.
    pub wielded: Vec<u32>,
    /// Where the journey was bound at death.
    pub trip: Option<Vec2>,
    /// What killed us, by name (what we were fighting).
    pub killer: Option<String>,
    /// Deaths since the recovery began, this one included.
    pub deaths: u32,
    /// When the walk back began: the limit counts from here, buffing
    /// not included.
    started: Option<Instant>,
    /// Alive again since (while `Dead`).
    alive_since: Option<Instant>,
    /// The buff rule found nothing more to cast.
    buffs_done: bool,
    /// Journeys back planned so far, and when the last was.
    replans: u32,
    last_plan: Option<Instant>,
    /// When the corpse was last asked to open.
    last_open: Option<Instant>,
    /// When a wield order last went out.
    last_wield: Option<Instant>,
    /// Corpse abandoned: the walk back finishes without it.
    abandoned: bool,
}

impl Default for Recovery {
    fn default() -> Self {
        Recovery {
            phase: Phase::None,
            since: Instant::now(),
            death_xy: None,
            death_block: 0,
            corpse: None,
            wielded: Vec::new(),
            trip: None,
            killer: None,
            deaths: 0,
            started: None,
            alive_since: None,
            buffs_done: false,
            replans: 0,
            last_plan: None,
            last_open: None,
            last_wield: None,
            abandoned: false,
        }
    }
}

impl Recovery {
    /// Whether a recovery is under way.
    pub fn active(&self) -> bool {
        self.phase != Phase::None
    }

    fn go(&mut self, phase: Phase, now: Instant) {
        if self.phase != phase {
            tracing::info!("recovery: {}", phase.label());
        }
        self.phase = phase;
        self.since = now;
    }

    /// The driver found nothing more worth buffing (or could not).
    pub fn buffs_done(&mut self) {
        self.buffs_done = true;
    }

    /// The driver could plan no way to the spot.
    pub fn no_way(&mut self) {
        self.abandoned = true;
    }

    /// Note a death: where, with what in hand, bound where, fighting
    /// what. A death in the middle of a recovery keeps the first
    /// corpse's place only when the new one fell on the same spot,
    /// which is the usual case; either way the count goes up.
    fn note_death(&mut self, v: &View) {
        let again = self.active();
        self.deaths = if again { self.deaths + 1 } else { 1 };
        // The first corpse's gear is on the first corpse; the second
        // death dropped what was left. Both lie at the death spot, so
        // the spot is the newer one, and the hands are remembered
        // across both.
        self.death_xy = Some(v.xy);
        self.death_block = v.landblock;
        for g in &v.wielded {
            if !self.wielded.contains(g) {
                self.wielded.push(*g);
            }
        }
        if !again {
            self.wielded = v.wielded.clone();
            self.trip = v.trip;
            self.abandoned = false;
        }
        if v.fighting.is_some() {
            self.killer = v.fighting.clone();
        }
        self.corpse = None;
        self.alive_since = None;
        self.buffs_done = false;
        self.replans = 0;
        self.last_plan = None;
        self.last_open = None;
        self.started = None;
        if self.deaths >= MAX_DEATHS {
            tracing::info!(
                "recovery: died {} times over this corpse, leaving it",
                self.deaths
            );
            self.abandoned = true;
        }
        self.go(Phase::Dead, v.now);
    }

    fn finish(&mut self, now: Instant) -> Action {
        let trip = self.trip.take();
        *self = Recovery::default();
        self.since = now;
        tracing::info!("recovery: done");
        Action::Finish { trip }
    }

    /// Decide what to do this tick.
    pub fn step(&mut self, v: &View) -> Action {
        let now = v.now;
        // A death is noticed in any phase.
        if v.dead && self.phase != Phase::Dead {
            self.note_death(v);
            return Action::Wait;
        }
        match self.phase {
            Phase::None => Action::Nothing,
            Phase::Dead => {
                if v.dead {
                    self.alive_since = None;
                    return Action::Wait;
                }
                let alive = *self.alive_since.get_or_insert(now);
                let moved = self.death_xy.is_some_and(|d| d.distance(v.xy) > TELEPORTED)
                    || v.landblock != self.death_block;
                if moved || now.duration_since(alive) >= ALIVE_SETTLE {
                    self.go(Phase::Landed, now);
                }
                Action::Wait
            }
            Phase::Landed => {
                if now.duration_since(self.since) < LANDED_SETTLE {
                    return Action::Wait;
                }
                if !v.recover || self.abandoned || self.death_xy.is_none() {
                    return self.finish(now);
                }
                self.buffs_done = false;
                self.go(Phase::Rebuff, now);
                Action::Buff
            }
            Phase::Rebuff => {
                if self.buffs_done || now.duration_since(self.since) >= REBUFF_CAP {
                    self.started = Some(now);
                    self.go(Phase::Returning, now);
                    return self.step_returning(v);
                }
                Action::Buff
            }
            Phase::Returning => self.step_returning(v),
            Phase::Looting => self.step_looting(v),
            Phase::Rewielding => self.step_rewielding(v),
        }
    }

    fn step_returning(&mut self, v: &View) -> Action {
        let now = v.now;
        let Some(spot) = self.death_xy else {
            return self.finish(now);
        };
        if self.abandoned {
            return self.finish(now);
        }
        let started = *self.started.get_or_insert(now);
        if now.duration_since(started) > v.limit {
            tracing::info!("recovery: could not reach the corpse in time, leaving it");
            return self.finish(now);
        }
        if let Some((guid, d)) = v.corpse {
            if d <= OPEN_RANGE {
                self.corpse = Some(guid);
                self.go(Phase::Looting, now);
                return self.step_looting(v);
            }
        }
        if v.traveling {
            return Action::Wait;
        }
        if spot.distance(v.xy) <= NEAR {
            self.go(Phase::Looting, now);
            return self.step_looting(v);
        }
        if self.replans >= MAX_REPLANS {
            tracing::info!("recovery: the way back keeps ending short, leaving the corpse");
            return self.finish(now);
        }
        if self
            .last_plan
            .is_some_and(|t| now.duration_since(t) < PLAN_EVERY)
        {
            return Action::Wait;
        }
        self.replans += 1;
        self.last_plan = Some(now);
        Action::Travel(spot)
    }

    fn step_looting(&mut self, v: &View) -> Action {
        let now = v.now;
        let Some((guid, d)) = v.corpse else {
            // Not in sight: give it a moment to appear, then take it
            // as gone (decayed, or already emptied). One that was seen
            // and has gone since was taken away by the server (an
            // empty corpse is destroyed the moment it is opened), and
            // is not waited for.
            let wait = if self.corpse.is_some() {
                CORPSE_GONE
            } else {
                CORPSE_WAIT
            };
            if now.duration_since(self.since) > wait {
                tracing::info!("recovery: no corpse at the spot, carrying on without it");
                self.go(Phase::Rewielding, now);
                return self.step_rewielding(v);
            }
            return Action::Wait;
        };
        self.corpse = Some(guid);
        if v.open == Some(guid) {
            self.go(Phase::Rewielding, now);
            return Action::Empty(guid);
        }
        if d > OPEN_RANGE {
            // Seen from further off than it can be opened: walk over.
            self.go(Phase::Returning, now);
            return self.step_returning(v);
        }
        if self
            .last_open
            .is_none_or(|t| now.duration_since(t) >= OPEN_EVERY)
        {
            self.last_open = Some(now);
            return Action::Open(guid);
        }
        if now.duration_since(self.since) > CORPSE_WAIT * 2 {
            tracing::info!("recovery: the corpse would not open, carrying on without it");
            self.go(Phase::Rewielding, now);
            return self.step_rewielding(v);
        }
        Action::Wait
    }

    fn step_rewielding(&mut self, v: &View) -> Action {
        let now = v.now;
        if now.duration_since(self.since) > REWIELD_CAP {
            return self.finish(now);
        }
        let Some(&g) = v.carried.iter().find(|g| self.wielded.contains(g)) else {
            // Nothing left to take up. Anything still carried was never
            // ours to wield; anything not carried is on a corpse that
            // was not emptied, and is not coming back.
            return self.finish(now);
        };
        if self
            .last_wield
            .is_some_and(|t| now.duration_since(t) < WIELD_EVERY)
        {
            return Action::Wait;
        }
        self.last_wield = Some(now);
        Action::Wield(g)
    }
}

impl Client {
    /// Health at zero with a sheet to say so: the server has us dead
    /// and refuses everything until we stand at the lifestone.
    pub fn is_dead(&self) -> bool {
        let stats = &self.world.stats;
        !stats.name.is_empty() && stats.vital_max(0) > 0 && stats.vitals[0].current == 0
    }

    /// The vitae penalty as a fraction: 0.05 after one death.
    pub fn vitae_penalty(&self) -> f32 {
        (1.0 - self.world.stats.vitae()).max(0.0)
    }

    /// Whether the vitae is high enough that the hard fights should
    /// wait (see `Survive::vitae_wait`).
    pub fn vitae_cautious(&self) -> bool {
        let cfg = &self.autoplay.config.survive;
        cfg.vitae_wait && self.vitae_penalty() >= cfg.vitae_above
    }

    /// A creature to leave alone while the vitae is high: the one that
    /// killed us, or anything with the team's idea of a hard fight's
    /// health. A character at three quarters strength does not go
    /// straight back into the fight it just lost.
    pub fn shy_of(&self, o: &ac_world::WorldObject) -> bool {
        if !self.vitae_cautious() {
            return false;
        }
        let killer = self.autoplay.recovery.killer.as_deref();
        if killer.is_some_and(|k| k.eq_ignore_ascii_case(&o.name)) {
            return true;
        }
        let hard = self.autoplay.config.team.hard_fight_health;
        hard > 0
            && ac_world::elements::known(o.weenie_class_id, &o.name)
                .is_some_and(|c| c.health >= hard)
    }

    /// Our own corpse in view, nearest the death spot: its guid and how
    /// far off it is from where we stand.
    fn own_corpse(&self, me: Vec2, spot: Option<Vec2>) -> Option<(u32, f32)> {
        let name = self.world.stats.name.to_lowercase();
        if name.is_empty() {
            return None;
        }
        let want = format!("corpse of {name}");
        self.world
            .objects
            .values()
            .filter(|o| o.object_desc_flags & ac_world::object_desc_flags::CORPSE != 0)
            .filter(|o| o.name.to_lowercase() == want)
            .filter_map(|o| {
                let p = o.world_pos()?;
                let xy = Vec2::new(p.x, p.y);
                let from_spot = spot.map_or(0.0, |s| s.distance(xy));
                Some((from_spot, o.guid, xy.distance(me)))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, g, d)| (g, d))
    }

    fn recovery_view(&self, now: Instant) -> Option<View> {
        let pl = self.player.as_ref()?;
        let p = pl.world_position();
        let xy = Vec2::new(p.x, p.y);
        let rec = &self.autoplay.recovery;
        let carried: Vec<u32> = rec
            .wielded
            .iter()
            .copied()
            .filter(|g| {
                self.world.is_carried(*g)
                    && self
                        .world
                        .objects
                        .get(g)
                        .is_some_and(|o| o.wielder.is_none())
            })
            .collect();
        let wielded: Vec<u32> = self.world.wielded().map(|o| o.guid).collect();
        let fighting = self
            .attack_target
            .or(self.autoplay.casting_at())
            .and_then(|g| self.world.objects.get(&g))
            .map(|o| o.name.clone());
        let trip = self.autoplay.resume_trip.or_else(|| self.travel_goal_xy());
        let cfg = &self.autoplay.config.survive;
        Some(View {
            now,
            dead: self.is_dead(),
            xy,
            landblock: pl.cell >> 16,
            corpse: self.own_corpse(xy, rec.death_xy),
            traveling: self.traveling(),
            open: self.world.open_container.as_ref().map(|c| c.0),
            carried,
            wielded,
            trip,
            fighting,
            recover: cfg.recover_corpse,
            limit: Duration::from_secs_f32(cfg.corpse_minutes.max(0.0) * 60.0),
        })
    }

    /// The death recovery rule: true while it has the character, so the
    /// rest of the rules wait. Runs right after "stay alive".
    pub fn autoplay_recover(&mut self, now: Instant) -> bool {
        let Some(view) = self.recovery_view(now) else {
            return false;
        };
        let was = self.autoplay.recovery.phase;
        let action = self.autoplay.recovery.step(&view);
        let phase = self.autoplay.recovery.phase;
        if was == Phase::None && phase == Phase::Dead {
            // Just died: nothing can be done from here, and a journey
            // carried on from the lifestone would walk to the wrong
            // place. It is remembered in `trip` for afterwards.
            self.cancel_travel();
            self.attack_target = None;
            self.autoplay.resume_trip = None;
            let spot = self.autoplay.recovery.death_xy.unwrap_or_default();
            tracing::info!(
                "recovery: died at ({:.0}, {:.0}) with {} item(s) wielded",
                spot.x,
                spot.y,
                self.autoplay.recovery.wielded.len()
            );
        }
        let status = |c: &Client| {
            let vitae = c.vitae_penalty() * 100.0;
            if vitae > 0.0 {
                format!("{} (vitae {vitae:.0}%)", c.autoplay.recovery.phase.label())
            } else {
                c.autoplay.recovery.phase.label().to_string()
            }
        };
        match action {
            Action::Nothing => false,
            Action::Wait => {
                let s = status(self);
                self.autoplay.say(Doing::Recovering, s);
                true
            }
            Action::Buff => {
                let s = status(self);
                self.autoplay.say(Doing::Recovering, s);
                let cast = self.autoplay_buff(now, false) || self.autoplay_vitals(now);
                // The buff rule said what it is doing; the rest of the
                // recovery keeps its own status.
                if !cast && self.autoplay.recovery.phase == Phase::Rebuff {
                    // Nothing due, or nothing castable (the wand may be
                    // on the corpse): off we go.
                    if self.autoplay_buffs_settled(now) {
                        self.autoplay.recovery.buffs_done();
                    }
                }
                true
            }
            Action::Travel(spot) => {
                let s = status(self);
                self.autoplay.say(Doing::Recovering, s);
                if self.combat {
                    self.toggle_combat();
                }
                if !self.travel_to(spot) {
                    tracing::info!("recovery: no way back to the death spot");
                    self.autoplay.recovery.no_way();
                }
                true
            }
            Action::Open(guid) => {
                let s = status(self);
                self.autoplay.say(Doing::Recovering, s);
                if self.combat {
                    self.toggle_combat();
                }
                self.interact(guid);
                true
            }
            Action::Empty(guid) => {
                let items: Vec<u32> = self
                    .world
                    .open_container
                    .as_ref()
                    .filter(|c| c.0 == guid)
                    .map(|c| c.1.clone())
                    .unwrap_or_default();
                let mut took = 0;
                for g in items {
                    if self.pack_full() {
                        tracing::info!("recovery: pack full, leaving the rest on the corpse");
                        break;
                    }
                    self.take(g);
                    took += 1;
                }
                self.close_container();
                // The loot rule need not open it again.
                if !self.autoplay.looted.contains(&guid) {
                    self.autoplay.looted.push(guid);
                }
                self.autoplay.say(
                    Doing::Recovering,
                    format!("took {took} item(s) back from the corpse"),
                );
                true
            }
            Action::Wield(guid) => {
                let name = self
                    .world
                    .objects
                    .get(&guid)
                    .map(|o| o.name.clone())
                    .unwrap_or_default();
                self.autoplay
                    .say(Doing::Recovering, format!("wielding {name} again"));
                self.wield_guid(guid);
                true
            }
            Action::Finish { trip } => {
                if let Some(goal) = trip {
                    self.autoplay.resume_trip = Some(goal);
                }
                self.autoplay.armed_for = None;
                self.autoplay.say(Doing::Idle, "back from the dead");
                false
            }
        }
    }

    /// Whether the buff rule has nothing left it could usefully do
    /// before the walk back: nothing due, or a wand it cannot wield.
    /// The rule itself pauses between casts, so one quiet tick is not
    /// the same as being done.
    fn autoplay_buffs_settled(&self, now: Instant) -> bool {
        let cfg = &self.autoplay.config.buffs;
        match self.due_buff(cfg.top_up_within, now) {
            None => true,
            Some((spell, ..)) => {
                // Due but without a wand to cast it with, and none
                // carried to wield: it is on the corpse.
                let no_caster = matches!(self.can_cast(spell), crate::magic::CastCheck::NoCaster);
                let carries_one = self
                    .world
                    .inventory()
                    .any(|o| o.item_type & ac_world::item_type::CASTER != 0);
                if no_caster && !carries_one {
                    return true;
                }
                // Or the buff rule is holding the mana back (its own
                // gate, mirrored): the walk is not worth waiting on a
                // mana bar for.
                let reserve = self.world.stats.vital_max_current(2) as f32 * cfg.keep_mana;
                let cost = self
                    .assets
                    .spell_table()
                    .ok()
                    .and_then(|t| t.get(spell).map(|s| self.mana_cost(s)))
                    .unwrap_or(0) as f32;
                let have = self.world.stats.vitals[2].current as f32;
                have - cost < reserve
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(now: Instant) -> View {
        View {
            now,
            dead: false,
            xy: Vec2::new(1000.0, 1000.0),
            landblock: 0xA9B4,
            corpse: None,
            traveling: false,
            open: None,
            carried: Vec::new(),
            wielded: vec![0x8000_0001, 0x8000_0002],
            trip: Some(Vec2::new(5000.0, 5000.0)),
            fighting: Some("Drudge Slinker".into()),
            recover: true,
            limit: Duration::from_secs(600),
        }
    }

    fn after(t0: Instant, secs: f32) -> Instant {
        t0 + Duration::from_secs_f32(secs)
    }

    /// Dies at the spot, lands at the lifestone, settles.
    fn die_and_land(r: &mut Recovery, t0: Instant) -> View {
        let mut v = view(t0);
        v.dead = true;
        assert_eq!(r.step(&v), Action::Wait);
        assert_eq!(r.phase, Phase::Dead);
        assert_eq!(r.death_xy, Some(Vec2::new(1000.0, 1000.0)));
        assert_eq!(r.wielded, vec![0x8000_0001, 0x8000_0002]);
        assert_eq!(r.killer.as_deref(), Some("Drudge Slinker"));
        // Still dead a moment later.
        v.now = after(t0, 2.0);
        assert_eq!(r.step(&v), Action::Wait);
        // Alive at the lifestone, well away.
        v.dead = false;
        v.xy = Vec2::new(1400.0, 1300.0);
        v.now = after(t0, 5.0);
        assert_eq!(r.step(&v), Action::Wait);
        assert_eq!(r.phase, Phase::Landed);
        v.now = after(t0, 9.0);
        v
    }

    #[test]
    fn nothing_happens_while_alive() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        assert_eq!(r.step(&view(t0)), Action::Nothing);
        assert!(!r.active());
    }

    #[test]
    fn the_whole_way_back() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        let mut v = die_and_land(&mut r, t0);
        // Settled: buff first.
        assert_eq!(r.step(&v), Action::Buff);
        assert_eq!(r.phase, Phase::Rebuff);
        v.now = after(t0, 12.0);
        assert_eq!(r.step(&v), Action::Buff);
        r.buffs_done();
        v.now = after(t0, 14.0);
        // Off to the spot.
        assert_eq!(r.step(&v), Action::Travel(Vec2::new(1000.0, 1000.0)));
        assert_eq!(r.phase, Phase::Returning);
        v.traveling = true;
        v.now = after(t0, 20.0);
        assert_eq!(r.step(&v), Action::Wait);
        // Arrived: the corpse is in view.
        v.traveling = false;
        v.xy = Vec2::new(1003.0, 998.0);
        v.corpse = Some((0xC000_0001, 4.0));
        v.now = after(t0, 60.0);
        assert_eq!(r.step(&v), Action::Open(0xC000_0001));
        assert_eq!(r.phase, Phase::Looting);
        // Asked once; not again straight away.
        v.now = after(t0, 61.0);
        assert_eq!(r.step(&v), Action::Wait);
        // It opened: empty it.
        v.open = Some(0xC000_0001);
        v.now = after(t0, 62.0);
        assert_eq!(r.step(&v), Action::Empty(0xC000_0001));
        assert_eq!(r.phase, Phase::Rewielding);
        // The gear is back in the pack: one wield at a time.
        v.open = None;
        v.carried = vec![0x8000_0002, 0x8000_0001];
        v.now = after(t0, 63.0);
        assert_eq!(r.step(&v), Action::Wield(0x8000_0002));
        v.now = after(t0, 63.5);
        assert_eq!(r.step(&v), Action::Wait, "one order a second");
        v.carried = vec![0x8000_0001];
        v.now = after(t0, 64.5);
        assert_eq!(r.step(&v), Action::Wield(0x8000_0001));
        v.carried = Vec::new();
        v.now = after(t0, 66.0);
        assert_eq!(
            r.step(&v),
            Action::Finish {
                trip: Some(Vec2::new(5000.0, 5000.0))
            }
        );
        assert_eq!(r.phase, Phase::None);
        assert_eq!(r.step(&v), Action::Nothing);
    }

    #[test]
    fn alive_in_place_counts_as_landed_after_a_while() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        let mut v = view(t0);
        v.dead = true;
        r.step(&v);
        v.dead = false;
        v.now = after(t0, 3.0);
        assert_eq!(r.step(&v), Action::Wait);
        assert_eq!(r.phase, Phase::Dead, "not moved: not yet landed");
        v.now = after(t0, 12.0);
        r.step(&v);
        assert_eq!(r.phase, Phase::Landed);
    }

    #[test]
    fn buffing_is_capped() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        let mut v = die_and_land(&mut r, t0);
        assert_eq!(r.step(&v), Action::Buff);
        v.now = after(t0, 9.0 + 121.0);
        assert!(matches!(r.step(&v), Action::Travel(_)));
    }

    #[test]
    fn recovery_can_be_turned_off() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        let mut v = die_and_land(&mut r, t0);
        v.recover = false;
        assert_eq!(
            r.step(&v),
            Action::Finish {
                trip: Some(Vec2::new(5000.0, 5000.0))
            }
        );
    }

    #[test]
    fn gives_up_when_the_way_back_takes_too_long() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        let mut v = die_and_land(&mut r, t0);
        v.limit = Duration::from_secs(60);
        r.step(&v);
        r.buffs_done();
        v.now = after(t0, 10.0);
        assert!(matches!(r.step(&v), Action::Travel(_)));
        v.traveling = true;
        v.now = after(t0, 40.0);
        assert_eq!(r.step(&v), Action::Wait);
        v.now = after(t0, 80.0);
        assert!(matches!(r.step(&v), Action::Finish { .. }));
        assert!(!r.active());
    }

    #[test]
    fn replans_a_journey_that_ends_short_then_gives_up() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        let mut v = die_and_land(&mut r, t0);
        r.step(&v);
        r.buffs_done();
        let mut t = 10.0;
        let mut plans = 0;
        for _ in 0..12 {
            v.now = after(t0, t);
            match r.step(&v) {
                Action::Travel(_) => plans += 1,
                Action::Finish { .. } => break,
                Action::Wait => {}
                other => panic!("unexpected {other:?}"),
            }
            t += 4.0;
        }
        assert_eq!(plans, MAX_REPLANS);
        assert!(!r.active());
    }

    #[test]
    fn no_way_there_finishes() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        let mut v = die_and_land(&mut r, t0);
        r.step(&v);
        r.buffs_done();
        v.now = after(t0, 10.0);
        assert!(matches!(r.step(&v), Action::Travel(_)));
        r.no_way();
        v.now = after(t0, 11.0);
        assert!(matches!(r.step(&v), Action::Finish { .. }));
    }

    #[test]
    fn a_corpse_that_is_gone_is_not_waited_for() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        let mut v = die_and_land(&mut r, t0);
        r.step(&v);
        r.buffs_done();
        v.now = after(t0, 10.0);
        r.step(&v);
        // Arrived at the spot, nothing there.
        v.xy = Vec2::new(1002.0, 1001.0);
        v.now = after(t0, 30.0);
        assert_eq!(r.step(&v), Action::Wait);
        assert_eq!(r.phase, Phase::Looting);
        v.now = after(t0, 50.0);
        // Nothing to wield either: done, and the journey is handed back.
        assert!(matches!(r.step(&v), Action::Finish { trip: Some(_) }));
    }

    #[test]
    fn a_corpse_that_vanishes_once_opened_is_not_waited_for() {
        // What ACE does with a corpse that has nothing on it: it is
        // destroyed the moment it is opened.
        let t0 = Instant::now();
        let mut r = Recovery::default();
        let mut v = die_and_land(&mut r, t0);
        r.step(&v);
        r.buffs_done();
        v.now = after(t0, 10.0);
        v.xy = Vec2::new(1002.0, 1001.0);
        v.corpse = Some((0xC000_0001, 3.0));
        assert_eq!(r.step(&v), Action::Open(0xC000_0001));
        v.corpse = None;
        v.now = after(t0, 11.0);
        assert_eq!(r.step(&v), Action::Wait);
        v.now = after(t0, 14.0);
        assert!(matches!(r.step(&v), Action::Finish { .. }));
    }

    #[test]
    fn a_second_death_over_the_corpse_abandons_it() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        let mut v = die_and_land(&mut r, t0);
        r.step(&v);
        r.buffs_done();
        v.now = after(t0, 10.0);
        r.step(&v);
        // Killed again on the way.
        v.dead = true;
        v.xy = Vec2::new(1010.0, 1005.0);
        v.now = after(t0, 40.0);
        assert_eq!(r.step(&v), Action::Wait);
        assert_eq!(r.phase, Phase::Dead);
        assert_eq!(r.deaths, 2);
        v.dead = false;
        v.xy = Vec2::new(1400.0, 1300.0);
        v.now = after(t0, 45.0);
        r.step(&v);
        assert_eq!(r.phase, Phase::Landed);
        v.now = after(t0, 50.0);
        assert!(
            matches!(r.step(&v), Action::Finish { .. }),
            "not worth a third corpse"
        );
    }

    #[test]
    fn a_corpse_seen_on_the_way_is_gone_to() {
        let t0 = Instant::now();
        let mut r = Recovery::default();
        let mut v = die_and_land(&mut r, t0);
        r.step(&v);
        r.buffs_done();
        v.now = after(t0, 10.0);
        r.step(&v);
        v.traveling = true;
        // In view but too far to open: keep walking.
        v.corpse = Some((0xC000_0001, 40.0));
        v.now = after(t0, 20.0);
        assert_eq!(r.step(&v), Action::Wait);
        assert_eq!(r.phase, Phase::Returning);
        // Near enough: open it, journey or no journey.
        v.corpse = Some((0xC000_0001, 15.0));
        v.now = after(t0, 25.0);
        assert_eq!(r.step(&v), Action::Open(0xC000_0001));
    }

    #[test]
    fn labels_read() {
        assert_eq!(Phase::Returning.label(), "going back for the corpse");
        assert_eq!(Phase::default(), Phase::None);
    }
}
