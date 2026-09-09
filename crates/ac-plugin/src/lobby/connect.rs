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
        remember: bool,
    },
    /// The player added or renamed one of their own servers.
    AddServer(Server),
    /// Forget a remembered login for (`host`, `account`).
    Forget { host: String, account: String },
}

/// The connect form's state (what is typed, and the add-server sub-form).
#[derive(Default)]
pub struct ConnectState {
    /// The chosen server as `host:port`.
    pub host: String,
    pub account: String,
    pub password: String,
    pub remember: bool,
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
            // The accounts saved for this server, the one last played
            // first, with the chosen one marked.
            let saved = servers.accounts_for(&st.host);
            if !saved.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(if saved.len() == 1 {
                        "Account:".to_string()
                    } else {
                        format!("{} accounts:", saved.len())
                    });
                    for l in &saved {
                        let chosen = l.account.eq_ignore_ascii_case(&st.account);
                        let mut label = egui::RichText::new(&l.account);
                        if chosen {
                            label = label.strong();
                        }
                        let hint = if l.password.is_empty() {
                            format!("{} (no password saved)", l.account)
                        } else {
                            l.account.clone()
                        };
                        if ui
                            .selectable_label(chosen, label)
                            .on_hover_text(hint)
                            .clicked()
                        {
                            let l = (*l).clone();
                            st.use_login(&l);
                        }
                        if ui
                            .small_button("✕")
                            .on_hover_text(format!("Forget {} on this server", l.account))
                            .clicked()
                        {
                            actions.push(ConnectAction::Forget {
                                host: st.host.clone(),
                                account: l.account.clone(),
                            });
                        }
                    }
                });
            }
            ui.add_space(6.0);
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
            ui.add_space(8.0);

            let ready = !st.host.is_empty() && !st.account.trim().is_empty();
            if ui
                .add_enabled(ready, egui::Button::new("Connect"))
                .clicked()
            {
                actions.push(ConnectAction::Connect {
                    host: st.host.clone(),
                    account: st.account.trim().to_string(),
                    password: st.password.clone(),
                    remember: st.remember,
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
