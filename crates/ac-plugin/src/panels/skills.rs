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
    pub name: &'static str,
    /// Current skill value (attribute base + creation bonus + ranks).
    pub value: u32,
    pub ranks: u16,
    /// Advancement class: 0 inactive, 1 untrained, 2 trained, 3 specialized.
    pub advancement: u32,
    pub training: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillsView {
    pub name: String,
    pub level: i32,
    pub total_xp: i64,
    pub available_xp: i64,
    pub skill_credits: i32,
    pub skills: Vec<SkillRow>,
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
            name: ac_world::stats::skill_name(sk.id),
            value: st.skill_value(sk, table.as_ref().and_then(|t| t.get(sk.id))),
            ranks: sk.ranks,
            advancement: sk.advancement,
            training: ac_world::stats::sac_name(sk.advancement),
        })
        .collect();
    sort(&mut skills);
    Some(SkillsView {
        name: st.name.clone(),
        level: st.level,
        total_xp: st.total_xp,
        available_xp: st.available_xp,
        skill_credits: st.skill_credits,
        skills,
    })
}

pub fn draw(egui: &egui::Context, v: &SkillsView) {
    window(
        "skills",
        egui::pos2(8.0, 132.0),
        egui::vec2(360.0, 380.0),
        170,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(348.0, 368.0));
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
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                ui.set_min_width(340.0);
                egui::Grid::new("skills_grid")
                    .num_columns(4)
                    .spacing([14.0, 2.0])
                    .show(ui, |ui| {
                        for h in ["Skill", "Value", "Ranks", "Training"] {
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
                            ui.end_row();
                        }
                    });
            });
    });
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
            name,
            value,
            ranks,
            advancement,
            training: ac_world::stats::sac_name(advancement),
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
                skills,
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
            draw(egui, &v);
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
