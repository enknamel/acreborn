//! Vitals: the character's name and level over three bars (health,
//! stamina, mana), top left under the status line.

use super::{frame, has_sheet, ItemDrag, Source};
use crate::{egui, Client, Ctx, Plugin};

/// One vital bar: label, current, maximum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VitalBar {
    pub name: &'static str,
    pub current: u32,
    pub max: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VitalsView {
    pub name: String,
    pub level: i32,
    pub bars: Vec<VitalBar>,
    /// The jump charge while Space is held, as power 0..=1.
    pub jump_charge: Option<f32>,
}

/// How full a bar is, 0..=1; empty when the maximum is unknown.
pub fn fraction(current: u32, max: u32) -> f32 {
    if max > 0 {
        (current as f32 / max as f32).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// What to draw for this session; nothing until the sheet arrived.
pub fn view(c: &Client) -> Option<VitalsView> {
    if !has_sheet(c) {
        return None;
    }
    let st = &c.world.stats;
    Some(VitalsView {
        name: st.name.clone(),
        level: st.level,
        jump_charge: c.jump_charge(),
        bars: (0..3)
            .map(|i| VitalBar {
                name: ac_world::stats::VITAL_NAMES[i],
                current: st.vitals[i].current,
                max: st.vital_max(i),
            })
            .collect(),
    })
}

/// Draw the bars; returns a carried item dropped on them (use it on
/// yourself: a healing kit, food, a potion).
pub fn draw(egui: &egui::Context, v: &VitalsView) -> Option<u32> {
    let mut dropped = None;
    egui::Area::new(egui::Id::new("vitals"))
        .fade_in(false)
        .default_pos(egui::pos2(8.0, 36.0))
        .movable(true)
        .show(egui, |ui| {
            let (zone, _) = ui.dnd_drop_zone::<ItemDrag, _>(egui::Frame::new(), |ui| {
                frame(160, 6).show(ui, |ui| {
                    ui.set_min_width(220.0);
                    ui.label(
                        egui::RichText::new(format!("{}  (level {})", v.name, v.level))
                            .color(egui::Color32::WHITE)
                            .strong(),
                    );
                    for (i, b) in v.bars.iter().enumerate() {
                        let color = [
                            egui::Color32::from_rgb(200, 40, 40),
                            egui::Color32::from_rgb(220, 180, 40),
                            egui::Color32::from_rgb(50, 90, 220),
                        ][i.min(2)];
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(220.0, 16.0), egui::Sense::hover());
                        let p = ui.painter();
                        p.rect_filled(rect, 3.0, egui::Color32::from_gray(40));
                        let mut fill = rect;
                        fill.set_width(rect.width() * fraction(b.current, b.max));
                        p.rect_filled(fill, 3.0, color);
                        p.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            format!("{} {}/{}", b.name, b.current, b.max),
                            egui::FontId::proportional(13.0),
                            egui::Color32::WHITE,
                        );
                    }
                    if let Some(charge) = v.jump_charge {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(220.0, 8.0), egui::Sense::hover());
                        let p = ui.painter();
                        p.rect_filled(rect, 2.0, egui::Color32::from_gray(40));
                        let mut fill = rect;
                        fill.set_width(rect.width() * charge.clamp(0.0, 1.0));
                        p.rect_filled(fill, 2.0, egui::Color32::from_rgb(120, 220, 120));
                    }
                });
            });
            if let Some(p) = zone.response.dnd_release_payload::<ItemDrag>() {
                dropped = Some(p.0);
            }
        });
    dropped
}

#[derive(Default)]
pub struct Vitals {
    source: Source<VitalsView>,
}

impl Vitals {
    pub fn demo() -> Self {
        Vitals {
            source: Source::Demo(VitalsView {
                name: "Demo".into(),
                level: 1,
                jump_charge: Some(0.6),
                bars: vec![
                    VitalBar {
                        name: "Health",
                        current: 42,
                        max: 60,
                    },
                    VitalBar {
                        name: "Stamina",
                        current: 90,
                        max: 100,
                    },
                    VitalBar {
                        name: "Mana",
                        current: 10,
                        max: 80,
                    },
                ],
            }),
        }
    }
}

impl Plugin for Vitals {
    fn name(&self) -> &str {
        "vitals"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().and_then(|c| view(c)),
        };
        if let Some(v) = v {
            let dropped = draw(egui, &v);
            if let (Some(item), Source::Live, Some(c)) = (dropped, &self.source, cx.try_client()) {
                // A kit or potion on yourself; anything else is "use".
                match c.world.player_guid {
                    Some(me) => {
                        c.use_on(item, me);
                    }
                    None => c.interact(item),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractions_clamp() {
        assert_eq!(fraction(0, 0), 0.0);
        assert_eq!(fraction(5, 10), 0.5);
        assert_eq!(fraction(20, 10), 1.0);
    }
}
