//! Fellowship (F): the server's group of up to nine players sharing XP
//! and optionally loot. Create one with a name, recruit the selected
//! player (they answer a confirmation), leave, or as leader dismiss
//! members or disband. The member list shows everyone's vitals.

use super::{caption, title, window, Source};
use crate::{egui, Client, Ctx, Plugin, Settings};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberRow {
    pub guid: u32,
    pub name: String,
    pub level: u32,
    pub health: (u32, u32),
    pub stamina: (u32, u32),
    pub mana: (u32, u32),
    pub leader: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FellowshipView {
    /// None when not in a fellowship.
    pub name: Option<String>,
    pub share_xp: bool,
    pub i_lead: bool,
    pub members: Vec<MemberRow>,
    /// The selected player, who Recruit would invite.
    pub selected: Option<(u32, String)>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Actions {
    pub create: Option<(String, bool)>,
    pub recruit: Option<u32>,
    pub quit: bool,
    pub disband: bool,
    pub dismiss: Vec<u32>,
}

pub fn view(c: &Client) -> FellowshipView {
    let me = c.world.player_guid;
    let selected = c
        .selected
        .and_then(|g| c.world.objects.get(&g))
        .filter(|o| {
            o.object_desc_flags & ac_world::object_desc_flags::PLAYER != 0 && Some(o.guid) != me
        })
        .map(|o| (o.guid, o.name.clone()));
    match c.world.fellowship.as_ref() {
        Some(f) => FellowshipView {
            name: Some(f.name.clone()),
            share_xp: f.share_xp,
            i_lead: Some(f.leader) == me,
            members: f
                .members
                .iter()
                .map(|m| MemberRow {
                    guid: m.guid,
                    name: m.name.clone(),
                    level: m.level,
                    health: m.health,
                    stamina: m.stamina,
                    mana: m.mana,
                    leader: m.guid == f.leader,
                })
                .collect(),
            selected,
        },
        None => FellowshipView {
            name: None,
            share_xp: true,
            i_lead: false,
            members: Vec::new(),
            selected,
        },
    }
}

/// `12/40` style vital text.
pub fn vital_text((cur, max): (u32, u32)) -> String {
    format!("{cur}/{max}")
}

pub fn draw(
    egui: &egui::Context,
    v: &FellowshipView,
    new_name: &mut String,
    share: &mut bool,
) -> Actions {
    let mut actions = Actions::default();
    let w = egui.viewport_rect().width();
    window(
        "fellowship",
        egui::pos2(w - 720.0, 2.0 * super::radar::RADIUS + 40.0),
        egui::vec2(440.0, 240.0),
        170,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(424.0, 224.0));
        match &v.name {
            None => {
                title(ui, "Fellowship");
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(new_name).desired_width(160.0));
                    ui.checkbox(share, "share XP");
                    if ui
                        .add_enabled(!new_name.trim().is_empty(), egui::Button::new("Create"))
                        .clicked()
                    {
                        actions.create = Some((new_name.trim().to_string(), *share));
                    }
                });
                ui.label(
                    egui::RichText::new(
                        "Not in a fellowship. Select a player and recruit them after creating one.",
                    )
                    .color(egui::Color32::from_gray(180))
                    .small(),
                );
            }
            Some(name) => {
                title(
                    ui,
                    format!(
                        "{name} ({}){}",
                        v.members.len(),
                        if v.share_xp { ", sharing XP" } else { "" }
                    ),
                );
                egui::Grid::new("fellows")
                    .num_columns(6)
                    .spacing([12.0, 2.0])
                    .show(ui, |ui| {
                        for h in ["Name", "Lvl", "Health", "Stamina", "Mana", ""] {
                            caption(ui, h);
                        }
                        ui.end_row();
                        for m in &v.members {
                            let color = if m.leader {
                                egui::Color32::from_rgb(255, 215, 120)
                            } else {
                                egui::Color32::WHITE
                            };
                            ui.label(egui::RichText::new(&m.name).color(color));
                            ui.label(egui::RichText::new(m.level.to_string()).color(color));
                            ui.label(
                                egui::RichText::new(vital_text(m.health))
                                    .color(egui::Color32::from_rgb(230, 120, 120)),
                            );
                            ui.label(
                                egui::RichText::new(vital_text(m.stamina))
                                    .color(egui::Color32::from_rgb(230, 200, 100)),
                            );
                            ui.label(
                                egui::RichText::new(vital_text(m.mana))
                                    .color(egui::Color32::from_rgb(120, 150, 255)),
                            );
                            if v.i_lead && !m.leader {
                                if ui.small_button("Dismiss").clicked() {
                                    actions.dismiss.push(m.guid);
                                }
                            } else {
                                ui.label("");
                            }
                            ui.end_row();
                        }
                    });
                ui.horizontal(|ui| {
                    match &v.selected {
                        Some((g, n)) if v.i_lead => {
                            if ui.button(format!("Recruit {n}")).clicked() {
                                actions.recruit = Some(*g);
                            }
                        }
                        _ => {}
                    }
                    if ui.button("Leave").clicked() {
                        actions.quit = true;
                    }
                    if v.i_lead && ui.button("Disband").clicked() {
                        actions.disband = true;
                    }
                });
            }
        }
    });
    actions
}

pub struct Fellowship {
    source: Source<FellowshipView>,
    pub show: bool,
    new_name: String,
    share: bool,
}

impl Default for Fellowship {
    fn default() -> Self {
        Fellowship {
            source: Source::Live,
            show: false,
            new_name: String::new(),
            share: true,
        }
    }
}

impl Fellowship {
    pub fn demo() -> Self {
        let m = |guid, name: &str, level, h, s, mn, leader| MemberRow {
            guid,
            name: name.into(),
            level,
            health: h,
            stamina: s,
            mana: mn,
            leader,
        };
        Fellowship {
            source: Source::Demo(FellowshipView {
                name: Some("Demo Fellows".into()),
                share_xp: true,
                i_lead: true,
                members: vec![
                    m(1, "Demo", 12, (42, 60), (90, 100), (10, 80), true),
                    m(2, "Reborn", 3, (20, 20), (30, 30), (12, 12), false),
                ],
                selected: Some((3, "Aluvian Archer".into())),
            }),
            show: true,
            new_name: String::new(),
            share: true,
        }
    }
}

impl Plugin for Fellowship {
    fn name(&self) -> &str {
        "fellowship"
    }

    fn load(&mut self, settings: &Settings) {
        if let Some(v) = settings.get("fellowship.show") {
            self.show = v;
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("fellowship.show", self.show);
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
        let a = draw(egui, &v, &mut self.new_name, &mut self.share);
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            if let Some((name, share)) = a.create {
                c.fellowship_create(&name, share);
                self.new_name.clear();
            }
            if let Some(g) = a.recruit {
                c.fellowship_recruit(g);
            }
            if a.quit {
                c.fellowship_quit(false);
            }
            if a.disband {
                c.fellowship_quit(true);
            }
            for g in a.dismiss {
                c.fellowship_dismiss(g);
            }
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if key == egui::Key::F && pressed {
            self.show = !self.show;
            return true;
        }
        false
    }
}
