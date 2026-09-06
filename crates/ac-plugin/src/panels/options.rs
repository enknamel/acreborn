//! Character options (X): the server-side switches, as checkboxes, and
//! at the bottom a "Reset window layout" button that forgets where every
//! panel was dragged (`egui::Memory::reset_areas`) so they all return to
//! their default positions.

use super::{title, window, Source};
use crate::{egui, Client, Ctx, Plugin};
use ac_client::options::{CharacterOption, OPTIONS};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionsView {
    /// (option, enabled) in panel order.
    pub rows: Vec<(CharacterOption, bool)>,
}

pub fn view(c: &Client) -> OptionsView {
    OptionsView {
        rows: OPTIONS.iter().map(|o| (*o, c.option_enabled(o))).collect(),
    }
}

/// Returns the options toggled this frame with their new value. The
/// layout reset is applied here directly: it only touches egui's memory.
pub fn draw(egui: &egui::Context, v: &OptionsView) -> Vec<(CharacterOption, bool)> {
    let mut changed = Vec::new();
    let mut reset_layout = false;
    let w = egui.viewport_rect().width();
    window(
        "options",
        egui::pos2(w * 0.5 - 180.0, 60.0),
        egui::vec2(360.0, 420.0),
        170,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(344.0, 408.0));
        title(ui, "Character options");
        egui::ScrollArea::vertical()
            .max_height(350.0)
            .show(ui, |ui| {
                for (o, on) in &v.rows {
                    let mut b = *on;
                    if ui.checkbox(&mut b, o.label).changed() {
                        changed.push((*o, b));
                    }
                }
            });
        ui.separator();
        if ui
            .button("Reset window layout")
            .on_hover_text("Put every panel back where it opens by default")
            .clicked()
        {
            reset_layout = true;
        }
    });
    if reset_layout {
        // Forget every area's remembered position (and size): each panel
        // reopens at its `default_pos` on the next frame.
        egui.memory_mut(|m| m.reset_areas());
    }
    changed
}

#[derive(Default)]
pub struct Options {
    source: Source<OptionsView>,
    pub show: bool,
}

impl Options {
    pub fn demo() -> Self {
        Options {
            source: Source::Demo(OptionsView {
                rows: OPTIONS
                    .iter()
                    .enumerate()
                    .map(|(i, o)| (*o, i % 2 == 0))
                    .collect(),
            }),
            show: false,
        }
    }
}

impl Plugin for Options {
    fn name(&self) -> &str {
        "options"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if !self.show {
            return;
        }
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().map(|c| view(c)),
        };
        let Some(v) = v else { return };
        let changed = draw(egui, &v);
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            for (o, on) in changed {
                c.set_option(&o, on);
            }
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if key == egui::Key::X && pressed {
            self.show = !self.show;
            return true;
        }
        false
    }
}
