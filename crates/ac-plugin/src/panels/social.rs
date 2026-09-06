//! Social (N): the title shown under your name (pick one of those you
//! have earned), the friends list with who is online, and the squelch
//! list. Add a friend or squelch someone by name or from the selected
//! player; remove with the buttons.

use super::{caption, title, window, Source};
use crate::{egui, Client, Ctx, Plugin};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocialView {
    /// (id, name) of every earned title, and the shown one's id.
    pub titles: Vec<(u32, String)>,
    pub current_title: u32,
    /// (guid, name, online).
    pub friends: Vec<(u32, String, bool)>,
    /// (guid, name, description of what is squelched).
    pub squelches: Vec<(u32, String, String)>,
    /// The selected player, for the Add friend / Squelch buttons.
    pub selected: Option<(u32, String)>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Actions {
    pub set_title: Option<u32>,
    pub add_friend: Option<String>,
    pub remove_friend: Option<u32>,
    /// (guid, name, on).
    pub squelch: Option<(u32, String, bool)>,
}

/// Words for a squelch mask.
pub fn mask_text(mask: u32) -> String {
    if mask == 0xFFFF_FFFF {
        return "everything".into();
    }
    let names = [
        (0x4, "speech"),
        (0x8, "tells"),
        (0x40, "combat"),
        (0x80, "magic"),
        (0x1000, "emotes"),
        (0x10000, "appraisal"),
        (0x20000, "spellcasting"),
        (0x40000, "allegiance"),
        (0x80000, "fellowship"),
        (0x200000, "enemy combat"),
        (0x400000, "own combat"),
        (0x800000, "recall"),
        (0x1000000, "craft"),
        (0x2000000, "salvaging"),
    ];
    let parts: Vec<&str> = names
        .iter()
        .filter(|(b, _)| mask & b != 0)
        .map(|(_, n)| *n)
        .collect();
    if parts.is_empty() {
        "nothing".into()
    } else {
        parts.join(", ")
    }
}

/// "ID_CharacterTitle_Bearer_of_Darkness" -> "Bearer of Darkness";
/// "AbhorrentWarrior" -> "Abhorrent Warrior".
pub fn title_words(name: &str) -> String {
    let name = name.strip_prefix("ID_CharacterTitle_").unwrap_or(name);
    if name.contains('_') {
        return name.replace('_', " ");
    }
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if i > 0 && ch.is_uppercase() {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

pub fn view(c: &Client) -> SocialView {
    let me = c.world.player_guid;
    let names = c.assets.character_titles().ok();
    let titles = c
        .world
        .titles
        .ids
        .iter()
        .map(|id| {
            let n = names
                .as_ref()
                .and_then(|t| t.get(*id).map(title_words))
                .unwrap_or_else(|| format!("title {id}"));
            (*id, n)
        })
        .collect();
    let selected = c
        .selected
        .and_then(|g| c.world.objects.get(&g))
        .filter(|o| {
            o.object_desc_flags & ac_world::object_desc_flags::PLAYER != 0 && Some(o.guid) != me
        })
        .map(|o| (o.guid, o.name.clone()));
    SocialView {
        titles,
        current_title: c.world.titles.current,
        friends: c
            .world
            .friends
            .iter()
            .map(|f| (f.guid, f.name.clone(), f.online))
            .collect(),
        squelches: c
            .world
            .squelches
            .characters
            .iter()
            .map(|s| {
                let mut what = mask_text(s.mask);
                if s.account {
                    what.push_str(" (account)");
                }
                (s.guid, s.name.clone(), what)
            })
            .collect(),
        selected,
    }
}

pub fn draw(egui: &egui::Context, v: &SocialView, name_box: &mut String) -> Actions {
    let mut actions = Actions::default();
    let w = egui.viewport_rect().width();
    window(
        "social",
        egui::pos2(w * 0.5 - 220.0, 60.0),
        egui::vec2(440.0, 320.0),
        170,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(424.0, 304.0));
        title(ui, "Social");
        ui.horizontal(|ui| {
            caption(ui, "Title");
            let current = v
                .titles
                .iter()
                .find(|(id, _)| *id == v.current_title)
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| "(none)".into());
            egui::ComboBox::from_id_salt("title_pick")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (id, n) in &v.titles {
                        if ui.selectable_label(*id == v.current_title, n).clicked() {
                            actions.set_title = Some(*id);
                        }
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(name_box).desired_width(140.0));
            let has = !name_box.trim().is_empty();
            if ui
                .add_enabled(has, egui::Button::new("Add friend"))
                .clicked()
            {
                actions.add_friend = Some(name_box.trim().to_string());
            }
            if ui.add_enabled(has, egui::Button::new("Squelch")).clicked() {
                actions.squelch = Some((0, name_box.trim().to_string(), true));
            }
        });
        if let Some((g, n)) = &v.selected {
            ui.horizontal(|ui| {
                if ui.button(format!("Add {n} as friend")).clicked() {
                    actions.add_friend = Some(n.clone());
                }
                if ui.button(format!("Squelch {n}")).clicked() {
                    actions.squelch = Some((*g, n.clone(), true));
                }
            });
        }
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            caption(ui, format!("Friends ({})", v.friends.len()));
            egui::ScrollArea::vertical()
                .id_salt("friends")
                .max_height(180.0)
                .show(ui, |ui| {
                    for (g, n, online) in &v.friends {
                        ui.horizontal(|ui| {
                            let color = if *online {
                                egui::Color32::from_rgb(140, 230, 140)
                            } else {
                                egui::Color32::from_gray(150)
                            };
                            ui.label(egui::RichText::new(n).color(color));
                            if ui.small_button("x").clicked() {
                                actions.remove_friend = Some(*g);
                            }
                        });
                    }
                });
            let ui = &mut cols[1];
            caption(ui, format!("Squelched ({})", v.squelches.len()));
            egui::ScrollArea::vertical()
                .id_salt("squelches")
                .max_height(180.0)
                .show(ui, |ui| {
                    for (g, n, what) in &v.squelches {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(n).color(egui::Color32::WHITE));
                            ui.label(
                                egui::RichText::new(what)
                                    .color(egui::Color32::from_gray(160))
                                    .small(),
                            );
                            if ui.small_button("x").clicked() {
                                actions.squelch = Some((*g, n.clone(), false));
                            }
                        });
                    }
                });
        });
    });
    actions
}

pub struct Social {
    source: Source<SocialView>,
    pub show: bool,
    name_box: String,
}

impl Default for Social {
    fn default() -> Self {
        Social {
            source: Source::Live,
            show: false,
            name_box: String::new(),
        }
    }
}

impl Social {
    pub fn demo() -> Self {
        Social {
            source: Source::Demo(SocialView {
                titles: vec![(1, "Adventurer".into()), (2, "Archer".into())],
                current_title: 2,
                friends: vec![(1, "Reborn".into(), true), (2, "Test Mage".into(), false)],
                squelches: vec![(3, "Loudmouth".into(), "speech, tells".into())],
                selected: Some((4, "Aluvian Archer".into())),
            }),
            show: true,
            name_box: String::new(),
        }
    }
}

impl Plugin for Social {
    fn name(&self) -> &str {
        "social"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if !self.show {
            return;
        }
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().map(|c| view(c)),
        };
        let Some(v) = v else { return };
        let a = draw(egui, &v, &mut self.name_box);
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            if let Some(t) = a.set_title {
                c.set_title(t);
            }
            if let Some(n) = a.add_friend {
                c.add_friend(&n);
                self.name_box.clear();
            }
            if let Some(g) = a.remove_friend {
                c.remove_friend(Some(g));
            }
            if let Some((g, n, on)) = a.squelch {
                c.squelch(g, &n, on);
                self.name_box.clear();
            }
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if key == egui::Key::N && pressed {
            self.show = !self.show;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        assert_eq!(title_words("AbhorrentWarrior"), "Abhorrent Warrior");
        assert_eq!(
            title_words("ID_CharacterTitle_Bearer_of_Darkness"),
            "Bearer of Darkness"
        );
        assert_eq!(title_words("Archer"), "Archer");
        assert_eq!(mask_text(0xFFFF_FFFF), "everything");
        assert_eq!(mask_text(0xC), "speech, tells");
        assert_eq!(mask_text(0), "nothing");
    }
}
