//! Plugin host: owns the registered plugins, the blackboard and the
//! settings store, and fans callbacks out with every session in reach.
//! Shared by every binary that runs sessions (the windowed viewer,
//! headless runners).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::icons::{IconCache, IconLoader};
use crate::{panels, Blackboard, BusClient, Client, Ctx, Event, Plugin, SessionSpec, Settings};

/// How often [`Host::autosave`] writes the settings file when something
/// changed.
pub const AUTOSAVE_EVERY: Duration = Duration::from_secs(30);

/// The blackboard topic every [`Event::Autoplay`] is repeated on, as
/// `{"session", "name", "doing", "text"}`: readable next frame by every
/// plugin (`cx.board.messages_on`), every script (`messages`) and every
/// other process on the bus (`origin` names it).
pub const AUTOPLAY_TOPIC: &str = "autoplay.event";

pub struct Host {
    plugins: Vec<Box<dyn Plugin>>,
    pub board: Blackboard,
    /// Icons plugins draw; empty until [`Host::set_icon_loader`].
    pub icons: IconCache,
    /// What survives a restart; empty until [`Host::load_settings`].
    pub settings: Settings,
    /// Where the settings are written; `None` until loaded.
    settings_path: Option<PathBuf>,
    /// The egui context of the last `ui` pass, for window positions.
    egui: Option<egui::Context>,
    last_save: Instant,
}

/// What a batch of callbacks asked the host for.
#[derive(Default)]
pub struct Requests {
    pub chat: Vec<(String, u32)>,
    pub activate: Option<usize>,
    pub consumed: bool,
    /// A plugin asked the host to close the client.
    pub quit: bool,
    /// Sessions to start (see [`Ctx::start_session`]); the host
    /// connects each and appends it to its sessions, in this order.
    pub start_sessions: Vec<SessionSpec>,
    /// Sessions to disconnect and drop, by index (see
    /// [`Ctx::stop_session`]); the host removes each and calls
    /// [`Host::session_removed`].
    pub stop_sessions: Vec<usize>,
}

impl Requests {
    /// Nothing was asked.
    pub fn is_empty(&self) -> bool {
        self.chat.is_empty()
            && self.activate.is_none()
            && !self.quit
            && self.start_sessions.is_empty()
            && self.stop_sessions.is_empty()
    }
}

impl Host {
    pub fn new() -> Self {
        Host {
            plugins: Vec::new(),
            board: Blackboard::default(),
            icons: IconCache::default(),
            settings: Settings::new(),
            settings_path: None,
            egui: None,
            last_save: Instant::now(),
        }
    }

    /// Install the RenderSurface decoder behind `cx.icons()`. Call once,
    /// before the first `ui` pass; a host that never draws needs none.
    pub fn set_icon_loader(&mut self, loader: IconLoader) {
        self.icons.set_loader(loader);
    }

    /// Add a plugin. When the settings were already loaded it gets its
    /// [`Plugin::load`] right away.
    pub fn register(&mut self, mut plugin: Box<dyn Plugin>) {
        if self.settings_path.is_some() {
            plugin.load(&self.settings);
        }
        self.plugins.push(plugin);
    }

    pub fn names(&self) -> Vec<&str> {
        self.plugins.iter().map(|p| p.name()).collect()
    }

    /// Read the settings file at `path` (missing is fine), remember the
    /// saved window positions for [`panels::window`], and give every
    /// registered plugin its [`Plugin::load`]. Later saves go to `path`.
    pub fn load_settings(&mut self, path: PathBuf) {
        self.settings = Settings::load(&path);
        tracing::info!(
            path = %path.display(),
            keys = self.settings.values().len(),
            "settings loaded"
        );
        self.settings_path = Some(path);
        panels::restore_positions(self.settings.get("windows").unwrap_or_default());
        for p in &mut self.plugins {
            p.load(&self.settings);
        }
    }

    /// Where [`Host::save_settings`] writes, once loaded.
    pub fn settings_path(&self) -> Option<&PathBuf> {
        self.settings_path.as_ref()
    }

    /// Collect every plugin's [`Plugin::save`] and the panel windows'
    /// positions, then write the file when anything changed. Returns
    /// whether the file was written. Does nothing before
    /// [`Host::load_settings`].
    pub fn save_settings(&mut self) -> bool {
        self.last_save = Instant::now();
        let Some(path) = self.settings_path.clone() else {
            return false;
        };
        if let Some(egui) = &self.egui {
            self.settings.set("windows", panels::positions(egui));
        }
        for p in &self.plugins {
            p.save(&mut self.settings);
        }
        if !self.settings.dirty {
            return false;
        }
        match self.settings.save(&path) {
            Ok(()) => {
                tracing::debug!(path = %path.display(), "settings saved");
                true
            }
            Err(e) => {
                tracing::warn!("settings: cannot write {}: {e}", path.display());
                false
            }
        }
    }

    /// [`Host::save_settings`] when [`AUTOSAVE_EVERY`] passed since the
    /// last one; call it once per frame.
    pub fn autosave(&mut self) -> bool {
        if self.last_save.elapsed() < AUTOSAVE_EVERY {
            return false;
        }
        self.save_settings()
    }

    /// Run every plugin's per-frame hooks for session `index`: its events
    /// first, then tick. An [`Event::Autoplay`] is also posted on the
    /// blackboard topic [`AUTOPLAY_TOPIC`], so plugins and scripts in
    /// every session, and every other process on the bus, hear what each
    /// character is doing.
    pub fn frame(
        &mut self,
        clients: Vec<&mut Client>,
        index: usize,
        events: &[Event],
        dt: f32,
        now: Instant,
    ) -> Requests {
        for ev in events {
            if let Event::Autoplay { doing, text } = ev {
                let name = clients
                    .get(index)
                    .map(|c| c.world.stats.name.clone())
                    .unwrap_or_default();
                self.board.post(
                    index,
                    AUTOPLAY_TOPIC,
                    serde_json::json!({
                        "session": index,
                        "name": name,
                        "doing": doing,
                        "text": text,
                    }),
                );
            }
        }
        let mut cx = Ctx {
            clients,
            index,
            board: &mut self.board,
            settings: &mut self.settings,
            icons: &mut self.icons,
            dt,
            now,
            chat: Vec::new(),
            activate: None,
            quit: false,
            start_sessions: Vec::new(),
            stop_sessions: Vec::new(),
        };
        for p in &mut self.plugins {
            for ev in events {
                p.on_event(&mut cx, ev);
            }
            p.tick(&mut cx);
        }
        Requests {
            chat: cx.chat,
            activate: cx.activate,
            quit: cx.quit,
            consumed: false,
            start_sessions: cx.start_sessions,
            stop_sessions: cx.stop_sessions,
        }
    }

    /// Once all sessions ran this frame: rotate the bus.
    pub fn end_frame(&mut self) {
        self.board.end_frame();
    }

    /// The host dropped session `index` (a [`Requests::stop_sessions`]
    /// it applied, or a connection that ended): every plugin hears
    /// [`Plugin::session_removed`] so state kept by session index moves
    /// down with the sessions.
    pub fn session_removed(&mut self, index: usize) {
        for p in &mut self.plugins {
            p.session_removed(index);
        }
    }

    /// Link the blackboard to the cross-process bus: this frame's posts
    /// go out tagged `from: name`, other processes' posts come in as
    /// messages from [`crate::REMOTE`], and values are shared.
    pub fn attach_bus(&mut self, client: BusClient, name: String) {
        tracing::info!(
            addr = client.addr(),
            name,
            hosting = client.is_hosting(),
            "bus attached"
        );
        self.board.attach_bus(client, name);
    }

    /// [`attach_bus`](Self::attach_bus) with a client that joins the hub
    /// at `addr` (`HOST:PORT`, a bare port, or empty for the default) or
    /// becomes it when none listens.
    pub fn join_bus(&mut self, addr: Option<&str>, name: &str) -> std::io::Result<()> {
        let addr = ac_bus::resolve_addr(addr);
        let client = BusClient::connect_or_host(&addr, name)?;
        self.attach_bus(client, name.to_string());
        Ok(())
    }

    pub fn ui(
        &mut self,
        clients: Vec<&mut Client>,
        index: usize,
        egui: &egui::Context,
    ) -> Requests {
        if self.egui.is_none() {
            self.egui = Some(egui.clone());
        }
        let mut cx = Ctx {
            clients,
            index,
            board: &mut self.board,
            settings: &mut self.settings,
            icons: &mut self.icons,
            dt: 0.0,
            now: Instant::now(),
            chat: Vec::new(),
            activate: None,
            quit: false,
            start_sessions: Vec::new(),
            stop_sessions: Vec::new(),
        };
        for p in &mut self.plugins {
            p.ui(&mut cx, egui);
        }
        Requests {
            chat: cx.chat,
            activate: cx.activate,
            quit: cx.quit,
            consumed: false,
            start_sessions: cx.start_sessions,
            stop_sessions: cx.stop_sessions,
        }
    }

    pub fn key(
        &mut self,
        clients: Vec<&mut Client>,
        index: usize,
        key: egui::Key,
        pressed: bool,
    ) -> Requests {
        let mut cx = Ctx {
            clients,
            index,
            board: &mut self.board,
            settings: &mut self.settings,
            icons: &mut self.icons,
            dt: 0.0,
            now: Instant::now(),
            chat: Vec::new(),
            activate: None,
            quit: false,
            start_sessions: Vec::new(),
            stop_sessions: Vec::new(),
        };
        let mut consumed = false;
        for p in &mut self.plugins {
            if p.key(&mut cx, key, pressed) {
                consumed = true;
                break;
            }
        }
        Requests {
            chat: cx.chat,
            activate: cx.activate,
            quit: cx.quit,
            consumed,
            start_sessions: cx.start_sessions,
            stop_sessions: cx.stop_sessions,
        }
    }

    /// A chat line starting with `/`.
    pub fn command(&mut self, clients: Vec<&mut Client>, index: usize, line: &str) -> Requests {
        let Some((name, args)) = crate::parse_command(line) else {
            return Requests::default();
        };
        let mut cx = Ctx {
            clients,
            index,
            board: &mut self.board,
            settings: &mut self.settings,
            icons: &mut self.icons,
            dt: 0.0,
            now: Instant::now(),
            chat: Vec::new(),
            activate: None,
            quit: false,
            start_sessions: Vec::new(),
            stop_sessions: Vec::new(),
        };
        let mut consumed = false;
        for p in &mut self.plugins {
            if p.command(&mut cx, name, args) {
                consumed = true;
                break;
            }
        }
        Requests {
            chat: cx.chat,
            activate: cx.activate,
            quit: cx.quit,
            consumed,
            start_sessions: cx.start_sessions,
            stop_sessions: cx.stop_sessions,
        }
    }
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ctx;

    /// A plugin that keeps the autoplay events it was handed.
    #[derive(Default)]
    struct Listener {
        heard: std::rc::Rc<std::cell::RefCell<Vec<(String, String)>>>,
    }

    impl Plugin for Listener {
        fn name(&self) -> &str {
            "listener"
        }
        fn on_event(&mut self, cx: &mut Ctx, ev: &Event) {
            if let Event::Autoplay { doing, text } = ev {
                self.heard.borrow_mut().push((doing.clone(), text.clone()));
                cx.log(format!("autoplay: {text}"));
            }
        }
    }

    /// What the [`Starter`] plugin is told to ask for and what it keeps.
    #[derive(Default)]
    struct StarterState {
        ask_start: Vec<SessionSpec>,
        ask_stop: Vec<usize>,
        /// Session index -> a label, shifted on removal (the way the
        /// team plugin keeps when each session last spoke).
        seen: std::collections::BTreeMap<usize, String>,
        removed: Vec<usize>,
    }

    /// A plugin that asks for sessions the way the fleet panel does.
    #[derive(Default)]
    struct Starter(std::rc::Rc<std::cell::RefCell<StarterState>>);

    impl Plugin for Starter {
        fn name(&self) -> &str {
            "starter"
        }
        fn tick(&mut self, cx: &mut Ctx) {
            let mut st = self.0.borrow_mut();
            for s in st.ask_start.drain(..) {
                cx.start_session(s);
            }
            for i in st.ask_stop.drain(..) {
                cx.stop_session(i);
            }
        }
        fn session_removed(&mut self, index: usize) {
            let mut st = self.0.borrow_mut();
            st.removed.push(index);
            crate::shift_removed(&mut st.seen, index);
        }
    }

    /// What a viewer does with the requests: its sessions are a list
    /// of accounts here. Stops are applied highest index first so each
    /// index still means what the plugin meant.
    fn apply(host: &mut Host, sessions: &mut Vec<String>, r: Requests) {
        for spec in r.start_sessions {
            sessions.push(spec.account);
        }
        let mut stops = r.stop_sessions;
        stops.sort_unstable();
        stops.dedup();
        for i in stops.into_iter().rev() {
            if i < sessions.len() {
                sessions.remove(i);
                host.session_removed(i);
            }
        }
    }

    #[test]
    fn plugins_start_and_stop_sessions_through_requests() {
        let mut host = Host::new();
        let starter = Starter::default();
        let state = starter.0.clone();
        {
            let mut st = state.borrow_mut();
            for a in ["bot1", "bot2"] {
                st.ask_start.push(SessionSpec {
                    account: a.into(),
                    password: "pw".into(),
                    ..Default::default()
                });
            }
            st.seen.insert(0, "lead".into());
        }
        host.register(Box::new(starter));
        let mut sessions = vec!["lead".to_string()];
        let r = host.frame(Vec::new(), 0, &[], 0.05, Instant::now());
        assert!(!r.is_empty());
        assert_eq!(r.start_sessions.len(), 2);
        assert!(r.stop_sessions.is_empty());
        apply(&mut host, &mut sessions, r);
        assert_eq!(sessions, ["lead", "bot1", "bot2"]);
        // Nothing asked: an empty request.
        let r = host.frame(Vec::new(), 0, &[], 0.05, Instant::now());
        assert!(r.is_empty());
        apply(&mut host, &mut sessions, r);
        assert_eq!(sessions.len(), 3);
        // Stop the middle one (asked twice, applied once); the index
        // above it moves down and the plugins hear it.
        {
            let mut st = state.borrow_mut();
            st.seen.insert(2, "bot2".into());
            st.ask_stop = vec![1, 1];
        }
        let r = host.ui(Vec::new(), 0, &egui::Context::default());
        assert!(r.is_empty(), "ui does not tick");
        let r = host.frame(Vec::new(), 0, &[], 0.05, Instant::now());
        assert_eq!(r.stop_sessions, [1, 1]);
        apply(&mut host, &mut sessions, r);
        assert_eq!(sessions, ["lead", "bot2"]);
        let st = state.borrow();
        assert_eq!(st.removed, [1]);
        assert_eq!(
            st.seen
                .iter()
                .map(|(i, s)| (*i, s.as_str()))
                .collect::<Vec<_>>(),
            [(0, "lead"), (1, "bot2")]
        );
    }

    #[test]
    fn autoplay_events_reach_plugins_and_the_board() {
        let mut host = Host::new();
        let listener = Listener::default();
        let heard = listener.heard.clone();
        host.register(Box::new(listener));
        let events = vec![
            Event::Autoplay {
                doing: "fighting".into(),
                text: "fighting Drudge Skulker".into(),
            },
            Event::Chat {
                text: "hello".into(),
                kind: 3,
            },
        ];
        // No session at all (the way `--demo-ui` runs): the host must
        // not need one to forward the event.
        let r = host.frame(Vec::new(), 0, &events, 0.05, Instant::now());
        assert_eq!(
            r.chat,
            [("autoplay: fighting Drudge Skulker".to_string(), 0)]
        );
        assert_eq!(
            *heard.borrow(),
            [(
                "fighting".to_string(),
                "fighting Drudge Skulker".to_string()
            )]
        );
        // Posted this frame, readable next frame, on the documented topic.
        assert_eq!(host.board.messages_on(AUTOPLAY_TOPIC).count(), 0);
        host.end_frame();
        let posted: Vec<_> = host.board.messages_on(AUTOPLAY_TOPIC).collect();
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].from, 0);
        assert_eq!(
            posted[0].value,
            serde_json::json!({
                "session": 0,
                "name": "",
                "doing": "fighting",
                "text": "fighting Drudge Skulker",
            })
        );
        // The chat line was not repeated on the board.
        assert_eq!(host.board.messages().count(), 1);
    }
}
