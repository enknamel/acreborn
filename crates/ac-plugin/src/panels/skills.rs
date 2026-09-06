//! Skills: the character sheet (level, experience, skill credits) and every
//! skill with its value, ranks and training. K toggles it. Whether it is
//! open is published on the blackboard as [`OPEN_KEY`] so the spellbook
//! can sit beside it.

use super::{caption, has_sheet, window, Source};
use crate::{egui, Client, Ctx, Plugin};

/// Blackboard key: `true` while the skills panel is open.
pub const OPEN_KEY: &str = "panels.skills_open";

/// One line of the skills panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillRow {
    pub id: u32,
    pub name: &'static str,
    /// Current skill value (attribute base + creation bonus + ranks).
    pub value: u32,
    pub ranks: u16,
    /// Advancement class: 0 inactive, 1 untrained, 2 trained, 3 specialized.
    pub advancement: u32,
    pub training: &'static str,
    /// XP for the next rank (trained/specialized skills below max).
    pub raise_xp: Option<u32>,
    /// Credits to train (untrained/inactive skills).
    pub train_credits: Option<u32>,
}

/// An attribute or vital line: current value, ranks bought, next cost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatRow {
    pub name: &'static str,
    pub value: u32,
    pub ranks: u32,
    pub raise_xp: Option<u32>,
}

/// What the player clicked this frame.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Actions {
    pub raise_attribute: Vec<usize>,
    pub raise_vital: Vec<usize>,
    pub raise_skill: Vec<u32>,
    pub train_skill: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillsView {
    pub name: String,
    pub level: i32,
    pub total_xp: i64,
    pub available_xp: i64,
    pub skill_credits: i32,
    pub attributes: Vec<StatRow>,
    pub vitals: Vec<StatRow>,
    pub skills: Vec<SkillRow>,
    /// Augmentations taken: (name, times, maximum).
    pub augmentations: Vec<(String, u32, u32)>,
}

/// `1234567` -> `1,234,567`.
pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Specialized first, then trained, untrained, inactive; by name within.
pub fn sort(skills: &mut [SkillRow]) {
    skills.sort_by(|a, b| b.advancement.cmp(&a.advancement).then(a.name.cmp(b.name)));
}

/// The sheet for this session; `None` until it arrived. Skill values need
/// the portal's SkillTable for the attribute formula.
pub fn view(c: &Client) -> Option<SkillsView> {
    if !has_sheet(c) {
        return None;
    }
    let st = &c.world.stats;
    let table = c.assets.skill_table().ok();
    let mut skills: Vec<SkillRow> = st
        .skills
        .iter()
        .map(|sk| SkillRow {
            id: sk.id,
            name: ac_world::stats::skill_name(sk.id),
            value: st.skill_value(sk, table.as_ref().and_then(|t| t.get(sk.id))),
            ranks: sk.ranks,
            advancement: sk.advancement,
            training: ac_world::stats::sac_name(sk.advancement),
            raise_xp: c.skill_raise_cost(sk.id).xp(),
            train_credits: c.skill_train_cost(sk.id),
        })
        .collect();
    // Skills the sheet has no record for yet are unusable until trained.
    if let Some(t) = table.as_ref() {
        for (id, base) in &t.skills {
            if skills.iter().any(|s| s.id == *id) {
                continue;
            }
            skills.push(SkillRow {
                id: *id,
                name: ac_world::stats::skill_name(*id),
                value: 0,
                ranks: 0,
                advancement: 0,
                training: ac_world::stats::sac_name(0),
                raise_xp: None,
                train_credits: Some(base.trained_cost.max(0) as u32),
            });
        }
    }
    sort(&mut skills);
    let attributes = ac_client::advance::ATTRIBUTE_NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| StatRow {
            name,
            value: st.attributes[i].value(),
            ranks: st.attributes[i].ranks,
            raise_xp: c.attribute_raise_cost(i).xp(),
        })
        .collect();
    let vitals = ac_client::advance::VITAL_NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| StatRow {
            name,
            value: st.vital_max(i),
            ranks: st.vitals[i].ranks,
            raise_xp: c.vital_raise_cost(i).xp(),
        })
        .collect();
    Some(SkillsView {
        name: st.name.clone(),
        level: st.level,
        total_xp: st.total_xp,
        available_xp: st.available_xp,
        skill_credits: st.skill_credits,
        attributes,
        vitals,
        skills,
        augmentations: c
            .augmentations()
            .into_iter()
            .map(|a| (a.name.to_string(), a.count, a.max))
            .collect(),
    })
}

pub fn draw(egui: &egui::Context, v: &SkillsView) -> Actions {
    let mut actions = Actions::default();
    window(
        "skills",
        egui::pos2(8.0, 132.0),
        egui::vec2(500.0, 410.0),
        170,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(488.0, 398.0));
        ui.label(
            egui::RichText::new(format!("{}  level {}", v.name, v.level))
                .color(egui::Color32::WHITE)
                .strong(),
        );
        ui.label(
            egui::RichText::new(format!(
                "XP {}   unassigned {}   skill credits {}",
                thousands(v.total_xp),
                thousands(v.available_xp),
                v.skill_credits
            ))
            .color(egui::Color32::from_gray(200))
            .small(),
        );
        ui.add_space(4.0);
        let can_pay = |xp: Option<u32>| xp.is_some_and(|x| x as i64 <= v.available_xp);
        egui::ScrollArea::vertical()
            .max_height(350.0)
            .show(ui, |ui| {
                ui.set_min_width(480.0);
                egui::Grid::new("stats_grid")
                    .num_columns(4)
                    .spacing([14.0, 2.0])
                    .show(ui, |ui| {
                        for h in ["Attribute", "Value", "Ranks", "Raise"] {
                            caption(ui, h);
                        }
                        ui.end_row();
                        for (i, a) in v.attributes.iter().chain(v.vitals.iter()).enumerate() {
                            ui.label(egui::RichText::new(a.name).color(egui::Color32::WHITE));
                            ui.label(
                                egui::RichText::new(a.value.to_string())
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                            );
                            ui.label(
                                egui::RichText::new(a.ranks.to_string())
                                    .color(egui::Color32::from_gray(200)),
                            );
                            match a.raise_xp {
                                Some(xp) => {
                                    let b = ui.add_enabled(
                                        can_pay(Some(xp)),
                                        egui::Button::new(format!("+1  {}", thousands(xp as i64)))
                                            .small(),
                                    );
                                    if b.on_hover_text("XP for the next point").clicked() {
                                        if i < v.attributes.len() {
                                            actions.raise_attribute.push(i);
                                        } else {
                                            actions.raise_vital.push(i - v.attributes.len());
                                        }
                                    }
                                }
                                None => {
                                    ui.label(
                                        egui::RichText::new("max")
                                            .color(egui::Color32::from_gray(140)),
                                    );
                                }
                            }
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
                egui::Grid::new("skills_grid")
                    .num_columns(5)
                    .spacing([14.0, 2.0])
                    .show(ui, |ui| {
                        for h in ["Skill", "Value", "Ranks", "Training", "Raise / Train"] {
                            caption(ui, h);
                        }
                        ui.end_row();
                        for s in &v.skills {
                            let color = match s.advancement {
                                3 => egui::Color32::from_rgb(255, 215, 120),
                                2 => egui::Color32::WHITE,
                                1 => egui::Color32::from_gray(160),
                                _ => egui::Color32::from_gray(110),
                            };
                            ui.label(egui::RichText::new(s.name).color(color));
                            ui.label(
                                egui::RichText::new(s.value.to_string())
                                    .color(color)
                                    .strong(),
                            );
                            ui.label(egui::RichText::new(s.ranks.to_string()).color(color));
                            ui.label(egui::RichText::new(s.training).color(color));
                            if let Some(xp) = s.raise_xp {
                                let b = ui.add_enabled(
                                    can_pay(Some(xp)),
                                    egui::Button::new(format!("+1  {}", thousands(xp as i64)))
                                        .small(),
                                );
                                if b.on_hover_text("XP for the next rank").clicked() {
                                    actions.raise_skill.push(s.id);
                                }
                            } else if let Some(credits) = s.train_credits {
                                let b = ui.add_enabled(
                                    v.skill_credits >= credits as i32,
                                    egui::Button::new(format!("Train  {credits}")).small(),
                                );
                                if b.on_hover_text("skill credits to train").clicked() {
                                    actions.train_skill.push(s.id);
                                }
                            } else {
                                ui.label(
                                    egui::RichText::new("max").color(egui::Color32::from_gray(140)),
                                );
                            }
                            ui.end_row();
                        }
                    });
                if !v.augmentations.is_empty() {
                    caption(ui, format!("Augmentations ({})", v.augmentations.len()));
                    for (name, count, max) in &v.augmentations {
                        ui.label(
                            egui::RichText::new(format!("{name}  {count}/{max}"))
                                .color(egui::Color32::from_rgb(200, 220, 255)),
                        );
                    }
                }
            });
    });
    actions
}

#[derive(Default)]
pub struct Skills {
    source: Source<SkillsView>,
    /// Open (K toggles it). Starts closed.
    pub show: bool,
}

impl Skills {
    /// A small sheet; closed until K, like the live one.
    pub fn demo() -> Self {
        let row = |name, value, ranks, advancement| SkillRow {
            id: 0,
            name,
            value,
            ranks,
            advancement,
            training: ac_world::stats::sac_name(advancement),
            raise_xp: (advancement >= 2).then_some(1_200),
            train_credits: (advancement < 2).then_some(4),
        };
        let mut skills = vec![
            row("Dagger", 120, 30, 3),
            row("Melee Defense", 80, 12, 2),
            row("Run", 60, 0, 1),
            row("Alchemy", 20, 0, 0),
            row("Life Magic", 95, 20, 2),
        ];
        sort(&mut skills);
        Skills {
            source: Source::Demo(SkillsView {
                name: "Demo".into(),
                level: 12,
                total_xp: 1_234_567,
                available_xp: 12_345,
                skill_credits: 2,
                attributes: ac_client::advance::ATTRIBUTE_NAMES
                    .iter()
                    .map(|n| StatRow {
                        name: n,
                        value: 60,
                        ranks: 10,
                        raise_xp: Some(2_300),
                    })
                    .collect(),
                vitals: ac_client::advance::VITAL_NAMES
                    .iter()
                    .map(|n| StatRow {
                        name: n,
                        value: 90,
                        ranks: 5,
                        raise_xp: Some(900),
                    })
                    .collect(),
                skills,
                augmentations: vec![("Might of the Seventh Mule (carrying capacity)".into(), 2, 5)],
            }),
            show: false,
        }
    }
}

impl Plugin for Skills {
    fn name(&self) -> &str {
        "skills"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        cx.board.set(OPEN_KEY, self.show);
        if !self.show {
            return;
        }
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().and_then(|c| view(c)),
        };
        if let Some(v) = v {
            let a = draw(egui, &v);
            if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
                for i in a.raise_attribute {
                    c.raise_attribute(i);
                }
                for i in a.raise_vital {
                    c.raise_vital(i);
                }
                for id in a.raise_skill {
                    c.raise_skill(id);
                }
                for id in a.train_skill {
                    c.train_skill(id);
                }
            }
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if key == egui::Key::K && pressed {
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
    fn thousands_groups_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(-1234), "-1,234");
    }

    #[test]
    fn specialized_first() {
        let Skills {
            source: Source::Demo(v),
            ..
        } = Skills::demo()
        else {
            panic!("demo() is not a demo source");
        };
        let order: Vec<(u32, &str)> = v.skills.iter().map(|s| (s.advancement, s.name)).collect();
        assert_eq!(
            order,
            [
                (3, "Dagger"),
                (2, "Life Magic"),
                (2, "Melee Defense"),
                (1, "Run"),
                (0, "Alchemy")
            ]
        );
    }
}
