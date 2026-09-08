//! A template plugin: copy this crate, rename it, and fill in the hooks.
//!
//! It shows the four things most plugins need:
//!
//! * a **panel** (`ui`): an egui window drawn with the client's own
//!   window helper, so it is movable and its position is remembered;
//! * a **key** (`key`): F7 opens and closes the panel;
//! * a **setting** (`load`/`save`): whether the panel is open and the
//!   greeting it shows survive a restart through the shared settings file;
//! * a **command** (`command`): `/hello [name]` says hello in local chat.
//!
//! It also reacts to events (`on_event`: chat lines and autoplay's status
//! changes), acts on the session in `tick`, and talks to other sessions
//! and processes over the blackboard (`cx.post`, `messages_on`). Every
//! hook copes with running without a session (`cx.try_client()` is
//! `None` in the offline runner and in `--demo-ui`).

use ac_plugin::{egui, panels, Ctx, Event, Plugin, Settings};
use serde::{Deserialize, Serialize};

/// The topic this plugin posts on when it greets someone; other
/// sessions and processes read it with `messages_on(HELLO_TOPIC)`.
pub const HELLO_TOPIC: &str = "template.hello";

/// The settings keys this plugin owns (prefixed with its name so plugins
/// do not collide).
const SHOW_KEY: &str = "template.show";
const OPTIONS_KEY: &str = "template.options";

/// What survives a restart, as one JSON value in the settings file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Options {
    pub greeting: String,
    /// Log every chat line the panel counts (noisy; off by default).
    pub verbose: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            greeting: "Hello".into(),
            verbose: false,
        }
    }
}

#[derive(Default)]
pub struct Template {
    /// The panel is open.
    pub show: bool,
    pub options: Options,
    /// Chat lines seen since the plugin started.
    pub chat_lines: usize,
    /// What autoplay last said it was doing.
    pub autoplay: Option<(String, String)>,
    /// The last greeting heard on the bus (from any session or process).
    pub last_hello: Option<String>,
}

impl Template {
    pub fn new() -> Self {
        Self::default()
    }

    /// The panel window; returns what was clicked.
    fn draw(&mut self, egui: &egui::Context, session: Option<String>) -> PanelClicks {
        let mut clicks = PanelClicks::default();
        panels::window(
            "template",
            egui::pos2(300.0, 120.0),
            egui::vec2(260.0, 150.0),
            200,
            8,
        )
        .show(egui, |ui| {
            panels::title_bar(ui, "template", "Template  [F7]");
            match session {
                Some(name) => ui.label(format!("Session: {name}")),
                None => ui.label("No session (offline)"),
            };
            ui.label(format!("Chat lines seen: {}", self.chat_lines));
            if let Some((doing, text)) = &self.autoplay {
                ui.label(format!("Autoplay: {doing} ({text})"));
            }
            if let Some(h) = &self.last_hello {
                panels::caption(ui, format!("last hello: {h}"));
            }
            ui.horizontal(|ui| {
                ui.label("Greeting");
                ui.text_edit_singleline(&mut self.options.greeting);
            });
            ui.checkbox(&mut self.options.verbose, "Log every chat line");
            if ui.button("Say hello").clicked() {
                clicks.say_hello = true;
            }
        });
        if panels::closed("template") {
            clicks.close = true;
        }
        clicks
    }

    /// Greet `who` in local chat (or only in the log with no session).
    fn greet(&mut self, cx: &mut Ctx, who: &str) {
        let who = if who.trim().is_empty() {
            "everyone"
        } else {
            who.trim()
        };
        let line = format!("{}, {who}!", self.options.greeting);
        match cx.try_client() {
            Some(c) => c.say(&line),
            None => cx.log(format!("(no session) would say: {line}")),
        }
        cx.post(HELLO_TOPIC, serde_json::json!({ "who": who, "line": line }));
    }
}

/// What the panel's widgets asked for this frame.
#[derive(Default)]
struct PanelClicks {
    say_hello: bool,
    close: bool,
}

impl Plugin for Template {
    fn name(&self) -> &str {
        "template"
    }

    fn on_event(&mut self, cx: &mut Ctx, ev: &Event) {
        match ev {
            Event::Chat { text, .. } => {
                self.chat_lines += 1;
                if self.options.verbose {
                    cx.log(format!("template: chat #{}: {text}", self.chat_lines));
                }
            }
            Event::Autoplay { doing, text } => {
                self.autoplay = Some((doing.clone(), text.clone()));
            }
            _ => {}
        }
    }

    fn tick(&mut self, cx: &mut Ctx) {
        // Hear greetings from every session and process (our own too).
        let heard: Vec<String> = cx
            .board
            .messages_on(HELLO_TOPIC)
            .map(|m| {
                let line = m.value["line"].as_str().unwrap_or("").to_string();
                match &m.origin {
                    Some(process) => format!("{line} (from {process})"),
                    None => format!("{line} (session {})", m.from),
                }
            })
            .collect();
        if let Some(last) = heard.into_iter().last() {
            self.last_hello = Some(last);
        }
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if let Some(ask) = panels::take_open(cx.board, "template") {
            self.show = ask.apply(self.show);
        }
        if !self.show {
            return;
        }
        let session = cx.try_client().map(|c| c.world.stats.name.clone());
        let clicks = self.draw(egui, session);
        if clicks.say_hello {
            self.greet(cx, "");
        }
        if clicks.close {
            self.show = false;
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if pressed && key == egui::Key::F7 {
            self.show = !self.show;
            return true;
        }
        false
    }

    fn command(&mut self, cx: &mut Ctx, name: &str, args: &str) -> bool {
        match name {
            "hello" => {
                self.greet(cx, args);
                true
            }
            "template" => {
                self.show = !self.show;
                cx.log(format!(
                    "template panel {}",
                    if self.show { "open" } else { "closed" }
                ));
                true
            }
            _ => false,
        }
    }

    fn load(&mut self, settings: &Settings) {
        if let Some(show) = settings.get(SHOW_KEY) {
            self.show = show;
        }
        if let Some(options) = settings.get(OPTIONS_KEY) {
            self.options = options;
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set(SHOW_KEY, self.show);
        settings.set(OPTIONS_KEY, &self.options);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_plugin::Host;
    use std::time::Instant;

    /// The whole plugin, driven by a host with no session: events, a
    /// command, the key, the panel drawn by a headless egui, and the
    /// settings round trip.
    #[test]
    fn events_command_key_panel_and_settings() {
        let mut host = Host::new();
        host.register(Box::new(Template::new()));

        // Events reach `on_event`; the command answers in the chat log.
        let events = [
            Event::Chat {
                text: "Drudge Skulker says, \"Grr\"".into(),
                kind: 3,
            },
            Event::Autoplay {
                doing: "fighting".into(),
                text: "fighting Drudge Skulker".into(),
            },
        ];
        let r = host.frame(Vec::new(), 0, &events, 0.05, Instant::now());
        assert!(r.chat.is_empty(), "verbose is off");
        let r = host.command(Vec::new(), 0, "/hello Asheron");
        assert!(r.consumed);
        assert_eq!(r.chat[0].0, "(no session) would say: Hello, Asheron!");
        // The greeting went out on the bus and is heard next frame.
        host.end_frame();
        let r = host.frame(Vec::new(), 0, &[], 0.05, Instant::now());
        assert!(r.chat.is_empty());

        // F7 opens the panel; a headless egui pass draws it.
        assert!(host.key(Vec::new(), 0, egui::Key::F7, true).consumed);
        assert!(!host.key(Vec::new(), 0, egui::Key::F7, false).consumed);
        let egui = egui::Context::default();
        let _ = egui.run_ui(egui::RawInput::default(), |ctx| {
            host.ui(Vec::new(), 0, ctx);
        });
        let drawn = egui.memory(|m| m.area_rect(egui::Id::new("template")));
        assert!(drawn.is_some(), "the panel window was laid out");

        // Settings: what `save` stores, `load` takes back.
        let mut settings = Settings::new();
        let mut p = Template::new();
        p.show = true;
        p.options.greeting = "Greetings".into();
        p.save(&mut settings);
        let mut back = Template::new();
        back.load(&settings);
        assert!(back.show);
        assert_eq!(back.options.greeting, "Greetings");
    }
}
