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
//!
//! A recall spell the character can cast (see [`Recall`] and
//! `crate::recalls`) is a hop from wherever it stands straight to the
//! spell's destination, costing only the cast. The caller says which
//! ones it can cast right now; the planner takes one when it beats the
//! walk or the portals, and a journey can go on from where it lands.

use crate::portals::{self, Portal};
use glam::Vec2;
use std::collections::BinaryHeap;

/// How fast the character covers ground (m/s), for pricing legs on foot.
pub const WALK_SPEED: f32 = 5.0;
/// A leg on foot is longer than the straight line between its ends.
pub const DETOUR: f32 = 1.3;
/// Taking a portal costs this many seconds (walking into it, the load).
pub const PORTAL_SECONDS: f32 = 20.0;
/// Casting a recall costs this many seconds: coming to a stop, the cast
/// itself, the server's two-second pause before the teleport, the load.
pub const RECALL_SECONDS: f32 = 10.0;
/// The farthest a single leg on foot is believed (metres). Longer than
/// this and the trip has to find a portal.
pub const WALK_REACH: f32 = 1200.0;
/// Portals this far from a spot are candidates for the next hop.
pub const PORTAL_REACH: f32 = 900.0;
/// Close enough to the goal to stop (metres).
pub const ARRIVED: f32 = 15.0;

/// A recall spell the character can cast right now, and where it lands:
/// what the caller hands the planner. The destination of a named
/// recall comes from `crate::recalls::fixed`; that of Lifestone Sending
/// or Portal Recall is one of the character's saved positions.
#[derive(Clone, Debug, PartialEq)]
pub struct Recall {
    pub spell: u32,
    pub name: String,
    pub exit: Vec2,
    pub exit_cell: u32,
}

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
    /// Stand still, cast this spell, and come out at its destination.
    Recall {
        spell: u32,
        name: String,
        exit: Vec2,
        exit_cell: u32,
    },
}

impl Step {
    /// Where this step takes the character.
    pub fn end(&self) -> Vec2 {
        match self {
            Step::Walk(p) => *p,
            Step::Portal { exit, .. } | Step::Recall { exit, .. } => *exit,
        }
    }

    /// Where this step takes the character, and the cell there: `None`
    /// for a walk, whose end is wherever the ground is.
    pub fn exit(&self) -> Option<(Vec2, u32)> {
        match self {
            Step::Walk(_) => None,
            Step::Portal {
                exit, exit_cell, ..
            }
            | Step::Recall {
                exit, exit_cell, ..
            } => Some((*exit, *exit_cell)),
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

    /// How many recall spells it casts.
    pub fn recalls(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| matches!(s, Step::Recall { .. }))
            .count()
    }

    /// A line for the player: "on foot, about 3 min", "2 portals, about
    /// 4 min", "Lifestone Sending, then one portal, about 2 min".
    pub fn summary(&self) -> String {
        let mins = (self.seconds / 60.0).round() as u32;
        let recall = self.steps.iter().find_map(|s| match s {
            Step::Recall { name, .. } => Some(name.as_str()),
            _ => None,
        });
        let hops = match self.portals() {
            0 => "on foot".to_string(),
            1 => "one portal".to_string(),
            n => format!("{n} portals"),
        };
        match (recall, self.portals()) {
            (None, _) => format!("{hops}, about {mins} min"),
            (Some(r), 0) => format!("{r}, about {mins} min"),
            (Some(r), _) => format!("{r}, then {hops}, about {mins} min"),
        }
    }
}

fn walk_seconds(a: Vec2, b: Vec2) -> f32 {
    a.distance(b) * DETOUR / WALK_SPEED
}

/// Whether a leg on foot between two spots is believable: short enough,
/// and not between an indoor cell and somewhere else (the character
/// cannot walk out of the Town Network hub into the countryside).
fn can_walk(a: Vec2, a_cell: u32, b: Vec2, b_cell: u32, reach: f32) -> bool {
    let indoors = |c: u32| c & 0xFFFF >= 0x100;
    // A goal given by position alone (cell 0) is outdoors in the block
    // its position lies in: from inside a shop in that block, the
    // walk out of the door is a walk, not a portal hop.
    let block_of = |p: Vec2| (((p.x / 192.0) as u32) << 24) | (((p.y / 192.0) as u32) << 16);
    let b_block = if b_cell == 0 {
        block_of(b)
    } else {
        b_cell & 0xFFFF_0000
    };
    let same_block = a_cell & 0xFFFF_0000 == b_block;
    if indoors(a_cell) || indoors(b_cell) {
        // Inside, only within the same landblock (the hub, a dungeon).
        return same_block;
    }
    a.distance(b) <= reach
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

/// How a journey should be chosen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prefs {
    /// What one portal is reckoned to cost, in seconds. Raise it and the
    /// journey takes fewer, longer hops; lower it and it hops more.
    pub portal_seconds: f32,
    /// Give up on a chain longer than this many portals.
    pub max_portals: usize,
    /// The farthest a single leg on foot is believed, metres. A player
    /// will walk a long way between towns; too small a figure here and
    /// journeys that plainly exist are reported as impossible.
    pub walk_reach: f32,
    /// How far from a spot a portal is still a candidate for the next
    /// hop, metres.
    pub portal_reach: f32,
    /// What casting a recall is reckoned to cost, in seconds.
    pub recall_seconds: f32,
    /// Whether to cast recall spells at all. Off, the recalls handed to
    /// the planner are ignored: a corpse run that must not leave the
    /// dungeon, a character saving its components.
    pub use_recalls: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs::quick()
    }
}

impl Prefs {
    /// The quickest way, however many portals it takes.
    pub fn quick() -> Self {
        Prefs {
            portal_seconds: PORTAL_SECONDS,
            max_portals: 8,
            walk_reach: WALK_REACH,
            portal_reach: PORTAL_REACH,
            recall_seconds: RECALL_SECONDS,
            use_recalls: true,
        }
    }

    /// The same journey without casting anything.
    pub fn without_recalls(self) -> Self {
        Prefs {
            use_recalls: false,
            ..self
        }
    }

    /// Fewer hops: every portal is a chance to be turned away, to land
    /// somewhere unexpected, or to walk into something on the way.
    pub fn steady() -> Self {
        Prefs {
            portal_seconds: PORTAL_SECONDS * 6.0,
            max_portals: 3,
            ..Prefs::quick()
        }
    }

    /// Walk as far as it takes: the last resort when nothing shorter
    /// reaches the goal.
    pub fn far() -> Self {
        Prefs {
            walk_reach: 12_000.0,
            portal_reach: 8_000.0,
            ..Prefs::quick()
        }
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
    plan_with(
        from,
        from_cell,
        goal,
        level,
        quests_done,
        avoid,
        Prefs::default(),
    )
}

/// [`plan_for`] with the journey's preferences (see [`Prefs`]).
#[allow(clippy::too_many_arguments)]
pub fn plan_with(
    from: Vec2,
    from_cell: u32,
    goal: Vec2,
    level: u32,
    quests_done: &[String],
    avoid: &[Vec2],
    prefs: Prefs,
) -> Option<Trip> {
    plan_with_recalls(from, from_cell, goal, level, quests_done, avoid, &[], prefs)
}

/// Which way a node of the search was reached.
#[derive(Clone, Copy)]
enum Edge {
    Portal(usize),
    Recall(usize),
}

/// [`plan_with`] for a character that can cast `recalls` right now
/// (known, with the components and mana, and somewhere for each to
/// go). A recall is a hop from the start straight to its destination
/// for [`Prefs::recall_seconds`]; it is taken when that beats walking
/// and portals, and the journey goes on from where it lands, through
/// portals if need be. One cast at most: a second recall from where the
/// first lands would have been no better cast from the start.
#[allow(clippy::too_many_arguments)]
pub fn plan_with_recalls(
    from: Vec2,
    from_cell: u32,
    goal: Vec2,
    level: u32,
    quests_done: &[String],
    avoid: &[Vec2],
    recalls: &[Recall],
    prefs: Prefs,
) -> Option<Trip> {
    let recalls: &[Recall] = if prefs.use_recalls { recalls } else { &[] };
    // Straight there, when that is a believable walk and there is no
    // spell that might be quicker.
    if recalls.is_empty() && can_walk(from, from_cell, goal, 0, prefs.walk_reach) {
        return Some(Trip {
            steps: vec![Step::Walk(goal)],
            seconds: walk_seconds(from, goal),
        });
    }
    // Otherwise search over portal exits. A node is a place to stand:
    // the start, or where a portal comes out. Only the ones this
    // character may take are in the search: walking to a portal that
    // turns us away wastes the whole trip.
    // Landblocks that have a portal standing in them: coming out inside
    // one of those is safe, because there is a way on. A portal whose
    // exit is indoors somewhere with no portal of its own is a one-way
    // trip into a dungeon the planner cannot walk out of.
    let mut has_portal = std::collections::HashSet::new();
    for p in portals::all() {
        has_portal.insert(p.from_cell & 0xFFFF_0000);
    }
    let usable: Vec<&Portal> = portals::all()
        .iter()
        .filter(|p| p.works())
        .filter(|p| level == 0 || p.usable_by(level, quests_done))
        .filter(|p| !avoid.iter().any(|a| a.distance(p.from_xy()) < 2.0))
        .filter(|p| p.exit_outdoors() || has_portal.contains(&(p.to_cell & 0xFFFF_0000)))
        .collect();
    let portals = &usable[..];
    let mut nodes = vec![Node {
        at: from,
        cell: from_cell,
    }];
    let mut came: Vec<Option<(usize, Edge)>> = vec![None]; // (node, how)
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
        if can_walk(node.at, node.cell, goal, 0, prefs.walk_reach) {
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
        // Cast a recall: from the start only, since a spell that goes to
        // the same place from anywhere is never better cast later.
        if q.idx == 0 {
            for (ri, r) in recalls.iter().enumerate() {
                let next = q.cost + prefs.recall_seconds;
                if best.map(|(b, _)| next >= b).unwrap_or(false) {
                    continue;
                }
                nodes.push(Node {
                    at: r.exit,
                    cell: r.exit_cell,
                });
                came.push(Some((q.idx, Edge::Recall(ri))));
                cost.push(next);
                let idx = nodes.len() - 1;
                queue.push(Queued {
                    cost: next,
                    est: next + walk_seconds(r.exit, goal),
                    idx,
                });
            }
        }
        // Take a portal whose mouth we can reach from here.
        for (pi, p) in portals.iter().enumerate() {
            if seen_portal[pi] {
                continue;
            }
            let mouth = p.from_xy();
            if !can_walk(node.at, node.cell, mouth, p.from_cell, prefs.walk_reach) {
                continue;
            }
            if node.at.distance(mouth) > prefs.portal_reach && node.cell & 0xFFFF < 0x100 {
                continue;
            }
            let step_cost = walk_seconds(node.at, mouth) + prefs.portal_seconds;
            let next = q.cost + step_cost;
            if best.map(|(b, _)| next >= b).unwrap_or(false) {
                continue;
            }
            seen_portal[pi] = true;
            nodes.push(Node {
                at: p.to_xy(),
                cell: p.to_cell,
            });
            came.push(Some((q.idx, Edge::Portal(pi))));
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
    // Too long a chain of portals is more chance to go wrong than it is
    // worth; the caller asked for fewer.
    let mut hops = 0;
    let mut at = end;
    while let Some((prev, how)) = came[at] {
        if matches!(how, Edge::Portal(_)) {
            hops += 1;
        }
        at = prev;
    }
    if hops > prefs.max_portals {
        return None;
    }
    // Walk back through the portals taken and the spell cast.
    let mut steps = Vec::new();
    let mut at = end;
    while let Some((prev, how)) = came[at] {
        steps.push(match how {
            Edge::Portal(pi) => {
                let p: &Portal = portals[pi];
                Step::Portal {
                    name: p.name.clone(),
                    mouth: p.from_xy(),
                    mouth_cell: p.from_cell,
                    exit: p.to_xy(),
                    exit_cell: p.to_cell,
                }
            }
            Edge::Recall(ri) => {
                let r = &recalls[ri];
                Step::Recall {
                    spell: r.spell,
                    name: r.name.clone(),
                    exit: r.exit,
                    exit_cell: r.exit_cell,
                }
            }
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

    #[test]
    fn a_walk_out_of_a_shop_to_its_own_block_is_a_walk() {
        // Inside a Holtburg shop (cell A9B4016A), bound for a spot
        // outdoors in the same block (cell unknown): a walk.
        let inside = Vec2::new(0xA9 as f32 * 192.0 + 100.0, 0xB4 as f32 * 192.0 + 100.0);
        let outside = inside + Vec2::new(20.0, 10.0);
        assert!(can_walk(inside, 0xA9B4_016A, outside, 0, 1000.0));
        // The next block over is not walked to from indoors.
        let next = inside + Vec2::new(192.0, 0.0);
        assert!(!can_walk(inside, 0xA9B4_016A, next, 0, 1000.0));
    }
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

    /// An outdoor cell for a world position (the landblock's first cell).
    fn cell_of(p: Vec2) -> u32 {
        ((p.x / 192.0) as u32) << 24 | ((p.y / 192.0) as u32) << 16 | 1
    }

    fn recall_to(name: &str, spell: u32, exit: Vec2) -> Recall {
        Recall {
            spell,
            name: name.to_string(),
            exit,
            exit_cell: cell_of(exit),
        }
    }

    #[test]
    fn a_recall_makes_a_far_journey_short() {
        let from = place("Holtburg");
        let goal = place("Arwic");
        let by_portal = plan(from, 0xA9B4_0019, goal).expect("no trip to Arwic");
        // A spell that lands a short walk from Arwic beats the Town Network.
        let recalls = [recall_to("Arwic Recall", 9001, goal + Vec2::new(80.0, 0.0))];
        let trip = plan_with_recalls(
            from,
            0xA9B4_0019,
            goal,
            0,
            &[],
            &[],
            &recalls,
            Prefs::quick(),
        )
        .expect("no trip with the recall");
        assert!(
            matches!(&trip.steps[0], Step::Recall { spell: 9001, name, .. } if name == "Arwic Recall"),
            "{trip:?}"
        );
        assert_eq!(trip.steps.last(), Some(&Step::Walk(goal)));
        assert_eq!(trip.portals(), 0);
        assert_eq!(trip.recalls(), 1);
        assert!(
            trip.seconds < by_portal.seconds,
            "{} vs {}",
            trip.seconds,
            by_portal.seconds
        );
        assert!(
            trip.summary().starts_with("Arwic Recall"),
            "{}",
            trip.summary()
        );
        assert_eq!(
            trip.steps[0].exit(),
            Some((recalls[0].exit, recalls[0].exit_cell))
        );
        // Switched off, the same journey goes by portal.
        let walked = plan_with_recalls(
            from,
            0xA9B4_0019,
            goal,
            0,
            &[],
            &[],
            &recalls,
            Prefs::quick().without_recalls(),
        )
        .expect("no trip without the recall");
        assert_eq!(walked.recalls(), 0);
        assert_eq!(walked.steps, by_portal.steps);
    }

    #[test]
    fn a_recall_that_lands_farther_than_walking_is_not_used() {
        let from = place("Holtburg");
        let near = from + Vec2::new(150.0, 80.0);
        // Lands on the far side of the continent: worse than the walk.
        let far = [recall_to("Yaraq Recall", 9002, place("Yaraq"))];
        let trip = plan_with_recalls(from, 0xA9B4_0019, near, 0, &[], &[], &far, Prefs::quick())
            .expect("no trip");
        assert_eq!(trip.steps, vec![Step::Walk(near)]);
        // Lands where we already stand: the cast is wasted.
        let here = [recall_to("Holtburg Recall", 9003, from)];
        let trip = plan_with_recalls(from, 0xA9B4_0019, near, 0, &[], &[], &here, Prefs::quick())
            .expect("no trip");
        assert_eq!(trip.steps, vec![Step::Walk(near)]);
        let goal = place("Arwic");
        let trip = plan_with_recalls(from, 0xA9B4_0019, goal, 0, &[], &[], &here, Prefs::quick())
            .expect("no trip");
        assert_eq!(trip.recalls(), 0, "{trip:?}");
        assert!(trip.portals() >= 1);
    }

    #[test]
    fn a_recall_can_be_followed_by_portals() {
        // From the open sea nothing can be walked to; a recall to
        // Holtburg puts the Town Network in reach of Arwic.
        let sea = Vec2::new(0x60 as f32 * 192.0, 0x70 as f32 * 192.0);
        let goal = place("Arwic");
        let recalls = [recall_to("Lifestone Sending", 1636, place("Holtburg"))];
        let trip = plan_with_recalls(
            sea,
            cell_of(sea),
            goal,
            0,
            &[],
            &[],
            &recalls,
            Prefs::quick(),
        )
        .expect("no trip from the sea");
        assert!(
            matches!(&trip.steps[0], Step::Recall { spell: 1636, .. }),
            "{trip:?}"
        );
        assert!(trip.portals() >= 1, "{trip:?}");
        assert!(
            trip.summary().starts_with("Lifestone Sending, then"),
            "{}",
            trip.summary()
        );
        assert_eq!(trip.steps.last(), Some(&Step::Walk(goal)));
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
