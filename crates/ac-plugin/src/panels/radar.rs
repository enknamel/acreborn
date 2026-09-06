//! Radar: a circle in the top right corner with every object in view as a
//! dot, relative to the character and rotated so forward is up.

use glam::{Vec2, Vec3};

use super::{has_sheet, Source};
use crate::{egui, Client, Ctx, Plugin};

/// Radar range in metres (the edge of the circle).
pub const RANGE: f32 = 100.0;
/// Radius of the drawn circle in points.
pub const RADIUS: f32 = 80.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlipKind {
    Player,
    Creature,
    Other,
}

/// A radar blip in radar space: x right, y forward, metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blip {
    pub x: f32,
    pub y: f32,
    pub kind: BlipKind,
}

/// Where `at` lands on a radar centred on `me` whose up is `heading`
/// (radians, the character's yaw): (right, forward) in metres.
pub fn relative(me: Vec3, heading: f32, at: Vec3) -> (f32, f32) {
    let fwd = Vec2::new(-heading.sin(), heading.cos());
    let right = Vec2::new(heading.cos(), heading.sin());
    let d = at - me;
    let d = Vec2::new(d.x, d.y);
    (d.dot(right), d.dot(fwd))
}

/// Players by their description flag, creatures by having a motion table,
/// everything else (doors, items, scenery) as other.
pub fn kind_of(o: &ac_world::WorldObject) -> BlipKind {
    if o.object_desc_flags & ac_world::object_desc_flags::PLAYER != 0 {
        BlipKind::Player
    } else if o.motion_table_id != 0 {
        BlipKind::Creature
    } else {
        BlipKind::Other
    }
}

/// The blips for this session; `None` until the sheet arrived. Empty
/// while we have no position yet.
pub fn view(c: &Client) -> Option<Vec<Blip>> {
    if !has_sheet(c) {
        return None;
    }
    let (me, heading) = match (&c.player, c.world.player()) {
        (Some(p), _) => (ac_world::landblock_origin(p.cell) + p.local, p.heading),
        (None, Some(o)) => match o.display.or(o.position) {
            Some(pos) => (ac_world::landblock_origin(pos.cell) + pos.local, 0.0),
            None => return Some(Vec::new()),
        },
        _ => return Some(Vec::new()),
    };
    Some(
        c.world
            .drawable()
            .filter(|o| !o.is_player)
            .filter_map(|o| {
                let pos = o.display.or(o.position)?;
                let (x, y) = relative(
                    me,
                    heading,
                    ac_world::landblock_origin(pos.cell) + pos.local,
                );
                Some(Blip {
                    x,
                    y,
                    kind: kind_of(o),
                })
            })
            .collect(),
    )
}

pub fn draw(egui: &egui::Context, blips: &[Blip], range: f32) {
    let r = RADIUS;
    let w = egui.viewport_rect().width();
    let center = egui::pos2(w - r - 16.0, r + 16.0);
    egui::Area::new(egui::Id::new("radar"))
        .fade_in(false)
        .default_pos(egui::pos2(center.x - r - 4.0, center.y - r - 4.0))
        .movable(true)
        .show(egui, |ui| {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(2.0 * r + 8.0, 2.0 * r + 8.0),
                egui::Sense::hover(),
            );
            let c = rect.center();
            let p = ui.painter();
            p.circle_filled(c, r, egui::Color32::from_black_alpha(160));
            p.circle_stroke(c, r, egui::Stroke::new(1.5, egui::Color32::from_gray(180)));
            p.circle_stroke(
                c,
                r * 0.5,
                egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
            );
            p.line_segment(
                [egui::pos2(c.x, c.y - r), egui::pos2(c.x, c.y + r)],
                egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
            );
            p.line_segment(
                [egui::pos2(c.x - r, c.y), egui::pos2(c.x + r, c.y)],
                egui::Stroke::new(1.0, egui::Color32::from_gray(90)),
            );
            let scale = r / range.max(1.0);
            for b in blips {
                let d = (b.x * b.x + b.y * b.y).sqrt();
                if d > range {
                    continue;
                }
                let at = egui::pos2(c.x + b.x * scale, c.y - b.y * scale);
                let (col, rad) = match b.kind {
                    BlipKind::Player => (egui::Color32::from_rgb(90, 220, 90), 3.5),
                    BlipKind::Creature => (egui::Color32::from_rgb(230, 120, 40), 3.0),
                    BlipKind::Other => (egui::Color32::from_gray(200), 2.0),
                };
                p.circle_filled(at, rad, col);
            }
            p.circle_filled(c, 3.0, egui::Color32::WHITE);
        });
}

pub struct Radar {
    source: Source<Vec<Blip>>,
    /// Radar range in metres (edge of the circle).
    pub range: f32,
}

impl Default for Radar {
    fn default() -> Self {
        Radar {
            source: Source::Live,
            range: RANGE,
        }
    }
}

impl Radar {
    pub fn demo() -> Self {
        let blip = |x, y, kind| Blip { x, y, kind };
        Radar {
            source: Source::Demo(vec![
                blip(12.0, 30.0, BlipKind::Player),
                blip(-40.0, 15.0, BlipKind::Creature),
                blip(25.0, -20.0, BlipKind::Creature),
                blip(-8.0, -45.0, BlipKind::Other),
                blip(60.0, 55.0, BlipKind::Other),
                blip(150.0, 0.0, BlipKind::Creature), // out of range, not drawn
            ]),
            range: RANGE,
        }
    }
}

impl Plugin for Radar {
    fn name(&self) -> &str {
        "radar"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        let blips = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().and_then(|c| view(c)),
        };
        if let Some(blips) = blips {
            draw(egui, &blips, self.range);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4
    }

    #[test]
    fn relative_rotates_with_heading() {
        let me = Vec3::new(10.0, 10.0, 0.0);
        // Facing +y (heading 0): something north is straight ahead.
        assert!(close(
            relative(me, 0.0, Vec3::new(10.0, 15.0, 3.0)),
            (0.0, 5.0)
        ));
        // Something east is to the right.
        assert!(close(
            relative(me, 0.0, Vec3::new(14.0, 10.0, 0.0)),
            (4.0, 0.0)
        ));
        // Turned a quarter left (heading pi/2 faces -x): east is now behind.
        let q = std::f32::consts::FRAC_PI_2;
        assert!(close(
            relative(me, q, Vec3::new(14.0, 10.0, 0.0)),
            (0.0, -4.0)
        ));
    }
}
