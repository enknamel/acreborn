//! Exercising a script without a server: a [`ScriptHarness`] loads a
//! script, feeds it events, ticks, commands and keys against a canned
//! view of the character (the [`Recorder`], an [`Api`] that answers
//! reads from its fields and writes every action down), and lets a test
//! assert on what the script did.
//!
//! ```no_run
//! use ac_script::testing::ScriptHarness;
//!
//! let mut h = ScriptHarness::from_source(
//!     "greet.rhai",
//!     r#"fn command(name, args) { if name == "hi" { say("hi " + args); return true; } false }"#,
//! );
//! assert!(h.command("hi", "there"));
//! assert_eq!(h.calls(), ["[0] say hi there"]);
//! ```
//!
//! The module is always compiled (not only under `cfg(test)`) so a
//! plugin crate or a script author's own tests can use it too.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use ac_client::Event;
use ac_plugin::Message;
use rhai::{Array, Dynamic, Map};
use serde_json::Value;

use crate::api::{Api, Bound};
use crate::scripts::Scripts;

/// An [`Api`] that records every action and answers reads from canned
/// data. Fill the `pub` fields in before a call and read them after:
/// `calls` are the actions taken (`"[session] say hi"`), `logs` the chat
/// lines logged, `posted` the bus posts, `board` the values set.
#[derive(Default)]
pub struct Recorder {
    pub calls: Vec<String>,
    pub logs: Vec<String>,
    pub autoplay_on: bool,
    pub session: i64,
    pub sessions: i64,
    pub me: Map,
    pub objects: Array,
    pub board: HashMap<String, Value>,
    pub posted: Vec<(String, Value)>,
    pub inbox: Vec<Message>,
    pub activate: Option<i64>,
    /// What `item_stats()` / `find_items()` answer, and how many of
    /// them count as unappraised.
    pub items: Array,
    pub unappraised: i64,
    pub traveling: bool,
    /// The loot rules as `(query, action)`, what `loot_rules()` answers
    /// and `loot_rule_add` / `loot_rules_clear` edit.
    pub loot_rules: Vec<(String, String)>,
}

impl Recorder {
    /// Two sessions, session 0 current, nothing canned.
    pub fn new() -> Self {
        Recorder {
            sessions: 2,
            ..Default::default()
        }
    }

    /// Note an action taken by the current session.
    pub fn record(&mut self, s: impl std::fmt::Display) {
        self.calls.push(format!("[{}] {s}", self.session));
    }
}

impl Api for Recorder {
    fn me(&mut self) -> Map {
        let mut m = self.me.clone();
        m.insert("session".into(), Dynamic::from_int(self.session));
        m
    }
    fn travel_style(&mut self, style: &str) -> String {
        self.calls.push(format!("travel_style({style})"));
        style.to_string()
    }
    fn autoplay(&mut self, on: bool) -> bool {
        self.calls.push(format!("autoplay({on})"));
        self.autoplay_on = on;
        on
    }
    fn autoplay_status(&mut self) -> String {
        if self.autoplay_on {
            "fighting Drudge Skulker".into()
        } else {
            String::new()
        }
    }
    fn fight_style(&mut self, style: &str) -> String {
        self.calls.push(format!("fight_style({style})"));
        style.to_string()
    }
    fn team(&mut self, on: bool) -> bool {
        self.calls.push(format!("team({on})"));
        on
    }
    fn team_lead(&mut self, on: bool) -> bool {
        self.record(format!("team_lead {on}"));
        on
    }
    fn growth(&mut self, on: bool) -> bool {
        self.record(format!("growth {on}"));
        on
    }
    fn fight(&mut self, on: bool) -> bool {
        self.record(format!("fight {on}"));
        on
    }
    fn follow_distances(&mut self, keep: f64, fight: f64) -> f64 {
        self.record(format!("follow_distances {keep} {fight}"));
        fight
    }
    fn follow(&mut self, on: bool) -> bool {
        self.record(format!("follow {on}"));
        on
    }
    fn team_role(&mut self, role: &str) -> String {
        self.calls.push(format!("team_role({role})"));
        role.to_string()
    }
    fn teammates(&mut self) -> Array {
        self.calls.push("teammates()".into());
        Array::new()
    }
    fn enchantments(&mut self) -> Array {
        self.calls.push("enchantments()".into());
        Array::new()
    }
    fn wanted_buffs(&mut self) -> Array {
        self.calls.push("wanted_buffs()".into());
        Array::new()
    }
    fn attack_spells(&mut self, names: Array) -> Array {
        let listed: Vec<String> = names.iter().map(|v| v.to_string()).collect();
        self.calls
            .push(format!("attack_spells({})", listed.join(",")));
        names
    }
    fn objects(&mut self) -> Array {
        self.objects.clone()
    }
    fn inventory(&mut self) -> Array {
        Array::new()
    }
    fn container(&mut self) -> Array {
        Array::new()
    }
    fn session_count(&mut self) -> i64 {
        self.sessions
    }
    fn session(&mut self, i: i64) -> Option<Map> {
        (i >= 0 && i < self.sessions).then(|| {
            let mut m = Map::new();
            m.insert("session".into(), Dynamic::from_int(i));
            m
        })
    }
    fn current_session(&mut self) -> i64 {
        self.session
    }
    fn set_session(&mut self, i: i64) -> bool {
        let ok = i >= 0 && i < self.sessions;
        if ok {
            self.session = i;
        }
        ok
    }
    fn use_name(&mut self, name: &str) -> bool {
        self.record(format!("use {name}"));
        true
    }
    fn use_guid(&mut self, guid: i64) -> bool {
        self.record(format!("use #{guid}"));
        true
    }
    fn activate(&mut self, _g: i64) -> bool {
        false
    }
    fn pickup(&mut self, _g: i64) -> bool {
        false
    }
    fn attack(&mut self, name: &str) -> bool {
        self.record(format!("attack {name}"));
        true
    }
    fn attack_guid(&mut self, guid: i64) -> bool {
        self.record(format!("attack #{guid}"));
        true
    }
    fn cast(&mut self, name: &str) -> bool {
        self.record(format!("cast {name}"));
        true
    }
    fn can_cast(&mut self, name: &str) -> String {
        self.record(format!("can_cast {name}"));
        "ok".into()
    }
    fn components(&mut self) -> Array {
        Array::new()
    }
    fn fill_components(&mut self) -> i64 {
        self.record("fill_components");
        0
    }
    fn set_desired_component(&mut self, _name: &str, _quantity: i64) -> bool {
        false
    }
    fn say(&mut self, text: &str) {
        self.record(format!("say {text}"));
    }
    fn loot(&mut self, name: &str) -> bool {
        self.record(format!("loot {name}"));
        true
    }
    fn take(&mut self, guid: i64) -> bool {
        self.record(format!("take #{guid}"));
        true
    }
    fn raise(&mut self, _w: &str) -> bool {
        false
    }
    fn train(&mut self, _s: &str) -> bool {
        false
    }
    fn trade_open(&mut self, _p: i64) {}
    fn trade_add(&mut self, _i: i64) -> bool {
        false
    }
    fn trade_accept(&mut self) {}
    fn trade_decline(&mut self) {}
    fn trade_reset(&mut self) {}
    fn trade_close(&mut self) {}
    fn trade(&mut self) -> Map {
        Map::new()
    }
    fn fellow_create(&mut self, _n: &str, _s: bool) {}
    fn fellow_recruit(&mut self, _p: i64) {}
    fn fellow_quit(&mut self, _d: bool) {}
    fn confirmations(&mut self) -> rhai::Array {
        rhai::Array::new()
    }
    fn confirm(&mut self, _y: bool) -> bool {
        false
    }
    fn fellowship(&mut self) -> Dynamic {
        Dynamic::UNIT
    }
    fn swear(&mut self, _p: i64) -> bool {
        false
    }
    fn break_allegiance(&mut self, _m: i64) -> bool {
        false
    }
    fn allegiance(&mut self) -> Dynamic {
        Dynamic::UNIT
    }
    fn allegiance_refresh(&mut self) {}
    fn salvageable(&mut self) -> rhai::Array {
        rhai::Array::new()
    }
    fn salvage(&mut self, _items: rhai::Array) -> bool {
        false
    }
    fn item_stats(&mut self) -> Array {
        self.items.clone()
    }
    fn find_items(&mut self, query: &str) -> Array {
        self.record(format!("find_items {query}"));
        self.items.clone()
    }
    fn loot_rules(&mut self) -> Array {
        self.loot_rules
            .iter()
            .map(|(q, a)| {
                let mut m = Map::new();
                m.insert("query".into(), q.clone().into());
                m.insert("action".into(), a.clone().into());
                Dynamic::from_map(m)
            })
            .collect()
    }
    fn loot_rule_add(&mut self, query: &str, action: &str) -> bool {
        self.record(format!("loot_rule_add {query} {action}"));
        if ac_client::autoplay::LootAction::parse(action).is_none()
            || ac_client::items::Query::check(query).is_err()
        {
            return false;
        }
        self.loot_rules
            .push((query.trim().to_string(), action.trim().to_lowercase()));
        true
    }
    fn loot_rules_clear(&mut self) {
        self.record("loot_rules_clear");
        self.loot_rules.clear();
    }
    fn loot_action(&mut self, guid: i64) -> String {
        self.record(format!("loot_action {guid}"));
        "keep".into()
    }
    fn salvager(&mut self) -> Dynamic {
        Dynamic::UNIT
    }
    fn appraise_all(&mut self) -> i64 {
        self.record("appraise_all");
        std::mem::take(&mut self.unappraised)
    }
    fn unappraised(&mut self) -> i64 {
        self.unappraised
    }
    fn allegiance_name(&mut self, _n: &str) {}
    fn house_profile(&mut self) -> Dynamic {
        Dynamic::UNIT
    }
    fn house(&mut self) -> Dynamic {
        Dynamic::UNIT
    }
    fn house_query(&mut self) {}
    fn buy_house(&mut self) -> bool {
        false
    }
    fn rent_house(&mut self) -> bool {
        false
    }
    fn abandon_house(&mut self) {}
    fn house_guests(&mut self) -> Dynamic {
        Dynamic::UNIT
    }
    fn house_guest(&mut self, _n: &str, _add: bool) {}
    fn house_storage(&mut self, _n: &str, _on: bool) {}
    fn house_open(&mut self, _on: bool) {}
    fn chat(&mut self, _c: &str, _t: &str) -> bool {
        false
    }
    fn option(&mut self, _n: &str, _on: bool) -> bool {
        false
    }
    fn use_on(&mut self, _i: i64, _t: i64) -> bool {
        false
    }
    fn drop_item(&mut self, _guid: i64) -> bool {
        false
    }
    fn give(&mut self, _t: i64, _i: i64, _n: i64) -> bool {
        false
    }
    fn put_in(&mut self, _i: i64, _c: i64) -> bool {
        false
    }
    fn appraise(&mut self, _g: i64) {}
    fn book(&mut self) -> Dynamic {
        Dynamic::UNIT
    }
    fn read_page(&mut self, _i: i64) {}
    fn augmentations(&mut self) -> rhai::Array {
        rhai::Array::new()
    }
    fn emote(&mut self, _w: &str) -> bool {
        false
    }
    fn friends(&mut self) -> rhai::Array {
        rhai::Array::new()
    }
    fn add_friend(&mut self, _n: &str) {}
    fn remove_friend(&mut self, _g: i64) {}
    fn titles(&mut self) -> Map {
        Map::new()
    }
    fn set_title(&mut self, _t: i64) {}
    fn squelch(&mut self, _n: &str, _on: bool) {}
    fn squelches(&mut self) -> rhai::Array {
        rhai::Array::new()
    }
    fn appraisal(&mut self, _g: i64) -> Dynamic {
        Dynamic::UNIT
    }
    fn split(&mut self, _i: i64, _n: i64) -> bool {
        false
    }
    fn merge(&mut self, _f: i64, _t: i64) -> bool {
        false
    }
    fn take_all(&mut self) -> i64 {
        self.record("take_all");
        0
    }
    fn close_container(&mut self) {
        self.record("close_container");
    }
    fn buy(&mut self, name: &str) -> bool {
        self.record(format!("buy {name}"));
        true
    }
    fn sell(&mut self, name: &str) -> bool {
        self.record(format!("sell {name}"));
        true
    }
    fn combat(&mut self, on: bool) {
        self.record(format!("combat {on}"));
    }
    fn jump(&mut self, p: f64) {
        self.record(format!("jump {p}"));
    }
    fn speed_boost(&mut self, b: f64) {
        self.record(format!("speed_boost {b}"));
    }
    fn jump_height(&mut self, m: f64) {
        self.record(format!("jump_height {m}"));
    }
    fn noclip(&mut self, on: bool) {
        self.record(format!("noclip {on}"));
    }
    fn select(&mut self, guid: i64) {
        self.record(format!("select #{guid}"));
    }
    fn log(&mut self, text: &str) {
        self.logs.push(text.to_string());
    }
    fn post(&mut self, topic: &str, value: Value) {
        self.posted.push((topic.to_string(), value));
    }
    fn messages(&mut self, topic: &str) -> Vec<Message> {
        self.inbox
            .iter()
            .filter(|m| m.topic == topic)
            .cloned()
            .collect()
    }
    fn board_get(&mut self, key: &str) -> Option<Value> {
        self.board.get(key).cloned()
    }
    fn board_set(&mut self, key: &str, value: Value) {
        self.board.insert(key.to_string(), value);
    }
    fn switch(&mut self, i: i64) {
        self.activate = Some(i);
    }
    fn travel_to(&mut self, destination: &str) -> bool {
        self.record(format!("travel_to {destination}"));
        self.traveling = ac_world::towns::parse_destination(destination).is_some();
        self.traveling
    }
    fn traveling(&mut self) -> bool {
        self.traveling
    }
    fn cancel_travel(&mut self) {
        self.record("cancel_travel");
        self.traveling = false;
    }
    fn place(&mut self, name: &str) -> Dynamic {
        crate::api::place_map(name)
    }
}

/// A fresh, empty directory for one harness's scripts.
fn fresh_dir() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "ac-script-harness-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("creating the harness directory");
    dir
}

/// A script (or a directory of them) and a [`Recorder`] to run it
/// against. Every call binds the recorder for its duration, so hooks see
/// the API the way they do in the client; between calls the recorder's
/// fields can be read and changed freely (`h.api.me.insert(..)`).
pub struct ScriptHarness {
    scripts: Scripts,
    /// The canned world and the record of what the script did.
    pub api: Recorder,
    dir: PathBuf,
}

impl ScriptHarness {
    /// Load every `*.rhai` in `dir` (an examples directory, say).
    pub fn from_dir(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        let mut h = ScriptHarness {
            scripts: Scripts::new(&dir),
            api: Recorder::new(),
            dir,
        };
        h.reload();
        h
    }

    /// Load one script from its source text, written to a fresh directory
    /// under `name` (so errors name it).
    pub fn from_source(name: &str, source: &str) -> Self {
        let dir = fresh_dir();
        std::fs::write(dir.join(name), source).expect("writing the script");
        Self::from_dir(dir)
    }

    /// Load one script file on its own (copied into a fresh directory,
    /// so its neighbours are not loaded with it).
    pub fn from_file(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let dir = fresh_dir();
        let name = path.file_name().expect("a file name");
        std::fs::copy(path, dir.join(name)).expect("copying the script");
        Self::from_dir(dir)
    }

    /// The directory the scripts are loaded from; write another file
    /// there and [`reload`](Self::reload) to test hot reloading.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Rescan the directory now (the client does so once a second).
    pub fn reload(&mut self) -> bool {
        let _bound = Bound::new(&mut self.api);
        self.scripts.rescan()
    }

    /// The loaded scripts with their hooks, as `/scripts` lists them.
    pub fn names(&self) -> Vec<String> {
        self.scripts.names()
    }

    /// Make session `i` the current one for the calls that follow (the
    /// recorder has two sessions unless `api.sessions` is changed).
    pub fn session(&mut self, i: i64) -> &mut Self {
        assert!(
            i >= 0 && i < self.api.sessions,
            "no session {i} (the recorder has {})",
            self.api.sessions
        );
        self.api.session = i;
        self
    }

    /// Set a field of what `me()` answers.
    pub fn me(&mut self, key: &str, value: impl Into<Dynamic>) -> &mut Self {
        self.api.me.insert(key.into(), value.into());
        self
    }

    /// What `objects()` answers, nearest first.
    pub fn objects(&mut self, objects: Array) -> &mut Self {
        self.api.objects = objects;
        self
    }

    /// Put a message where `messages(topic)` finds it, as if another
    /// session (`from`) had posted it last frame; `origin` names another
    /// process, `None` is this one.
    pub fn deliver(
        &mut self,
        from: usize,
        origin: Option<&str>,
        topic: &str,
        value: Value,
    ) -> &mut Self {
        self.api.inbox.push(Message {
            from,
            origin: origin.map(str::to_string),
            topic: topic.to_string(),
            value,
        });
        self
    }

    /// Drop the delivered messages (they live one frame in the client).
    pub fn clear_messages(&mut self) -> &mut Self {
        self.api.inbox.clear();
        self
    }

    /// A server event for the current session, converted the way the
    /// client converts it.
    pub fn event(&mut self, ev: &Event) {
        self.event_map(crate::event_map(ev));
    }

    /// An event as the map `on_event(ev)` receives (`kind` and fields).
    pub fn event_map(&mut self, ev: Map) {
        let session = self.api.session as usize;
        let _bound = Bound::new(&mut self.api);
        self.scripts.on_event(session, ev);
    }

    /// A chat line arrived (`ev.kind == "chat"`).
    pub fn chat(&mut self, text: &str, kind: u32) {
        self.event(&Event::Chat {
            text: text.to_string(),
            kind,
        });
    }

    /// One frame of `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        let session = self.api.session as usize;
        let _bound = Bound::new(&mut self.api);
        self.scripts.scan(Instant::now());
        self.scripts.tick(session, dt);
    }

    /// `n` frames of `dt` seconds each.
    pub fn ticks(&mut self, n: usize, dt: f32) {
        for _ in 0..n {
            self.tick(dt);
        }
    }

    /// `/name args` typed in the chat box; whether a script claimed it.
    pub fn command(&mut self, name: &str, args: &str) -> bool {
        let session = self.api.session as usize;
        let _bound = Bound::new(&mut self.api);
        self.scripts.command(session, name, args)
    }

    /// A key (by egui name: "F5", "A", "Space") went down or up; whether
    /// a script consumed it.
    pub fn key(&mut self, name: &str, pressed: bool) -> bool {
        let session = self.api.session as usize;
        let _bound = Bound::new(&mut self.api);
        self.scripts.key(session, name, pressed)
    }

    /// A key pressed and released.
    pub fn press(&mut self, name: &str) -> bool {
        let down = self.key(name, true);
        self.key(name, false);
        down
    }

    /// The actions taken so far, oldest first: `"[session] say hi"`,
    /// `"[0] attack #77"`, `"[1] loot Corpse of Rat"`.
    pub fn calls(&self) -> &[String] {
        &self.api.calls
    }

    /// The chat lines logged so far, the loader's own ("Script x.rhai
    /// loaded (...)", "Script error: ...") included.
    pub fn logs(&self) -> &[String] {
        &self.api.logs
    }

    /// The logged lines that are not the loader's: what the script said
    /// with `log` and `print`.
    pub fn script_logs(&self) -> Vec<&str> {
        self.api
            .logs
            .iter()
            .filter(|l| !l.starts_with("Script "))
            .map(String::as_str)
            .collect()
    }

    /// The "Script error: ..." lines logged so far.
    pub fn errors(&self) -> Vec<&str> {
        self.api
            .logs
            .iter()
            .filter(|l| l.starts_with("Script error"))
            .map(String::as_str)
            .collect()
    }

    /// What the script posted on the bus, oldest first.
    pub fn posted(&self) -> &[(String, Value)] {
        &self.api.posted
    }

    /// A blackboard value the script (or the test) set.
    pub fn board(&self, key: &str) -> Option<&Value> {
        self.api.board.get(key)
    }

    /// Forget the recorded calls, logs and posts (not the canned data).
    pub fn clear(&mut self) -> &mut Self {
        self.api.calls.clear();
        self.api.logs.clear();
        self.api.posted.clear();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_runs_against_the_canned_world() {
        let mut h = ScriptHarness::from_source(
            "t.rhai",
            r#"
            fn on_event(ev) { if ev.kind == "chat" { log("heard " + ev.text); } }
            fn tick(dt) {
                let m = me();
                if m.health < 30 { cast("Heal Self"); }
                for o in objects() { if o.is_creature { attack(o.guid); break; } }
            }
            fn command(name, args) { if name == "go" { say("going " + args); return true; } false }
            fn key(name, pressed) { if name == "F5" && pressed { loot(); return true; } false }
            "#,
        );
        assert_eq!(h.names(), ["t.rhai (on_event, tick, command, key)"]);
        h.chat("hello", 3);
        assert_eq!(h.script_logs(), ["heard hello"]);
        h.me("health", 20i64);
        let mut rat = Map::new();
        rat.insert("guid".into(), Dynamic::from_int(77));
        rat.insert("is_creature".into(), true.into());
        h.objects(vec![Dynamic::from_map(rat)]);
        h.tick(0.1);
        assert!(h.command("go", "north"));
        assert!(!h.command("stay", ""));
        assert!(h.press("F5"));
        assert!(!h.press("F6"));
        // Session 1 has no low health: no cast.
        h.session(1).me("health", 90i64);
        h.tick(0.1);
        assert_eq!(
            h.calls(),
            [
                "[0] cast Heal Self",
                "[0] attack #77",
                "[0] say going north",
                "[0] loot ",
                "[1] attack #77",
            ]
        );
        assert!(h.errors().is_empty(), "{:?}", h.logs());
    }

    #[test]
    fn loot_rules_are_read_added_and_cleared() {
        let mut h = ScriptHarness::from_source(
            "r.rhai",
            r#"
            fn command(name, args) {
                if name == "rules" {
                    loot_rules_clear();
                    log("added " + loot_rule_add("slot:ring epics>=2", "keep"));
                    log("added " + loot_rule_add("ws<6 -epics>0", "salvage"));
                    log("bad action " + loot_rule_add("value>250", "burn"));
                    log("bad query " + loot_rule_add("(value>250", "keep"));
                    for r in loot_rules() { log(r.query + " -> " + r.action); }
                    log("item 7: " + loot_action(7));
                    log("salvager: " + type_of(salvager()));
                    return true;
                }
                false
            }
            "#,
        );
        assert!(h.command("rules", ""));
        assert_eq!(
            h.script_logs(),
            [
                "added true",
                "added true",
                "bad action false",
                "bad query false",
                "slot:ring epics>=2 -> keep",
                "ws<6 -epics>0 -> salvage",
                "item 7: keep",
                "salvager: ()",
            ]
        );
        assert!(h.calls().contains(&"[0] loot_rules_clear".to_string()));
        assert!(h.errors().is_empty(), "{:?}", h.logs());
    }

    #[test]
    fn autoplay_events_and_bus_messages_are_fed_in() {
        let mut h = ScriptHarness::from_source(
            "w.rhai",
            r#"
            fn on_event(ev) { if ev.kind == "autoplay" { log(ev.doing + ": " + ev.text); } }
            fn tick(dt) {
                for m in messages("autoplay.event") {
                    log("remote " + m.origin + " " + m.value.name + " " + m.value.doing);
                }
            }
            "#,
        );
        h.event(&Event::Autoplay {
            doing: "looting".into(),
            text: "looting Drudge Skulker".into(),
        });
        h.deliver(
            0,
            Some("bob"),
            "autoplay.event",
            serde_json::json!({"session": 0, "name": "Bob", "doing": "fighting", "text": "fighting Rat"}),
        );
        h.tick(0.1);
        h.clear_messages().tick(0.1);
        assert_eq!(
            h.script_logs(),
            ["looting: looting Drudge Skulker", "remote bob Bob fighting"]
        );
    }

    #[test]
    fn a_broken_script_reports_and_a_file_can_be_reloaded() {
        let mut h = ScriptHarness::from_source("b.rhai", "fn tick(dt) { nope(); }");
        h.tick(0.1);
        assert_eq!(h.errors().len(), 1);
        assert!(h.errors()[0].contains("nope"));
        std::fs::write(h.dir().join("ok.rhai"), "fn tick(dt) { say(\"ok\"); }").unwrap();
        assert!(h.reload());
        h.tick(0.1);
        assert_eq!(h.calls(), ["[0] say ok"]);
    }
}
