//! Drawing the planned route on the ground.
//!
//! A character being steered somewhere is following waypoints nobody can
//! see, which makes it hard to tell a route round a wall from a route
//! into it. This lays a line of marks along the ground from the
//! character to where it is heading: the steering route first (the
//! waypoints actually being walked, which is what goes round obstacles),
//! then the rest of the overland leg beyond it.
//!
//! The marks are the particle system's quads with the flat flag set, so
//! they lie in the world's x/y plane rather than turning to face the
//! camera, and they are drawn additively so they read as light on the
//! ground rather than paint.

use ac_client::Client;
use ac_scene::particles::{Quad, SpriteImage};
use glam::{Vec2, Vec3};

/// The colour of a jump being charged: its arc, and its landing spot
/// (or, for a flight that never comes down, the last point known).
const ARC: [f32; 3] = [0.6, 0.95, 1.0];
const LANDING: [f32; 3] = [1.0, 0.82, 0.35];
const NO_LANDING: [f32; 3] = [1.0, 0.35, 0.3];
/// Every so many frames of the flight get a dot.
const ARC_EVERY: usize = 2;

/// The arc and landing spot of the jump being charged (the jump key
/// held), so the player can see where it goes and adjust it before
/// letting go. Nothing while no jump is being charged.
pub fn jump_quads(client: &mut Client) -> Vec<Quad> {
    let Some(preview) = client.jump_preview() else {
        return Vec::new();
    };
    let mut out: Vec<Quad> = Vec::with_capacity(preview.path.len() / ARC_EVERY + 1);
    for (i, p) in preview.path.iter().enumerate() {
        if i % ARC_EVERY != 0 || i + 1 == preview.path.len() {
            continue;
        }
        // Small and brighter with the power, hovering a little above the
        // feet line so the dots are not lost in the ground on take-off.
        let dot = 0.18 + 0.12 * preview.power;
        out.push(Quad {
            position: *p + Vec3::new(0.0, 0.0, 0.25),
            size: Vec2::splat(dot),
            color: [ARC[0], ARC[1], ARC[2], 0.9],
            image: SpriteImage::Solid(0x00FF_FFFF),
            additive: true,
            flat: false,
            angle: 0.0,
        });
    }
    let rgb = if preview.landed { LANDING } else { NO_LANDING };
    out.push(Quad {
        position: preview.landing + Vec3::new(0.0, 0.0, LIFT),
        size: Vec2::splat(1.0),
        color: [rgb[0], rgb[1], rgb[2], 0.95],
        image: SpriteImage::Solid(0x00FF_FFFF),
        additive: true,
        flat: true,
        angle: 0.0,
    });
    out
}

/// Marks this far apart along the path (metres).
const SPACING: f32 = 1.8;
/// How much of the route ahead is drawn. Beyond this it is off in the
/// fog and only costs us ground samples.
const AHEAD: f32 = 140.0;
/// One mark: a rung laid across the path, along by across (metres).
/// Rungs read better than dashes from behind the character, which is
/// where the camera almost always is: a dash pointing away is seen
/// nearly edge-on and all but disappears.
const DASH: Vec2 = Vec2::new(0.45, 1.1);
/// Marks are lifted this far off the ground so they do not fight with
/// it for the same depth.
const LIFT: f32 = 0.12;
/// The mark on the goal itself, a square.
const GOAL_SIZE: Vec2 = Vec2::new(1.7, 1.7);

/// The colour of the trail, and of the goal marker at the end of it.
const TRAIL: [f32; 3] = [0.35, 0.75, 1.0];
const GOAL: [f32; 3] = [1.0, 0.82, 0.35];

/// Marks along the route the client is currently walking, if any.
pub fn quads(client: &mut Client, now: f32) -> Vec<Quad> {
    let Some(player) = client.player.as_ref() else {
        return Vec::new();
    };
    let me = player.world_position();
    // The waypoints still to walk: the steering route we are on, then
    // whatever is left of the overland leg past its end.
    let mut points: Vec<Vec3> = Vec::new();
    if let Some(route) = &client.steering.route {
        points.extend(route.waypoints.iter().skip(route.next).copied());
    }
    let tail_from = points.last().copied().unwrap_or(me);
    if let Some(overland) = client.travel_route() {
        // The overland route is the whole leg, including the part
        // already walked, and it is flat: heights come from the ground.
        // Start after the waypoint nearest where the trail so far ends,
        // or nothing would stop the line being drawn back the way we
        // came.
        let nearest = overland
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                let d = |p: &Vec2| Vec2::new(p.x - tail_from.x, p.y - tail_from.y).length();
                d(a).total_cmp(&d(b))
            })
            .map(|(i, _)| i);
        if let Some(i) = nearest {
            points.extend(
                overland[i + 1..]
                    .iter()
                    .map(|p| Vec3::new(p.x, p.y, tail_from.z)),
            );
        }
    }
    if points.is_empty() {
        return Vec::new();
    }

    // Walk the path at a fixed spacing, sampling the ground under each
    // mark so the trail follows hills and stairs.
    let mut out: Vec<Quad> = Vec::new();
    let mut walked = 0.0f32;
    let mut prev = me;
    let assets = client.assets.clone();
    for (i, &next) in points.iter().enumerate() {
        let leg = Vec2::new(next.x - prev.x, next.y - prev.y).length();
        if leg > 1e-3 {
            let steps = (leg / SPACING).floor() as i32;
            for k in 1..=steps {
                let t = k as f32 * SPACING / leg;
                let p = prev.lerp(next, t);
                walked += SPACING;
                if walked > AHEAD {
                    break;
                }
                let heading = (next.y - prev.y).atan2(next.x - prev.x);
                out.push(mark(client, &assets, p, TRAIL, DASH, heading, now, walked));
            }
        }
        prev = next;
        if walked > AHEAD {
            break;
        }
        // The end of the path gets a bigger, steadier mark.
        if i + 1 == points.len() {
            out.push(mark(client, &assets, next, GOAL, GOAL_SIZE, 0.0, now, 0.0));
        }
    }
    out
}

/// One mark laid on the ground under `p`, pulsing so the trail reads as
/// running away from the character rather than sitting still.
#[allow(clippy::too_many_arguments)]
fn mark(
    client: &mut Client,
    assets: &ac_scene::Assets,
    p: Vec3,
    rgb: [f32; 3],
    size: Vec2,
    heading: f32,
    now: f32,
    along: f32,
) -> Quad {
    let z = client
        .player
        .as_mut()
        .and_then(|pl| pl.ground_height(assets, p.x, p.y, p.z))
        .unwrap_or(p.z);
    // A wave travelling along the path at about 6 m/s.
    let phase = now * 6.0 - along;
    let pulse = 0.55 + 0.45 * (phase * 0.9).sin().max(0.0);
    Quad {
        position: Vec3::new(p.x, p.y, z + LIFT),
        size,
        color: [rgb[0], rgb[1], rgb[2], pulse],
        image: SpriteImage::Solid(0x00FF_FFFF),
        additive: true,
        flat: true,
        angle: heading,
    }
}
