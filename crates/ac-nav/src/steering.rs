//! Steering a move-to through corridors: when the straight line to the
//! goal is blocked by static geometry, plan a route on the landblock's
//! navigation graph and aim at its waypoints one after another.

use std::time::{Duration, Instant};

use glam::Vec3;

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

/// What the steering needs of the world, and of the character standing
/// in it.
///
/// Six questions, no more. Behind them in the client sit the physics,
/// the landblock's triangles and a planner on its own thread; behind
/// them in a test sit a few rectangles. That is the whole point: every
/// navigation fault found the hard way in a live dungeon -- a goal with
/// no path and no node to start from, a straight line walked into a
/// wall, a dungeon treated as somewhere you can stroll out of -- is a
/// unit test here now.
pub trait Ground {
    /// Where the character is.
    fn at(&self) -> Vec3;
    /// The cell it stands in (indoors when the low word is 0x100 or
    /// more).
    fn cell(&self) -> u32;
    /// The landblock it stands in.
    fn block(&self) -> u32;
    /// Whether anything stands between these two points.
    fn line_blocked(&mut self, block: u32, from: Vec3, to: Vec3) -> bool;
    /// A walkable route within one landblock, or `None` when the graph
    /// knows of none.
    fn find_path(&mut self, block: u32, from: Vec3, to: Vec3, goal_cell: u32) -> Option<Vec<Vec3>>;
    /// Ask the neighbourhood planner for a route that may leave this
    /// block; it answers later, through `take_wide`.
    fn ask_wide(&mut self, from: Vec3, to: Vec3, block: u32, outdoors: bool, exact_to: bool);
    /// A neighbourhood route that has come back, if one has.
    fn take_wide(&mut self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>>;
}

/// Where to head this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Aim {
    /// Walk at this point. It may be the goal or the next waypoint.
    Go(Vec3),
    /// There is no way there from here: the line is blocked and no
    /// route was found. Stand still and let whoever set the goal
    /// choose another.
    ///
    /// This is the answer that was missing. Heading for the goal anyway
    /// is a character leaning on the wall it has just decided is in the
    /// way, and it never arrives.
    NoWay,
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
        ground: &mut impl Ground,
        goal: Vec3,
        goal_block: u32,
        now: Instant,
    ) -> Aim {
        let me = ground.at();
        let block = ground.block();
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
            // Standing outside the square of the block we belong to, so
            // there is no edge to aim at and nothing sensible to plan
            // on.
            //
            // A dungeon is exactly this: its cells lie off the side of
            // its own block, which is why the mover has to be told the
            // block by the cell rather than by the position. Heading
            // for the goal from here was the old answer, and from a
            // dungeon the goal is a town thirty kilometres off through
            // rock -- so it walked at the wall, and this branch let it
            // do so behind the back of every other guard.
            //
            // Near enough to be in the same place, still worth trying.
            // Anything further is refused like any other goal there is
            // no way to.
            None => {
                self.route = None;
                self.route_is_wide = false;
                if glam::Vec2::new(goal.x - me.x, goal.y - me.y).length() > A_BLOCK {
                    tracing::debug!("route: {goal:?} is not in reach and there is no way to plan");
                    self.no_way = true;
                    return Aim::NoWay;
                }
                self.no_way = false;
                return Aim::Go(goal);
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
        if let Some(waypoints) = ground.take_wide(me, far_goal) {
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
                now >= self.straight_blocked_until && !ground.line_blocked(block, me, goal);
            // Anything in the way, or a goal past the edge of this
            // block, is worth a neighbourhood route: the way around it
            // may leave the block entirely.
            if !straight_ok || leaves_block {
                ground.ask_wide(
                    me,
                    far_goal,
                    block,
                    ground.cell() & 0xFFFF < 0x100 && goal_block & 0xFFFF < 0x100,
                    // A goal in an indoor cell is where something
                    // stands, so its height is the answer, not a guess
                    // to be dropped onto the ground under it.
                    goal_block & 0xFFFF >= 0x100,
                );
            }
            if straight_ok {
                if self.route.take().is_some() {
                    tracing::debug!("route: straight line clear again");
                }
                self.route_is_wide = false;
                self.no_way = false;
                return Aim::Go(goal);
            }
            match ground.find_path(block, me, goal, goal_block) {
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
                        return Aim::NoWay;
                    }
                    tracing::debug!("route: no path to {goal:?}, going straight");
                    self.no_way = false;
                    return Aim::Go(goal);
                }
            }
        }
        match self.route.take() {
            Some(mut r) => {
                let aim = r.target(me, |from, to| !ground.line_blocked(block, from, to));
                self.route = Some(r);
                Aim::Go(aim)
            }
            None => Aim::Go(goal),
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

    /// A world of the caller's choosing: what is blocked, what routes
    /// exist, and where the character stands. Everything the steering
    /// asks about, answered by hand.
    #[derive(Default)]
    struct Fake {
        at: Vec3,
        cell: u32,
        block: u32,
        /// The straight line is blocked.
        blocked: bool,
        /// What the landblock's graph answers with.
        path: Option<Vec<Vec3>>,
        /// What the neighbourhood planner answers with.
        wide: Option<Vec<Vec3>>,
        asked_wide: bool,
    }

    impl Ground for Fake {
        fn at(&self) -> Vec3 {
            self.at
        }
        fn cell(&self) -> u32 {
            self.cell
        }
        fn block(&self) -> u32 {
            self.block
        }
        fn line_blocked(&mut self, _block: u32, _from: Vec3, _to: Vec3) -> bool {
            self.blocked
        }
        fn find_path(
            &mut self,
            _block: u32,
            _from: Vec3,
            _to: Vec3,
            _goal_cell: u32,
        ) -> Option<Vec<Vec3>> {
            self.path.clone()
        }
        fn ask_wide(&mut self, _f: Vec3, _t: Vec3, _b: u32, _o: bool, _e: bool) {
            self.asked_wide = true;
        }
        fn take_wide(&mut self, _f: Vec3, _t: Vec3) -> Option<Vec<Vec3>> {
            self.wide.take()
        }
    }

    fn near(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 0.5
    }

    #[test]
    fn a_clear_line_is_walked_straight() {
        let mut st = Steering::new(Instant::now());
        let mut g = Fake {
            at: Vec3::new(10.0, 10.0, 0.0),
            ..Default::default()
        };
        let goal = Vec3::new(20.0, 10.0, 0.0);
        match st.steer(&mut g, goal, 0, Instant::now()) {
            Aim::Go(aim) => assert!(near(aim, goal), "{aim:?}"),
            Aim::NoWay => panic!("refused a clear walk"),
        }
    }

    #[test]
    fn a_blocked_line_with_a_route_follows_the_route() {
        let mut st = Steering::new(Instant::now());
        let corner = Vec3::new(10.0, 20.0, 0.0);
        let goal = Vec3::new(20.0, 20.0, 0.0);
        let mut g = Fake {
            at: Vec3::new(10.0, 10.0, 0.0),
            blocked: true,
            path: Some(vec![corner, goal]),
            ..Default::default()
        };
        match st.steer(&mut g, goal, 0, Instant::now()) {
            // The first waypoint, not the goal through the wall.
            Aim::Go(aim) => assert!(!near(aim, goal), "walked at the goal through a wall"),
            Aim::NoWay => panic!("refused a walk it had a route for"),
        }
    }

    #[test]
    fn no_route_and_a_blocked_line_nearby_still_tries() {
        // Within the block, leaning on what is in the way often works:
        // the graph is coarser than the world and a character sliding
        // along a crate reaches the far side of the room. Taking this
        // away stopped a ten-metre walk across Holtburg dead.
        let mut st = Steering::new(Instant::now());
        let goal = Vec3::new(20.0, 10.0, 0.0);
        let mut g = Fake {
            at: Vec3::new(10.0, 10.0, 0.0),
            blocked: true,
            path: None,
            ..Default::default()
        };
        assert_eq!(st.steer(&mut g, goal, 0, Instant::now()), Aim::Go(goal));
        assert!(!st.no_way());
    }

    #[test]
    fn no_route_to_somewhere_far_is_refused_rather_than_walked_at() {
        // The whole of the wall-running: the line is blocked, no route
        // exists, and the goal is a landblock away -- through rock, as
        // often as not. Walking at it is a character pressed against a
        // wall with its legs going, and it never arrives.
        let mut st = Steering::new(Instant::now());
        let mut g = Fake {
            at: Vec3::new(277.0, 47217.0, 0.0),
            cell: 0x01F6_0285,
            block: 0x01F6_0000,
            blocked: true,
            path: None,
            ..Default::default()
        };
        // Thelnoth Cort the Healer, in Holtburg, thirty kilometres off.
        let goal = Vec3::new(32478.0, 34719.0, 42.0);
        assert_eq!(
            st.steer(&mut g, goal, 0xA9B4_0111, Instant::now()),
            Aim::NoWay
        );
        assert!(st.no_way(), "the caller must be able to ask");
    }

    #[test]
    fn a_goal_out_of_the_block_asks_the_wider_planner() {
        let mut st = Steering::new(Instant::now());
        let mut g = Fake {
            at: Vec3::new(32_500.0, 34_600.0, 42.0),
            block: 0xA9B4_0000,
            blocked: true,
            ..Default::default()
        };
        // The next landblock over: the gate through a city wall is
        // usually not in the block you are standing in.
        st.steer(
            &mut g,
            Vec3::new(32_700.0, 34_600.0, 42.0),
            0xABB3_0000,
            Instant::now(),
        );
        assert!(g.asked_wide, "never asked for a route out of the block");
    }

    #[test]
    fn a_way_found_again_clears_the_refusal() {
        let mut st = Steering::new(Instant::now());
        // Inside Holtburg's square, with the goal across the street.
        let goal = Vec3::new(32_520.0, 34_600.0, 42.0);
        let mut g = Fake {
            at: Vec3::new(32_500.0, 34_600.0, 42.0),
            block: 0xA9B4_0000,
            blocked: true,
            path: None,
            ..Default::default()
        };
        // Blocked and no route, but near: worth leaning on.
        assert!(matches!(
            st.steer(&mut g, goal, 0, Instant::now()),
            Aim::Go(_)
        ));
        assert!(!st.no_way());
        // The door opens and the line comes clear.
        g.blocked = false;
        st.reset();
        assert!(matches!(
            st.steer(&mut g, goal, 0, Instant::now()),
            Aim::Go(_)
        ));
        assert!(!st.no_way());
    }

    #[test]
    fn standing_off_the_edge_of_your_own_block_is_not_a_reason_to_charge() {
        // A dungeon's cells lie off the side of the block they belong
        // to, so a character in one is outside its own square and there
        // is no block edge to aim at. That used to mean "head for the
        // goal", which from a dungeon is a town thirty kilometres away
        // through rock -- and it did it behind the back of every other
        // guard.
        let mut st = Steering::new(Instant::now());
        let mut g = Fake {
            at: Vec3::new(277.0, 47_217.0, 0.0),
            cell: 0x01F6_0285,
            block: 0x01F6_0000,
            blocked: false,
            ..Default::default()
        };
        let town = Vec3::new(32_478.0, 34_719.0, 42.0);
        assert_eq!(
            st.steer(&mut g, town, 0xA9B4_0111, Instant::now()),
            Aim::NoWay
        );
        // Something in the dungeon with it is still walked to.
        let near_by = Vec3::new(290.0, 47_210.0, 0.0);
        st.reset();
        assert!(matches!(
            st.steer(&mut g, near_by, 0x01F6_0290, Instant::now()),
            Aim::Go(_)
        ));
    }
}
