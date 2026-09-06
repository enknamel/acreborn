//! Travel: "take me to Arwic". The journey is planned first as a trip
//! (`ac_world::trip`), which decides which portals to take: from
//! Holtburg to Arwic that is the Town Network, in through the town's
//! portal and out of the hub's. Each step of the trip is then walked
//! with a route on the world terrain grid (`ac_scene::worldroute`:
//! slopes, water and roads; no buildings), followed waypoint by
//! waypoint through the per-landblock move-to (`route::Steering`), which
//! steers around the buildings and fences on the way. Each leg handed to
//! the local planner is short and stays inside the current landblock, so
//! the landblock navigation graph can plan it. A portal step is walked
//! the same way, into the portal's mouth, and is done when the character
//! comes out somewhere else.
//!
//! The grid takes a few seconds to load the first time (and a few hundred
//! milliseconds from its cache after that); it is loaded on the first
//! travel request and kept.

use std::rc::Rc;
use std::time::{Duration, Instant};

use ac_formats::region::Region;
use ac_scene::worldgrid::WorldGrid;
use ac_scene::worldroute;
use ac_world::trip::{self, Prefs, Step, Trip};
use glam::{Vec2, Vec3};

use crate::Client;

/// A waypoint counts as reached within this distance (metres, flat).
pub const ARRIVE: f32 = 3.0;
/// Longest leg handed to the local move-to (metres).
pub const LEG: f32 = 60.0;
/// The leg is cut this far short of the landblock edge so its end lies in
/// the block the character stands in.
const EDGE_MARGIN: f32 = 2.0;
/// A portal that has not taken us in this long is one we cannot use
/// (a level range, an unfinished quest, or it simply is not there): the
/// journey is planned again without it.
pub const PORTAL_GIVE_UP: Duration = Duration::from_secs(12);
/// A step that has come no closer to its target in this long is planned
/// again from where the character stands.
pub const STEP_GIVE_UP: Duration = Duration::from_secs(30);
/// Coming this much closer to the step's target counts as progress.
const STEP_PROGRESS: f32 = 5.0;
/// No progress toward the current waypoint for this long: skip it.
pub const STUCK_AFTER: Duration = Duration::from_secs(8);
/// Getting closer to the waypoint by less than this is not progress.
const PROGRESS: f32 = 1.0;
/// The character stops this close to the end of a leg (the local move-to's
/// stop distance); legs are replaced before that, at [`ARRIVE`].
const LEG_STOP: f32 = 1.0;

/// The journey being made, and the terrain data it needs.
#[derive(Default)]
pub struct Travel {
    grid: Option<Rc<WorldGrid>>,
    region: Option<Rc<Region>>,
    /// The whole journey, portals included, and the step being made.
    trip: Option<Trip>,
    step: usize,
    /// The landblock the character was in when a portal step began, and
    /// when it began; the step is done when they are somewhere else, and
    /// given up on when the portal will not take them.
    portal_from: Option<u32>,
    portal_since: Option<Instant>,
    /// The landblock the current step is aimed into, and when the step
    /// began: a step that gets nowhere is planned again from where the
    /// character actually stands.
    step_block: Option<u32>,
    /// The cell the step is aimed at, so an indoor target is recognised
    /// even when the character stands outdoors in the same landblock.
    step_cell: Option<u32>,
    step_since: Option<Instant>,
    /// Closest the character has come to the step's target: the step is
    /// only "going nowhere" when this stops improving.
    step_best: f32,
    /// Where the step is aimed, to measure that.
    step_target: Option<Vec2>,
    /// Where the character stood last frame, to notice a portal that
    /// took them somewhere the journey did not ask for.
    last_seen: Option<Vec2>,
    /// When we last tried jumping into a portal that has not taken us.
    last_hop: Option<Instant>,
    /// How journeys are chosen: the quickest chain, or fewer hops.
    pub prefs: Prefs,
    /// The mouths of portals that would not take us *on this journey*:
    /// they are left out while the rest of the way is planned again, and
    /// forgotten as soon as the player asks to go somewhere else. Named
    /// by place, not by name, since every dungeon has a "Surface Portal".
    refused: Vec<Vec2>,
    /// Where the journey is bound, to replan around a refusal.
    goal: Option<Vec2>,
    /// World xy waypoints of the step being walked, start and goal
    /// included.
    route: Option<Vec<Vec2>>,
    /// Index of the waypoint being walked to.
    next: usize,
    /// End of the leg the local move-to is aimed at right now.
    leg: Option<Vec2>,
    /// Closest the character has been to the current waypoint, and when
    /// that last improved.
    best: f32,
    last_progress: Option<Instant>,
}

impl Travel {
    fn restart_waypoint(&mut self) {
        self.leg = None;
        self.best = f32::INFINITY;
        self.last_progress = None;
    }
}

impl Client {
    /// The world grid and region, loaded on first use.
    fn travel_terrain(&mut self) -> Option<(Rc<WorldGrid>, Rc<Region>)> {
        if self.travel.grid.is_none() {
            let t0 = Instant::now();
            match WorldGrid::load_cached(&self.assets, &WorldGrid::cache_dir()) {
                Ok(g) => {
                    tracing::info!("travel: world grid loaded in {:?}", t0.elapsed());
                    self.travel.grid = Some(Rc::new(g));
                }
                Err(e) => {
                    tracing::warn!("travel: could not load the world grid: {e}");
                    return None;
                }
            }
        }
        if self.travel.region.is_none() {
            match self.assets.region() {
                Ok(r) => self.travel.region = Some(r),
                Err(e) => {
                    tracing::warn!("travel: could not load the region: {e}");
                    return None;
                }
            }
        }
        Some((self.travel.grid.clone()?, self.travel.region.clone()?))
    }

    /// Plan a journey from where the character stands to `goal` (world
    /// xy) and start it. Portals are used when they are quicker than
    /// walking. False when nothing reaches the goal.
    pub fn travel_to(&mut self, goal: Vec2) -> bool {
        let Some(pl) = self.player.as_ref() else {
            tracing::warn!("travel: the character is not in the world");
            return false;
        };
        let me = pl.world_position();
        let cell = pl.cell;
        let t0 = Instant::now();
        // A new destination starts afresh: what turned us away on the
        // last journey says nothing about this one.
        if self.travel.goal.is_none_or(|g| g.distance(goal) > 1.0) {
            self.travel.refused.clear();
        }
        let level = self.world.stats.level.max(1) as u32;
        let mut refused = self.travel.refused.clone();
        // The planner prices a leg on foot by the straight line between
        // its ends, so it can pick a portal that stands across water or
        // up a cliff. Walk the plan through the terrain router before
        // setting off and, when a portal cannot be reached from where
        // the step before it leaves the character, plan again without
        // that one. This holds for the journey being planned only.
        let mut planned = None;
        for _ in 0..6 {
            let Some(t) = trip::plan_for(Vec2::new(me.x, me.y), cell, goal, level, &[], &refused)
            else {
                break;
            };
            let mut at = Vec2::new(me.x, me.y);
            let mut at_outdoors = cell & 0xFFFF < 0x100;
            let mut bad = None;
            for step in &t.steps {
                match step {
                    trip::Step::Portal {
                        mouth,
                        mouth_cell,
                        exit,
                        exit_cell,
                        ..
                    } => {
                        let mouth_outdoors = mouth_cell & 0xFFFF < 0x100;
                        if at_outdoors && mouth_outdoors && !self.travel_can_reach(at, *mouth) {
                            bad = Some(*mouth);
                            break;
                        }
                        at = *exit;
                        at_outdoors = exit_cell & 0xFFFF < 0x100;
                    }
                    trip::Step::Walk(p) => at = *p,
                }
            }
            match bad {
                Some(mouth) => {
                    tracing::info!(
                        "travel: no way on foot to the portal at {mouth:?}; another way"
                    );
                    refused.push(mouth);
                }
                None => {
                    planned = Some(t);
                    break;
                }
            }
        }
        let Some(trip) = planned else {
            // Say why. Often there is a way, but not one this character
            // may take yet: every chain to the far continents runs
            // through a portal with a level on it.
            let anyone = trip::plan_with(
                Vec2::new(me.x, me.y),
                cell,
                goal,
                0,
                &[],
                &[],
                self.travel.prefs,
            );
            let why = match anyone {
                Some(t) => {
                    let asks = t
                        .steps
                        .iter()
                        .filter_map(|s| match s {
                            Step::Portal { mouth, name, .. } => {
                                ac_world::portals::near(*mouth, 3.0)
                                    .first()
                                    .and_then(|p| p.refusal(level))
                                    .map(|r| format!("{name} {r}"))
                            }
                            Step::Walk(_) => None,
                        })
                        .next();
                    match asks {
                        Some(asks) => format!(
                            "No way there yet: the only route runs through a portal that {asks}."
                        ),
                        None => "No way there from here.".to_string(),
                    }
                }
                None => "No way there from here.".to_string(),
            };
            tracing::warn!("travel: {why} ({goal:?}, level {level})");
            self.events.push(crate::Event::Chat { text: why, kind: 1 });
            return false;
        };
        self.travel.refused = refused;
        tracing::info!(
            "travel: {} ({} steps, planned in {:?})",
            trip.summary(),
            trip.steps.len(),
            t0.elapsed()
        );
        self.travel.trip = Some(trip);
        self.travel.step = 0;
        self.travel.route = None;
        self.travel.portal_from = None;
        self.travel.portal_since = None;
        self.travel.goal = Some(goal);
        self.travel.restart_waypoint();
        self.travel_start_step()
    }

    /// Whether the terrain router can get from `me` to `target`, or to
    /// somewhere just beside it.
    fn travel_can_reach(&mut self, me: Vec2, target: Vec2) -> bool {
        let Some((grid, region)) = self.travel_terrain() else {
            return true;
        };
        if worldroute::find(&grid, &region, me, target).is_some() {
            return true;
        }
        (0..8).any(|i| {
            let a = i as f32 * std::f32::consts::TAU / 8.0;
            let near = target + Vec2::new(a.cos(), a.sin()) * 16.0;
            worldroute::find(&grid, &region, me, near).is_some()
        })
    }

    /// Build the route for the step the trip is on. False when the step
    /// cannot be routed (and the journey is given up).
    fn travel_start_step(&mut self) -> bool {
        let Some(t) = self.travel.trip.as_ref() else {
            return false;
        };
        let Some(step) = t.steps.get(self.travel.step).cloned() else {
            tracing::info!("travel: arrived");
            self.cancel_travel();
            return false;
        };
        let Some(pl) = self.player.as_ref() else {
            return false;
        };
        let me3 = pl.world_position();
        let me = Vec2::new(me3.x, me3.y);
        let indoors = pl.is_indoors();
        let pl_cell = pl.cell;
        let (target, label) = match &step {
            Step::Walk(p) => (*p, "walk".to_string()),
            Step::Portal { name, mouth, .. } => (*mouth, format!("portal {name:?}")),
        };
        self.travel.portal_from = match &step {
            Step::Portal { .. } => Some(pl.cell & 0xFFFF_0000),
            Step::Walk(_) => None,
        };
        self.travel.portal_since = None;
        self.travel.last_hop = None;
        self.travel.step_since = Some(Instant::now());
        self.travel.step_best = f32::INFINITY;
        self.travel.step_target = Some(target);
        self.travel.step_cell = match &step {
            Step::Portal { mouth_cell, .. } => Some(*mouth_cell),
            Step::Walk(_) => None,
        };
        self.travel.step_block = Some(match &step {
            Step::Portal { mouth_cell, .. } => *mouth_cell & 0xFFFF_0000,
            Step::Walk(p) => WorldGrid::block_of(*p),
        });
        // Inside a landblock -- the character in it, or the target in one
        // of its interior cells -- the terrain grid says nothing. Aim
        // straight at the target and let the landblock's own navigation
        // graph steer. A dungeon's exit portal stands in an interior cell
        // of the block the character walks out into, so the target being
        // indoors matters as much as the character being indoors.
        let target_indoors = match &step {
            Step::Portal { mouth_cell, .. } => mouth_cell & 0xFFFF >= 0x100,
            Step::Walk(_) => false,
        };
        let same_block = self.travel.step_block == Some(pl_cell & 0xFFFF_0000);
        if indoors || (target_indoors && same_block) {
            tracing::info!("travel: step {} ({label}) inside", self.travel.step);
            self.travel.route = Some(vec![me, target]);
            self.travel.next = 1;
            self.travel.restart_waypoint();
            return true;
        }
        let Some((grid, region)) = self.travel_terrain() else {
            return false;
        };
        // The terrain router can fail on the last few metres to a portal
        // that stands against a cliff or inside a doorway. Walking to
        // just beside it is as good: the local move-to covers the rest.
        // Every other portal near the way is a trap: walking into one
        // takes the character wherever it leads. Give them a wide berth.
        let keep = match &step {
            Step::Portal { mouth, .. } => Some(*mouth),
            Step::Walk(_) => None,
        };
        let mid = (me + target) * 0.5;
        let reach = me.distance(target) * 0.5 + 200.0;
        let avoid: Vec<Vec2> = ac_world::portals::near(mid, reach)
            .into_iter()
            .filter(|p| p.mouth_outdoors())
            .map(|p| p.from_xy())
            .filter(|m| keep.is_none_or(|k| k.distance(*m) > 5.0))
            .collect();
        let route = worldroute::find_avoiding(&grid, &region, me, target, &avoid).or_else(|| {
            [8.0f32, 16.0, 28.0].into_iter().find_map(|r| {
                (0..8).find_map(|i| {
                    let a = i as f32 * std::f32::consts::TAU / 8.0;
                    let near = target + Vec2::new(a.cos(), a.sin()) * r;
                    worldroute::find_avoiding(&grid, &region, me, near, &avoid)
                })
            })
        });
        match route {
            Some(route) => {
                let len: f32 = route.windows(2).map(|w| w[0].distance(w[1])).sum();
                tracing::info!(
                    "travel: step {} ({label}): {} waypoints, {len:.0} m",
                    self.travel.step,
                    route.len()
                );
                self.travel.route = Some(route);
                self.travel.next = 0;
                self.travel.restart_waypoint();
                true
            }
            None => {
                // A portal we cannot walk to is no use, whatever the
                // straight line said: drop it and find another way.
                if let Some((_, mouth)) = self.travel_portal() {
                    let step = self.travel.step;
                    let target_cell = self.travel.step_cell.unwrap_or(0);
                    tracing::warn!(
                        "travel: step {step} ({label}): no way there on foot; going another way \
                         (me cell {pl_cell:#010x} indoors {indoors}, target {target:?} \
                         cell {target_cell:#010x}, same block {same_block})"
                    );
                    self.travel.refused.push(mouth);
                    let goal = self.travel.goal;
                    self.cancel_travel_keeping_refusals();
                    return goal.is_some_and(|g| self.travel_to(g));
                }
                tracing::warn!(
                    "travel: step {} ({label}): no route to {target:?}, giving up",
                    self.travel.step
                );
                self.cancel_travel();
                false
            }
        }
    }

    /// The step is done: move to the next one.
    fn travel_next_step(&mut self) -> bool {
        self.travel.step += 1;
        self.travel.step_since = None;
        self.travel.step_cell = None;
        self.travel.step_target = None;
        self.travel.step_best = f32::INFINITY;
        self.travel.route = None;
        self.travel.portal_from = None;
        self.travel.portal_since = None;
        self.travel.restart_waypoint();
        self.travel_start_step()
    }

    /// [`travel_to`](Self::travel_to) a place of the gazetteer by name
    /// (`ac_world::towns::find`: case-insensitive, prefix or substring).
    pub fn travel_to_place(&mut self, name: &str) -> Result<(), String> {
        let place = ac_world::towns::find(name).ok_or_else(|| format!("unknown place '{name}'"))?;
        if self.travel_to(place.world_xy()) {
            Ok(())
        } else {
            Err(format!("no way to {} from here", place.name))
        }
    }

    /// The route being walked (world xy waypoints), for a map to draw.
    pub fn travel_route(&self) -> Option<&[Vec2]> {
        self.travel.route.as_deref()
    }

    /// `(step being made, steps in the journey)` while travelling.
    pub fn travel_progress(&self) -> Option<(usize, usize)> {
        self.travel
            .trip
            .as_ref()
            .map(|t| (self.travel.step.min(t.steps.len()), t.steps.len()))
    }

    /// The journey being made, for a map to draw or describe.
    pub fn travel_trip(&self) -> Option<&Trip> {
        self.travel.trip.as_ref()
    }

    /// How journeys are chosen. `Prefs::quick()` takes the fastest chain
    /// of portals; `Prefs::steady()` prefers fewer hops, since each one
    /// is a chance to be turned away or land somewhere unexpected.
    pub fn set_travel_prefs(&mut self, prefs: Prefs) {
        self.travel.prefs = prefs;
    }

    pub fn travel_prefs(&self) -> Prefs {
        self.travel.prefs
    }

    pub fn traveling(&self) -> bool {
        self.travel.trip.is_some()
    }

    /// Stop a journey because the player is doing something with the
    /// world instead: talking to a vendor, opening a chest, attacking.
    /// The server walks the character to whatever they are using, and a
    /// trip that carried on afterwards would walk them away again.
    pub(crate) fn interrupt_travel(&mut self, what: &str) {
        if self.traveling() {
            tracing::info!("travel: stopped, {what}");
            self.cancel_travel();
        }
    }

    pub fn cancel_travel(&mut self) {
        if self.travel.trip.take().is_some() {
            tracing::info!("travel: cancelled");
        }
        self.travel.route = None;
        self.travel.step = 0;
        self.travel.portal_from = None;
        self.travel.portal_since = None;
        self.travel.refused.clear();
        self.travel.goal = None;
        self.travel.restart_waypoint();
    }

    /// The portal the current step is aimed at: its name and its mouth.
    fn travel_portal(&self) -> Option<(String, Vec2)> {
        match self.travel.trip.as_ref()?.steps.get(self.travel.step)? {
            Step::Portal { name, mouth, .. } => Some((name.clone(), *mouth)),
            Step::Walk(_) => None,
        }
    }

    /// Give up the journey but remember which portals turned us away, so
    /// the next plan leaves them out.
    fn cancel_travel_keeping_refusals(&mut self) {
        let refused = std::mem::take(&mut self.travel.refused);
        let goal = self.travel.goal;
        self.cancel_travel();
        self.travel.refused = refused;
        self.travel.goal = goal;
    }

    /// The route no longer starts where the character stands (a teleport
    /// or server correction): aim the next leg afresh.
    pub(crate) fn travel_displaced(&mut self) {
        self.travel.leg = None;
    }

    /// Per frame while travelling: advance past reached waypoints, skip
    /// one the character cannot get closer to, and return the end of the
    /// current leg for the local move-to as `(world position, stop
    /// distance, cell)`. `None` when the route is done (or none is set).
    pub(crate) fn travel_goal(&mut self, now: Instant) -> Option<(Vec3, f32, u32)> {
        self.travel.trip.as_ref()?;
        let (me3, block, indoors) = {
            let pl = self.player.as_ref()?;
            (pl.world_position(), pl.cell & 0xFFFF_0000, pl.is_indoors())
        };
        let me = Vec2::new(me3.x, me3.y);
        // A portal step is over when the character is where the portal
        // comes out. Merely being in another landblock is not enough:
        // walking to a portal often crosses a boundary, and counting
        // that as having gone through skipped the rest of the journey.
        if let Some(from) = self.travel.portal_from {
            let exit = match self
                .travel
                .trip
                .as_ref()
                .and_then(|t| t.steps.get(self.travel.step))
            {
                Some(Step::Portal {
                    exit, exit_cell, ..
                }) => Some((*exit, *exit_cell)),
                _ => None,
            };
            let arrived = match exit {
                Some((exit, exit_cell)) => {
                    block == exit_cell & 0xFFFF_0000 || me.distance(exit) < 30.0
                }
                None => block != from,
            };
            if arrived {
                tracing::info!("travel: through the portal");
                if let Some((_, mouth)) = self.travel_portal() {
                    self.travel.refused.retain(|r| r.distance(mouth) > 2.0);
                }
                if !self.travel_next_step() {
                    return None;
                }
            }
        }
        // Indoors (the Town Network hub, a dungeon) the world grid says
        // nothing and a landblock's cells can lie outside its own square,
        // so the leg is aimed straight at the target in the character's
        // own landblock and the landblock's navigation graph steers.
        // Inside the landblock the step is aimed into (the Town Network
        // hub, a dungeon), walk straight at the target and let the
        // landblock's own graph steer. Inside a *building* on the way,
        // with the target elsewhere, keep to the overland legs so the
        // character walks back out instead of pressing against a wall.
        let target_indoors = self.travel.step_cell.is_some_and(|c| c & 0xFFFF >= 0x100);
        if (indoors || target_indoors) && self.travel.step_block == Some(block) {
            let target = *self.travel.route.as_ref()?.last()?;
            if me.distance(target) <= ARRIVE && self.travel.portal_from.is_none() {
                if !self.travel_next_step() {
                    return None;
                }
            } else {
                return Some((Vec3::new(target.x, target.y, me3.z), LEG_STOP, block));
            }
        }
        // Coming no closer to this step's target for a long while: plan
        // again from where the character actually stands. No portal is
        // held against us; the plan is simply out of date.
        if let Some(target) = self.travel.step_target {
            let d = me.distance(target);
            if d < self.travel.step_best - STEP_PROGRESS {
                self.travel.step_best = d;
                self.travel.step_since = Some(now);
            }
        }
        if self
            .travel
            .step_since
            .is_some_and(|t| now.duration_since(t) > STEP_GIVE_UP)
        {
            if let Some(goal) = self.travel.goal {
                tracing::warn!(
                    "travel: step {} is going nowhere; planning again",
                    self.travel.step
                );
                self.travel.step_since = Some(now);
                if self.travel_to(goal) {
                    return self.travel_goal(now);
                }
            }
            return None;
        }
        // A portal takes whoever touches it, so the character can be
        // carried off mid-walk by one the journey never meant to use.
        // Wherever they have landed, plan again from there.
        if let Some(last) = self.travel.last_seen {
            let jumped = me.distance(last) > 60.0;
            let expected = matches!(
                self.travel.trip.as_ref().and_then(|t| t.steps.get(self.travel.step)),
                Some(Step::Portal { exit, .. }) if me.distance(*exit) < 60.0
            );
            if jumped && !expected {
                self.travel.last_seen = Some(me);
                // Whatever swept us up can still be walked into, so it is
                // not refused: refusing it once shut the way off the
                // continent. The route already pays to pass near one, and
                // the plan from here takes the new position as it is.
                if let Some(goal) = self.travel.goal {
                    tracing::info!("travel: carried off to {me:?}; planning again from here");
                    if self.travel_to(goal) {
                        return self.travel_goal(now);
                    }
                }
                return None;
            }
        }
        self.travel.last_seen = Some(me);
        loop {
            let n = self.travel.route.as_ref()?.len();
            let waypoint = |t: &Travel, i: usize| t.route.as_ref().map(|r| r[i]);
            let mut next = self.travel.next;
            while next < n && waypoint(&self.travel, next).is_some_and(|w| me.distance(w) <= ARRIVE)
            {
                next += 1;
            }
            if next != self.travel.next {
                self.travel.next = next;
                self.travel.restart_waypoint();
            }
            if next >= n {
                // The step's route is walked. A portal step is not done
                // until the character has actually gone through, so wait
                // at its mouth rather than give up.
                if self.travel.portal_from.is_some() {
                    // The clock starts when we reach the mouth, not when
                    // the step began: the walk there can be long.
                    if self.travel.portal_since.is_none() {
                        self.travel.portal_since = Some(now);
                    }
                    // Standing at its mouth and still here: it will not
                    // take us. Plan the rest of the way without it.
                    if self
                        .travel
                        .portal_since
                        .is_some_and(|t| now.duration_since(t) > PORTAL_GIVE_UP)
                    {
                        if let Some((name, mouth)) = self.travel_portal() {
                            let level = self.world.stats.level.max(1) as u32;
                            let why = ac_world::portals::near(mouth, 3.0)
                                .first()
                                .and_then(|p| p.refusal(level))
                                .unwrap_or_else(|| "would not take us".into());
                            tracing::warn!("travel: the portal {name:?} {why}; going another way");
                            self.travel.refused.push(mouth);
                        }
                        let goal = self.travel.goal;
                        self.cancel_travel_keeping_refusals();
                        if let Some(goal) = goal {
                            if self.travel_to(goal) {
                                continue;
                            }
                        }
                        return None;
                    }
                    // A few portals sit above the ground and have to be
                    // jumped into; try that while we wait.
                    if self
                        .travel
                        .portal_since
                        .is_some_and(|t| now.duration_since(t) > Duration::from_secs(3))
                        && self
                            .travel
                            .last_hop
                            .is_none_or(|t| now.duration_since(t) > Duration::from_secs(2))
                    {
                        self.travel.last_hop = Some(now);
                        tracing::info!("travel: jumping into the portal");
                        self.jump(1.0);
                    }
                    let mouth = waypoint(&self.travel, n - 1)?;
                    let z = self
                        .travel
                        .grid
                        .as_ref()
                        .map(|g| g.height_at(mouth))
                        .unwrap_or(me.extend(0.0).z);
                    return Some((
                        Vec3::new(mouth.x, mouth.y, z),
                        0.0,
                        WorldGrid::block_of(mouth),
                    ));
                }
                if !self.travel_next_step() {
                    return None;
                }
                continue;
            }
            let wp = waypoint(&self.travel, next)?;
            let d = me.distance(wp);
            match self.travel.last_progress {
                Some(t) if d >= self.travel.best - PROGRESS => {
                    if now.duration_since(t) >= STUCK_AFTER {
                        tracing::warn!(
                            "travel: no progress toward waypoint {next}/{n} at {wp:?} for {:?}, skipping it",
                            STUCK_AFTER
                        );
                        self.travel.next = next + 1;
                        self.travel.restart_waypoint();
                        continue;
                    }
                }
                _ => {
                    self.travel.best = d;
                    self.travel.last_progress = Some(now);
                }
            }
            let leg = match self.travel.leg {
                Some(l) if me.distance(l) > ARRIVE && me.distance(l) <= 2.0 * LEG => l,
                _ => {
                    let l = leg_end(me, wp);
                    self.travel.leg = Some(l);
                    l
                }
            };
            let z = self
                .travel
                .grid
                .as_ref()
                .map(|g| g.height_at(leg))
                .unwrap_or(0.0);
            return Some((
                Vec3::new(leg.x, leg.y, z),
                LEG_STOP,
                WorldGrid::block_of(leg),
            ));
        }
    }
}

/// Where the next leg from `me` toward `wp` ends: at most [`LEG`] away,
/// and inside the landblock `me` is in (cut short of its edge) unless
/// that would leave nothing to walk, in which case the leg crosses the
/// edge straight and the next one is planned in the new block.
fn leg_end(me: Vec2, wp: Vec2) -> Vec2 {
    let d = wp - me;
    let dist = d.length();
    if dist < 1e-3 {
        return wp;
    }
    let dir = d / dist;
    let far = if dist <= LEG { wp } else { me + dir * LEG };
    let block = 192.0;
    let lo = (me / block).floor() * block;
    let hi = lo + Vec2::splat(block);
    // Parametric distance along the leg to the block boundary.
    let mut t_exit = f32::INFINITY;
    for axis in 0..2 {
        let (p, v) = (me[axis], (far - me)[axis]);
        if v > 1e-6 {
            t_exit = t_exit.min((hi[axis] - p) / v);
        } else if v < -1e-6 {
            t_exit = t_exit.min((lo[axis] - p) / v);
        }
    }
    if t_exit >= 1.0 {
        return far;
    }
    let clipped = me + (far - me) * t_exit - dir * EDGE_MARGIN;
    if me.distance(clipped) > ARRIVE + 1.0 {
        clipped
    } else {
        far
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legs_are_short_and_stay_in_the_block() {
        // Mid-block, far goal along +x: a LEG-long leg.
        let me = Vec2::new(192.0 * 10.0 + 20.0, 192.0 * 10.0 + 96.0);
        let wp = me + Vec2::new(1000.0, 0.0);
        let l = leg_end(me, wp);
        assert!((l - (me + Vec2::new(LEG, 0.0))).length() < 1e-3, "{l:?}");
        // Near the block's east edge with the goal beyond it: cut short
        // of the edge...
        let me = Vec2::new(192.0 * 11.0 - 30.0, 192.0 * 10.0 + 96.0);
        let l = leg_end(me, wp);
        assert!((l.x - (192.0 * 11.0 - EDGE_MARGIN)).abs() < 1e-3, "{l:?}");
        assert_eq!(WorldGrid::block_of(l), WorldGrid::block_of(me));
        // ...unless that leaves nothing to walk: then straight across.
        let me = Vec2::new(192.0 * 11.0 - 3.0, 192.0 * 10.0 + 96.0);
        let l = leg_end(me, wp);
        assert!(l.x > 192.0 * 11.0, "{l:?}");
        // A close goal is the leg's end.
        let wp = me + Vec2::new(0.0, -10.0);
        assert_eq!(leg_end(me, wp), wp);
        // Diagonal toward the north-west corner: the nearer edge cuts.
        let me = Vec2::new(192.0 * 10.0 + 10.0, 192.0 * 11.0 - 40.0);
        let l = leg_end(me, me + Vec2::new(-100.0, 100.0));
        assert_eq!(WorldGrid::block_of(l), WorldGrid::block_of(me));
        assert!(l.x > 192.0 * 10.0 && l.y < 192.0 * 11.0, "{l:?}");
    }
}
