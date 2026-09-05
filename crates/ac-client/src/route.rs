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

/// Where to head this frame to reach `goal` (world space, in landblock
/// `goal_block`): the goal itself while the straight line is clear or
/// the goal lies in another landblock, else the next waypoint of a route
/// planned (and re-planned as `Route::stale` says) on the graph.
/// `next_check` throttles the straight-line test while no route exists.
pub fn steer(
    route: &mut Option<Route>,
    next_check: &mut Instant,
    player: &mut Player,
    assets: &Assets,
    goal: Vec3,
    goal_block: u32,
    now: Instant,
) -> Vec3 {
    let me = player.world_position();
    let block = player.landblock();
    if goal_block & 0xFFFF_0000 != block {
        *route = None;
        return goal;
    }
    let replan = match route {
        None => now >= *next_check,
        Some(r) => r.stale(goal, now),
    };
    if replan {
        *next_check = now + LINE_CHECK;
        if !player.line_blocked(assets, block, me, goal) {
            if route.take().is_some() {
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
                *route = Some(Route::new(goal, waypoints, now));
            }
            None => {
                tracing::debug!("route: no path to {goal:?}, going straight");
                *route = None;
                *next_check = now + REPLAN_AFTER;
                return goal;
            }
        }
    }
    match route {
        Some(r) => r.target(me),
        None => goal,
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
