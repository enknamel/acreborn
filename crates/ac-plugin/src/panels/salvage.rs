//! Salvage: opened by using the Ust from the pack. Lists the carried
//! items the server would salvage (loot with a material and a
//! workmanship), tick the ones to destroy, or drag items in, and Salvage
//! sends them; the yields come back in chat and the salvage bags land in
//! the pack. Tinkering is applying a salvage bag to an item: drag the bag
//! onto it in the inventory panel, and answer the chance-of-success
//! question.

use super::{item_row, title, window, Item, ItemDrag, Source};
use crate::icons::IconCache;
use crate::{egui, Client, Ctx, Plugin};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub item: Item,
    pub material: String,
    /// Workmanship times 100 (an integer for the view's `Eq`).
    pub workmanship_x100: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SalvageView {
    pub has_tool: bool,
    pub candidates: Vec<Candidate>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Actions {
    pub toggle: Vec<u32>,
    /// Carried items dragged in: tick them.
    pub add: Vec<u32>,
    pub salvage: bool,
    pub clear: bool,
    pub close: bool,
}

pub fn view(c: &Client) -> Option<SalvageView> {
    if !c.salvage_open {
        return None;
    }
    let candidates = c
        .salvageable()
        .into_iter()
        .filter_map(|g| c.world.objects.get(&g))
        .map(|o| Candidate {
            item: Item::of(o, false),
            material: ac_world::material::name(o.material).to_string(),
            workmanship_x100: (o.workmanship * 100.0).round() as u32,
        })
        .collect();
    Some(SalvageView {
        has_tool: c.salvage_tool().is_some(),
        candidates,
    })
}

/// "Oak, workmanship 8.0".
pub fn detail(c: &Candidate) -> String {
    format!(
        "{}, workmanship {:.1}",
        c.material,
        c.workmanship_x100 as f32 / 100.0
    )
}

pub fn draw(
    egui: &egui::Context,
    icons: &mut IconCache,
    v: &SalvageView,
    chosen: &[u32],
) -> Actions {
    let mut actions = Actions::default();
    let rect = egui.viewport_rect();
    window(
        "salvage",
        egui::pos2(rect.width() * 0.5 - 200.0, rect.height() * 0.25),
        egui::vec2(400.0, 300.0),
        170,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(384.0, 284.0));
        title(ui, "Salvage");
        if !v.has_tool {
            ui.label(
                egui::RichText::new("You need an Ust in your pack.")
                    .color(egui::Color32::from_rgb(255, 150, 150)),
            );
        }
        let (r, _) = ui.dnd_drop_zone::<ItemDrag, _>(egui::Frame::new().inner_margin(2), |ui| {
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .show(ui, |ui| {
                    ui.set_min_width(370.0);
                    if v.candidates.is_empty() {
                        ui.label(
                            egui::RichText::new(
                                "Nothing salvageable in the pack: loot with a material and a workmanship.",
                            )
                            .color(egui::Color32::from_gray(170))
                            .small(),
                        );
                    }
                    for c in &v.candidates {
                        ui.horizontal(|ui| {
                            let mut on = chosen.contains(&c.item.guid);
                            if ui.checkbox(&mut on, "").changed() {
                                actions.toggle.push(c.item.guid);
                            }
                            if item_row(ui, icons, &c.item, egui::Color32::WHITE).clicked() {
                                actions.toggle.push(c.item.guid);
                            }
                            ui.label(
                                egui::RichText::new(detail(c))
                                    .color(egui::Color32::from_gray(180))
                                    .small(),
                            );
                        });
                    }
                    ui.label(
                        egui::RichText::new("drop items here to add them")
                            .color(egui::Color32::from_gray(140))
                            .small(),
                    );
                });
        });
        if let Some(p) = r.response.dnd_release_payload::<ItemDrag>() {
            actions.add.push(p.0);
        }
        ui.horizontal(|ui| {
            let n = chosen.len();
            if ui
                .add_enabled(
                    v.has_tool && n > 0,
                    egui::Button::new(format!("Salvage {n} item{}", if n == 1 { "" } else { "s" })),
                )
                .clicked()
            {
                actions.salvage = true;
            }
            if ui.button("Clear").clicked() {
                actions.clear = true;
            }
            if ui.button("Close").clicked() {
                actions.close = true;
            }
        });
        ui.label(
            egui::RichText::new(
                "Salvaged items are destroyed. Units: 1 + skill / 194 × workmanship, \
                 the Salvaging skill or the best trained tinkering skill.",
            )
            .color(egui::Color32::from_gray(150))
            .small(),
        );
    });
    actions
}

#[derive(Default)]
pub struct Salvage {
    source: Source<SalvageView>,
    chosen: Vec<u32>,
}

impl Salvage {
    pub fn demo() -> Self {
        use super::inventory::demo_item;
        let cand = |guid, name: &str, icon, material: &str, ws| Candidate {
            item: demo_item(guid, name, 1, false, icon),
            material: material.into(),
            workmanship_x100: ws,
        };
        Salvage {
            source: Source::Demo(SalvageView {
                has_tool: true,
                candidates: vec![
                    cand(21, "Stormwood Bow", 0x0600_2C0D, "Oak", 800),
                    cand(22, "Iron Dagger", 0x0600_601C, "Iron", 550),
                    cand(23, "Leather Cap", 0x0600_6A21, "Leather", 300),
                ],
            }),
            chosen: vec![21, 22],
        }
    }
}

impl Plugin for Salvage {
    fn name(&self) -> &str {
        "salvage"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().and_then(|c| view(c)),
        };
        let Some(v) = v else {
            self.chosen.clear();
            return;
        };
        // Items that left the pack (salvaged, dropped) leave the selection.
        self.chosen
            .retain(|g| v.candidates.iter().any(|c| c.item.guid == *g));
        let a = draw(egui, cx.icons(), &v, &self.chosen);
        for g in a.toggle {
            if let Some(i) = self.chosen.iter().position(|x| *x == g) {
                self.chosen.remove(i);
            } else {
                self.chosen.push(g);
            }
        }
        for g in a.add {
            if v.candidates.iter().any(|c| c.item.guid == g) && !self.chosen.contains(&g) {
                self.chosen.push(g);
            }
        }
        if a.clear {
            self.chosen.clear();
        }
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            if a.salvage && c.salvage(&self.chosen) {
                self.chosen.clear();
            }
            if a.close {
                c.salvage_open = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_text() {
        let c = Candidate {
            item: super::super::inventory::demo_item(1, "Bow", 1, false, 0),
            material: "Oak".into(),
            workmanship_x100: 825,
        };
        assert_eq!(detail(&c), "Oak, workmanship 8.2");
    }
}
