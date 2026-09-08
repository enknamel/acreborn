//! Character options (X): the server-side switches, as checkboxes, and
//! at the bottom a "Reset window layout" button that forgets where every
//! panel was dragged (`egui::Memory::reset_areas`) so they all return to
//! their default positions.

use super::{title, window, Source};
use crate::{egui, Client, Ctx, Plugin, Settings};
use ac_client::options::{CharacterOption, OPTIONS};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionsView {
    /// (option, enabled) in panel order.
    pub rows: Vec<(CharacterOption, bool)>,
    /// The client-side run speed multiplier, times 100 (so the view can
    /// stay `Eq`).
    pub speed_boost_pct: u32,
    /// The jump height multiplier, times 100.
    pub jump_boost_pct: u32,
}

pub fn view(c: &Client) -> OptionsView {
    OptionsView {
        rows: OPTIONS.iter().map(|o| (*o, c.option_enabled(o))).collect(),
        speed_boost_pct: (c.speed_boost * 100.0).round() as u32,
        jump_boost_pct: (c.jump_boost * 100.0).round() as u32,
    }
}

/// What the panel changed this frame.
#[derive(Default)]
pub struct Changes {
    pub options: Vec<(CharacterOption, bool)>,
    /// A new run speed multiplier.
    pub speed_boost: Option<f32>,
    /// A new jump height multiplier.
    pub jump_boost: Option<f32>,
}

/// Returns the options toggled this frame with their new value. The
/// layout reset is applied here directly: it only touches egui's memory.
pub fn draw(egui: &egui::Context, v: &OptionsView) -> Changes {
    let mut changed = Changes::default();
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
                        changed.options.push((*o, b));
                    }
                }
                ui.separator();
                ui.label("This client");
                // The game runs a character at its Run skill's pace; this
                // is on top. The server does not mind, but other players
                // see the character at its proper pace, so a big boost
                // looks like skating to them.
                let mut boost = v.speed_boost_pct as f32 / 100.0;
                if ui
                    .add(
                        egui::Slider::new(&mut boost, 0.5..=4.0)
                            .text("run speed ×")
                            .fixed_decimals(2),
                    )
                    .changed()
                {
                    changed.speed_boost = Some(boost);
                }
                // Likewise the jump; the client keeps the height under
                // what the server calls a hack (10 m above the ground).
                let mut jump = v.jump_boost_pct as f32 / 100.0;
                if ui
                    .add(
                        egui::Slider::new(&mut jump, 0.5..=6.0)
                            .text("jump height ×")
                            .fixed_decimals(2),
                    )
                    .changed()
                {
                    changed.jump_boost = Some(jump);
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
        // Forget every area's remembered position (and size), and the
        // positions read back from the settings file, so each panel
        // reopens at its built-in place on the next frame.
        egui.memory_mut(|m| m.reset_areas());
        super::forget_positions();
    }
    changed
}

#[derive(Default)]
pub struct Options {
    source: Source<OptionsView>,
    pub show: bool,
    /// The run speed multiplier chosen here, kept between sessions and
    /// given to each client as it appears.
    speed_boost: Option<f32>,
    jump_boost: Option<f32>,
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
                speed_boost_pct: 200,
                jump_boost_pct: 200,
            }),
            show: false,
            speed_boost: None,
            jump_boost: None,
        }
    }
}

impl Plugin for Options {
    fn name(&self) -> &str {
        "options"
    }

    fn load(&mut self, settings: &Settings) {
        if let Some(v) = settings.get("options.show") {
            self.show = v;
        }
        if let Some(b) = settings.get::<f32>("options.speed_boost") {
            self.speed_boost = Some(b);
        }
        if let Some(b) = settings.get::<f32>("options.jump_boost") {
            self.jump_boost = Some(b);
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("options.show", self.show);
        if let Some(b) = self.speed_boost {
            settings.set("options.speed_boost", b);
        }
        if let Some(b) = self.jump_boost {
            settings.set("options.jump_boost", b);
        }
    }

    fn tick(&mut self, cx: &mut Ctx) {
        // A remembered boost applies to whatever client is here now.
        if let Some(c) = cx.try_client() {
            if let Some(b) = self.speed_boost {
                if (c.speed_boost - b).abs() > 1e-3 {
                    c.set_speed_boost(b);
                }
            }
            if let Some(b) = self.jump_boost {
                if (c.jump_boost - b).abs() > 1e-3 {
                    c.set_jump_boost(b);
                }
            }
        }
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
            for (o, on) in changed.options {
                c.set_option(&o, on);
            }
            if let Some(b) = changed.speed_boost {
                c.set_speed_boost(b);
                self.speed_boost = Some(c.speed_boost);
            }
            if let Some(b) = changed.jump_boost {
                c.set_jump_boost(b);
                self.jump_boost = Some(c.jump_boost);
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
