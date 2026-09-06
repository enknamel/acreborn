//! Nameplates: names over the creatures, players and portals near the
//! character in the 3D view, with a health bar under anything that has
//! been hurt or is the target. V toggles them.
//!
//! The host publishes the frame's camera on the blackboard as
//! `camera.view_proj` (16 floats, column-major) and the plugin projects
//! each object's position through it; anything behind the camera or off
//! screen is skipped. Labels fade with distance and stop at 80 m.

use super::map::Kind;
use super::Source;
use crate::{egui, Client, Ctx, Plugin, Settings};
use glam::{Mat4, Vec3, Vec4};

/// Blackboard key of the camera matrix the host sets every frame.
pub const CAMERA_KEY: &str = "camera.view_proj";
/// Farthest labelled object, metres.
pub const RANGE: f32 = 80.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Plate {
    pub guid: u32,
    pub name: String,
    pub kind: Kind,
    /// World position of the label anchor (above the head).
    pub anchor: Vec3,
    pub distance: f32,
    /// Health fraction when known and worth showing.
    pub health: Option<f32>,
    pub selected: bool,
    pub target: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlatesView {
    pub plates: Vec<Plate>,
    pub view_proj: Option<Mat4>,
}

/// The camera matrix from the blackboard value the host publishes.
pub fn camera_from(v: Option<&crate::Value>) -> Option<Mat4> {
    let arr = v?.as_array()?;
    if arr.len() != 16 {
        return None;
    }
    let mut m = [0.0f32; 16];
    for (i, x) in arr.iter().enumerate() {
        m[i] = x.as_f64()? as f32;
    }
    Some(Mat4::from_cols_array(&m))
}

/// Screen point (egui points) of a world position, or None when behind
/// the camera.
pub fn project(view_proj: &Mat4, screen: egui::Rect, world: Vec3) -> Option<egui::Pos2> {
    let clip = *view_proj * Vec4::new(world.x, world.y, world.z, 1.0);
    if clip.w <= 1e-4 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if ndc.z < -1.0 || ndc.z > 1.0 {
        return None;
    }
    Some(egui::pos2(
        screen.min.x + (ndc.x + 1.0) * 0.5 * screen.width(),
        screen.min.y + (1.0 - ndc.y) * 0.5 * screen.height(),
    ))
}

pub fn view(c: &Client, view_proj: Option<Mat4>) -> Option<PlatesView> {
    let me = c
        .player
        .as_ref()
        .map(|p| p.world_position())
        .or_else(|| c.world.player().and_then(|o| o.world_pos()))?;
    let mut plates: Vec<Plate> = c
        .world
        .drawable()
        .filter(|o| !o.is_player && !o.name.is_empty())
        .filter_map(|o| {
            let pos = o.display.or(o.position)?;
            let w = ac_world::landblock_origin(pos.cell) + pos.local;
            let distance = (w - me).length();
            if distance > RANGE {
                return None;
            }
            let kind = Kind::of(o);
            if matches!(kind, Kind::Item | Kind::Corpse | Kind::Door) {
                return None;
            }
            let target = c.attack_target == Some(o.guid);
            let selected = c.selected == Some(o.guid);
            let health = o.health.filter(|h| *h < 0.999 || target || selected);
            Some(Plate {
                guid: o.guid,
                name: o.name.clone(),
                kind,
                anchor: w + Vec3::new(0.0, 0.0, 2.1),
                distance,
                health,
                selected,
                target,
            })
        })
        .collect();
    // Farthest first so the near ones paint on top.
    plates.sort_by(|a, b| b.distance.total_cmp(&a.distance));
    Some(PlatesView { plates, view_proj })
}

pub fn draw(egui: &egui::Context, v: &PlatesView) {
    let Some(vp) = &v.view_proj else { return };
    let screen = egui.viewport_rect();
    let painter = egui.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("nameplates"),
    ));
    for p in &v.plates {
        let Some(at) = project(vp, screen, p.anchor) else {
            continue;
        };
        if !screen.expand(20.0).contains(at) {
            continue;
        }
        let fade = (1.0 - (p.distance / RANGE).powi(2)).clamp(0.25, 1.0);
        let alpha = (fade * 255.0) as u8;
        let color = p.kind.color().gamma_multiply(fade);
        let font = egui::FontId::proportional(if p.distance < 25.0 { 14.0 } else { 12.0 });
        let galley = painter.layout_no_wrap(p.name.clone(), font, color);
        let size = galley.size();
        let rect = egui::Rect::from_center_size(at, size + egui::vec2(8.0, 2.0));
        painter.rect_filled(
            rect,
            3.0,
            egui::Color32::from_black_alpha(alpha.saturating_sub(100)),
        );
        if p.selected || p.target {
            painter.rect_stroke(
                rect,
                3.0,
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(alpha)),
                egui::StrokeKind::Outside,
            );
        }
        painter.galley(rect.min + egui::vec2(4.0, 1.0), galley, color);
        if let Some(h) = p.health {
            let bar = egui::Rect::from_min_size(
                egui::pos2(rect.min.x, rect.max.y + 1.0),
                egui::vec2(rect.width(), 4.0),
            );
            painter.rect_filled(bar, 1.0, egui::Color32::from_black_alpha(alpha));
            let mut fill = bar;
            fill.set_width(bar.width() * h.clamp(0.0, 1.0));
            painter.rect_filled(
                fill,
                1.0,
                egui::Color32::from_rgb(200, 40, 40).gamma_multiply(fade),
            );
        }
    }
}

pub struct Nameplates {
    source: Source<PlatesView>,
    pub show: bool,
}

impl Default for Nameplates {
    fn default() -> Self {
        Nameplates {
            source: Source::Live,
            show: true,
        }
    }
}

impl Nameplates {
    /// A few plates in front of a demo camera.
    pub fn demo() -> Self {
        let vp = Mat4::perspective_rh(1.0, 1.6, 0.1, 500.0)
            * Mat4::look_to_rh(Vec3::new(0.0, -10.0, 2.0), Vec3::Y, Vec3::Z);
        let plate = |guid, name: &str, kind, x: f32, y: f32, health| Plate {
            guid,
            name: name.into(),
            kind,
            anchor: Vec3::new(x, y, 2.1),
            distance: (x * x + (y + 10.0) * (y + 10.0)).sqrt(),
            health,
            selected: guid == 2,
            target: guid == 2,
        };
        Nameplates {
            source: Source::Demo(PlatesView {
                plates: vec![
                    plate(1, "Reborn", Kind::Player, -3.0, 8.0, None),
                    plate(2, "Drudge Skulker", Kind::Monster, 2.0, 6.0, Some(0.45)),
                    plate(3, "Samuel the Blacksmith", Kind::Npc, 6.0, 14.0, None),
                    plate(4, "Town Portal", Kind::Portal, -8.0, 20.0, None),
                ],
                view_proj: Some(vp),
            }),
            show: true,
        }
    }
}

impl Plugin for Nameplates {
    fn name(&self) -> &str {
        "nameplates"
    }

    fn load(&mut self, settings: &Settings) {
        if let Some(v) = settings.get("nameplates.show") {
            self.show = v;
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("nameplates.show", self.show);
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if !self.show {
            return;
        }
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => {
                let vp = camera_from(cx.board.get(CAMERA_KEY));
                cx.try_client().and_then(|c| view(c, vp))
            }
        };
        if let Some(v) = v {
            draw(egui, &v);
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if key == egui::Key::V && pressed {
            self.show = !self.show;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_in_front_and_rejects_behind() {
        let vp = Mat4::perspective_rh(1.0, 1.0, 0.1, 100.0)
            * Mat4::look_to_rh(Vec3::ZERO, Vec3::Y, Vec3::Z);
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 100.0));
        // Straight ahead lands in the centre.
        let p = project(&vp, screen, Vec3::new(0.0, 10.0, 0.0)).unwrap();
        assert!(
            (p.x - 100.0).abs() < 1e-3 && (p.y - 50.0).abs() < 1e-3,
            "{p:?}"
        );
        // To the right and above: right of centre and higher up.
        let p = project(&vp, screen, Vec3::new(2.0, 10.0, 1.0)).unwrap();
        assert!(p.x > 100.0 && p.y < 50.0, "{p:?}");
        assert!(project(&vp, screen, Vec3::new(0.0, -10.0, 0.0)).is_none());
        let json = crate::Value::Array(
            vp.to_cols_array()
                .iter()
                .map(|x| (*x as f64).into())
                .collect(),
        );
        let back = camera_from(Some(&json)).unwrap();
        assert!((back - vp).abs_diff_eq(Mat4::ZERO, 1e-6));
        assert!(camera_from(Some(&crate::Value::Array(vec![]))).is_none());
        assert!(camera_from(None).is_none());
    }
}
