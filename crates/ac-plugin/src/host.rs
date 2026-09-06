//! Plugin host: owns the registered plugins, the blackboard and the
//! settings store, and fans callbacks out with every session in reach.
//! Shared by every binary that runs sessions (the windowed viewer,
//! headless runners).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::icons::{IconCache, IconLoader};
use crate::{panels, Blackboard, BusClient, Client, Ctx, Event, Plugin, Settings};

/// How often [`Host::autosave`] writes the settings file when something
/// changed.
pub const AUTOSAVE_EVERY: Duration = Duration::from_secs(30);

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
    /// first, then tick.
    pub fn frame(
        &mut self,
        clients: Vec<&mut Client>,
        index: usize,
        events: &[Event],
        dt: f32,
        now: Instant,
    ) -> Requests {
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
            consumed: false,
        }
    }

    /// Once all sessions ran this frame: rotate the bus.
    pub fn end_frame(&mut self) {
        self.board.end_frame();
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
        };
        for p in &mut self.plugins {
            p.ui(&mut cx, egui);
        }
        Requests {
            chat: cx.chat,
            activate: cx.activate,
            consumed: false,
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
            consumed,
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
            consumed,
        }
    }
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}
