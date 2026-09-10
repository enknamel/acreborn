//! Route planning across landblocks, off the frame thread.
//!
//! [`crate::route::Steering`] plans on one landblock, which is enough to
//! walk around a house but not around a city wall: the gate through it
//! is usually in the next block, and a route that cannot leave the block
//! it started in has nowhere to go but into the wall. Planning over a
//! neighbourhood ([`ac_scene::navarea::Area`]) finds the gate, but the
//! first search over nine fresh blocks costs a few hundred milliseconds,
//! which is a visible stall in a frame.
//!
//! So the planning happens on a thread of its own, and the frame asks
//! for a path and picks the answer up whenever it is ready, walking the
//! single-block route in the meantime. One thread serves every session
//! in the process: it shares the archives' mapping with the sessions
//! (its own `Assets`, so its decoded-asset caches are its own), and it
//! keeps the last few neighbourhoods it built, so a party walking
//! together plans on one, and the searches after the first come back
//! in about a millisecond.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ac_scene::collision::Capsule;
use ac_scene::navarea::{self, Area};
use ac_scene::{Assets, SharedArchives};
use glam::Vec3;

/// Ask again no more often than this, answered or not. Without a floor
/// here the frame asks, gets its answer on the next frame, and asks
/// again straight away: a neighbourhood already assembled answers in
/// microseconds, so nothing else would throttle the loop.
const ASK_EVERY: Duration = Duration::from_millis(1500);
/// An answer planned from further than this from where we now stand is
/// too old to follow.
const ANSWER_REACH: f32 = 15.0;
/// An answer to a goal this far from the one we now want is not ours.
const ANSWER_GOAL: f32 = 3.0;
/// How many assembled neighbourhoods the planner keeps. Sessions
/// walking together share one; this many parties apart can each keep
/// theirs. Each is nine blocks of collision, about 15 MB.
const AREAS: usize = 4;

/// A request for a walkable route.
#[derive(Debug, Clone)]
struct Ask {
    /// Which handle asked: a newer ask from the same one replaces it.
    who: u64,
    /// Where the answer goes.
    reply: Sender<Answer>,
    from: Vec3,
    to: Vec3,
    capsule: Capsule,
    /// The landblock the character stands in. A dungeon's cells lie
    /// outside its landblock's square on the map, so the block cannot
    /// be read off the position; the character knows it.
    block: u32,
    /// Both ends are outdoors: the walk should stay out of buildings.
    outdoors: bool,
}

/// What the planner found.
#[derive(Debug)]
pub struct Answer {
    pub from: Vec3,
    pub to: Vec3,
    /// Waypoints ending at the goal, or `None` when nothing connects
    /// the two.
    pub waypoints: Option<Vec<Vec3>>,
    pub took: Duration,
}

/// The process's planner thread, started by the first handle that asks
/// (so a process that never routes never pays for the thread or its
/// caches), and shared by every handle after it.
static PLANNER: Mutex<Option<(u64, Sender<Ask>)>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The line to the planner (and which start of it this is), starting
/// it if need be.
fn planner(archives: &SharedArchives) -> Option<(u64, Sender<Ask>)> {
    let mut slot = PLANNER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(line) = slot.as_ref() {
        return Some(line.clone());
    }
    let (jobs_tx, jobs_rx) = std::sync::mpsc::channel::<Ask>();
    let archives = archives.clone();
    match std::thread::Builder::new()
        .name("pathfinder".into())
        .spawn(move || plan_forever(archives, jobs_rx))
    {
        Ok(_) => {
            let line = (NEXT_ID.fetch_add(1, Ordering::Relaxed), jobs_tx);
            *slot = Some(line.clone());
            Some(line)
        }
        Err(e) => {
            tracing::warn!("pathfinder: could not start: {e}");
            None
        }
    }
}

/// Forget planner start `generation`, whose thread has gone, so the
/// next ask starts one.
fn planner_gone(generation: u64) {
    let mut slot = PLANNER.lock().unwrap_or_else(|e| e.into_inner());
    if slot.as_ref().is_some_and(|(g, _)| *g == generation) {
        *slot = None;
    }
}

/// The handle a frame holds: send a goal, pick up the route later.
pub struct Pathfinder {
    archives: SharedArchives,
    id: u64,
    /// Our line to the process's planner (which start of it) and its
    /// answers to us, taken on the first ask.
    line: Option<(u64, Sender<Ask>, Receiver<Answer>)>,
    /// The sending end of our answers, cloned into every ask.
    reply: Option<Sender<Answer>>,
    /// When the last ask went out, whether it was answered or not.
    asked: Option<Instant>,
    /// That ask has not been answered yet.
    waiting: bool,
    /// The planner could not be started.
    dead: bool,
}

impl Pathfinder {
    /// A handle planning over `assets`' archives (the planner thread
    /// shares their mapping; see [`Assets::archives`]).
    pub fn new(assets: &Assets) -> Self {
        Pathfinder {
            archives: assets.archives(),
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            line: None,
            reply: None,
            asked: None,
            waiting: false,
            dead: false,
        }
    }

    /// Ask for a route from `from` to `to`, unless one is already on its
    /// way. Cheap to call every frame.
    pub fn ask(
        &mut self,
        from: Vec3,
        to: Vec3,
        capsule: Capsule,
        block: u32,
        outdoors: bool,
        now: Instant,
    ) {
        if self.dead {
            return;
        }
        if self
            .asked
            .is_some_and(|t| now.duration_since(t) < ASK_EVERY)
        {
            return;
        }
        if self.line.is_none() {
            let Some((generation, jobs)) = planner(&self.archives) else {
                self.dead = true;
                return;
            };
            let (ans_tx, ans_rx) = std::sync::mpsc::channel::<Answer>();
            self.line = Some((generation, jobs, ans_rx));
            self.reply = Some(ans_tx);
        }
        let (Some((generation, jobs, _)), Some(reply)) = (&self.line, &self.reply) else {
            return;
        };
        if jobs
            .send(Ask {
                who: self.id,
                reply: reply.clone(),
                from,
                to,
                capsule,
                block: block & 0xFFFF_0000,
                outdoors,
            })
            .is_err()
        {
            tracing::warn!("pathfinder: the planner stopped");
            planner_gone(*generation);
            self.line = None;
            self.reply = None;
            return;
        }
        self.asked = Some(now);
        self.waiting = true;
    }

    /// The newest answer, if one has arrived. Answers planned from too
    /// far away, or for a goal that has moved on, are dropped here.
    pub fn take(&mut self, me: Vec3, goal: Vec3) -> Option<Vec<Vec3>> {
        let (_, _, answers) = self.line.as_ref()?;
        let mut newest: Option<Answer> = None;
        // We hold a sender ourselves, so the channel never closes; a
        // planner that stopped shows up on the next ask.
        while let Ok(a) = answers.try_recv() {
            newest = Some(a);
        }
        let a = newest?;
        self.waiting = false;
        if a.to.distance(goal) > ANSWER_GOAL {
            tracing::debug!("pathfinder: answer for a goal we no longer want");
            return None;
        }
        if a.from.distance(me) > ANSWER_REACH {
            tracing::debug!(
                "pathfinder: answer planned {:.0} m back, too stale to follow",
                a.from.distance(me)
            );
            return None;
        }
        match a.waypoints {
            Some(w) => {
                tracing::debug!(
                    "pathfinder: {} waypoints around the neighbourhood in {:?}",
                    w.len(),
                    a.took
                );
                Some(w)
            }
            None => {
                tracing::debug!("pathfinder: nothing connects the two ({:?})", a.took);
                None
            }
        }
    }

    /// An ask is outstanding.
    pub fn busy(&self) -> bool {
        self.waiting
    }

    /// Forget any outstanding ask (the goal went away).
    pub fn reset(&mut self) {
        self.waiting = false;
        self.asked = None;
    }
}

/// The planner thread: keeps the last few neighbourhoods it built and
/// answers the newest ask of every handle, dropping the ones that piled
/// up behind it.
fn plan_forever(archives: SharedArchives, jobs: Receiver<Ask>) {
    let assets = Assets::with_archives(archives);
    // Most recently used last.
    let mut areas: Vec<Area> = Vec::new();
    while let Ok(first) = jobs.recv() {
        // Only the newest ask from each handle is worth planning; the
        // rest are stale.
        let mut batch = vec![first];
        while let Ok(next) = jobs.try_recv() {
            batch.retain(|a| a.who != next.who);
            batch.push(next);
        }
        for ask in batch {
            let answer = plan(&assets, &mut areas, &ask);
            // A handle that has gone is nobody's loss.
            let _ = ask.reply.send(answer);
        }
    }
}

fn plan(assets: &Assets, areas: &mut Vec<Area>, ask: &Ask) -> Answer {
    let started = Instant::now();
    let here = ask.block;
    let found = areas.iter().position(|a| {
        a.capsule().same(&ask.capsule) && a.blocks.contains(&here) && a.holds(ask.from, ask.to)
    });
    let area = match found {
        Some(i) => {
            let a = areas.remove(i);
            areas.push(a);
            areas.last_mut()
        }
        None => {
            let want = navarea::blocks_for(ask.from, ask.to);
            match Area::build(assets, &want, &ask.capsule, here) {
                Ok(a) => {
                    tracing::debug!(
                        "pathfinder: assembled {} blocks around {here:#010x} ({} triangles) in {:?}",
                        a.blocks.len(),
                        a.collision.tris.len(),
                        started.elapsed()
                    );
                    while areas.len() >= AREAS {
                        areas.remove(0);
                    }
                    areas.push(a);
                    areas.last_mut()
                }
                Err(e) => {
                    tracing::debug!("pathfinder: cannot assemble around {here:#010x}: {e}");
                    None
                }
            }
        }
    };
    let waypoints = match area {
        // A dungeon is planned on its own block: a goal outside it
        // is reached through a portal, not by walking there.
        Some(a) if a.dungeon && !a.blocks.contains(&navarea::block_at(ask.to)) => None,
        Some(a) => {
            // Keep a berth from every portal mouth about, save the
            // one the walk is going to: a portal takes whoever
            // touches it.
            a.avoid = portal_mouths_to_avoid(ask.from, ask.to);
            a.berth = PORTAL_BERTH;
            if a.outdoors_only != ask.outdoors {
                // The graph built so far was for the other rule.
                a.outdoors_only = ask.outdoors;
                let (lo, hi, spacing, cap) =
                    (a.nav.min(), a.nav.max(), a.nav.spacing, a.nav.capsule);
                a.nav = ac_scene::nav::NavGraph::new(lo, hi, spacing, &cap);
            }
            a.path(ask.from, ask.to)
        }
        None => None,
    };
    Answer {
        from: ask.from,
        to: ask.to,
        waypoints,
        took: started.elapsed(),
    }
}

/// How close a walk may come to a portal it is not going to.
pub const PORTAL_BERTH: f32 = 3.0;

/// The mouths of the portals within reach of a walk from `from` to
/// `to`, except any the walk is going to (within a stride of `to`).
pub fn portal_mouths_to_avoid(from: Vec3, to: Vec3) -> Vec<glam::Vec2> {
    let (f, t) = (glam::Vec2::new(from.x, from.y), glam::Vec2::new(to.x, to.y));
    let mid = (f + t) * 0.5;
    let reach = f.distance(t) * 0.5 + 200.0;
    ac_world::portals::near(mid, reach)
        .into_iter()
        .map(|p| p.from_xy())
        .filter(|m| m.distance(t) > PORTAL_BERTH + 1.5)
        .collect()
}
