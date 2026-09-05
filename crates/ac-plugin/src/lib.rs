//! The plugin interface.
//!
//! A plugin sees every session in the process through [`Ctx`]: the full
//! game state (`ac_client::Client` exposes the world, the character sheet,
//! inventory, vendors, containers), every action a player can take
//! (use, attack, cast, buy, sell, say, move), and the events the server
//! produced. It can draw egui panels and windows, react to keys, and handle
//! `/commands` typed into the chat box. Plugins are plain Rust types
//! registered with the host; several sessions may be driven by one plugin.

use std::time::Instant;

pub use ac_client::{self, Client, Event};
pub use egui;

/// What a plugin callback can see and do. `client` is the session the
/// callback is about (for `ui`/`key`/`command`, the active one).
pub struct Ctx<'a> {
    pub client: &'a mut Client,
    /// Index of `client` among the process's sessions, and how many there are.
    pub client_index: usize,
    pub client_count: usize,
    pub dt: f32,
    pub now: Instant,
    /// Lines to add to the active chat log (kind 0 = system yellow).
    pub chat: Vec<(String, u32)>,
    /// Ask the host to switch the active session (drawn, steered by keys).
    pub activate: Option<usize>,
}

impl Ctx<'_> {
    /// Say something in the chat log without sending it to the server.
    pub fn log(&mut self, text: impl Into<String>) {
        self.chat.push((text.into(), 0));
    }
}

pub trait Plugin {
    fn name(&self) -> &str;

    /// A server event for `cx.client` (chat lines, sounds, placement...).
    fn on_event(&mut self, _cx: &mut Ctx, _ev: &Event) {}

    /// Once per frame per session, after the session ticked.
    fn tick(&mut self, _cx: &mut Ctx) {}

    /// Draw panels for the active session. Runs inside the frame's egui
    /// pass; use `egui::Window`/`egui::Area` freely.
    fn ui(&mut self, _cx: &mut Ctx, _egui: &egui::Context) {}

    /// A key went down or up while no text box had focus. Return true to
    /// consume it so the client's own bindings ignore it.
    fn key(&mut self, _cx: &mut Ctx, _key: egui::Key, _pressed: bool) -> bool {
        false
    }

    /// `/name args` typed in the chat box. Return true when handled.
    fn command(&mut self, _cx: &mut Ctx, _name: &str, _args: &str) -> bool {
        false
    }
}

/// Split `/attack Drudge Skulker` into `("attack", "Drudge Skulker")`.
pub fn parse_command(line: &str) -> Option<(&str, &str)> {
    let rest = line.strip_prefix('/')?;
    let (name, args) = match rest.split_once(' ') {
        Some((n, a)) => (n, a.trim()),
        None => (rest, ""),
    };
    (!name.is_empty()).then_some((name, args))
}

#[cfg(test)]
mod tests {
    use super::parse_command;

    #[test]
    fn commands_parse() {
        assert_eq!(
            parse_command("/attack Drudge Skulker"),
            Some(("attack", "Drudge Skulker"))
        );
        assert_eq!(parse_command("/loot"), Some(("loot", "")));
        assert_eq!(parse_command("hello"), None);
        assert_eq!(parse_command("/"), None);
    }
}
