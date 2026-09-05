//! Plugin host: owns the registered plugins and the blackboard, and fans
//! callbacks out with every session in reach.

pub mod console;

use std::time::Instant;

use ac_plugin::{Blackboard, Client, Ctx, Event, Plugin};

pub struct Host {
    plugins: Vec<Box<dyn Plugin>>,
    pub board: Blackboard,
}

/// What a batch of callbacks asked the host for.
#[derive(Default)]
pub struct Requests {
    pub chat: Vec<(String, u32)>,
    pub activate: Option<usize>,
    pub consumed: bool,
}

impl Host {
    /// The built-in plugins. Add yours here (or load them at runtime later).
    pub fn builtin() -> Self {
        Host {
            plugins: vec![Box::new(console::Console::default())],
            board: Blackboard::default(),
        }
    }

    pub fn names(&self) -> Vec<&str> {
        self.plugins.iter().map(|p| p.name()).collect()
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

    pub fn ui(
        &mut self,
        clients: Vec<&mut Client>,
        index: usize,
        egui: &egui::Context,
    ) -> Requests {
        let mut cx = Ctx {
            clients,
            index,
            board: &mut self.board,
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
        let Some((name, args)) = ac_plugin::parse_command(line) else {
            return Requests::default();
        };
        let mut cx = Ctx {
            clients,
            index,
            board: &mut self.board,
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
        if !consumed {
            cx.log(format!("Unknown command /{name}"));
        }
        Requests {
            chat: cx.chat,
            activate: cx.activate,
            consumed,
        }
    }
}
