//! The engine and the loaded scripts: compiling `*.rhai` files from a
//! directory, reloading them when they change, and dispatching the hook
//! functions. Everything here expects an [`Api`](crate::Api) to be bound
//! (see [`Bound`](crate::Bound)) while it runs; when none is, script API
//! calls fail with a script error, which is reported like any other.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use rhai::{CallFnOptions, Dynamic, Engine, Map, Scope, AST};

use crate::api::{register, with_api};

/// Operations one hook call may perform before it is cut off, so a
/// runaway loop in a script costs a frame hitch, not the client.
const MAX_OPERATIONS: u64 = 2_000_000;
const MAX_CALL_LEVELS: usize = 48;
/// Parser nesting allowed (Rhai's default of 32 inside functions rejects
/// a few levels of `if`/`else` with a string expression at the bottom).
const MAX_EXPR_DEPTH: usize = 160;
const SCAN_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Hooks {
    on_event: bool,
    tick: bool,
    command: bool,
    key: bool,
}

impl Hooks {
    fn of(ast: &AST) -> Self {
        let mut h = Hooks::default();
        for f in ast.iter_functions() {
            match (f.name, f.params.len()) {
                ("on_event", 1) => h.on_event = true,
                ("tick", 1) => h.tick = true,
                ("command", 2) => h.command = true,
                ("key", 2) => h.key = true,
                _ => {}
            }
        }
        h
    }

    fn describe(&self) -> String {
        let mut names = Vec::new();
        if self.on_event {
            names.push("on_event");
        }
        if self.tick {
            names.push("tick");
        }
        if self.command {
            names.push("command");
        }
        if self.key {
            names.push("key");
        }
        if names.is_empty() {
            "no hooks".to_string()
        } else {
            names.join(", ")
        }
    }
}

struct Script {
    path: PathBuf,
    name: String,
    mtime: SystemTime,
    /// None when the file did not compile.
    ast: Option<AST>,
    scope: Scope<'static>,
    hooks: Hooks,
    /// The script's `this` map, one per session, kept between calls.
    state: Vec<Dynamic>,
    /// Last error reported, to log a repeating one only once.
    last_error: Option<String>,
}

pub struct Scripts {
    engine: Engine,
    dir: PathBuf,
    scripts: Vec<Script>,
    last_scan: Option<Instant>,
}

impl Scripts {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let mut engine = Engine::new();
        engine.set_max_operations(MAX_OPERATIONS);
        engine.set_max_call_levels(MAX_CALL_LEVELS);
        engine.set_max_expr_depths(MAX_EXPR_DEPTH, MAX_EXPR_DEPTH);
        register(&mut engine);
        Scripts {
            engine,
            dir: dir.into(),
            scripts: Vec::new(),
            last_scan: None,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Names of the loaded scripts, with their hooks.
    pub fn names(&self) -> Vec<String> {
        self.scripts
            .iter()
            .map(|s| match &s.ast {
                Some(_) => format!("{} ({})", s.name, s.hooks.describe()),
                None => format!("{} (broken)", s.name),
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.scripts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.scripts.is_empty()
    }

    /// Look for new, changed and removed files, at most once per second.
    /// Returns whether anything was (re)loaded or dropped.
    pub fn scan(&mut self, now: Instant) -> bool {
        if self
            .last_scan
            .is_some_and(|t| now.duration_since(t) < SCAN_INTERVAL)
        {
            return false;
        }
        self.last_scan = Some(now);
        self.rescan()
    }

    /// Look for new, changed and removed files right now.
    pub fn rescan(&mut self) -> bool {
        let files = list_scripts(&self.dir);
        let mut changed = false;
        self.scripts.retain(|s| {
            let keep = files.contains_key(&s.path);
            if !keep {
                changed = true;
                let name = s.name.clone();
                tracing::info!("script {name} removed");
                let _ = with_api(|a| a.log(&format!("Script {name} unloaded")));
            }
            keep
        });
        for (path, mtime) in files {
            let existing = self.scripts.iter().position(|s| s.path == path);
            if existing.is_some_and(|i| self.scripts[i].mtime == mtime) {
                continue;
            }
            changed = true;
            let script = self.load(path, mtime);
            match existing {
                Some(i) => self.scripts[i] = script,
                None => self.scripts.push(script),
            }
        }
        self.scripts.sort_by(|a, b| a.path.cmp(&b.path));
        changed
    }

    /// Drop everything and load the directory afresh.
    pub fn reload_all(&mut self) -> bool {
        self.scripts.clear();
        self.rescan()
    }

    fn load(&self, path: PathBuf, mtime: SystemTime) -> Script {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut script = Script {
            path: path.clone(),
            name: name.clone(),
            mtime,
            ast: None,
            scope: Scope::new(),
            hooks: Hooks::default(),
            state: Vec::new(),
            last_error: None,
        };
        match self.engine.compile_file(path) {
            Ok(ast) => {
                // Run the top level once, so `let`/`const` and any setup
                // calls happen at load; hooks are called later.
                if let Err(e) = self.engine.run_ast_with_scope(&mut script.scope, &ast) {
                    report(&mut script, format!("{name}: {e}"));
                }
                script.hooks = Hooks::of(&ast);
                tracing::info!("script {name} loaded ({})", script.hooks.describe());
                let _ = with_api(|a| {
                    a.log(&format!(
                        "Script {name} loaded ({})",
                        script.hooks.describe()
                    ))
                });
                script.ast = Some(ast);
            }
            Err(e) => report(&mut script, format!("{name}: {e}")),
        }
        script
    }

    /// Call `fname` in script `i` for `session`, if the script has it.
    /// Returns None when the hook is missing or failed.
    fn call(
        &mut self,
        i: usize,
        session: usize,
        fname: &str,
        args: impl rhai::FuncArgs,
    ) -> Option<Dynamic> {
        let Scripts {
            engine, scripts, ..
        } = self;
        let s = &mut scripts[i];
        let ast = s.ast.as_ref()?;
        while s.state.len() <= session {
            s.state.push(Dynamic::from_map(Map::new()));
        }
        let options = CallFnOptions::new()
            .eval_ast(false)
            .rewind_scope(true)
            .bind_this_ptr(&mut s.state[session]);
        match engine.call_fn_with_options::<Dynamic>(options, &mut s.scope, ast, fname, args) {
            Ok(v) => {
                s.last_error = None;
                Some(v)
            }
            Err(e) => {
                let name = s.name.clone();
                report(s, format!("{name}: {fname}: {e}"));
                None
            }
        }
    }

    pub fn on_event(&mut self, session: usize, ev: Map) {
        for i in 0..self.scripts.len() {
            if self.scripts[i].hooks.on_event {
                self.call(i, session, "on_event", (Dynamic::from_map(ev.clone()),));
            }
        }
    }

    pub fn tick(&mut self, session: usize, dt: f32) {
        for i in 0..self.scripts.len() {
            if self.scripts[i].hooks.tick {
                self.call(i, session, "tick", (dt as f64,));
            }
        }
    }

    /// The first script whose `command` returns `true` claims the command.
    pub fn command(&mut self, session: usize, name: &str, args: &str) -> bool {
        for i in 0..self.scripts.len() {
            if self.scripts[i].hooks.command {
                let r = self.call(i, session, "command", (name.to_string(), args.to_string()));
                if r.is_some_and(|v| v.as_bool().unwrap_or(false)) {
                    return true;
                }
            }
        }
        false
    }

    /// The first script whose `key` returns `true` consumes the key.
    pub fn key(&mut self, session: usize, name: &str, pressed: bool) -> bool {
        for i in 0..self.scripts.len() {
            if self.scripts[i].hooks.key {
                let r = self.call(i, session, "key", (name.to_string(), pressed));
                if r.is_some_and(|v| v.as_bool().unwrap_or(false)) {
                    return true;
                }
            }
        }
        false
    }
}

/// Log a script error to the chat and the trace log, once per distinct
/// message per script so a hook failing every frame does not flood.
fn report(script: &mut Script, message: String) {
    if script.last_error.as_deref() == Some(message.as_str()) {
        return;
    }
    tracing::warn!("script error: {message}");
    let _ = with_api(|a| a.log(&format!("Script error: {message}")));
    script.last_error = Some(message);
}

fn list_scripts(dir: &Path) -> BTreeMap<PathBuf, SystemTime> {
    let mut files = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "rhai") {
            if let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) {
                files.insert(path, mtime);
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ac_plugin::Message;
    use rhai::{Array, Dynamic, Map};
    use serde_json::Value;

    use super::*;
    use crate::api::{Api, Bound};

    /// An `Api` that records every call and answers reads from canned data.
    #[derive(Default)]
    struct Recorder {
        calls: Vec<String>,
        logs: Vec<String>,
        session: i64,
        sessions: i64,
        me: Map,
        objects: Array,
        board: HashMap<String, Value>,
        posted: Vec<(String, Value)>,
        inbox: Vec<Message>,
        activate: Option<i64>,
    }

    impl Recorder {
        fn new() -> Self {
            Recorder {
                sessions: 2,
                ..Default::default()
            }
        }

        fn record(&mut self, s: impl std::fmt::Display) {
            self.calls.push(format!("[{}] {s}", self.session));
        }
    }

    impl Api for Recorder {
        fn me(&mut self) -> Map {
            let mut m = self.me.clone();
            m.insert("session".into(), Dynamic::from_int(self.session));
            m
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
    }

    /// A fresh directory for one test's scripts.
    fn script_dir() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ac-script-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, name: &str, src: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, src).unwrap();
        path
    }

    /// Bump a file's mtime well past whatever the filesystem recorded.
    fn touch_later(path: &Path, secs: u64) {
        let f = std::fs::File::options().write(true).open(path).unwrap();
        let t = std::fs::metadata(path).unwrap().modified().unwrap() + Duration::from_secs(secs);
        f.set_modified(t).unwrap();
    }

    #[test]
    fn hooks_are_detected_and_dispatched() {
        let dir = script_dir();
        write(
            &dir,
            "a.rhai",
            r#"
            fn on_event(ev) { if ev.kind == "chat" { log("saw " + ev.text); } }
            fn tick(dt) { this.n = if this.n == () { 1 } else { this.n + 1 }; if this.n == 3 { say("third tick " + dt); } }
            fn command(name, args) { if name == "hi" { say("hi " + args); return true; } false }
            fn key(name, pressed) { if name == "F5" && pressed { attack("Drudge"); return true; } false }
            "#,
        );
        write(&dir, "b.rhai", "fn command(name, args) { name == \"b\" }");
        write(&dir, "notes.txt", "fn tick(dt) { say(\"never\"); }");
        let mut scripts = Scripts::new(&dir);
        let mut rec = Recorder::new();
        {
            let _bound = Bound::new(&mut rec);
            assert!(scripts.rescan());
            assert_eq!(scripts.len(), 2);
            assert_eq!(
                scripts.names(),
                ["a.rhai (on_event, tick, command, key)", "b.rhai (command)"]
            );
            let mut ev = Map::new();
            ev.insert("kind".into(), "chat".into());
            ev.insert("text".into(), "hello".into());
            scripts.on_event(0, ev);
            for _ in 0..3 {
                scripts.tick(0, 0.5);
            }
            // session 1 has its own `this`
            scripts.tick(1, 0.5);
            assert!(scripts.command(0, "hi", "there"));
            assert!(scripts.command(0, "b", ""));
            assert!(!scripts.command(0, "nope", ""));
            assert!(scripts.key(0, "F5", true));
            assert!(!scripts.key(0, "F5", false));
            assert!(!scripts.key(0, "A", true));
        }
        assert_eq!(
            rec.logs,
            [
                "Script a.rhai loaded (on_event, tick, command, key)",
                "Script b.rhai loaded (command)",
                "saw hello"
            ]
        );
        assert_eq!(
            rec.calls,
            [
                "[0] say third tick 0.5",
                "[0] say hi there",
                "[0] attack Drudge"
            ]
        );
    }

    #[test]
    fn broken_scripts_are_reported_and_isolated() {
        let dir = script_dir();
        write(&dir, "bad.rhai", "fn tick(dt) { this is not rhai");
        write(&dir, "good.rhai", "fn tick(dt) { say(\"ok\"); }");
        write(&dir, "loud.rhai", "fn tick(dt) { no_such_function(); }");
        let mut scripts = Scripts::new(&dir);
        let mut rec = Recorder::new();
        {
            let _bound = Bound::new(&mut rec);
            scripts.rescan();
            assert_eq!(scripts.len(), 3);
            assert!(scripts.names()[0].starts_with("bad.rhai (broken)"));
            scripts.tick(0, 0.1);
            scripts.tick(0, 0.1);
            scripts.tick(0, 0.1);
        }
        assert_eq!(rec.calls, ["[0] say ok", "[0] say ok", "[0] say ok"]);
        let errors: Vec<&String> = rec
            .logs
            .iter()
            .filter(|l| l.starts_with("Script error"))
            .collect();
        assert_eq!(errors.len(), 2, "{:?}", rec.logs);
        assert!(errors[0].contains("bad.rhai"));
        assert!(errors[1].contains("loud.rhai") && errors[1].contains("no_such_function"));
    }

    #[test]
    fn runaway_loops_are_cut_off() {
        let dir = script_dir();
        write(&dir, "spin.rhai", "fn tick(dt) { loop { } }");
        let mut scripts = Scripts::new(&dir);
        let mut rec = Recorder::new();
        {
            let _bound = Bound::new(&mut rec);
            scripts.rescan();
            scripts.tick(0, 0.1);
        }
        assert!(rec
            .logs
            .iter()
            .any(|l| l.contains("Script error") && l.contains("spin.rhai")));
    }

    #[test]
    fn hot_reload_follows_mtime_once_per_second() {
        let dir = script_dir();
        let path = write(&dir, "v.rhai", "fn command(n, a) { say(\"v1\"); true }");
        let mut scripts = Scripts::new(&dir);
        let mut rec = Recorder::new();
        let t0 = Instant::now();
        {
            let _bound = Bound::new(&mut rec);
            assert!(scripts.scan(t0));
            assert!(scripts.command(0, "x", ""));

            std::fs::write(&path, "fn command(n, a) { say(\"v2\"); true }").unwrap();
            touch_later(&path, 10);
            // Too soon: the change is not picked up yet.
            assert!(!scripts.scan(t0 + Duration::from_millis(500)));
            assert!(scripts.command(0, "x", ""));
            // A second later it is.
            assert!(scripts.scan(t0 + Duration::from_millis(1001)));
            assert!(scripts.command(0, "x", ""));
            // Same mtime: nothing to do.
            assert!(!scripts.scan(t0 + Duration::from_secs(3)));

            std::fs::remove_file(&path).unwrap();
            assert!(scripts.scan(t0 + Duration::from_secs(5)));
            assert!(scripts.is_empty());
            assert!(!scripts.command(0, "x", ""));
        }
        assert_eq!(rec.calls, ["[0] say v1", "[0] say v1", "[0] say v2"]);
        assert!(rec.logs.iter().any(|l| l == "Script v.rhai unloaded"));
    }

    #[test]
    fn reload_resets_state_and_errors() {
        let dir = script_dir();
        let path = write(
            &dir,
            "s.rhai",
            "fn tick(dt) { this.n = if this.n == () { 1 } else { this.n + 1 }; boom(); }",
        );
        let mut scripts = Scripts::new(&dir);
        let mut rec = Recorder::new();
        {
            let _bound = Bound::new(&mut rec);
            scripts.rescan();
            scripts.tick(0, 0.1);
            scripts.tick(0, 0.1);
            std::fs::write(&path, "fn tick(dt) { this.n = if this.n == () { 1 } else { this.n + 1 }; log(\"n=\" + this.n); }").unwrap();
            touch_later(&path, 10);
            assert!(scripts.rescan());
            scripts.tick(0, 0.1);
        }
        let errors = rec
            .logs
            .iter()
            .filter(|l| l.starts_with("Script error"))
            .count();
        assert_eq!(errors, 1);
        assert_eq!(rec.logs.last().unwrap(), "n=1");
    }

    #[test]
    fn with_session_switches_and_restores() {
        let dir = script_dir();
        write(
            &dir,
            "w.rhai",
            r#"
            fn command(n, a) {
                let r = with_session(1, || { say("from one"); 42 });
                say("back " + r + " " + session_index());
                let bad = with_session(7, || say("never"));
                true
            }
            "#,
        );
        let mut scripts = Scripts::new(&dir);
        let mut rec = Recorder::new();
        {
            let _bound = Bound::new(&mut rec);
            scripts.rescan();
            assert!(!scripts.command(0, "x", ""));
        }
        assert_eq!(rec.calls, ["[1] say from one", "[0] say back 42 0"]);
        assert_eq!(rec.session, 0);
        assert!(rec.logs.iter().any(|l| l.contains("no session 7")));
    }

    #[test]
    fn bus_and_board_round_trip_through_json() {
        let dir = script_dir();
        write(
            &dir,
            "bus.rhai",
            r#"
            fn tick(dt) {
                post("assist", #{ guid: 0x8000_0001, name: "Drudge" });
                board_set("leader", 1);
                for m in messages("assist") {
                    log("msg from " + m.from + ": " + m.value.name + " " + m.value.guid);
                }
                log("leader=" + board_get("leader") + " none=" + board_get("none"));
            }
            "#,
        );
        let mut scripts = Scripts::new(&dir);
        let mut rec = Recorder::new();
        rec.inbox.push(Message {
            from: 1,
            topic: "assist".into(),
            value: serde_json::json!({"guid": 5, "name": "Rat"}),
            origin: None,
        });
        rec.inbox.push(Message {
            from: 1,
            topic: "other".into(),
            value: Value::Null,
            origin: None,
        });
        {
            let _bound = Bound::new(&mut rec);
            scripts.rescan();
            scripts.tick(0, 0.1);
        }
        assert_eq!(
            rec.posted,
            [(
                "assist".to_string(),
                serde_json::json!({"guid": 0x8000_0001u32, "name": "Drudge"})
            )]
        );
        assert_eq!(rec.board.get("leader"), Some(&Value::from(1)));
        assert!(rec.logs.contains(&"msg from 1: Rat 5".to_string()));
        assert!(rec.logs.contains(&"leader=1 none=".to_string()));
    }

    #[test]
    fn api_is_unavailable_outside_a_callback() {
        let dir = script_dir();
        write(&dir, "u.rhai", "fn command(n, a) { say(\"x\"); true }");
        let mut scripts = Scripts::new(&dir);
        // No `Bound`: loading and calling both survive; the call fails.
        scripts.rescan();
        assert_eq!(scripts.len(), 1);
        assert!(!scripts.command(0, "x", ""));
    }

    fn examples_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/examples")
    }

    #[test]
    fn examples_load_without_errors() {
        let mut scripts = Scripts::new(examples_dir());
        let mut rec = Recorder::new();
        {
            let _bound = Bound::new(&mut rec);
            scripts.rescan();
        }
        assert_eq!(scripts.len(), 3, "{:?}", scripts.names());
        assert!(
            rec.logs
                .iter()
                .all(|l| l.starts_with("Script ") && l.contains("loaded")),
            "{:?}",
            rec.logs
        );
    }

    #[test]
    fn example_greeter_answers_hello() {
        let mut scripts = Scripts::new(examples_dir());
        let mut rec = Recorder::new();
        {
            let _bound = Bound::new(&mut rec);
            scripts.rescan();
            assert!(scripts.command(0, "hello", ""));
            assert!(scripts.command(0, "hello", "Asheron"));
            assert!(!scripts.command(0, "goodbye", ""));
        }
        assert_eq!(
            rec.calls,
            ["[0] say Hello, everyone!", "[0] say Hello, Asheron!"]
        );
    }

    #[test]
    fn example_autoloot_loots_the_dead_target() {
        let mut scripts = Scripts::new(examples_dir());
        let mut rec = Recorder::new();
        rec.me
            .insert("target".into(), Dynamic::from_int(0x8000_0001));
        rec.me.insert("target_name".into(), "Drudge Skulker".into());
        rec.me.insert("container_open".into(), false.into());
        {
            let _bound = Bound::new(&mut rec);
            scripts.rescan();
            scripts.tick(0, 0.1);
        }
        // The target dies: no more target; the corpse takes a moment to appear.
        rec.me.insert("target".into(), Dynamic::UNIT);
        {
            let _bound = Bound::new(&mut rec);
            for _ in 0..30 {
                scripts.tick(0, 0.1);
            }
        }
        assert_eq!(rec.calls, ["[0] loot Corpse of Drudge Skulker"]);
        // The corpse is open: everything is taken and it is closed.
        rec.me.insert("container_open".into(), true.into());
        {
            let _bound = Bound::new(&mut rec);
            scripts.tick(0, 0.1);
            scripts.tick(0, 0.1);
        }
        assert_eq!(
            rec.calls,
            [
                "[0] loot Corpse of Drudge Skulker",
                "[0] take_all",
                "[0] close_container"
            ]
        );
    }

    #[test]
    fn example_assist_posts_and_follows() {
        let mut scripts = Scripts::new(examples_dir());
        let mut rec = Recorder::new();
        rec.me.insert("target".into(), Dynamic::from_int(77));
        rec.me.insert("target_name".into(), "Rat".into());
        {
            let _bound = Bound::new(&mut rec);
            scripts.rescan();
            scripts.tick(0, 0.1);
            scripts.tick(0, 0.1);
        }
        assert_eq!(rec.posted.len(), 1, "posted once per target change");
        assert_eq!(rec.posted[0].0, "assist");
        assert_eq!(rec.posted[0].1["guid"], Value::from(77));
        // Another session reads the post and attacks.
        rec.inbox.push(Message {
            from: 0,
            topic: "assist".into(),
            value: rec.posted[0].1.clone(),
            origin: None,
        });
        rec.me.insert("target".into(), Dynamic::UNIT);
        rec.session = 1;
        {
            let _bound = Bound::new(&mut rec);
            scripts.tick(1, 0.1);
        }
        assert_eq!(rec.calls, ["[1] attack #77"]);
    }
}
