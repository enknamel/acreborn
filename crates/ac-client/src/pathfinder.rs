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
//! So the planning happens on a thread of its own with its own copy of
//! the archives, and the frame asks for a path and picks the answer up
//! whenever it is ready, walking the single-block route in the meantime.
//! The thread keeps the neighbourhood it last built, so the searches
//! after the first one come back in about a millisecond.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use ac_scene::collision::Capsule;
use ac_scene::navarea::{self, Area};
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

/// A request for a walkable route.
#[derive(Debug, Clone, Copy)]
struct Ask {
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

/// The handle a frame holds: send a goal, pick up the route later.
pub struct Pathfinder {
    data_dir: std::path::PathBuf,
    /// Started on the first ask, so a session that never routes never
    /// pays for the thread or its copy of the archives.
    thread: Option<(Sender<Ask>, Receiver<Answer>)>,
    /// When the last ask went out, whether it was answered or not.
    asked: Option<Instant>,
    /// That ask has not been answered yet.
    waiting: bool,
    /// The planner thread died (the archives would not open).
    dead: bool,
}

impl Pathfinder {
    pub fn new(data_dir: impl Into<std::path::PathBuf>) -> Self {
        Pathfinder {
            data_dir: data_dir.into(),
            thread: None,
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
        if self.thread.is_none() {
            let (jobs_tx, jobs_rx) = std::sync::mpsc::channel::<Ask>();
            let (ans_tx, ans_rx) = std::sync::mpsc::channel::<Answer>();
            let dir = self.data_dir.clone();
            match std::thread::Builder::new()
                .name("pathfinder".into())
                .spawn(move || plan_forever(dir, jobs_rx, ans_tx))
            {
                Ok(_) => self.thread = Some((jobs_tx, ans_rx)),
                Err(e) => {
                    tracing::warn!("pathfinder: could not start: {e}");
                    self.dead = true;
                    return;
                }
            }
        }
        let Some((jobs, _)) = &self.thread else {
            return;
        };
        if jobs
            .send(Ask {
                from,
                to,
                capsule,
                block: block & 0xFFFF_0000,
                outdoors,
            })
            .is_err()
        {
            tracing::warn!("pathfinder: the planner stopped");
            self.dead = true;
            self.thread = None;
            return;
        }
        self.asked = Some(now);
        self.waiting = true;
    }

    /// The newest answer, if one has arrived. Answers planned from too
    /// far away, or for a goal that has moved on, are dropped here.
    pub fn take(&mut self, me: Vec3, goal: Vec3) -> Option<Vec<Vec3>> {
        let (_, answers) = self.thread.as_ref()?;
        let mut newest: Option<Answer> = None;
        loop {
            match answers.try_recv() {
                Ok(a) => newest = Some(a),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.dead = true;
                    self.thread = None;
                    break;
                }
            }
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

/// The planner thread: keeps the last neighbourhood it built and
/// answers the newest ask, dropping the ones that piled up behind it.
fn plan_forever(data_dir: std::path::PathBuf, jobs: Receiver<Ask>, answers: Sender<Answer>) {
    let assets = match ac_scene::Assets::open(&data_dir) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!("pathfinder: cannot read the archives: {e}");
            return;
        }
    };
    let mut area: Option<Area> = None;
    while let Ok(first) = jobs.recv() {
        // Only the newest ask is worth planning; the rest are stale.
        let mut ask = first;
        while let Ok(next) = jobs.try_recv() {
            ask = next;
        }
        let started = Instant::now();
        let here = ask.block;
        let reusable = area.as_ref().is_some_and(|a| {
            same_capsule(&a.capsule(), &ask.capsule)
                && a.blocks.contains(&here)
                && a.holds(ask.from, ask.to)
        });
        if !reusable {
            let want = navarea::blocks_for(ask.from, ask.to);
            match Area::build(&assets, &want, &ask.capsule, here) {
                Ok(a) => {
                    tracing::debug!(
                        "pathfinder: assembled {} blocks around {here:#010x} ({} triangles) in {:?}",
                        a.blocks.len(),
                        a.collision.tris.len(),
                        started.elapsed()
                    );
                    area = Some(a);
                }
                Err(e) => {
                    tracing::debug!("pathfinder: cannot assemble around {here:#010x}: {e}");
                    area = None;
                }
            }
        }
        let waypoints = match area.as_mut() {
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
        let answer = Answer {
            from: ask.from,
            to: ask.to,
            waypoints,
            took: started.elapsed(),
        };
        if answers.send(answer).is_err() {
            return;
        }
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

fn same_capsule(a: &Capsule, b: &Capsule) -> bool {
    a.radius == b.radius
        && a.height == b.height
        && a.step_up == b.step_up
        && a.step_down == b.step_down
}
