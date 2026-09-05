//! Scripting plugin: extend the client without recompiling. Every
//! `*.rhai` file in a directory (`~/.acreborn/scripts`, or
//! `$ACREBORN_SCRIPTS`) is loaded by a [Rhai](https://rhai.rs) engine and
//! reloaded when it changes (checked at most once a second). A script
//! defines any of these functions:
//!
//! ```text
//! fn on_event(ev)           // a server event for a session: ev.kind is
//!                           // "chat" (text, chat_kind), "sound" (volume),
//!                           // "connected", "terminated" (reason),
//!                           // "refused" (code) or "placed" (cell)
//! fn tick(dt)               // once per frame per session, dt in seconds
//! fn command(name, args)    // "/name args" typed in chat; true = handled
//! fn key(name, pressed)     // a key such as "F5" went down/up; true = consumed
//! ```
//!
//! Hooks run once per session: inside one, `me()` and every action refer
//! to that session; `with_session(i, || ...)` runs a closure as another.
//! `this` is a map that persists between calls, one per script per
//! session, for state (`this.count = 1`). The blackboard (`board_get`,
//! `board_set`) and the bus (`post`, `messages`) are shared by every
//! session and every plugin. A script that fails to compile, or whose
//! hook throws or loops forever, is reported in the chat log and skipped;
//! it never takes the client down. Session indices are zero-based.
//!
//! The functions scripts can call are listed on [`api::Api`]; how they
//! reach the live session is explained in [`api`].

pub mod api;
pub mod bridge;
pub mod scripts;

use std::path::PathBuf;

use ac_plugin::{egui, Ctx, Event, Plugin};
use rhai::{Dynamic, Map};

pub use api::{with_api, Api, Bound};
pub use bridge::CtxApi;
pub use scripts::Scripts;

/// `$ACREBORN_SCRIPTS`, else `~/.acreborn/scripts`.
pub fn default_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ACREBORN_SCRIPTS") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".acreborn").join("scripts")
}

/// The event map handed to `on_event(ev)`.
pub fn event_map(ev: &Event) -> Map {
    let mut m = Map::new();
    let kind = match ev {
        Event::Chat { text, kind } => {
            m.insert("text".into(), text.clone().into());
            m.insert("chat_kind".into(), Dynamic::from_int(*kind as i64));
            "chat"
        }
        Event::Sound { volume, .. } => {
            m.insert("volume".into(), Dynamic::from_float(*volume as f64));
            "sound"
        }
        Event::Connected => "connected",
        Event::Terminated(reason) => {
            m.insert("reason".into(), reason.clone().into());
            "terminated"
        }
        Event::Refused(code) => {
            m.insert("code".into(), Dynamic::from_int(*code as i64));
            "refused"
        }
        Event::Placed { cell } => {
            m.insert("cell".into(), Dynamic::from_int(*cell as i64));
            "placed"
        }
        Event::SpellLearned(spell) => {
            m.insert("spell".into(), Dynamic::from_int(*spell as i64));
            "spell_learned"
        }
        Event::SpellForgotten(spell) => {
            m.insert("spell".into(), Dynamic::from_int(*spell as i64));
            "spell_forgotten"
        }
    };
    m.insert("kind".into(), kind.into());
    m
}

pub struct ScriptPlugin {
    scripts: Scripts,
}

impl ScriptPlugin {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        ScriptPlugin {
            scripts: Scripts::new(dir),
        }
    }
}

impl Plugin for ScriptPlugin {
    fn name(&self) -> &str {
        "script"
    }

    fn on_event(&mut self, cx: &mut Ctx, ev: &Event) {
        let session = cx.index;
        let ev = event_map(ev);
        let mut api = CtxApi { cx };
        let _bound = Bound::new(&mut api);
        self.scripts.on_event(session, ev);
    }

    fn tick(&mut self, cx: &mut Ctx) {
        let (session, dt, now) = (cx.index, cx.dt, cx.now);
        let mut api = CtxApi { cx };
        let _bound = Bound::new(&mut api);
        self.scripts.scan(now);
        self.scripts.tick(session, dt);
    }

    fn key(&mut self, cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        let session = cx.index;
        let mut api = CtxApi { cx };
        let _bound = Bound::new(&mut api);
        self.scripts.key(session, &format!("{key:?}"), pressed)
    }

    fn command(&mut self, cx: &mut Ctx, name: &str, args: &str) -> bool {
        if name == "scripts" {
            let mut api = CtxApi { cx };
            let _bound = Bound::new(&mut api);
            if args == "reload" {
                self.scripts.reload_all();
            }
            let names = self.scripts.names();
            let dir = self.scripts.dir().display().to_string();
            let line = if names.is_empty() {
                format!("No scripts in {dir} (/scripts reload)")
            } else {
                format!("Scripts in {dir}: {}", names.join("; "))
            };
            let _ = with_api(|a| a.log(&line));
            return true;
        }
        let session = cx.index;
        let mut api = CtxApi { cx };
        let _bound = Bound::new(&mut api);
        self.scripts.command(session, name, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_become_maps() {
        let m = event_map(&Event::Chat {
            text: "hi".into(),
            kind: 3,
        });
        assert_eq!(m["kind"].clone().into_string().unwrap(), "chat");
        assert_eq!(m["text"].clone().into_string().unwrap(), "hi");
        assert_eq!(m["chat_kind"].as_int().unwrap(), 3);
        let m = event_map(&Event::Placed { cell: 0xA9B4_0001 });
        assert_eq!(m["kind"].clone().into_string().unwrap(), "placed");
        assert_eq!(m["cell"].as_int().unwrap(), 0xA9B4_0001);
        assert_eq!(
            event_map(&Event::Terminated("bye".into()))["reason"]
                .clone()
                .into_string()
                .unwrap(),
            "bye"
        );
    }

    #[test]
    fn default_dir_honours_override() {
        // Only checks the shape: the env var is process-global.
        let d = default_dir();
        assert!(d.ends_with("scripts") || std::env::var_os("ACREBORN_SCRIPTS").is_some());
    }
}
