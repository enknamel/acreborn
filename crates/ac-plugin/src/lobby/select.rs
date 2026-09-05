//! Character select: the account's characters, Enter, Delete (with a
//! confirmation), Restore for one pending deletion, and New character.
//! Up/Down move the highlight, Enter enters the highlighted character.

use crate::panels::{caption, fmt_seconds, frame, title};
use crate::{egui, Client};

/// One character of the account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharacterRow {
    pub id: u32,
    pub name: String,
    /// Non-zero while the character is pending deletion.
    pub seconds_until_deleted: u32,
}

impl CharacterRow {
    pub fn pending_deletion(&self) -> bool {
        self.seconds_until_deleted > 0
    }
}

/// What the select screen draws.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectView {
    pub account: String,
    pub host: String,
    pub characters: Vec<CharacterRow>,
    /// The character an enter request went out for: the screen waits on
    /// the server and disables the buttons.
    pub entering: Option<String>,
}

/// Highlight, confirmation and status of the select screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectState {
    pub highlighted: usize,
    /// Delete was pressed for this character; Yes/No decide.
    pub confirm_delete: Option<u32>,
    /// A line under the list (a refusal, "deleted", ...).
    pub message: Option<String>,
}

/// What was clicked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectAction {
    Enter(u32),
    Delete(u32),
    Restore(u32),
    New,
}

pub fn view(c: &Client) -> SelectView {
    SelectView {
        account: c.config.account.clone(),
        host: c.config.host.clone(),
        characters: c
            .characters
            .iter()
            .map(|e| CharacterRow {
                id: e.id,
                name: e.name.clone(),
                seconds_until_deleted: e.seconds_until_deleted,
            })
            .collect(),
        entering: c.enter_requested.then(|| {
            c.config
                .character
                .clone()
                .or_else(|| c.characters.first().map(|e| e.name.clone()))
                .unwrap_or_default()
        }),
    }
}

/// `deleting in 12m 34s`.
pub fn deletion_label(seconds: u32) -> String {
    format!("deleting in {}", fmt_seconds(seconds as f64))
}

/// Move the highlight by `delta` rows, clamped to the list.
pub fn move_highlight(highlighted: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (highlighted as i64 + delta as i64).clamp(0, len as i64 - 1) as usize
}

/// A key while the select screen is up: Up/Down move, Enter enters the
/// highlighted character, Escape cancels a pending confirmation.
pub fn key(st: &mut SelectState, v: &SelectView, key: egui::Key) -> Option<SelectAction> {
    let len = v.characters.len();
    st.highlighted = st.highlighted.min(len.saturating_sub(1));
    match key {
        egui::Key::ArrowUp => st.highlighted = move_highlight(st.highlighted, -1, len),
        egui::Key::ArrowDown => st.highlighted = move_highlight(st.highlighted, 1, len),
        egui::Key::Enter if v.entering.is_none() => {
            let row = v.characters.get(st.highlighted)?;
            if let Some(id) = st.confirm_delete.take() {
                return Some(SelectAction::Delete(id));
            }
            if !row.pending_deletion() {
                return Some(SelectAction::Enter(row.id));
            }
        }
        egui::Key::Escape => st.confirm_delete = None,
        _ => {}
    }
    None
}

pub fn draw(egui: &egui::Context, v: &SelectView, st: &mut SelectState) -> Vec<SelectAction> {
    let mut actions = Vec::new();
    let busy = v.entering.is_some();
    st.highlighted = st.highlighted.min(v.characters.len().saturating_sub(1));
    egui::Window::new("character_select")
        .fade_in(false)
        .title_bar(false)
        .resizable(false)
        .frame(frame(200, 12))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
        .fixed_size(egui::vec2(520.0, 0.0))
        .show(egui, |ui| {
            ui.set_min_width(500.0);
            title(ui, "Choose a character");
            caption(ui, format!("{} on {}", v.account, v.host));
            ui.add_space(8.0);
            if v.characters.is_empty() {
                ui.label(
                    egui::RichText::new("No characters on this account yet.")
                        .color(egui::Color32::from_gray(200)),
                );
            }
            for (i, row) in v.characters.iter().enumerate() {
                let selected = i == st.highlighted;
                let color = if row.pending_deletion() {
                    egui::Color32::from_gray(150)
                } else {
                    egui::Color32::WHITE
                };
                ui.horizontal(|ui| {
                    let label = egui::RichText::new(&row.name).color(color).size(16.0);
                    if ui.selectable_label(selected, label).clicked() {
                        st.highlighted = i;
                        st.confirm_delete = None;
                    }
                    if row.pending_deletion() {
                        ui.label(
                            egui::RichText::new(deletion_label(row.seconds_until_deleted))
                                .color(egui::Color32::from_rgb(255, 170, 120))
                                .small(),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if row.pending_deletion() {
                            if ui
                                .add_enabled(!busy, egui::Button::new("Restore"))
                                .clicked()
                            {
                                actions.push(SelectAction::Restore(row.id));
                            }
                        } else {
                            if ui
                                .add_enabled(!busy, egui::Button::new("Delete"))
                                .clicked()
                            {
                                st.highlighted = i;
                                st.confirm_delete = Some(row.id);
                            }
                            if ui
                                .add_enabled(!busy, egui::Button::new("Enter"))
                                .clicked()
                            {
                                st.highlighted = i;
                                actions.push(SelectAction::Enter(row.id));
                            }
                        }
                    });
                });
                if st.confirm_delete == Some(row.id) {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "Delete {}? The server keeps it for a while; Restore brings it back.",
                                row.name
                            ))
                            .color(egui::Color32::from_rgb(255, 120, 120)),
                        );
                        if ui.button("Yes, delete").clicked() {
                            st.confirm_delete = None;
                            actions.push(SelectAction::Delete(row.id));
                        }
                        if ui.button("No").clicked() {
                            st.confirm_delete = None;
                        }
                    });
                }
            }
            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new("New character"))
                    .clicked()
                {
                    actions.push(SelectAction::New);
                }
                if let Some(name) = &v.entering {
                    ui.label(
                        egui::RichText::new(format!("Entering the world as {name}..."))
                            .color(egui::Color32::from_rgb(255, 220, 120)),
                    );
                } else {
                    caption(ui, "Up/Down highlight, Enter enters the highlighted character");
                }
            });
            if let Some(m) = &st.message {
                ui.label(egui::RichText::new(m).color(egui::Color32::from_rgb(255, 220, 120)));
            }
        });
    actions
}

/// Three characters, one pending deletion, for `--demo-select`.
pub fn demo_view() -> SelectView {
    SelectView {
        account: "demo".into(),
        host: "127.0.0.1".into(),
        characters: vec![
            CharacterRow {
                id: 0x5000_0001,
                name: "Reborn".into(),
                seconds_until_deleted: 0,
            },
            CharacterRow {
                id: 0x5000_0002,
                name: "Aluvian Archer".into(),
                seconds_until_deleted: 0,
            },
            CharacterRow {
                id: 0x5000_0003,
                name: "Old Mage".into(),
                seconds_until_deleted: 754,
            },
        ],
        entering: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_clamps() {
        assert_eq!(move_highlight(0, -1, 3), 0);
        assert_eq!(move_highlight(0, 1, 3), 1);
        assert_eq!(move_highlight(2, 1, 3), 2);
        assert_eq!(move_highlight(5, 0, 0), 0);
    }

    #[test]
    fn enter_key_enters_the_highlighted_character() {
        let v = demo_view();
        let mut st = SelectState::default();
        assert_eq!(key(&mut st, &v, egui::Key::ArrowDown), None);
        assert_eq!(st.highlighted, 1);
        assert_eq!(
            key(&mut st, &v, egui::Key::Enter),
            Some(SelectAction::Enter(0x5000_0002))
        );
        // A character pending deletion cannot be entered.
        st.highlighted = 2;
        assert_eq!(key(&mut st, &v, egui::Key::Enter), None);
        // Enter confirms a pending delete; Escape cancels it.
        st.highlighted = 0;
        st.confirm_delete = Some(0x5000_0001);
        assert_eq!(
            key(&mut st, &v, egui::Key::Enter),
            Some(SelectAction::Delete(0x5000_0001))
        );
        st.confirm_delete = Some(0x5000_0001);
        assert_eq!(key(&mut st, &v, egui::Key::Escape), None);
        assert_eq!(st.confirm_delete, None);
    }

    #[test]
    fn nothing_while_entering() {
        let mut v = demo_view();
        v.entering = Some("Reborn".into());
        let mut st = SelectState::default();
        assert_eq!(key(&mut st, &v, egui::Key::Enter), None);
    }

    #[test]
    fn deletion_labels() {
        assert_eq!(deletion_label(754), "deleting in 12m 34s");
        assert!(demo_view().characters[2].pending_deletion());
    }
}
