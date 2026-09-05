//! The plugin interface.
//!
//! A plugin sees every session in the process through [`Ctx`]: the full
//! game state of each (`ac_client::Client` exposes the world, the character
//! sheet, inventory, vendors, containers), every action a player can take
//! (use, attack, cast, buy, sell, say, move), and the events the server
//! produced. It can draw egui panels and windows, react to keys, and handle
//! `/commands` typed into the chat box. Sessions coordinate through the
//! shared [`Blackboard`]: named values plus a message bus that any plugin
//! (for any session) can post to and read. Plugins are plain Rust types
//! registered with the host.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

pub mod console;
pub mod host;

pub use ac_client::{self, Client, Event};
pub use egui;
pub use host::{Host, Requests};
pub use serde_json::{self, Value};

/// A message on the in-process bus.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// Session index of the poster.
    pub from: usize,
    pub topic: String,
    pub value: Value,
}

/// Shared state for coordination: named values that persist, and messages
/// that stay readable for one full frame after they were posted.
#[derive(Debug, Default)]
pub struct Blackboard {
    pub values: HashMap<String, Value>,
    inbox: VecDeque<Message>,
    outbox: Vec<Message>,
}

impl Blackboard {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        self.values.insert(key.into(), value.into());
    }

    /// Post a message every plugin sees during the next frame.
    pub fn post(&mut self, from: usize, topic: impl Into<String>, value: impl Into<Value>) {
        self.outbox.push(Message {
            from,
            topic: topic.into(),
            value: value.into(),
        });
    }

    /// Messages posted during the previous frame.
    pub fn messages(&self) -> impl Iterator<Item = &Message> {
        self.inbox.iter()
    }

    pub fn messages_on<'a>(&'a self, topic: &'a str) -> impl Iterator<Item = &'a Message> + 'a {
        self.inbox.iter().filter(move |m| m.topic == topic)
    }

    /// Called by the host once per frame: last frame's posts become readable.
    pub fn end_frame(&mut self) {
        self.inbox.clear();
        self.inbox.extend(self.outbox.drain(..));
    }
}

/// What a plugin callback can see and do. `index` is the session the
/// callback is about (for `ui`/`key`/`command`, the active one); every
/// session is reachable through `clients`.
pub struct Ctx<'a> {
    pub clients: Vec<&'a mut Client>,
    pub index: usize,
    pub board: &'a mut Blackboard,
    pub dt: f32,
    pub now: Instant,
    /// Lines to add to the active chat log (kind 0 = system yellow).
    pub chat: Vec<(String, u32)>,
    /// Ask the host to switch the active session (drawn, steered by keys).
    pub activate: Option<usize>,
}

impl Ctx<'_> {
    /// The session this callback is about.
    pub fn client(&mut self) -> &mut Client {
        self.clients[self.index]
    }

    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// Say something in the chat log without sending it to the server.
    pub fn log(&mut self, text: impl Into<String>) {
        self.chat.push((text.into(), 0));
    }

    /// Post on the bus as this session.
    pub fn post(&mut self, topic: impl Into<String>, value: impl Into<Value>) {
        let from = self.index;
        self.board.post(from, topic, value);
    }
}

pub trait Plugin {
    fn name(&self) -> &str;

    /// A server event for `cx.client()` (chat lines, sounds, placement...).
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
    use super::*;

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

    #[test]
    fn bus_messages_live_one_frame() {
        let mut b = Blackboard::default();
        b.post(0, "assist", serde_json::json!({"target": 0x8000_0001u32}));
        assert_eq!(b.messages().count(), 0);
        b.end_frame();
        assert_eq!(b.messages_on("assist").count(), 1);
        b.end_frame();
        assert_eq!(b.messages().count(), 0);
        b.set("leader", 1);
        assert_eq!(b.get("leader"), Some(&Value::from(1)));
    }
}
