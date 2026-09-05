//! The plugin interface.
//!
//! A plugin sees every session in the process through [`Ctx`]: the full
//! game state of each (`ac_client::Client` exposes the world, the character
//! sheet, inventory, vendors, containers), every action a player can take
//! (use, attack, cast, buy, sell, say, move), and the events the server
//! produced. It can draw egui panels and windows, react to keys, and handle
//! `/commands` typed into the chat box. Sessions coordinate through the
//! shared [`Blackboard`]: named values plus a message bus that any plugin
//! (for any session) can post to and read. With a [`BusClient`] attached
//! ([`Host::attach_bus`]) the blackboard also spans processes: posts go
//! out to every other process on the local bus and theirs come in as
//! messages from [`REMOTE`]. Plugins are plain Rust types registered with
//! the host.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

pub mod console;
pub mod host;
pub mod party;

pub use ac_bus::{self, BusClient, Incoming};
pub use ac_client::{self, Client, Event};
pub use egui;
pub use host::{Host, Requests};
pub use serde_json::{self, Value};

/// The `from` of a message that came over the cross-process bus; its
/// `origin` names the process.
pub const REMOTE: usize = usize::MAX;

/// A message on the bus.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// Session index of the poster, or [`REMOTE`] for another process.
    pub from: usize,
    /// The posting process's name when the message came over the
    /// cross-process bus; `None` for a local post.
    pub origin: Option<String>,
    pub topic: String,
    pub value: Value,
}

impl Message {
    pub fn is_remote(&self) -> bool {
        self.from == REMOTE
    }
}

/// Shared state for coordination: named values that persist, and messages
/// that stay readable for one full frame after they were posted.
#[derive(Debug, Default)]
pub struct Blackboard {
    pub values: HashMap<String, Value>,
    inbox: VecDeque<Message>,
    outbox: Vec<Message>,
    /// The cross-process link, if attached, and this process's name on it.
    bus: Option<(BusClient, String)>,
}

impl Blackboard {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// Set a value; with a bus attached every other process gets it too.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        let key = key.into();
        let value = value.into();
        if let Some((bus, _)) = &self.bus {
            bus.set(key.clone(), value.clone());
        }
        self.values.insert(key, value);
    }

    /// Post a message every plugin sees during the next frame (in every
    /// process on the bus, if one is attached).
    pub fn post(&mut self, from: usize, topic: impl Into<String>, value: impl Into<Value>) {
        self.outbox.push(Message {
            from,
            origin: None,
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

    /// Link this blackboard to the cross-process bus. Local posts are
    /// tagged `from: name`; the bus's current values are merged in as
    /// they arrive.
    pub fn attach_bus(&mut self, client: BusClient, name: impl Into<String>) {
        self.bus = Some((client, name.into()));
    }

    pub fn bus(&self) -> Option<&BusClient> {
        self.bus.as_ref().map(|(c, _)| c)
    }

    /// This process's name on the bus, if attached.
    pub fn bus_name(&self) -> Option<&str> {
        self.bus.as_ref().map(|(_, n)| n.as_str())
    }

    /// Called by the host once per frame: last frame's posts become
    /// readable. With a bus attached, this frame's local posts go out and
    /// the other processes' posts come in alongside them.
    pub fn end_frame(&mut self) {
        self.inbox.clear();
        if let Some((bus, name)) = &self.bus {
            for m in self.outbox.iter().filter(|m| m.origin.is_none()) {
                bus.post_as(name.as_str(), m.topic.clone(), m.value.clone());
            }
        }
        self.inbox.extend(self.outbox.drain(..));
        let incoming = match &self.bus {
            Some((bus, _)) => bus.poll(),
            None => Vec::new(),
        };
        for i in incoming {
            match i {
                Incoming::Post { from, topic, value } => self.inbox.push_back(Message {
                    from: REMOTE,
                    origin: Some(from),
                    topic,
                    value,
                }),
                Incoming::Set { key, value } => {
                    self.values.insert(key, value);
                }
                Incoming::State { values } => self.values.extend(values),
                Incoming::Connected { hosting } => {
                    tracing::info!(hosting, "bus: joined");
                }
                Incoming::Disconnected => tracing::warn!("bus: link lost, reconnecting"),
            }
        }
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
    use std::time::Duration;

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
        assert!(!b.messages().next().unwrap().is_remote());
        b.end_frame();
        assert_eq!(b.messages().count(), 0);
        b.set("leader", 1);
        assert_eq!(b.get("leader"), Some(&Value::from(1)));
    }

    /// Run `end_frame` on `b` until `pred` holds (the bus is asynchronous).
    fn frames_until(b: &mut Blackboard, what: &str, pred: impl Fn(&Blackboard) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            b.end_frame();
            if pred(b) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn blackboards_in_two_processes_share_posts_and_values() {
        // Two blackboards stand in for two processes; a real loopback hub
        // links them.
        let server = ac_bus::BusServer::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().to_string();
        let mut alice = Blackboard::default();
        alice.attach_bus(BusClient::connect(&addr, "alice").unwrap(), "alice");
        let mut bob = Blackboard::default();
        bob.attach_bus(BusClient::connect(&addr, "bob").unwrap(), "bob");
        assert_eq!(alice.bus_name(), Some("alice"));
        frames_until(&mut alice, "alice joined", |b| {
            b.bus().unwrap().is_connected()
        });
        frames_until(&mut bob, "bob joined", |b| b.bus().unwrap().is_connected());

        // A local post is readable at home next frame, and abroad soon
        // after, tagged with the process it came from.
        alice.post(0, "party.target", serde_json::json!({"guid": 42}));
        alice.end_frame();
        let home: Vec<_> = alice.messages_on("party.target").cloned().collect();
        assert_eq!(home.len(), 1);
        assert_eq!(home[0].from, 0);
        assert_eq!(home[0].origin, None);
        frames_until(&mut bob, "bob sees the post", |b| {
            b.messages_on("party.target").next().is_some()
        });
        let abroad: Vec<_> = bob.messages_on("party.target").cloned().collect();
        assert_eq!(abroad.len(), 1);
        assert_eq!(abroad[0].from, REMOTE);
        assert!(abroad[0].is_remote());
        assert_eq!(abroad[0].origin.as_deref(), Some("alice"));
        assert_eq!(abroad[0].value, serde_json::json!({"guid": 42}));
        // Like a local post it lives one frame.
        bob.end_frame();
        assert_eq!(bob.messages().count(), 0);
        // And it is not echoed back to alice.
        std::thread::sleep(Duration::from_millis(50));
        alice.end_frame();
        assert_eq!(alice.messages().count(), 0);

        // A remote post is not re-published: bob's copy stays in bob.
        // Values set on one side appear on the other and on the hub.
        bob.set("leader", "bob");
        assert_eq!(bob.get("leader"), Some(&Value::from("bob")));
        frames_until(&mut alice, "alice sees leader", |b| {
            b.get("leader").is_some()
        });
        assert_eq!(alice.get("leader"), Some(&Value::from("bob")));
        assert_eq!(server.values().get("leader"), Some(&Value::from("bob")));

        // A late process gets the values with its join.
        let mut carol = Blackboard::default();
        carol.attach_bus(BusClient::connect(&addr, "carol").unwrap(), "carol");
        frames_until(&mut carol, "carol sees leader", |b| {
            b.get("leader").is_some()
        });
        assert_eq!(carol.get("leader"), Some(&Value::from("bob")));
    }
}
