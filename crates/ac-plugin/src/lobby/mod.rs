//! The lobby: what the client shows between login and the world when it
//! is not auto-entering. [`select`] lists the account's characters
//! (Enter, Delete, Restore, New); [`create`] builds a new one from the
//! CharGen rules. [`Lobby`] holds both, turns their clicks into `Client`
//! calls, and follows the client's events (`Characters`, `Placed`,
//! `CharacterCreated`, `CharacterCreateFailed`). The host draws the 3D
//! preview of the character being created from [`Lobby::preview`].
//!
//! Both screens are the same shape as the panels (`view` / `draw` /
//! state) and have a demo mode with no session: `acviewer --demo-select`
//! and `--demo-create`.

pub mod create;
pub mod select;

use std::rc::Rc;

use ac_client::creation::{self, CharacterBuild};
use ac_scene::Assets;

use crate::{egui, Client, Ctx, Event, Plugin};
use create::{CreateAction, CreateState};
use select::{SelectAction, SelectState, SelectView};

/// Which screen is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Select,
    Create,
}

#[derive(Default)]
pub struct Lobby {
    pub screen: Option<Screen>,
    pub select: SelectState,
    pub create: Option<CreateState>,
    /// A canned character list for the offline demo (no session).
    demo: Option<SelectView>,
}

impl Lobby {
    /// The select screen on three sample characters, no session.
    pub fn demo_select() -> Self {
        Lobby {
            screen: Some(Screen::Select),
            demo: Some(select::demo_view()),
            ..Default::default()
        }
    }

    /// The creation screen on the real CharGen table, no session.
    pub fn demo_create(assets: Rc<Assets>) -> Result<Self, creation::CreateError> {
        let mut st = CreateState::new(assets, 1, 1)?;
        st.build.name = "Reborn".into();
        Ok(Lobby {
            screen: Some(Screen::Create),
            create: Some(st),
            demo: Some(select::demo_view()),
            ..Default::default()
        })
    }

    pub fn visible(&self) -> bool {
        self.screen.is_some()
    }

    /// The character being created, for the host's 3D preview.
    pub fn preview(&self) -> Option<&CharacterBuild> {
        match (self.screen, &self.create) {
            (Some(Screen::Create), Some(st)) => Some(&st.build),
            _ => None,
        }
    }

    /// The archives the creation screen reads, once it is open.
    pub fn preview_assets(&self) -> Option<Rc<Assets>> {
        self.create.as_ref().map(|st| st.assets.clone())
    }

    /// Open the creation screen for `assets` (the session's, or the
    /// demo's), keeping an earlier build if one is in progress.
    pub fn open_create(&mut self, assets: Rc<Assets>) {
        if self.create.is_none() {
            match CreateState::new(assets, 1, 1) {
                Ok(st) => self.create = Some(st),
                Err(e) => {
                    self.select.message = Some(create::describe_error(&e));
                    return;
                }
            }
        }
        if let Some(st) = &mut self.create {
            st.pending = false;
        }
        self.screen = Some(Screen::Create);
    }

    /// An event of the active session.
    pub fn on_event(&mut self, ev: &Event) {
        match ev {
            Event::Characters(_) => {
                if self.screen != Some(Screen::Create) {
                    self.screen = Some(Screen::Select);
                }
            }
            Event::CharacterCreated { name, .. } => {
                self.create = None;
                self.select.message = Some(format!("Created {name}; entering the world..."));
                self.screen = Some(Screen::Select);
            }
            Event::CharacterCreateFailed(code) => {
                if let Some(st) = &mut self.create {
                    st.pending = false;
                    st.message = Some(creation::create_failure_message(*code).to_string());
                }
            }
            Event::Placed { .. } => {
                self.screen = None;
                self.create = None;
            }
            Event::Refused(op) => {
                self.select.message = Some(format!("The server refused (opcode {op:#06x})"));
            }
            Event::Terminated(why) => {
                self.select.message = Some(format!("Disconnected: {why}"));
            }
            _ => {}
        }
    }

    /// Once per frame: hide once the character stands in the world.
    pub fn tick(&mut self, client: &Client) {
        if self.visible() && client.placed() {
            self.screen = None;
            self.create = None;
        }
    }

    fn select_view(&self, client: Option<&Client>) -> SelectView {
        match (client, &self.demo) {
            (Some(c), _) => select::view(c),
            (None, Some(d)) => d.clone(),
            (None, None) => SelectView::default(),
        }
    }

    fn apply_select(&mut self, action: SelectAction, client: Option<&mut Client>) {
        match action {
            SelectAction::New => {
                let assets = client
                    .as_ref()
                    .map(|c| c.assets.clone())
                    .or_else(|| self.preview_assets());
                match assets {
                    Some(a) => self.open_create(a),
                    None => self.select.message = Some("No archives open".into()),
                }
            }
            SelectAction::Enter(id) => {
                if let Some(c) = client {
                    c.enter_world(id);
                    self.select.message = None;
                }
            }
            SelectAction::Delete(id) => {
                if let Some(c) = client {
                    c.delete_character(id);
                    self.select.message = Some("Delete requested; the list refreshes".into());
                }
            }
            SelectAction::Restore(id) => {
                if let Some(c) = client {
                    c.restore_character(id);
                    self.select.message = Some("Restore requested; the list refreshes".into());
                }
            }
        }
    }

    fn apply_create(&mut self, action: CreateAction, client: Option<&mut Client>) {
        match action {
            CreateAction::Cancel => self.screen = Some(Screen::Select),
            CreateAction::Create => {
                let Some(st) = &mut self.create else { return };
                match client {
                    Some(c) => match c.create_character(&st.build) {
                        Ok(()) => {
                            st.pending = true;
                            st.message = None;
                        }
                        Err(e) => st.message = Some(create::describe_error(&e)),
                    },
                    None => st.message = Some("No session: nothing was sent".into()),
                }
            }
        }
    }

    /// Draw the current screen and act on it.
    pub fn ui(&mut self, egui: &egui::Context, mut client: Option<&mut Client>) {
        match self.screen {
            None => {}
            Some(Screen::Select) => {
                let v = self.select_view(client.as_deref());
                for a in select::draw(egui, &v, &mut self.select) {
                    self.apply_select(a, client.as_deref_mut());
                }
            }
            Some(Screen::Create) => {
                let Some(st) = &mut self.create else {
                    self.screen = Some(Screen::Select);
                    return;
                };
                for a in create::draw(egui, st) {
                    self.apply_create(a, client.as_deref_mut());
                }
            }
        }
    }

    /// A key while a screen is up; true when it was used.
    pub fn key(&mut self, key: egui::Key, pressed: bool, client: Option<&mut Client>) -> bool {
        if !pressed {
            return self.visible();
        }
        match self.screen {
            None => false,
            Some(Screen::Select) => {
                let v = self.select_view(client.as_deref());
                // Escape is only ours while a delete waits for Yes/No, so
                // it still quits the client otherwise.
                let used = matches!(
                    key,
                    egui::Key::ArrowUp | egui::Key::ArrowDown | egui::Key::Enter
                ) || (key == egui::Key::Escape && self.select.confirm_delete.is_some());
                if let Some(a) = select::key(&mut self.select, &v, key) {
                    self.apply_select(a, client);
                }
                used
            }
            Some(Screen::Create) => {
                let Some(st) = &mut self.create else {
                    return false;
                };
                let (action, used) = create::key(st, key);
                if let Some(a) = action {
                    self.apply_create(a, client);
                }
                used
            }
        }
    }
}

impl Plugin for Lobby {
    fn name(&self) -> &str {
        "lobby"
    }

    fn on_event(&mut self, cx: &mut Ctx, ev: &Event) {
        if cx.index == 0 {
            Lobby::on_event(self, ev);
        }
    }

    fn tick(&mut self, cx: &mut Ctx) {
        if let Some(c) = cx.try_client() {
            Lobby::tick(self, c);
        }
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        let c = cx.try_client();
        Lobby::ui(self, egui, c);
    }

    fn key(&mut self, cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        let c = cx.try_client();
        Lobby::key(self, key, pressed, c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_drive_the_screens() {
        let mut l = Lobby::default();
        assert!(!l.visible());
        l.on_event(&Event::Characters(Vec::new()));
        assert_eq!(l.screen, Some(Screen::Select));
        l.on_event(&Event::Placed { cell: 0xA9B4_0001 });
        assert!(!l.visible());
        l.on_event(&Event::CharacterCreated {
            id: 1,
            name: "Reborn".into(),
        });
        assert_eq!(l.screen, Some(Screen::Select));
        assert!(l.select.message.as_deref().unwrap().contains("Reborn"));
    }

    #[test]
    fn demo_select_keys_work_without_a_session() {
        let mut l = Lobby::demo_select();
        assert!(l.key(egui::Key::ArrowDown, true, None));
        assert_eq!(l.select.highlighted, 1);
        // Enter with no session is used but sends nothing.
        assert!(l.key(egui::Key::Enter, true, None));
        assert_eq!(l.screen, Some(Screen::Select));
        assert!(l.preview().is_none());
        // Escape is left to the host unless a delete is waiting for Yes/No.
        assert!(!l.key(egui::Key::Escape, true, None));
        l.select.confirm_delete = Some(1);
        assert!(l.key(egui::Key::Escape, true, None));
        assert_eq!(l.select.confirm_delete, None);
    }

    #[test]
    fn create_screen_steps_and_cancels() {
        let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
            eprintln!("AC_DATA_DIR unset; skipping");
            return;
        };
        let assets = Rc::new(Assets::open(std::path::Path::new(&dir)).unwrap());
        let mut l = Lobby::demo_create(assets).unwrap();
        assert!(l.preview().is_some());
        assert!(l.key(egui::Key::ArrowRight, true, None));
        assert_eq!(l.create.as_ref().unwrap().step, create::Step::Appearance);
        assert!(!l.key(egui::Key::A, true, None));
        assert!(l.key(egui::Key::Escape, true, None));
        assert_eq!(l.screen, Some(Screen::Select));
        assert!(l.preview().is_none());
        // New character reopens the same build.
        l.apply_select(SelectAction::New, None);
        assert_eq!(l.screen, Some(Screen::Create));
        assert_eq!(l.create.as_ref().unwrap().build.name, "Reborn");
        l.on_event(&Event::CharacterCreateFailed(3));
        assert!(l.create.as_ref().unwrap().message.is_some());
    }
}
