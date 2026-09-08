//! Character options (bindable from the menu): the server-side switches, as checkboxes, and
//! at the bottom a "Reset window layout" button that forgets where every
//! panel was dragged (`egui::Memory::reset_areas`) so they all return to
//! their default positions.

use super::{title_bar, window, Source};
use crate::{egui, Client, Ctx, Plugin, Settings};
use ac_client::options::{CharacterOption, OPTIONS};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OptionsView {
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
}

/// Blackboard key the draw distance is published on, metres as a number
/// (0 = no limit); the viewer reads it each frame.
pub const DRAW_DISTANCE_KEY: &str = "render.draw_distance";

pub fn view(c: &Client) -> OptionsView {
    OptionsView {
        rows: OPTIONS.iter().map(|o| (*o, c.option_enabled(o))).collect(),
        speed_boost_pct: (c.speed_boost * 100.0).round() as u32,
        jump_height_cm: (c.jump_height * 100.0).round() as u32,
        draw_distance_m: 0,
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
                // Likewise the jump, as the height of a full one; the
                // server calls more than 10 m above the ground a hack,
                // so 9.5 is the most. The skill's own height still wins
                // when it is more.
                let mut jump = v.jump_height_cm as f32 / 100.0;
                if ui
                    .add(
                        egui::Slider::new(&mut jump, 0.0..=9.5)
                            .text("full jump, m (0 = by skill)")
                            .fixed_decimals(1),
                    )
                    .changed()
                {
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
                jump_height_cm: 900,
                draw_distance_m: 300,
            }),
            show: false,
            speed_boost: None,
            jump_height: None,
            draw_distance: None,
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
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("options.show", self.show);
        if let Some(b) = self.speed_boost {
            settings.set("options.speed_boost", b);
        }
        if let Some(b) = self.jump_height {
            settings.set("options.jump_height", b);
        }
        if let Some(d) = self.draw_distance {
            settings.set("options.draw_distance", d);
        }
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
        let changed = draw(egui, &v);
        if super::closed("options") {
            self.show = false;
        }
        if let Some(d) = changed.draw_distance {
            self.draw_distance = Some(d);
            cx.board.set_local(DRAW_DISTANCE_KEY, d as f64);
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
    fn draw_distance_is_kept_and_published_for_the_viewer() {
        let mut o = Options::default();
        o.draw_distance = Some(250.0);
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
