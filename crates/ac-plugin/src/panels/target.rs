//! Target bar: the creature we are attacking (or have selected) and its
//! health, top centre.

use super::{frame, ItemDrag, Source};
use crate::{egui, Client, Ctx, Plugin};

/// The target's name and health fraction.
#[derive(Clone, Debug, PartialEq)]
pub struct TargetView {
    pub guid: u32,
    pub name: String,
    pub health: f32,
}

/// `Drudge Skulker  60%`.
pub fn bar_text(name: &str, health: f32) -> String {
    format!("{name}  {:.0}%", health.clamp(0.0, 1.0) * 100.0)
}

/// The attack target first, else the selection, if it is a creature.
/// Creatures we have not hit yet show as full health.
pub fn view(c: &Client) -> Option<TargetView> {
    c.attack_target
        .or(c.selected)
        .and_then(|g| c.world.objects.get(&g))
        .filter(|o| o.item_type & ac_world::item_type::CREATURE != 0)
        .map(|o| TargetView {
            guid: o.guid,
            name: o.name.clone(),
            health: o.health.unwrap_or(1.0),
        })
}

/// Draw the bar; returns a carried item dropped on it (give it to the
/// target).
pub fn draw(egui: &egui::Context, t: &TargetView) -> Option<u32> {
    let w = egui.viewport_rect().width();
    let mut given = None;
    egui::Area::new(egui::Id::new("target"))
        .fade_in(false)
        .fixed_pos(egui::pos2(w * 0.5 - 130.0, 8.0))
        .show(egui, |ui| {
            let (zone, _) = ui.dnd_drop_zone::<ItemDrag, _>(egui::Frame::new(), |ui| {
                frame(160, 6).show(ui, |ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(248.0, 18.0), egui::Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(rect, 3.0, egui::Color32::from_gray(40));
                    let mut fill = rect;
                    fill.set_width(rect.width() * t.health.clamp(0.0, 1.0));
                    p.rect_filled(fill, 3.0, egui::Color32::from_rgb(200, 40, 40));
                    p.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        bar_text(&t.name, t.health),
                        egui::FontId::proportional(14.0),
                        egui::Color32::WHITE,
                    );
                });
            });
            if let Some(p) = zone.response.dnd_release_payload::<ItemDrag>() {
                given = Some(p.0);
            }
        });
    given
}

#[derive(Default)]
pub struct Target {
    source: Source<TargetView>,
}

impl Target {
    pub fn demo() -> Self {
        Target {
            source: Source::Demo(TargetView {
                guid: 0,
                name: "Demo Drudge".into(),
                health: 0.6,
            }),
        }
    }
}

impl Plugin for Target {
    fn name(&self) -> &str {
        "target"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        let t = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().and_then(|c| view(c)),
        };
        if let Some(t) = t {
            let given = draw(egui, &t);
            if let (Some(item), Source::Live, Some(c)) = (given, &self.source, cx.try_client()) {
                // Dropping on a creature hands the item over; on a selected
                // object (a chest, an item on the ground) a kit, stone or
                // key is applied to it. Using an item on a creature is
                // "select them, then use the item".
                let creature = c
                    .world
                    .objects
                    .get(&t.guid)
                    .is_some_and(|o| o.item_type & ac_world::item_type::CREATURE != 0);
                let usable = c
                    .world
                    .objects
                    .get(&item)
                    .is_some_and(|o| ac_world::usable::needs_target(o.usable));
                if creature {
                    c.give(t.guid, item, None);
                } else if usable {
                    c.use_on(item, t.guid);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_text_rounds_percent() {
        assert_eq!(bar_text("Drudge Skulker", 0.6), "Drudge Skulker  60%");
        assert_eq!(bar_text("Rat", 1.0), "Rat  100%");
        assert_eq!(bar_text("Rat", 1.7), "Rat  100%");
        assert_eq!(bar_text("Rat", 0.004), "Rat  0%");
    }
}
