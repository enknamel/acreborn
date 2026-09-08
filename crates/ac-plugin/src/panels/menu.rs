//! The menu (Escape): every panel the client has, each with the key
//! that opens it, and the keys themselves, changeable by a click and a
//! keypress and kept in the settings for every client on the machine.
//! Escape first closes the window opened last; with none open it shows
//! the menu, which also quits the client.
//!
//! A small "Menu" button sits in the top-left corner while in the world
//! for anyone who would rather click.

use super::{title_bar, window, Ask};
use crate::{egui, keys, Ctx, Plugin};

/// What the menu's draw returned.
#[derive(Default)]
struct Actions {
    /// Panels clicked: toggle them.
    open: Vec<&'static str>,
    /// A binding row clicked: wait for a key for this action.
    rebind: Option<&'static str>,
    /// The "none" button beside a binding: unbind it.
    unbind: Option<&'static str>,
    reset_keys: bool,
    quit: bool,
    /// The menu itself, from its title bar or a button.
    close: bool,
}

#[derive(Default)]
pub struct Menu {
    pub show: bool,
    /// The action whose key is being chosen: the next key pressed is it.
    listening: Option<&'static str>,
    /// The egui frame drawn last, for Escape to know what is open.
    frame: u64,
    /// Whether the little corner button is drawn (only in the world).
    in_world: bool,
}

impl Menu {
    pub fn demo() -> Self {
        Menu {
            show: true,
            ..Default::default()
        }
    }
}

fn draw(egui: &egui::Context, listening: Option<&str>, in_world: bool) -> Actions {
    let mut a = Actions::default();
    let w = egui.viewport_rect().width();
    let h = egui.viewport_rect().height();
    window(
        "menu",
        egui::pos2(w * 0.5 - 200.0, h * 0.5 - 250.0),
        egui::vec2(400.0, 500.0),
        200,
        8,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(384.0, 484.0));
        title_bar(ui, "menu", "Menu");
        if let Some(l) = listening {
            let label = keys::ACTIONS
                .iter()
                .find(|x| x.id == l)
                .map(|x| x.label)
                .unwrap_or(l);
            ui.colored_label(
                egui::Color32::from_rgb(255, 210, 90),
                format!("Press a key for {label} (Escape: keep it as it is)"),
            );
        } else {
            ui.weak("Click a panel to open it; click its key to change it.");
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                egui::Grid::new("menu.panels")
                    .num_columns(3)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        for act in keys::ACTIONS.iter().filter(|x| x.id != "menu") {
                            if ui
                                .add(egui::Button::new(act.label).min_size(egui::vec2(160.0, 0.0)))
                                .clicked()
                            {
                                a.open.push(act.id);
                            }
                            let key = keys::binding_label(act.id);
                            let choosing = listening == Some(act.id);
                            let text = if choosing { "..." } else { key.as_str() };
                            if ui
                                .add(egui::Button::new(text).min_size(egui::vec2(64.0, 0.0)))
                                .on_hover_text("Click, then press the key to use")
                                .clicked()
                            {
                                a.rebind = Some(act.id);
                            }
                            if keys::binding(act.id).is_some()
                                && ui.small_button("none").on_hover_text("No key").clicked()
                            {
                                a.unbind = Some(act.id);
                            }
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
                ui.weak("Always:");
                for (k, what) in keys::FIXED {
                    ui.horizontal(|ui| {
                        ui.monospace(*k);
                        ui.weak(*what);
                    });
                }
            });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Keys back to default").clicked() {
                a.reset_keys = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Quit").clicked() {
                    a.quit = true;
                }
                if ui.button("Close").clicked() {
                    a.close = true;
                }
            });
        });
    });
    if in_world {
        // The corner button, always there for the mouse.
        egui::Area::new(egui::Id::new("menu.button"))
            .fixed_pos(egui::pos2(egui.viewport_rect().width() - 78.0, 6.0))
            .show(egui, |ui| {
                if ui
                    .add(egui::Button::new("☰ Menu").small())
                    .on_hover_text(format!("[{}]", keys::binding_label("menu")))
                    .clicked()
                {
                    a.close = false;
                    a.open.push("menu");
                }
            });
    }
    a
}

impl Plugin for Menu {
    fn name(&self) -> &str {
        "menu"
    }

    fn load(&mut self, settings: &crate::Settings) {
        keys::load(settings);
    }

    fn save(&self, settings: &mut crate::Settings) {
        keys::save(settings);
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        self.frame = egui.cumulative_frame_nr();
        self.in_world =
            cx.try_client().is_some_and(|c| super::has_sheet(c)) || cx.clients.is_empty();
        if let Some(ask) = super::take_open(cx.board, "menu") {
            self.show = ask.apply(self.show);
        }
        // The corner button is drawn whether the menu is open or not.
        let a = if self.show {
            draw(egui, self.listening, self.in_world)
        } else if self.in_world {
            let mut a = Actions::default();
            egui::Area::new(egui::Id::new("menu.button"))
                .fixed_pos(egui::pos2(egui.viewport_rect().width() - 78.0, 6.0))
                .show(egui, |ui| {
                    if ui
                        .add(egui::Button::new("☰ Menu").small())
                        .on_hover_text(format!("[{}]", keys::binding_label("menu")))
                        .clicked()
                    {
                        a.open.push("menu");
                    }
                });
            a
        } else {
            Actions::default()
        };
        for id in a.open {
            if id == "menu" {
                self.show = !self.show;
            } else {
                super::request_open(cx.board, id, Ask::Toggle);
            }
        }
        if let Some(id) = a.rebind {
            self.listening = Some(id);
        }
        if let Some(id) = a.unbind {
            keys::rebind(id, None);
            cx.settings.dirty = true;
        }
        if a.reset_keys {
            keys::reset();
            cx.settings.dirty = true;
        }
        if a.close || super::closed("menu") {
            self.show = false;
            self.listening = None;
        }
        if a.quit {
            cx.quit = true;
        }
    }

    fn key(&mut self, cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if !pressed {
            return false;
        }
        // Choosing a key: this is it (Escape leaves the binding alone).
        if let Some(id) = self.listening.take() {
            if key != egui::Key::Escape {
                keys::rebind(id, Some(key));
                cx.settings.dirty = true;
            }
            return true;
        }
        if keys::bound("menu", key) {
            // Escape (or whatever the menu is on) closes the newest
            // window first; the menu itself opens when nothing is.
            if self.show {
                self.show = false;
                return true;
            }
            if super::close_newest(self.frame) {
                return true;
            }
            self.show = true;
            return true;
        }
        false
    }
}
