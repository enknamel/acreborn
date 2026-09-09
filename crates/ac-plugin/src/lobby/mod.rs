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

pub mod connect;
pub mod create;
pub mod select;
pub mod store;

use std::rc::Rc;

use ac_client::creation::{self, CharacterBuild};
use ac_scene::Assets;

use crate::servers::{Login, Server, Servers};
use crate::{egui, Client, Ctx, Event, Plugin};
use connect::{ConnectAction, ConnectState};
use create::{CreateAction, CreateState};
use select::{SelectAction, SelectState, SelectView};

/// Which screen is up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// Pick a server, account and password.
    Connect,
    Select,
    Create,
}

/// A change the connect screen made to the login store, kept until it
/// is folded into the file (the fleet panel writes the same file, so the
/// whole of an in-memory copy is never written over it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerEdit {
    Add(Server),
    Remember {
        host: String,
        account: String,
        password: String,
    },
    Forget {
        host: String,
        account: String,
    },
}

impl ServerEdit {
    /// Make this change to `servers`.
    pub fn apply(self, servers: &mut Servers) {
        match self {
            ServerEdit::Add(s) => servers.add(s),
            ServerEdit::Remember {
                host,
                account,
                password,
            } => {
                // Keep the character the fleet panel remembered for the
                // account; the connect screen does not ask for one.
                let character = servers
                    .accounts_for(&host)
                    .into_iter()
                    .find(|l| l.account.eq_ignore_ascii_case(&account))
                    .map(|l| l.character.clone())
                    .unwrap_or_default();
                servers.remember(&host, &account, &password, &character);
            }
            ServerEdit::Forget { host, account } => servers.forget(&host, &account),
        }
    }
}

#[derive(Default)]
pub struct Lobby {
    pub screen: Option<Screen>,
    pub select: SelectState,
    pub create: Option<CreateState>,
    /// The connect screen's state and the servers/logins it draws from.
    pub servers: Servers,
    pub connect: ConnectState,
    /// A login the connect screen asked to start, for the host to pick up.
    pending: Option<Login>,
    /// Changes the connect screen made that are not on disk yet.
    edits: Vec<ServerEdit>,
    /// The login store's file, and when it was last read from: the fleet
    /// panel writes the same file, so an open connect screen re-reads it
    /// once it moves (see [`store`]).
    store: Option<std::path::PathBuf>,
    store_seen: Option<std::time::SystemTime>,
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

    /// Open the connect screen on the player's servers and logins.
    pub fn open_connect(&mut self, servers: Servers) {
        self.connect = ConnectState::from_servers(&servers);
        self.servers = servers;
        self.store_seen = store::changed_at(&self.store_path());
        self.screen = Some(Screen::Connect);
    }

    /// The login store's file: the usual place, or the one a test set.
    fn store_path(&self) -> std::path::PathBuf {
        self.store.clone().unwrap_or_else(store::path)
    }

    /// Read the login store again when it moved under us (the fleet
    /// panel added or forgot an account), so the connect screen offers
    /// the same accounts the fleet does. Changes of our own that are
    /// not written yet come first, so nothing typed is lost.
    fn refresh_servers(&mut self) {
        if self.screen != Some(Screen::Connect) || !self.edits.is_empty() {
            return;
        }
        let path = self.store_path();
        let now = store::changed_at(&path);
        if now == self.store_seen {
            return;
        }
        self.store_seen = now;
        self.servers = store::load_at(&path);
    }

    /// The login the connect screen asked to start, if any (taken once).
    pub fn take_connect(&mut self) -> Option<Login> {
        self.pending.take()
    }

    /// The servers/logins to write back, when they changed (taken once).
    /// What is on disk is read again first and the screen's changes made
    /// to that, so an account the fleet panel added since is kept.
    pub fn take_dirty_servers(&mut self) -> Option<Servers> {
        if self.edits.is_empty() {
            return None;
        }
        let merged = self.merged(store::load_at(&self.store_path()));
        self.store_seen = store::changed_at(&self.store_path());
        Some(merged)
    }

    /// `base` with this screen's pending changes made to it; they are
    /// taken, and the screen's own copy becomes the result.
    fn merged(&mut self, mut base: Servers) -> Servers {
        for e in self.edits.drain(..) {
            e.apply(&mut base);
        }
        self.servers = base.clone();
        base
    }

    /// Act on the connect screen: start a login, add a server, or forget
    /// a remembered account.
    fn apply_connect(&mut self, action: ConnectAction) {
        match action {
            ConnectAction::Connect {
                host,
                account,
                password,
                remember,
            } => {
                // Remember the account (and the password when asked); an
                // un-remembered account still comes back, without its
                // password.
                let kept = if remember {
                    password.clone()
                } else {
                    String::new()
                };
                self.edit(ServerEdit::Remember {
                    host: host.clone(),
                    account: account.clone(),
                    password: kept,
                });
                self.pending = Some(Login {
                    host,
                    account,
                    password,
                    character: String::new(),
                });
                // The character list will flip us to Select on arrival.
                self.screen = Some(Screen::Select);
            }
            ConnectAction::AddServer(server) => self.edit(ServerEdit::Add(server)),
            ConnectAction::Forget { host, account } => {
                self.edit(ServerEdit::Forget { host, account })
            }
        }
    }

    /// Make a change to the login store: it shows at once, and waits
    /// for [`Lobby::take_dirty_servers`] to reach the file.
    fn edit(&mut self, e: ServerEdit) {
        e.clone().apply(&mut self.servers);
        self.edits.push(e);
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
            Some(Screen::Connect) => {
                let actions = connect::draw(egui, &self.servers, &mut self.connect);
                for a in actions {
                    self.apply_connect(a);
                }
            }
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
            // egui's text fields handle their own keys; keep them from the
            // game, but let Escape through so the app can still quit.
            Some(Screen::Connect) => key != egui::Key::Escape,
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
        if cx.index == 0 {
            self.refresh_servers();
        }
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

    /// A scratch login store of this test's own.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("acswarm-lobby-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(store::FILE_NAME)
    }

    /// The connect screen and the fleet panel write the same file, so
    /// what the screen changes is folded into what is on disk rather
    /// than written over it, and the screen re-reads a change made
    /// while it is open.
    #[test]
    fn connect_changes_fold_into_the_shared_login_store() {
        let path = scratch("share");
        // The fleet panel remembered an account before the screen opened.
        store::update_at(&path, |s| s.remember("h:9000", "alice", "pw", "Alys"));
        let mut l = Lobby {
            store: Some(path.clone()),
            ..Default::default()
        };
        l.open_connect(store::load_at(&path));
        assert_eq!(l.screen, Some(Screen::Connect));
        assert_eq!(l.connect.account, "alice", "the screen offers it");
        assert!(l.take_dirty_servers().is_none(), "nothing changed yet");

        // The player connects as someone else and asks to be remembered.
        l.apply_connect(ConnectAction::Connect {
            host: "h:9000".into(),
            account: "bob".into(),
            password: "pw2".into(),
            remember: true,
        });
        assert_eq!(l.servers.accounts_for("h:9000").len(), 2, "shown at once");
        assert_eq!(l.take_connect().map(|c| c.account).as_deref(), Some("bob"));

        // The fleet panel adds another while the screen holds its change.
        store::update_at(&path, |s| s.remember("h:9000", "carol", "pw3", ""));
        let merged = l.take_dirty_servers().expect("a change to write");
        let names: Vec<&str> = merged
            .accounts_for("h:9000")
            .iter()
            .map(|l| l.account.as_str())
            .collect();
        assert_eq!(names, ["alice", "carol", "bob"], "nobody was written over");
        assert_eq!(merged.last_host, "h:9000");
        assert_eq!(merged.last_account, "bob");
        // alice's character survived a screen that never asks for one.
        assert_eq!(merged.accounts_for("h:9000")[0].character, "Alys");
        assert!(l.take_dirty_servers().is_none(), "taken once");

        // A change made while the screen sits open is read again; off
        // the connect screen nothing is re-read.
        store::save_at(&path, &merged).unwrap();
        store::update_at(&path, |s| s.remember("h:9000", "dain", "pw4", ""));
        l.store_seen = None;
        l.refresh_servers();
        assert_eq!(l.servers.accounts_for("h:9000").len(), 3, "on Select");
        l.screen = Some(Screen::Connect);
        l.refresh_servers();
        assert_eq!(l.servers.accounts_for("h:9000").len(), 4);
        // Forgetting one on the screen forgets it in the store.
        l.apply_connect(ConnectAction::Forget {
            host: "h:9000".into(),
            account: "dain".into(),
        });
        let merged = l.take_dirty_servers().unwrap();
        assert!(!merged
            .accounts_for("h:9000")
            .iter()
            .any(|l| l.account == "dain"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

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
