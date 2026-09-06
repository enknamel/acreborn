//! Planning a journey that uses portals as well as legs on foot: the
//! Town Network from a town to the hub and out again, a dungeon's
//! "Surface" portal, a town's own portal.
//!
//! The search works on positions, not on the landscape: walking between
//! two spots is priced by the straight line between them (with a detour
//! factor), which is enough to choose *which* portals to take. The exact
//! path of each leg on foot is planned by the caller when it walks it,
//! on the terrain grid or the landblock's own navigation graph.
//!
//! Only portals whose mouth can be walked to are considered, and a leg
//! on foot is only believed within [`WALK_REACH`]; the character cannot
//! walk between continents, so a trip across the sea has to be portals
//! all the way.

use crate::portals::{self, Portal};
use glam::Vec2;
use std::collections::BinaryHeap;

/// How fast the character covers ground (m/s), for pricing legs on foot.
pub const WALK_SPEED: f32 = 5.0;
/// A leg on foot is longer than the straight line between its ends.
pub const DETOUR: f32 = 1.3;
/// Taking a portal costs this many seconds (walking into it, the load).
pub const PORTAL_SECONDS: f32 = 20.0;
/// The farthest a single leg on foot is believed (metres). Longer than
/// this and the trip has to find a portal.
pub const WALK_REACH: f32 = 1200.0;
/// Portals this far from a spot are candidates for the next hop.
pub const PORTAL_REACH: f32 = 900.0;
/// Close enough to the goal to stop (metres).
pub const ARRIVED: f32 = 15.0;

/// One step of a journey.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Walk to this world position.
    Walk(Vec2),
    /// Walk into this portal's mouth and come out at its exit.
    Portal {
        name: String,
        mouth: Vec2,
        /// The cell the mouth stands in, so the walk to it knows whether
        /// it is indoors.
        mouth_cell: u32,
        exit: Vec2,
        exit_cell: u32,
    },
}

impl Step {
    /// Where this step takes the character.
    pub fn end(&self) -> Vec2 {
        match self {
            Step::Walk(p) => *p,
            Step::Portal { exit, .. } => *exit,
        }
    }
}

/// A planned journey and what it costs.
#[derive(Clone, Debug, PartialEq)]
pub struct Trip {
    pub steps: Vec<Step>,
    /// Rough time in seconds.
    pub seconds: f32,
}

impl Trip {
    /// How many portals it takes.
    pub fn portals(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| matches!(s, Step::Portal { .. }))
            .count()
    }

    /// A line for the player: "walk 320 m", "2 portals, then walk 180 m".
    pub fn summary(&self) -> String {
        let walk: f32 = self
            .steps
            .iter()
            .filter_map(|s| match s {
                Step::Walk(_) => Some(()),
                _ => None,
            })
            .count() as f32;
        let _ = walk;
        let mins = (self.seconds / 60.0).round() as u32;
        match self.portals() {
            0 => format!("on foot, about {mins} min"),
            1 => format!("one portal, about {mins} min"),
            n => format!("{n} portals, about {mins} min"),
        }
    }
}

fn walk_seconds(a: Vec2, b: Vec2) -> f32 {
    a.distance(b) * DETOUR / WALK_SPEED
}

/// Whether a leg on foot between two spots is believable: short enough,
/// and not between an indoor cell and somewhere else (the character
/// cannot walk out of the Town Network hub into the countryside).
fn can_walk(a: Vec2, a_cell: u32, b: Vec2, b_cell: u32) -> bool {
    let indoors = |c: u32| c & 0xFFFF >= 0x100;
    let same_block = a_cell & 0xFFFF_0000 == b_cell & 0xFFFF_0000;
    if indoors(a_cell) || indoors(b_cell) {
        // Inside, only within the same landblock (the hub, a dungeon).
        return same_block;
    }
    a.distance(b) <= WALK_REACH
}

/// A node of the search: somewhere the character can stand.
#[derive(Clone, Copy, PartialEq)]
struct Node {
    at: Vec2,
    cell: u32,
}

#[derive(PartialEq)]
struct Queued {
    cost: f32,
    est: f32,
    idx: usize,
}

impl Eq for Queued {}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // A min-heap on cost + estimate.
        other
            .est
            .total_cmp(&self.est)
            .then_with(|| other.cost.total_cmp(&self.cost))
    }
}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Plan a journey from `from` (in cell `from_cell`) to `goal` for a
/// character of any level, ignoring what portals ask for.
pub fn plan(from: Vec2, from_cell: u32, goal: Vec2) -> Option<Trip> {
    plan_for(from, from_cell, goal, 0, &[], &[])
}

/// Plan a journey the character can actually make: portals with a level
/// range outside `level`, or a quest not in `quests_done`, are left out,
/// as are the ones standing at any of `avoid` (mouths that have already
/// turned us away). Hundreds of portals share a name -- every dungeon
/// has a "Surface Portal" -- so one is named by where it stands.
///
/// Returns `None` when nothing reaches the goal: no walk is short enough
/// and no portal it can take comes out near it.
pub fn plan_for(
    from: Vec2,
    from_cell: u32,
    goal: Vec2,
    level: u32,
    quests_done: &[String],
    avoid: &[Vec2],
) -> Option<Trip> {
    // Straight there, when that is a believable walk.
    if can_walk(from, from_cell, goal, 0) {
        return Some(Trip {
            steps: vec![Step::Walk(goal)],
            seconds: walk_seconds(from, goal),
        });
    }
    // Otherwise search over portal exits. A node is a place to stand:
    // the start, or where a portal comes out. Only the ones this
    // character may take are in the search: walking to a portal that
    // turns us away wastes the whole trip.
    let usable: Vec<&Portal> = portals::all()
        .iter()
        .filter(|p| level == 0 || p.usable_by(level, quests_done))
        .filter(|p| !avoid.iter().any(|a| a.distance(p.from_xy()) < 2.0))
        .collect();
    let portals = &usable[..];
    let mut nodes = vec![Node {
        at: from,
        cell: from_cell,
    }];
    let mut came: Vec<Option<(usize, usize)>> = vec![None]; // (node, portal)
    let mut cost = vec![0.0f32];
    let mut queue = BinaryHeap::new();
    queue.push(Queued {
        cost: 0.0,
        est: walk_seconds(from, goal),
        idx: 0,
    });
    let mut seen_portal = vec![false; portals.len()];
    let mut best: Option<(f32, usize)> = None;
    let mut expanded = 0;
    while let Some(q) = queue.pop() {
        if q.cost > cost[q.idx] {
            continue;
        }
        let node = nodes[q.idx];
        // Could we simply walk the rest of the way?
        if can_walk(node.at, node.cell, goal, 0) {
            let total = q.cost + walk_seconds(node.at, goal);
            if best.map(|(b, _)| total < b).unwrap_or(true) {
                best = Some((total, q.idx));
            }
        }
        if best.map(|(b, _)| q.cost >= b).unwrap_or(false) {
            continue;
        }
        expanded += 1;
        if expanded > 4000 {
            break;
        }
        // Take a portal whose mouth we can reach from here.
        for (pi, p) in portals.iter().enumerate() {
            if seen_portal[pi] {
                continue;
            }
            let mouth = p.from_xy();
            if !can_walk(node.at, node.cell, mouth, p.from_cell) {
                continue;
            }
            if node.at.distance(mouth) > PORTAL_REACH && node.cell & 0xFFFF < 0x100 {
                continue;
            }
            let step_cost = walk_seconds(node.at, mouth) + PORTAL_SECONDS;
            let next = q.cost + step_cost;
            if best.map(|(b, _)| next >= b).unwrap_or(false) {
                continue;
            }
            seen_portal[pi] = true;
            nodes.push(Node {
                at: p.to_xy(),
                cell: p.to_cell,
            });
            came.push(Some((q.idx, pi)));
            cost.push(next);
            let idx = nodes.len() - 1;
            queue.push(Queued {
                cost: next,
                est: next + walk_seconds(p.to_xy(), goal),
                idx,
            });
        }
    }
    let (seconds, end) = best?;
    // Walk back through the portals taken.
    let mut steps = Vec::new();
    let mut at = end;
    while let Some((prev, pi)) = came[at] {
        let p: &Portal = portals[pi];
        steps.push(Step::Portal {
            name: p.name.clone(),
            mouth: p.from_xy(),
            mouth_cell: p.from_cell,
            exit: p.to_xy(),
            exit_cell: p.to_cell,
        });
        at = prev;
    }
    steps.reverse();
    steps.push(Step::Walk(goal));
    Some(Trip { steps, seconds })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::towns;

    fn place(name: &str) -> Vec2 {
        towns::find(name).unwrap().world_xy()
    }

    #[test]
    fn a_short_hop_is_walked() {
        let from = place("Holtburg");
        let near = from + Vec2::new(120.0, 60.0);
        let t = plan(from, 0xA9B4_0019, near).unwrap();
        assert_eq!(t.steps, vec![Step::Walk(near)]);
        assert_eq!(t.portals(), 0);
        assert!(t.summary().starts_with("on foot"));
    }

    #[test]
    fn holtburg_to_arwic_takes_portals() {
        let from = place("Holtburg");
        let goal = place("Arwic");
        let t0 = std::time::Instant::now();
        let trip = plan(from, 0xA9B4_0019, goal).expect("no trip to Arwic");
        let took = t0.elapsed();
        assert!(took.as_secs_f32() < 5.0, "planning took {took:?}");
        assert!(trip.portals() >= 1, "should use a portal: {trip:?}");
        // It ends by walking to the goal, and every portal step follows
        // one the character could reach.
        assert_eq!(trip.steps.last(), Some(&Step::Walk(goal)));
        // And it is quicker than the long walk.
        assert!(
            trip.seconds < walk_seconds(from, goal),
            "{} s by portal vs {} s on foot",
            trip.seconds,
            walk_seconds(from, goal)
        );
        // The first hop is a portal the character can reach on foot.
        if let Some(Step::Portal { mouth, .. }) = trip.steps.first() {
            assert!(
                from.distance(*mouth) <= PORTAL_REACH,
                "first portal is {:.0} m away",
                from.distance(*mouth)
            );
        }
    }

    #[test]
    fn a_portal_that_turns_us_away_is_left_out() {
        let from = place("Holtburg");
        let goal = place("Arwic");
        let trip = plan_for(from, 0xA9B4_0019, goal, 20, &[], &[]).expect("no trip");
        // Nothing in the plan asks for more than we have.
        for step in &trip.steps {
            if let Step::Portal { name, .. } = step {
                for p in crate::portals::named(name) {
                    if p.from_xy().distance(from) < 1e-3 {
                        assert!(p.usable_by(20, &[]), "{name} is not usable at level 20");
                    }
                }
            }
        }
        // Told that the first portal refused us, it finds another way.
        let first = match trip.steps.first() {
            Some(Step::Portal { mouth, .. }) => *mouth,
            other => panic!("expected a portal first, got {other:?}"),
        };
        let again = plan_for(from, 0xA9B4_0019, goal, 20, &[], &[first]);
        if let Some(t) = again {
            assert!(
                !matches!(t.steps.first(), Some(Step::Portal { mouth, .. }) if *mouth == first),
                "took the refused portal again"
            );
        }
    }

    #[test]
    fn nothing_reaches_the_open_sea() {
        let from = place("Holtburg");
        // The middle of the inland sea, far from any portal exit.
        let sea = Vec2::new(0x60 as f32 * 192.0, 0x70 as f32 * 192.0);
        let trip = plan(from, 0xA9B4_0019, sea);
        assert!(trip.is_none(), "planned a trip to the sea: {trip:?}");
    }
}
