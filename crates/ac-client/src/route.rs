//! Steering a move-to through corridors: when the straight line to the
//! goal is blocked by static geometry, plan a route on the landblock's
//! navigation graph and aim at its waypoints one after another.

use std::time::{Duration, Instant};

use ac_scene::Assets;
use glam::Vec3;

use crate::pathfinder::Pathfinder;
use crate::player::Player;

/// A waypoint counts as reached within this distance (metres, flat).
pub const ARRIVE: f32 = 0.7;
/// Standing this close to a waypoint, it is passed whatever lies beyond.
const ON_THE_SPOT: f32 = 0.25;
/// Re-plan when the goal has moved this far from the planned one.
pub const REPLAN_DISTANCE: f32 = 2.0;
/// Re-plan (or re-check the straight line) at least this often.
pub const REPLAN_AFTER: Duration = Duration::from_secs(2);
/// A route from the neighbourhood planner is kept longer: it cost more
/// to find and it goes out of date more slowly, since it already
/// accounts for what lies past this landblock's edge.
pub const WIDE_REPLAN_AFTER: Duration = Duration::from_secs(6);
/// How often the straight line is re-tested while no route is needed.
const LINE_CHECK: Duration = Duration::from_millis(500);

/// The route being followed.
#[derive(Debug, Clone)]
pub struct Route {
    /// Goal the route was planned for.
    pub goal: Vec3,
    /// Waypoints in order; the last one is the goal.
    pub waypoints: Vec<Vec3>,
    /// Index of the waypoint being steered at.
    pub next: usize,
    pub planned: Instant,
}

impl Route {
    pub fn new(goal: Vec3, waypoints: Vec<Vec3>, now: Instant) -> Self {
        Route {
            goal,
            waypoints,
            next: 0,
            planned: now,
        }
    }

    /// The goal moved or the plan is old.
    pub fn stale(&self, goal: Vec3, now: Instant) -> bool {
        goal.distance(self.goal) > REPLAN_DISTANCE
            || now.duration_since(self.planned) >= REPLAN_AFTER
    }

    /// The point to steer at from `me`: the next waypoint, advancing past
    /// the ones already within [`ARRIVE`]. The last waypoint (the goal)
    /// is never consumed; the caller decides when it has arrived.
    ///
    /// A waypoint sits where the route turns a corner, and turning early
    /// cuts that corner: `clear(from, to)` says whether the straight walk
    /// is open, and a waypoint whose successor cannot be walked to from
    /// here is kept until we are right on it.
    pub fn target(&mut self, me: Vec3, mut clear: impl FnMut(Vec3, Vec3) -> bool) -> Vec3 {
        while self.next + 1 < self.waypoints.len() {
            let w = self.waypoints[self.next];
            let d = glam::Vec2::new(w.x - me.x, w.y - me.y).length();
            if d > ARRIVE {
                break;
            }
            if d > ON_THE_SPOT && !clear(me, self.waypoints[self.next + 1]) {
                break;
            }
            self.next += 1;
        }
        self.waypoints.get(self.next).copied().unwrap_or(self.goal)
    }

    /// How far the route still runs from `me`: to the next waypoint and
    /// on through the rest.
    pub fn remaining(&self, me: Vec3) -> f32 {
        let mut from = me;
        let mut total = 0.0;
        for w in &self.waypoints[self.next.min(self.waypoints.len())..] {
            total += glam::Vec2::new(w.x - from.x, w.y - from.y).length();
            from = *w;
        }
        total
    }
}

/// Steering state of one character: the route being followed and the
/// throttles and stuck detection around it.
#[derive(Debug, Clone)]
pub struct Steering {
    pub route: Option<Route>,
    /// Next time the straight line is re-tested while no route exists.
    next_check: Instant,
    /// Where the character last made progress, and when.
    last_pos: Option<Vec3>,
    last_progress: Instant,
    /// The straight line is not trusted before this: the character got
    /// stuck walking it (the line test passed but the walk did not).
    straight_blocked_until: Instant,
    /// The route came from the neighbourhood planner, so a single-block
    /// re-plan should not quietly replace it with a worse one.
    route_is_wide: bool,
    /// There is no way to the goal from here: the straight line is
    /// blocked and no route was found. The steering stands still rather
    /// than lean on the obstacle, and whoever set the goal can ask for
    /// this and choose another.
    no_way: bool,
}

impl Steering {
    /// Whether the last steer found no way at all to its goal.
    ///
    /// Worth asking before deciding a character is merely slow: a walk
    /// that cannot be made does not get better with time, and the rules
    /// above can pick another way -- a recall, a portal, another shop
    /// -- instead of waiting out a timeout against a wall.
    pub fn no_way(&self) -> bool {
        self.no_way
    }
}

/// One landblock across, in metres. A goal further off than this is
/// not somewhere to be reached by leaning on whatever is in the way.
const A_BLOCK: f32 = 192.0;

/// No progress for this long while steering counts as stuck.
const STUCK_AFTER: Duration = Duration::from_millis(1500);
/// Movement below this (metres, flat) is not progress.
const PROGRESS: f32 = 0.1;
/// After getting stuck on the straight line, route on the graph for
/// this long before trusting the line again.
const AVOID_STRAIGHT: Duration = Duration::from_secs(8);

impl Steering {
    pub fn new(now: Instant) -> Self {
        Steering {
            route: None,
            next_check: now,
            last_pos: None,
            last_progress: now,
            straight_blocked_until: now,
            route_is_wide: false,
            no_way: false,
        }
    }

    /// Forget the route and the progress history (the goal went away or
    /// the user took over).
    pub fn reset(&mut self) {
        self.route = None;
        self.last_pos = None;
        self.route_is_wide = false;
        self.no_way = false;
    }

    /// Where to head this frame to reach `goal` (world space, in landblock
    /// `goal_block`): the goal itself while the straight line is clear,
    /// else the next waypoint of a route around what is in the way.
    ///
    /// Two planners answer that. The graph of the landblock we stand in
    /// is searched here and now, which is fast but cannot see past the
    /// block's edge; the `pathfinder` plans over the whole neighbourhood
    /// on a thread of its own, which is what gets a character through a
    /// city gate instead of into the wall beside it. The wide route is
    /// asked for whenever the goal is blocked or outside this block, and
    /// adopted when it arrives. A character that stops making progress
    /// drops its route, stops trusting the straight line for a while,
    /// and re-plans.
    pub fn steer(
        &mut self,
        player: &mut Player,
        assets: &Assets,
        wide: &mut Pathfinder,
        goal: Vec3,
        goal_block: u32,
        now: Instant,
    ) -> Vec3 {
        let me = player.world_position();
        let block = player.landblock();
        let far_goal = goal;
        // A goal in another landblock used to be run at in a straight
        // line, obstacles and all, which is how a character ends up
        // pressed against a city wall. Steer instead for the point where
        // the line leaves this landblock, so the graph in it can still
        // take us around what is in the way.
        let leaves_block = goal_block & 0xFFFF_0000 != block;
        let following_wide = self.route_is_wide && self.route.is_some();
        let goal = match plan_goal(me, goal, block, leaves_block, following_wide) {
            Some(g) => g,
            // Already outside the block we think we are in: nothing
            // sensible to plan on, so head for it.
            None => {
                self.route = None;
                self.route_is_wide = false;
                return goal;
            }
        };
        match self.last_pos {
            Some(p) if glam::Vec2::new(me.x - p.x, me.y - p.y).length() < PROGRESS => {
                if now.duration_since(self.last_progress) >= STUCK_AFTER {
                    tracing::debug!(
                        "route: stuck at {:?}, re-planning",
                        me - ac_world::landblock_origin(block)
                    );
                    self.route = None;
                    self.route_is_wide = false;
                    self.next_check = now;
                    self.straight_blocked_until = now + AVOID_STRAIGHT;
                    self.last_progress = now;
                }
            }
            _ => {
                self.last_pos = Some(me);
                self.last_progress = now;
            }
        }
        // A route from the neighbourhood planner, if one has come back.
        if let Some(waypoints) = wide.take(me, far_goal) {
            self.route = Some(Route::new(far_goal, waypoints, now));
            self.route_is_wide = true;
            self.next_check = now + REPLAN_AFTER;
        }
        let replan = match &self.route {
            None => now >= self.next_check,
            // A wide route is kept while it still leads where we want:
            // the single-block planner cannot do better than it. Running
            // out of waypoints is not staleness -- the last one is the
            // goal, and the caller decides when it has arrived.
            Some(r) if self.route_is_wide => {
                far_goal.distance(r.goal) > REPLAN_DISTANCE
                    || now.duration_since(r.planned) >= WIDE_REPLAN_AFTER
            }
            Some(r) => r.stale(goal, now),
        };
        if replan {
            self.next_check = now + LINE_CHECK;
            let straight_ok =
                now >= self.straight_blocked_until && !player.line_blocked(assets, block, me, goal);
            // Anything in the way, or a goal past the edge of this
            // block, is worth a neighbourhood route: the way around it
            // may leave the block entirely.
            if !straight_ok || leaves_block {
                wide.ask(
                    me,
                    far_goal,
                    player.capsule(),
                    block,
                    crate::pathfinder::Ends {
                        outdoors: player.cell & 0xFFFF < 0x100 && goal_block & 0xFFFF < 0x100,
                        // A goal in an indoor cell is where something
                        // stands, so its height is the answer, not a
                        // guess to be dropped onto the ground under it.
                        exact_to: goal_block & 0xFFFF >= 0x100,
                    },
                    now,
                );
            }
            if straight_ok {
                if self.route.take().is_some() {
                    tracing::debug!("route: straight line clear again");
                }
                self.route_is_wide = false;
                self.no_way = false;
                return goal;
            }
            match player.find_path(assets, block, me, goal, goal_block) {
                Some(waypoints) => {
                    let origin = ac_world::landblock_origin(block);
                    let local: Vec<[f32; 3]> = waypoints
                        .iter()
                        .map(|w| {
                            let l = *w - origin;
                            [
                                (l.x * 10.0).round() / 10.0,
                                (l.y * 10.0).round() / 10.0,
                                (l.z * 10.0).round() / 10.0,
                            ]
                        })
                        .collect();
                    tracing::debug!(
                        "route: {} waypoints to {:?} in {block:#010x}: {local:?}",
                        waypoints.len(),
                        goal - origin
                    );
                    self.route = Some(Route::new(goal, waypoints, now));
                    self.route_is_wide = false;
                    self.no_way = false;
                }
                None if following_wide => {
                    // The block's own graph finds nothing (the way on
                    // is over a slope it cannot see, or through the
                    // next block), but the neighbourhood route still
                    // stands and a fresh one has been asked for: keep
                    // walking it rather than run at the goal.
                    tracing::debug!("route: no path in this block; keeping the wide route");
                    if let Some(r) = self.route.as_mut() {
                        r.planned = now;
                    }
                }
                None => {
                    // Nothing found, and the straight line was already
                    // judged blocked -- that is why a path was looked
                    // for at all.
                    //
                    // Whether to set off anyway turns on how far the
                    // goal is. Inside this landblock, leaning on what
                    // is in the way often works: the graph is coarser
                    // than the world, and a character sliding along a
                    // crate reaches the far side of the room. That is
                    // worth keeping -- taking it away stopped a
                    // ten-metre walk across Holtburg dead.
                    //
                    // Out of the block it is never worth it. Nothing
                    // within reach leads there, the goal may be in
                    // another space entirely -- a dungeon's wall and a
                    // vendor on the surface a hundred metres overhead
                    // -- and walking at it is walking into rock until
                    // something else gives up. Stand still and say so,
                    // and let the rules above find another way out.
                    self.route = None;
                    self.route_is_wide = false;
                    self.next_check = now + REPLAN_AFTER;
                    // Out of the block, or simply too far to be in it:
                    // indoors the caller names the block we stand in
                    // whatever the goal is -- a dungeon's cells lie
                    // outside its square, and without that fudge the
                    // steering would never plan at all -- so the cell
                    // cannot be trusted to say and the distance is
                    // asked instead.
                    let far = glam::Vec2::new(goal.x - me.x, goal.y - me.y).length() > A_BLOCK;
                    if leaves_block || far {
                        tracing::debug!("route: no way to {goal:?}, and it is not within reach");
                        self.no_way = true;
                        return me;
                    }
                    tracing::debug!("route: no path to {goal:?}, going straight");
                    self.no_way = false;
                    return goal;
                }
            }
        }
        match &mut self.route {
            Some(r) => r.target(me, |from, to| !player.line_blocked(assets, block, from, to)),
            None => goal,
        }
    }

    /// How far the route being followed still runs from `me`, where it
    /// ends, and when it was planned (a new plan measures afresh);
    /// `None` while heading straight for the goal.
    pub fn remaining(&self, me: Vec3) -> Option<(f32, Vec3, Instant)> {
        self.route
            .as_ref()
            .map(|r| (r.remaining(me), r.goal, r.planned))
    }
}

/// The goal to plan on from `me` in landblock `block`: `goal` itself
/// when it lies in the block (`leaves_block` false), else the point
/// where the line to it leaves the block, so the block's own graph can
/// still steer around what is in the way; `None` when we stand at that
/// edge already and there is nothing left to plan on in this block.
///
/// A route from the neighbourhood planner is the exception: it was
/// planned across the blocks and is worth more than anything this
/// block's graph could say, so while one is being followed the goal is
/// left as it is, whatever block it is in. Clipping it used to drop the
/// route the moment it led across an edge, and a walker whose way
/// around an unclimbable slope ran through the next block was sent
/// straight at the slope again every time it reached the edge, for as
/// long as the journey would wait.
pub fn plan_goal(
    me: Vec3,
    goal: Vec3,
    block: u32,
    leaves_block: bool,
    following_wide: bool,
) -> Option<Vec3> {
    if !leaves_block || following_wide {
        return Some(goal);
    }
    clip_to_block(me, goal, block)
}

/// Where the line from `me` to `goal` leaves the landblock `block`,
/// pulled a stride back inside it. `None` when `me` is not in that
/// block, or the goal is not outside it after all.
pub fn clip_to_block(me: Vec3, goal: Vec3, block: u32) -> Option<Vec3> {
    const SIDE: f32 = 192.0;
    /// Far enough inside that the graph has somewhere to stand.
    const INSIDE: f32 = 3.0;
    let origin = ac_world::landblock_origin(block);
    let (lo, hi) = (origin, origin + Vec3::new(SIDE, SIDE, 0.0));
    if me.x < lo.x || me.y < lo.y || me.x > hi.x || me.y > hi.y {
        return None;
    }
    let d = goal - me;
    let mut t = 1.0f32;
    for axis in 0..2 {
        let (p, v) = (me[axis], d[axis]);
        if v > 1e-6 {
            t = t.min((hi[axis] - p) / v);
        } else if v < -1e-6 {
            t = t.min((lo[axis] - p) / v);
        }
    }
    if t >= 1.0 {
        return None;
    }
    let at = me + d * t;
    let dir = (goal - me).normalize_or_zero();
    let edge = at - dir * INSIDE;
    // Standing at the edge already, the clipped point is under our own
    // feet or behind them, and steering at it goes nowhere -- worse, a
    // walker a stride short of the edge stepped back to it, turned for
    // the goal, reached the edge again and stepped back again, for
    // ever. Only a point a stride ahead is worth walking to; otherwise
    // the goal itself is.
    let ahead = (edge - me).dot(dir);
    (ahead > 1.0).then_some(edge)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_goal_beyond_the_block_is_clipped_to_its_edge() {
        let block = 0x0A0B_0000;
        let origin = ac_world::landblock_origin(block);
        let me = origin + Vec3::new(96.0, 96.0, 0.0);
        // Straight east, well past the edge: clipped just inside it.
        let goal = me + Vec3::new(1000.0, 0.0, 0.0);
        let edge = clip_to_block(me, goal, block).expect("clipped");
        assert!((edge.x - (origin.x + 192.0 - 3.0)).abs() < 1e-3, "{edge:?}");
        assert!((edge.y - me.y).abs() < 1e-3);
        // A goal inside the block is left alone.
        assert_eq!(
            clip_to_block(me, me + Vec3::new(10.0, 0.0, 0.0), block),
            None
        );
        // Standing outside the block it claims: nothing to say.
        let elsewhere = origin + Vec3::new(500.0, 0.0, 0.0);
        assert_eq!(clip_to_block(elsewhere, goal, block), None);
        // A stride short of the edge with the goal beyond it: the
        // clipped point would be behind us, so head for the goal.
        let at_edge = origin + Vec3::new(96.0, 191.25, 0.0);
        let beyond = at_edge + Vec3::new(3.0, 60.0, 0.0);
        assert_eq!(clip_to_block(at_edge, beyond, block), None);
        let near_edge = origin + Vec3::new(96.0, 189.5, 0.0);
        assert_eq!(clip_to_block(near_edge, beyond, block), None);
        // Five metres short, the clipped point is two ahead: fine.
        let short = origin + Vec3::new(96.0, 187.0, 0.0);
        let e = clip_to_block(short, short + Vec3::new(0.0, 60.0, 0.0), block).expect("ahead");
        assert!((e.y - (origin.y + 189.0)).abs() < 1e-3, "{e:?}");
    }

    #[test]
    fn a_wide_route_is_not_clipped_at_the_block_edge() {
        // The landblock-edge hesitation: standing a stride into 0x6E8F
        // with the goal 55 m east, up a slope only a route through the
        // block behind gets around.
        let block = 0x6E8F_0000;
        let origin = ac_world::landblock_origin(block);
        let me = origin + Vec3::new(0.3, 92.7, 123.6);
        let goal = origin + Vec3::new(55.0, 93.0, 142.3);
        // The goal is in this block: nothing to clip either way.
        assert_eq!(plan_goal(me, goal, block, false, false), Some(goal));
        assert_eq!(plan_goal(me, goal, block, false, true), Some(goal));
        // Straddling the edge, the character's cell says the block
        // behind (0x6D8F): the clipped point would be under its feet,
        // so a single-block plan has nothing to work with...
        let behind = 0x6D8F_0000;
        let straddling = origin + Vec3::new(-0.1, 92.7, 123.5);
        assert_eq!(plan_goal(straddling, goal, behind, true, false), None);
        // ...but a neighbourhood route being followed is kept, and the
        // goal it was planned for stays the goal.
        assert_eq!(plan_goal(straddling, goal, behind, true, true), Some(goal));
        // Well inside a block with the goal beyond it and no wide route,
        // the edge is what is planned on.
        let inside = origin + Vec3::new(96.0, 96.0, 0.0);
        let beyond = inside + Vec3::new(500.0, 0.0, 0.0);
        let edge = plan_goal(inside, beyond, block, true, false).expect("clipped");
        assert!((edge.x - (origin.x + 189.0)).abs() < 1e-3, "{edge:?}");
    }

    #[test]
    fn waypoints_advance_within_reach_and_the_goal_stays() {
        let now = Instant::now();
        let goal = Vec3::new(10.0, 0.0, 0.0);
        let mut r = Route::new(
            goal,
            vec![Vec3::new(2.0, 0.0, 0.0), Vec3::new(5.0, 3.0, 0.0), goal],
            now,
        );
        let open = |_: Vec3, _: Vec3| true;
        assert_eq!(r.target(Vec3::ZERO, open), Vec3::new(2.0, 0.0, 0.0));
        assert!((r.remaining(Vec3::ZERO) - (2.0 + 18f32.sqrt() + 34f32.sqrt())).abs() < 1e-4);
        // Within 0.7 m of the first: on to the second.
        assert_eq!(
            r.target(Vec3::new(1.5, 0.2, 0.0), open),
            Vec3::new(5.0, 3.0, 0.0)
        );
        // Skipping two at once when both are close.
        assert_eq!(r.target(Vec3::new(5.0, 2.8, 0.0), open), goal);
        // Standing on the goal still aims at it.
        assert_eq!(r.target(goal, open), goal);
        assert_eq!(r.next, 2);
        assert_eq!(r.remaining(goal), 0.0);
    }

    #[test]
    fn a_corner_is_not_cut_when_the_wall_is_in_the_way() {
        let now = Instant::now();
        let goal = Vec3::new(10.0, 10.0, 0.0);
        let corner = Vec3::new(10.0, 0.0, 0.0);
        let mut r = Route::new(goal, vec![corner, goal], now);
        let walled = |_: Vec3, _: Vec3| false;
        // Near the corner but the way on is not open from here: keep
        // aiming at the corner.
        assert_eq!(r.target(Vec3::new(9.5, -0.3, 0.0), walled), corner);
        // Right on it: pass it whatever the wall says.
        assert_eq!(r.target(Vec3::new(9.9, -0.1, 0.0), walled), goal);
    }

    #[test]
    fn a_route_goes_stale_when_the_goal_moves_or_time_passes() {
        let now = Instant::now();
        let goal = Vec3::new(10.0, 0.0, 0.0);
        let r = Route::new(goal, vec![goal], now);
        assert!(!r.stale(goal + Vec3::new(1.0, 0.0, 0.0), now));
        assert!(r.stale(goal + Vec3::new(2.5, 0.0, 0.0), now));
        assert!(r.stale(goal, now + REPLAN_AFTER));
    }
}
