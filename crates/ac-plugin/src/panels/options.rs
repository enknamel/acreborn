//! Character options (bindable from the menu): the server-side switches, as checkboxes, and
//! at the bottom a "Reset window layout" button that forgets where every
//! panel was dragged (`egui::Memory::reset_areas`) so they all return to
//! their default positions.

use super::{title_bar, window, Source};
use crate::{egui, Client, Ctx, Plugin, Settings};
use ac_client::options::{CharacterOption, OPTIONS};
use ac_client::player::MovementRules;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionsView {
    /// The log filter in force, as a `tracing` line.
    pub log_filter: String,
    /// (option, enabled) in panel order.
    pub rows: Vec<(CharacterOption, bool)>,
    /// The client-side run speed multiplier, times 100 (so the view can
    /// stay `Eq`).
    pub speed_boost_pct: u32,
    /// The height of a full jump, in centimetres.
    pub jump_height_cm: u32,
    /// How far from the camera objects are drawn, metres (0 = no limit).
    /// The viewer's, not the client's: see [`DRAW_DISTANCE_KEY`].
    pub draw_distance_m: u32,
    /// Which movement rules the character is held to.
    pub movement_rules: MovementRules,
    /// What that comes to on the server we are connected to: true when
    /// the run boost, the jump height and flying are all held back.
    pub server_safe: bool,
    /// Flying was asked for and refused, for the panel to say so.
    pub noclip_refused: bool,
}

/// Settings key the movement rules are kept under.
pub const MOVEMENT_RULES_KEY: &str = "options.movement_rules";

/// Blackboard key the draw distance is published on, metres as a number
/// (0 = no limit); the viewer reads it each frame.
pub const DRAW_DISTANCE_KEY: &str = "render.draw_distance";
/// Which log lines reach the terminal, as a `tracing` filter line.
pub const LOG_FILTER_KEY: &str = "log.filter";

pub fn view(c: &Client) -> OptionsView {
    OptionsView {
        log_filter: String::new(),
        rows: OPTIONS.iter().map(|o| (*o, c.option_enabled(o))).collect(),
        speed_boost_pct: (c.speed_boost * 100.0).round() as u32,
        jump_height_cm: (c.jump_height * 100.0).round() as u32,
        draw_distance_m: 0,
        movement_rules: c.movement_rules,
        server_safe: c.movement_limits().is_server_safe(),
        noclip_refused: c.noclip_refused(),
    }
}

/// What the panel changed this frame.
#[derive(Default)]
pub struct Changes {
    pub options: Vec<(CharacterOption, bool)>,
    /// A new run speed multiplier.
    pub speed_boost: Option<f32>,
    /// A new full-jump height, metres.
    pub jump_height: Option<f32>,
    /// A new draw distance, metres (0 = no limit).
    pub draw_distance: Option<f32>,
    /// A new movement rules setting.
    pub movement_rules: Option<MovementRules>,
    /// The player clicked "Change…" to re-pick the game data folder.
    pub pick_data_dir: bool,
    /// A new log filter line, for the host to apply and remember.
    pub log_filter: Option<String>,
}

/// The remembered game data folder, for the Options panel to show.
fn saved_data_dir() -> String {
    std::fs::read_to_string(Settings::config_dir().join("data-dir"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "(not set)".to_string())
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
        title_bar(ui, "options", "Character options");
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
                // Fast running, high jumps and flying are the client's
                // own doing. A server that checks the moves it is told
                // about refuses them and keeps the character where its
                // own physics put it, so they are only for a server
                // that allows them -- the player's own.
                ui.horizontal(|ui| {
                    ui.label("movement rules");
                    egui::ComboBox::from_id_salt("options.movement_rules")
                        .selected_text(v.movement_rules.label())
                        .show_ui(ui, |ui| {
                            for r in MovementRules::ALL {
                                if ui
                                    .selectable_label(r == v.movement_rules, r.label())
                                    .on_hover_text(r.help())
                                    .clicked()
                                {
                                    changed.movement_rules = Some(r);
                                }
                            }
                        });
                })
                .response
                .on_hover_text(
                    "Your run speed, jump height and flying are yours to \
                     set and apply wherever you play. A server that checks \
                     the moves it is told about may refuse them, which \
                     leaves your character where it thinks you are; pick \
                     the game's rules if you would rather stay inside them.",
                );
                ui.label(egui::RichText::new(v.movement_rules.help()).weak().small());
                if v.server_safe {
                    ui.label(
                        egui::RichText::new(
                            "Held to the game's rules: running at the Run \
                             skill's pace, jumping by the Jump skill, no \
                             flying.",
                        )
                        .color(egui::Color32::from_rgb(220, 190, 120)),
                    );
                    if v.noclip_refused {
                        ui.label(
                            egui::RichText::new("Flying was asked for and refused.")
                                .color(egui::Color32::from_rgb(230, 120, 110)),
                        );
                    }
                }
                // The game runs a character at its Run skill's pace; this
                // is on top. The server does not mind, but other players
                // see the character at its proper pace, so a big boost
                // looks like skating to them.
                let mut boost = v.speed_boost_pct as f32 / 100.0;
                let mut r = ui.add(
                    egui::Slider::new(&mut boost, 0.5..=4.0)
                        .text("run speed ×")
                        .fixed_decimals(2),
                );
                if v.server_safe {
                    r = r.on_hover_text(
                        "Kept for later: the movement rules run this \
                         character at 1.00× on this server.",
                    );
                }
                if r.changed() {
                    changed.speed_boost = Some(boost);
                }
                // Likewise the jump, as the height of a full one; the
                // server calls more than 10 m above the ground a hack,
                // so 9.5 is the most. The skill's own height still wins
                // when it is more.
                let mut jump = v.jump_height_cm as f32 / 100.0;
                let mut r = ui.add(
                    egui::Slider::new(&mut jump, 0.0..=9.5)
                        .text("full jump, m (0 = by skill)")
                        .fixed_decimals(1),
                );
                if v.server_safe {
                    r = r.on_hover_text(
                        "Kept for later: the movement rules jump this \
                         character by its Jump skill on this server.",
                    );
                }
                if r.changed() {
                    changed.jump_height = Some(jump);
                }
                // Objects (creatures, items, other players) farther than
                // this are not drawn; the land and buildings always are.
                // Lower it on a slow machine or with many sessions up.
                let mut dd = v.draw_distance_m as f32;
                if ui
                    .add(
                        egui::Slider::new(&mut dd, 0.0..=1000.0)
                            .text("draw distance, m (0 = no limit)")
                            .step_by(10.0)
                            .fixed_decimals(0),
                    )
                    .changed()
                {
                    changed.draw_distance = Some(dd);
                }
            });
        ui.separator();
        ui.label("Game data folder");
        ui.horizontal_wrapped(|ui| {
            ui.monospace(saved_data_dir());
        });
        if ui
            .button("Change data folder…")
            .on_hover_text("Pick the folder holding client_portal.dat and client_cell_1.dat")
            .clicked()
        {
            changed.pick_data_dir = true;
        }
        ui.separator();
        // Logging. Each part of the client can be turned up on its own,
        // which is how you watch one thing misbehave without drowning
        // in everything else.
        ui.label("Logging");
        super::caption(ui, "what reaches the terminal, per part of the client");
        let filter = v.log_filter.clone();
        let mut next: Option<String> = None;
        ui.horizontal(|ui| {
            ui.label("everything else");
            let base = crate::logging::base_of(&filter);
            egui::ComboBox::from_id_salt("log.base")
                .selected_text(&base)
                .width(90.0)
                .show_ui(ui, |ui| {
                    for lvl in crate::logging::LEVELS {
                        if ui.selectable_label(base == *lvl, *lvl).clicked() {
                            next = Some(crate::logging::with_base(&filter, lvl));
                        }
                    }
                });
        });
        for (system, what) in crate::logging::SYSTEMS {
            ui.horizontal(|ui| {
                ui.add_sized(
                    egui::vec2(96.0, 18.0),
                    egui::Label::new(egui::RichText::new(*system).small()),
                )
                .on_hover_text(*what);
                let now = crate::logging::level_of(&filter, system);
                let shown = now.clone().unwrap_or_else(|| "(default)".to_string());
                egui::ComboBox::from_id_salt(("log", system))
                    .selected_text(shown)
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(now.is_none(), "(default)").clicked() {
                            next = Some(crate::logging::with_level(&filter, system, None));
                        }
                        for lvl in crate::logging::LEVELS {
                            let on = now.as_deref() == Some(*lvl);
                            if ui.selectable_label(on, *lvl).clicked() {
                                next = Some(crate::logging::with_level(&filter, system, Some(lvl)));
                            }
                        }
                    });
            });
        }
        super::caption(ui, &filter);
        changed.log_filter = next;

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
    jump_height: Option<f32>,
    /// The draw distance chosen here, metres (0 = no limit), kept between
    /// runs and published on the blackboard for the viewer.
    draw_distance: Option<f32>,
    /// The movement rules chosen here, kept between sessions and given
    /// to each client as it appears. Unset is
    /// [`MovementRules::Unrestricted`], so the player's own run, jump and
    /// flying settings apply until they ask to be held to the rules.
    movement_rules: MovementRules,
    /// Which log lines reach the terminal, as a `tracing` filter line.
    /// Empty means the app's default.
    log_filter: String,
    /// Whether the rules were holding the character back last tick, so
    /// a change can be said once in the chat log.
    was_safe: Option<bool>,
    /// A refusal to fly was already reported in the chat log.
    said_refusal: bool,
}

impl Options {
    pub fn demo() -> Self {
        Options {
            log_filter: crate::logging::DEFAULT.to_string(),
            source: Source::Demo(OptionsView {
                log_filter: crate::logging::DEFAULT.to_string(),
                rows: OPTIONS
                    .iter()
                    .enumerate()
                    .map(|(i, o)| (*o, i % 2 == 0))
                    .collect(),
                speed_boost_pct: 200,
                jump_height_cm: 900,
                draw_distance_m: 300,
                movement_rules: MovementRules::ServerSafe,
                server_safe: true,
                noclip_refused: false,
            }),
            show: false,
            speed_boost: None,
            jump_height: None,
            draw_distance: None,
            movement_rules: MovementRules::default(),
            was_safe: None,
            said_refusal: false,
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
        if let Some(b) = settings.get::<f32>("options.jump_height") {
            self.jump_height = Some(b);
        }
        if let Some(d) = settings.get::<f32>("options.draw_distance") {
            self.draw_distance = Some(d);
        }
        if let Some(r) = settings.get::<MovementRules>(MOVEMENT_RULES_KEY) {
            self.movement_rules = r;
        }
        if let Some(f) = settings.get::<String>(LOG_FILTER_KEY) {
            self.log_filter = f;
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("options.show", self.show);
        settings.set(LOG_FILTER_KEY, &self.log_filter);
        if let Some(b) = self.speed_boost {
            settings.set("options.speed_boost", b);
        }
        if let Some(b) = self.jump_height {
            settings.set("options.jump_height", b);
        }
        if let Some(d) = self.draw_distance {
            settings.set("options.draw_distance", d);
        }
        settings.set(MOVEMENT_RULES_KEY, self.movement_rules);
    }

    fn tick(&mut self, cx: &mut Ctx) {
        // The remembered draw distance goes on the board once, for the
        // viewer; after that a script may set the key itself.
        if let Some(d) = self.draw_distance {
            if cx.board.get(DRAW_DISTANCE_KEY).is_none() {
                cx.board.set_local(DRAW_DISTANCE_KEY, d as f64);
            }
        }
        // A remembered boost applies to whatever client is here now.
        if let Some(c) = cx.try_client() {
            // The rules come first: they decide what the rest comes to.
            c.movement_rules = self.movement_rules;
            let safe = c.movement_limits().is_server_safe();
            if let Some(b) = self.speed_boost {
                if (c.speed_boost - b).abs() > 1e-3 {
                    c.set_speed_boost(b);
                }
            }
            if let Some(b) = self.jump_height {
                if (c.jump_height - b).abs() > 1e-3 {
                    c.set_jump_height(b);
                }
            }
            // Say once, in the chat log, when the rules start or stop
            // holding this character back.
            let held_back = safe
                && (self.speed_boost.unwrap_or(c.speed_boost) > 1.0
                    || self.jump_height.unwrap_or(c.jump_height) > 0.0);
            // Flying asked for and refused: say it once, wherever it was
            // asked from (the viewer's Y key, a script, a fleet leader).
            let refused = c.noclip_refused();
            if refused && !self.said_refusal {
                cx.chat.push((
                    "Flying is off: this server checks how you move \
                     (Options, \"movement rules\")."
                        .to_string(),
                    0,
                ));
            }
            self.said_refusal = refused;
            if self.was_safe != Some(safe) {
                if held_back {
                    cx.chat.push((
                        "This server checks how you move: running at the game's own pace, \
                         jumping by the Jump skill, no flying (Options, \"movement rules\")."
                            .to_string(),
                        0,
                    ));
                } else if self.was_safe == Some(true) {
                    cx.chat.push((
                        "Movement rules relaxed: the run boost, the jump height and flying \
                         are yours again."
                            .to_string(),
                        0,
                    ));
                }
                self.was_safe = Some(safe);
            }
        }
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if let Some(ask) = super::take_open(cx.board, "options") {
            self.show = ask.apply(self.show);
        }
        if !self.show {
            return;
        }
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().map(|c| view(c)),
        };
        let Some(mut v) = v else { return };
        if matches!(self.source, Source::Live) {
            // The board's value wins: a script may have set it.
            let on_board = cx
                .board
                .get(DRAW_DISTANCE_KEY)
                .and_then(|x| x.as_f64())
                .map(|x| x as f32);
            v.draw_distance_m = on_board.or(self.draw_distance).unwrap_or(0.0).round() as u32;
        }
        if v.log_filter.is_empty() {
            v.log_filter = self.log_filter.clone();
        }
        let changed = draw(egui, &v);
        if super::closed("options") {
            self.show = false;
        }
        if let Some(d) = changed.draw_distance {
            self.draw_distance = Some(d);
            cx.board.set_local(DRAW_DISTANCE_KEY, d as f64);
        }
        if changed.pick_data_dir {
            cx.pick_data_dir = true;
        }
        if let Some(f) = changed.log_filter {
            // The app owns the filter; the board carries the new line to
            // it, and the panel remembers it for next time.
            self.log_filter = f.clone();
            cx.board.set_local(LOG_FILTER_KEY, f);
        }
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            for (o, on) in changed.options {
                c.set_option(&o, on);
            }
            if let Some(b) = changed.speed_boost {
                c.set_speed_boost(b);
                self.speed_boost = Some(c.speed_boost);
            }
            if let Some(b) = changed.jump_height {
                c.set_jump_height(b);
                self.jump_height = Some(c.jump_height);
            }
            if let Some(r) = changed.movement_rules {
                self.movement_rules = r;
                c.movement_rules = r;
            }
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if pressed && crate::keys::bound("options", key) {
            self.show = !self.show;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Blackboard, IconCache};
    use std::time::Instant;

    #[test]
    fn the_movement_rules_are_the_players_choice_and_are_remembered() {
        // Out of the box the player's own run, jump and flying settings
        // apply, wherever they are playing. Nothing decides that for them.
        let o = Options::default();
        assert_eq!(o.movement_rules, MovementRules::Unrestricted);
        assert!(!MovementRules::Unrestricted.limits().is_server_safe());
        // The opt-in for being held to the game's rules.
        assert!(MovementRules::ServerSafe.limits().is_server_safe());
        // A choice survives a restart.
        let mut settings = Settings::new();
        let chosen = Options {
            movement_rules: MovementRules::ServerSafe,
            ..Options::default()
        };
        chosen.save(&mut settings);
        let mut back = Options::default();
        back.load(&settings);
        assert_eq!(back.movement_rules, MovementRules::ServerSafe);
        // Settings with nothing saved leave the default alone.
        let mut fresh = Options::default();
        fresh.load(&Settings::new());
        assert_eq!(fresh.movement_rules, MovementRules::Unrestricted);
    }

    #[test]
    fn draw_distance_is_kept_and_published_for_the_viewer() {
        let o = Options {
            draw_distance: Some(250.0),
            ..Options::default()
        };
        let mut settings = Settings::new();
        o.save(&mut settings);
        let mut back = Options::default();
        back.load(&settings);
        assert_eq!(back.draw_distance, Some(250.0));
        // The first tick puts it on the board, where the viewer reads it;
        // a value already there (a script's) is left alone.
        let mut board = Blackboard::default();
        let mut icons = IconCache::default();
        let mut cx = Ctx {
            clients: Vec::new(),
            index: 0,
            board: &mut board,
            settings: &mut settings,
            icons: &mut icons,
            dt: 0.05,
            now: Instant::now(),
            chat: Vec::new(),
            activate: None,
            quit: false,
            pick_data_dir: false,
            start_sessions: Vec::new(),
            stop_sessions: Vec::new(),
        };
        back.tick(&mut cx);
        assert_eq!(
            cx.board.get(DRAW_DISTANCE_KEY).and_then(|v| v.as_f64()),
            Some(250.0)
        );
        cx.board.set_local(DRAW_DISTANCE_KEY, 80.0);
        back.tick(&mut cx);
        assert_eq!(
            cx.board.get(DRAW_DISTANCE_KEY).and_then(|v| v.as_f64()),
            Some(80.0)
        );
        // Nothing chosen: nothing published, the viewer draws everything.
        let mut board = Blackboard::default();
        cx.board = &mut board;
        Options::default().tick(&mut cx);
        assert!(cx.board.get(DRAW_DISTANCE_KEY).is_none());
    }
}
