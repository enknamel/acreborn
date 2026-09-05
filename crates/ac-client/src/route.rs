//! Steering a move-to through corridors: when the straight line to the
//! goal is blocked by static geometry, plan a route on the landblock's
//! navigation graph and aim at its waypoints one after another.

use std::time::{Duration, Instant};

use ac_scene::Assets;
use glam::Vec3;

use crate::player::Player;

/// A waypoint counts as reached within this distance (metres, flat).
pub const ARRIVE: f32 = 0.7;
/// Re-plan when the goal has moved this far from the planned one.
pub const REPLAN_DISTANCE: f32 = 2.0;
/// Re-plan (or re-check the straight line) at least this often.
pub const REPLAN_AFTER: Duration = Duration::from_secs(2);
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
    pub fn target(&mut self, me: Vec3) -> Vec3 {
        while self.next + 1 < self.waypoints.len() {
            let w = self.waypoints[self.next];
            if glam::Vec2::new(w.x - me.x, w.y - me.y).length() > ARRIVE {
                break;
            }
            self.next += 1;
        }
        self.waypoints.get(self.next).copied().unwrap_or(self.goal)
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
}

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
        }
    }

    /// Forget the route and the progress history (the goal went away or
    /// the user took over).
    pub fn reset(&mut self) {
        self.route = None;
        self.last_pos = None;
    }

    /// Where to head this frame to reach `goal` (world space, in landblock
    /// `goal_block`): the goal itself while the straight line is clear or
    /// the goal lies in another landblock, else the next waypoint of a
    /// route planned (and re-planned as `Route::stale` says) on the
    /// graph. A character that stops making progress drops its route,
    /// stops trusting the straight line for a while, and re-plans.
    pub fn steer(
        &mut self,
        player: &mut Player,
        assets: &Assets,
        goal: Vec3,
        goal_block: u32,
        now: Instant,
    ) -> Vec3 {
        let me = player.world_position();
        let block = player.landblock();
        if goal_block & 0xFFFF_0000 != block {
            self.route = None;
            return goal;
        }
        match self.last_pos {
            Some(p) if glam::Vec2::new(me.x - p.x, me.y - p.y).length() < PROGRESS => {
                if now.duration_since(self.last_progress) >= STUCK_AFTER {
                    tracing::debug!(
                        "route: stuck at {:?}, re-planning",
                        me - ac_world::landblock_origin(block)
                    );
                    self.route = None;
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
        let replan = match &self.route {
            None => now >= self.next_check,
            Some(r) => r.stale(goal, now),
        };
        if replan {
            self.next_check = now + LINE_CHECK;
            if now >= self.straight_blocked_until && !player.line_blocked(assets, block, me, goal) {
                if self.route.take().is_some() {
                    tracing::debug!("route: straight line clear again");
                }
                return goal;
            }
            match player.find_path(assets, block, me, goal) {
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
                }
                None => {
                    tracing::debug!("route: no path to {goal:?}, going straight");
                    self.route = None;
                    self.next_check = now + REPLAN_AFTER;
                    return goal;
                }
            }
        }
        match &mut self.route {
            Some(r) => r.target(me),
            None => goal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waypoints_advance_within_reach_and_the_goal_stays() {
        let now = Instant::now();
        let goal = Vec3::new(10.0, 0.0, 0.0);
        let mut r = Route::new(
            goal,
            vec![Vec3::new(2.0, 0.0, 0.0), Vec3::new(5.0, 3.0, 0.0), goal],
            now,
        );
        assert_eq!(r.target(Vec3::ZERO), Vec3::new(2.0, 0.0, 0.0));
        // Within 0.7 m of the first: on to the second.
        assert_eq!(r.target(Vec3::new(1.5, 0.2, 0.0)), Vec3::new(5.0, 3.0, 0.0));
        // Skipping two at once when both are close.
        assert_eq!(r.target(Vec3::new(5.0, 2.8, 0.0)), goal);
        // Standing on the goal still aims at it.
        assert_eq!(r.target(goal), goal);
        assert_eq!(r.next, 2);
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
