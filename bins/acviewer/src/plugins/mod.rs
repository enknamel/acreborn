//! Plugin host: owns the registered plugins and fans callbacks out to them.

pub mod console;

use std::time::Instant;

use ac_plugin::{Ctx, Event, Plugin};

pub struct Host {
    plugins: Vec<Box<dyn Plugin>>,
}

impl Host {
    /// The built-in plugins. Add yours here (or load them at runtime later).
    pub fn builtin() -> Self {
        Host {
            plugins: vec![Box::new(console::Console::default())],
        }
    }

    pub fn names(&self) -> Vec<&str> {
        self.plugins.iter().map(|p| p.name()).collect()
    }

    fn ctx<'a>(
        client: &'a mut ac_client::Client,
        index: usize,
        count: usize,
        dt: f32,
        now: Instant,
    ) -> Ctx<'a> {
        Ctx {
            client,
            client_index: index,
            client_count: count,
            dt,
            now,
            chat: Vec::new(),
            activate: None,
        }
    }

    /// Run every plugin's per-frame hooks for one session: events first,
    /// then tick. Returns chat lines the plugins want shown and any
    /// request to activate another session.
    pub fn frame(
        &mut self,
        client: &mut ac_client::Client,
        events: &[Event],
        index: usize,
        count: usize,
        dt: f32,
        now: Instant,
    ) -> (Vec<(String, u32)>, Option<usize>) {
        let mut cx = Self::ctx(client, index, count, dt, now);
        for p in &mut self.plugins {
            for ev in events {
                p.on_event(&mut cx, ev);
            }
            p.tick(&mut cx);
        }
        (cx.chat, cx.activate)
    }

    pub fn ui(
        &mut self,
        client: &mut ac_client::Client,
        index: usize,
        count: usize,
        egui: &egui::Context,
    ) -> (Vec<(String, u32)>, Option<usize>) {
        let mut cx = Self::ctx(client, index, count, 0.0, Instant::now());
        for p in &mut self.plugins {
            p.ui(&mut cx, egui);
        }
        (cx.chat, cx.activate)
    }

    pub fn key(
        &mut self,
        client: &mut ac_client::Client,
        index: usize,
        count: usize,
        key: egui::Key,
        pressed: bool,
    ) -> (bool, Vec<(String, u32)>, Option<usize>) {
        let mut cx = Self::ctx(client, index, count, 0.0, Instant::now());
        let mut used = false;
        for p in &mut self.plugins {
            if p.key(&mut cx, key, pressed) {
                used = true;
                break;
            }
        }
        (used, cx.chat, cx.activate)
    }

    /// A chat line starting with `/`. Returns true when a plugin took it.
    pub fn command(
        &mut self,
        client: &mut ac_client::Client,
        index: usize,
        count: usize,
        line: &str,
    ) -> (bool, Vec<(String, u32)>, Option<usize>) {
        let Some((name, args)) = ac_plugin::parse_command(line) else {
            return (false, Vec::new(), None);
        };
        let mut cx = Self::ctx(client, index, count, 0.0, Instant::now());
        let mut used = false;
        for p in &mut self.plugins {
            if p.command(&mut cx, name, args) {
                used = true;
                break;
            }
        }
        if !used {
            cx.log(format!("Unknown command /{name}"));
        }
        (used, cx.chat, cx.activate)
    }
}
