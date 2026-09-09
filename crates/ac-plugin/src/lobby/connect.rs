//! The connect screen: choose a server (Coldeve first), or add your own,
//! type an account and password, and connect. The servers and the
//! remembered logins are held by [`crate::servers::Servers`]; this screen
//! reads them and returns [`ConnectAction`]s for the host to apply.

use crate::egui;
use crate::panels::{caption, frame, title};
use crate::servers::{Server, Servers};

/// What the connect screen asks the host to do this frame.
pub enum ConnectAction {
    /// Connect to `host` (an `address:port`) with these credentials.
    Connect {
        host: String,
        account: String,
        password: String,
        /// The character to enter the world as. Empty stops at the
        /// character list to choose by hand.
        character: String,
        remember: bool,
    },
    /// The player added or renamed one of their own servers.
    AddServer(Server),
    /// Forget a remembered login for (`host`, `account`).
    Forget { host: String, account: String },
    /// The character an account should enter with from now on.
    SetCharacter {
        host: String,
        account: String,
        name: String,
    },
}

/// The connect form's state (what is typed, and the add-server sub-form).
#[derive(Default)]
pub struct ConnectState {
    /// The chosen server as `host:port`.
    pub host: String,
    pub account: String,
    pub password: String,
    pub remember: bool,
    /// The add-account form is open.
    pub adding_account: bool,
    pub adding: bool,
    pub new_name: String,
    pub new_host: String,
    pub new_port: String,
    pub message: Option<String>,
}

impl ConnectState {
    /// Fill the form from what was used last: the last server and account,
    /// with its remembered password.
    pub fn from_servers(s: &Servers) -> Self {
        let all = s.all();
        let host = if !s.last_host.is_empty() {
            s.last_host.clone()
        } else {
            all.first().map(|x| x.address()).unwrap_or_default()
        };
        // The account last used *on this server*, not the last account
        // used anywhere: a player with three accounts on Coldeve and
        // one on a test shard wants the Coldeve one they were just
        // playing, whichever server they were on before.
        let (account, password, remember) = match s.last_login(&host) {
            Some(l) => (
                l.account.clone(),
                l.password.clone(),
                !l.password.is_empty(),
            ),
            None => (String::new(), String::new(), false),
        };
        ConnectState {
            host,
            account,
            password,
            remember,
            new_port: "9000".into(),
            ..Default::default()
        }
    }

    /// Pull a server's saved account into the form.
    fn use_login(&mut self, l: &crate::servers::Login) {
        self.account = l.account.clone();
        self.password = l.password.clone();
        self.remember = !l.password.is_empty();
    }

    /// Empty the account fields, for a server nothing is saved for.
    fn clear_login(&mut self) {
        self.account.clear();
        self.password.clear();
        self.remember = false;
    }
}

pub fn draw(egui: &egui::Context, servers: &Servers, st: &mut ConnectState) -> Vec<ConnectAction> {
    let mut actions = Vec::new();
    let all = servers.all();
    egui::Window::new("connect")
        .fade_in(false)
        .title_bar(false)
        .resizable(false)
        .frame(frame(200, 12))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
        .fixed_size(egui::vec2(520.0, 0.0))
        .show(egui, |ui| {
            ui.set_min_width(500.0);
            title(ui, "Connect to a server");
            ui.add_space(8.0);

            // The server, chosen from the list (builtin + the player's).
            let current = all
                .iter()
                .find(|s| s.address() == st.host)
                .map(|s| format!("{} — {}", s.name, s.address()))
                .unwrap_or_else(|| {
                    if st.host.is_empty() {
                        "Choose a server".into()
                    } else {
                        st.host.clone()
                    }
                });
            ui.horizontal(|ui| {
                ui.label("Server");
                egui::ComboBox::from_id_salt("server")
                    .selected_text(current)
                    .width(360.0)
                    .show_ui(ui, |ui| {
                        for s in &all {
                            let label = format!("{} — {}", s.name, s.address());
                            if ui.selectable_label(st.host == s.address(), label).clicked() {
                                st.host = s.address();
                                match servers.last_login(&st.host) {
                                    Some(l) => st.use_login(l),
                                    // Nothing saved here: an empty form,
                                    // rather than the last server's
                                    // account sitting in it looking as
                                    // though it belonged.
                                    None => st.clear_login(),
                                }
                            }
                        }
                    });
            });

            // Quick-pick of remembered accounts on this server.
            ui.add_space(8.0);

            // The accounts saved for this server. One row each: who it
            // is, which character to go in as, and a button to play.
            let saved = servers.accounts_for(&st.host);
            if saved.is_empty() {
                caption(ui, "No accounts saved for this server yet.");
            } else {
                caption(
                    ui,
                    if saved.len() == 1 {
                        "1 account".to_string()
                    } else {
                        format!("{} accounts", saved.len())
                    },
                );
            }
            // A grid, so the character menus line up however long the
            // account names are.
            egui::Grid::new("accounts")
                .num_columns(4)
                .spacing(egui::vec2(8.0, 4.0))
                .show(ui, |ui| {
                    for l in &saved {
                        let chosen = l.account.eq_ignore_ascii_case(&st.account);
                        // The account. Clicking it makes it the one the
                        // password box below belongs to.
                        let mut name = egui::RichText::new(&l.account);
                        if chosen {
                            name = name.strong();
                        }
                        if ui.selectable_label(chosen, name).clicked() {
                            let l = (*l).clone();
                            st.use_login(&l);
                        }

                        // Which character to enter as. The names are the
                        // ones the server listed last time this account
                        // logged in; an account never logged into has
                        // none yet, and stops at the character list.
                        let current = if l.character.is_empty() {
                            "Choose at login".to_string()
                        } else {
                            l.character.clone()
                        };
                        egui::ComboBox::from_id_salt(("character", &l.account))
                            .selected_text(current)
                            .width(170.0)
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(l.character.is_empty(), "Choose at login")
                                    .clicked()
                                {
                                    actions.push(ConnectAction::SetCharacter {
                                        host: st.host.clone(),
                                        account: l.account.clone(),
                                        name: String::new(),
                                    });
                                }
                                for name in &l.characters {
                                    if ui.selectable_label(&l.character == name, name).clicked() {
                                        actions.push(ConnectAction::SetCharacter {
                                            host: st.host.clone(),
                                            account: l.account.clone(),
                                            name: name.clone(),
                                        });
                                    }
                                }
                                if l.characters.is_empty() {
                                    caption(ui, "Log in once to list this account's characters");
                                }
                            });

                        let known = !l.password.is_empty();
                        if ui
                            .add_enabled(known, egui::Button::new("Play"))
                            .on_hover_text(if l.character.is_empty() {
                                "Log in and stop at the character list".to_string()
                            } else {
                                format!("Log in and enter as {}", l.character)
                            })
                            .on_disabled_hover_text(
                                "No password saved for this account. Pick it, type the \
                                 password below, and connect.",
                            )
                            .clicked()
                        {
                            actions.push(ConnectAction::Connect {
                                host: st.host.clone(),
                                account: l.account.clone(),
                                password: l.password.clone(),
                                character: l.character.clone(),
                                remember: true,
                            });
                        }
                        if ui
                            .small_button("x")
                            .on_hover_text(format!("Forget {} on this server", l.account))
                            .clicked()
                        {
                            actions.push(ConnectAction::Forget {
                                host: st.host.clone(),
                                account: l.account.clone(),
                            });
                        }
                        ui.end_row();
                    }
                });

            ui.add_space(6.0);
            // Adding an account, or typing the password for one that
            // was saved without it.
            let needs_password = !st.account.trim().is_empty()
                && saved
                    .iter()
                    .any(|l| l.account.eq_ignore_ascii_case(&st.account) && l.password.is_empty());
            if !st.adding_account && !needs_password {
                if ui.button("Add an account…").clicked() {
                    st.adding_account = true;
                    st.clear_login();
                }
            } else {
                if needs_password && !st.adding_account {
                    caption(ui, format!("Password for {}", st.account.trim()));
                } else {
                    caption(ui, "Add an account");
                }
                ui.horizontal(|ui| {
                    ui.label("Account ");
                    ui.add(egui::TextEdit::singleline(&mut st.account).desired_width(300.0));
                });
                ui.horizontal(|ui| {
                    ui.label("Password");
                    ui.add(
                        egui::TextEdit::singleline(&mut st.password)
                            .password(true)
                            .desired_width(300.0),
                    );
                });
                ui.checkbox(&mut st.remember, "Remember this account and password");
                ui.horizontal(|ui| {
                    let ready = !st.host.is_empty() && !st.account.trim().is_empty();
                    if ui
                        .add_enabled(ready, egui::Button::new("Connect"))
                        .clicked()
                    {
                        let character = saved
                            .iter()
                            .find(|l| l.account.eq_ignore_ascii_case(st.account.trim()))
                            .map(|l| l.character.clone())
                            .unwrap_or_default();
                        actions.push(ConnectAction::Connect {
                            host: st.host.clone(),
                            account: st.account.trim().to_string(),
                            password: st.password.clone(),
                            character,
                            remember: st.remember,
                        });
                        st.adding_account = false;
                    }
                    if st.adding_account && ui.button("Cancel").clicked() {
                        st.adding_account = false;
                        st.clear_login();
                    }
                });
            }

            ui.separator();
            // Add your own server.
            if !st.adding {
                if ui.button("Add a server…").clicked() {
                    st.adding = true;
                    if st.new_port.trim().is_empty() {
                        st.new_port = "9000".into();
                    }
                }
            } else {
                caption(ui, "Add a server");
                ui.horizontal(|ui| {
                    ui.label("Name ");
                    ui.text_edit_singleline(&mut st.new_name);
                });
                ui.horizontal(|ui| {
                    ui.label("Host ");
                    ui.text_edit_singleline(&mut st.new_host);
                });
                ui.horizontal(|ui| {
                    ui.label("Port ");
                    ui.add(egui::TextEdit::singleline(&mut st.new_port).desired_width(80.0));
                });
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        match (st.new_host.trim(), st.new_port.trim().parse::<u16>()) {
                            (h, Ok(port)) if !h.is_empty() => {
                                let name = if st.new_name.trim().is_empty() {
                                    h.to_string()
                                } else {
                                    st.new_name.trim().to_string()
                                };
                                let server = Server {
                                    name,
                                    host: h.to_string(),
                                    port,
                                };
                                st.host = server.address();
                                actions.push(ConnectAction::AddServer(server));
                                st.adding = false;
                                st.new_name.clear();
                                st.new_host.clear();
                                st.new_port = "9000".into();
                                st.message = None;
                            }
                            _ => {
                                st.message = Some("Enter a host and a numeric port.".into());
                            }
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        st.adding = false;
                        st.message = None;
                    }
                });
            }
            if let Some(m) = &st.message {
                ui.add_space(4.0);
                ui.colored_label(egui::Color32::from_rgb(230, 130, 130), m);
            }
        });
    actions
}
