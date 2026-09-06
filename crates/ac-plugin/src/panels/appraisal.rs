//! The appraisal window: what the server told us about the last object
//! we assessed (clicking selects and appraises). Items show value,
//! burden, workmanship and material, armor level with per-damage
//! protections, weapon damage, speed, skill and bonuses, spells with
//! spellcraft and mana, wield requirements, tinkering and the flags
//! (bonded, attuned, retained); creatures show level, vitals,
//! attributes and armor by location. Close hides it until the next
//! appraisal.

use super::{caption, title, window, Source};
use crate::{egui, Client, Ctx, Plugin};
use ac_net::messages::Appraisal;

#[derive(Clone, Debug, PartialEq)]
pub struct AppraisalView {
    pub name: String,
    /// (label, value) lines in display order.
    pub lines: Vec<(String, String)>,
    /// Free text: descriptions, use, inscription.
    pub texts: Vec<String>,
    /// Spell names on the item.
    pub spells: Vec<String>,
    pub failed: bool,
}

/// The damage type bits as words.
pub fn damage_type_name(bits: u32) -> String {
    let names = [
        (0x1, "Slashing"),
        (0x2, "Piercing"),
        (0x4, "Bludgeoning"),
        (0x8, "Cold"),
        (0x10, "Fire"),
        (0x20, "Acid"),
        (0x40, "Electric"),
        (0x400, "Nether"),
    ];
    let parts: Vec<&str> = names
        .iter()
        .filter(|(b, _)| bits & b != 0)
        .map(|(_, n)| *n)
        .collect();
    if parts.is_empty() {
        "?".into()
    } else {
        parts.join(", ")
    }
}

/// `+5%` / `-3%` from a multiplier such as 1.05.
pub fn percent_bonus(mult: f64) -> String {
    let pct = ((mult - 1.0) * 100.0).round() as i64;
    format!("{pct:+}%")
}

/// Build the lines from an appraisal; `skill_name`/`spell_name` resolve
/// ids through the DAT tables.
pub fn build(
    name: &str,
    a: &Appraisal,
    material_name: &dyn Fn(u32) -> &'static str,
    skill_name: &dyn Fn(u32) -> String,
    spell_name: &dyn Fn(u32) -> String,
) -> AppraisalView {
    let mut lines: Vec<(String, String)> = Vec::new();
    let mut texts = Vec::new();
    for id in [15u32, 16, 14] {
        if let Some(t) = a.string(id) {
            if !t.is_empty() {
                texts.push(t.to_string());
            }
        }
    }
    if let Some(t) = a.string(7) {
        if !t.is_empty() {
            let by = a.string(8).unwrap_or("");
            texts.push(if by.is_empty() {
                format!("\"{t}\"")
            } else {
                format!("\"{t}\" -- {by}")
            });
        }
    }
    if let Some(c) = &a.creature {
        if let Some(l) = a.int(25) {
            lines.push(("Level".into(), l.to_string()));
        }
        lines.push(("Health".into(), format!("{} / {}", c.health, c.health_max)));
        if c.stamina_max > 0 || c.mana_max > 0 {
            lines.push((
                "Stamina".into(),
                format!("{} / {}", c.stamina, c.stamina_max),
            ));
            lines.push(("Mana".into(), format!("{} / {}", c.mana, c.mana_max)));
        }
        if let Some(at) = c.attributes {
            lines.push((
                "Attributes".into(),
                format!(
                    "Str {} End {} Qui {} Coo {} Foc {} Self {}",
                    at[0], at[1], at[2], at[3], at[4], at[5]
                ),
            ));
        }
        if let Some(l) = a.armor_levels {
            lines.push((
                "Armor".into(),
                format!(
                    "head {} chest {} abdomen {} arms {}/{} hands {} legs {}/{} feet {}",
                    l[0], l[1], l[2], l[3], l[4], l[5], l[6], l[7], l[8]
                ),
            ));
        }
    } else {
        if let Some(v) = a.int(19) {
            lines.push(("Value".into(), format!("{v} pyreals")));
        }
        if let Some(b) = a.int(5) {
            lines.push(("Burden".into(), format!("{b} bu")));
        }
        match (a.int(105), a.int(131)) {
            (Some(w), Some(m)) => lines.push((
                "Workmanship".into(),
                format!("{w} ({})", material_name(m as u32)),
            )),
            (Some(w), None) => lines.push(("Workmanship".into(), w.to_string())),
            (None, Some(m)) => lines.push(("Material".into(), material_name(m as u32).into())),
            _ => {}
        }
        if let Some(al) = a.int(28) {
            lines.push(("Armor level".into(), al.to_string()));
        }
        if let Some(p) = &a.armor {
            let al = a.int(28).unwrap_or(0) as f32;
            let show = |m: f32| format!("{:.0}", al * m);
            lines.push((
                "Protection".into(),
                format!(
                    "slash {} pierce {} bludgeon {} cold {} fire {} acid {} electric {} nether {}",
                    show(p.slash),
                    show(p.pierce),
                    show(p.bludgeon),
                    show(p.cold),
                    show(p.fire),
                    show(p.acid),
                    show(p.electric),
                    show(p.nether)
                ),
            ));
        }
        if let Some(w) = &a.weapon {
            let low = (w.damage as f64 * (1.0 - w.variance)).round() as u32;
            lines.push((
                "Damage".into(),
                format!("{low}-{} {}", w.damage, damage_type_name(w.damage_type)),
            ));
            lines.push(("Speed".into(), w.speed.to_string()));
            lines.push(("Skill".into(), skill_name(w.skill)));
            if (w.offense - 1.0).abs() > 1e-6 {
                lines.push(("Attack bonus".into(), percent_bonus(w.offense)));
            }
            if let Some(d) = a.float(29) {
                if (d - 1.0).abs() > 1e-6 {
                    lines.push(("Defense bonus".into(), percent_bonus(d)));
                }
            }
            if let Some(e) = a.int(204) {
                if e != 0 {
                    lines.push(("Elemental bonus".into(), e.to_string()));
                }
            }
            if w.max_velocity > 0.0 {
                lines.push(("Range".into(), format!("{:.0} m", w.max_velocity_estimated)));
            }
        }
        if let Some(s) = a.int(56) {
            lines.push(("Shield".into(), s.to_string()));
        }
        if let (Some(skill), Some(level)) = (a.int(159), a.int(160)) {
            lines.push((
                "Requires".into(),
                format!("{} {level}", skill_name(skill as u32)),
            ));
        }
        if let Some(c) = a.int(106) {
            lines.push(("Spellcraft".into(), c.to_string()));
        }
        if let (Some(cur), Some(max)) = (a.int(107), a.int(108)) {
            let rate = a
                .float(5)
                .map(|r| format!(" ({:.2}/s)", -r))
                .unwrap_or_default();
            lines.push(("Mana".into(), format!("{cur} / {max}{rate}")));
        }
        if let Some(d) = a.int(109) {
            lines.push(("Difficulty".into(), d.to_string()));
        }
        if let Some(t) = a.int(171) {
            if t > 0 {
                let by = a.string(39).unwrap_or("");
                lines.push((
                    "Tinkered".into(),
                    if by.is_empty() {
                        format!("{t} times")
                    } else {
                        format!("{t} times by {by}")
                    },
                ));
            }
        }
        if let (Some(s), Some(m)) = (a.int(92), a.int(91)) {
            lines.push(("Uses".into(), format!("{s} / {m}")));
        }
        let mut flags = Vec::new();
        if a.int(33).unwrap_or(0) != 0 {
            flags.push("bonded");
        }
        if a.int(114).unwrap_or(0) != 0 {
            flags.push("attuned");
        }
        if a.bool(91).unwrap_or(false) {
            flags.push("retained");
        }
        if a.bool(22).unwrap_or(false) {
            flags.push("inscribable");
        }
        if !flags.is_empty() {
            lines.push(("Flags".into(), flags.join(", ")));
        }
    }
    AppraisalView {
        name: name.to_string(),
        lines,
        texts,
        spells: a.spells.iter().map(|s| spell_name(*s)).collect(),
        failed: !a.success,
    }
}

pub fn view(c: &Client) -> Option<AppraisalView> {
    let guid = c.last_appraisal?;
    let a = c.appraisals.get(&guid)?;
    let name = c
        .world
        .objects
        .get(&guid)
        .map(|o| o.name.clone())
        .unwrap_or_else(|| format!("{guid:#010x}"));
    let skills = c.assets.skill_table().ok();
    let spells = c.assets.spell_table().ok();
    Some(build(
        &name,
        a,
        &ac_world::material::name,
        &|id| {
            skills
                .as_ref()
                .and_then(|t| t.get(id).map(|s| s.name.clone()))
                .unwrap_or_else(|| format!("skill {id}"))
        },
        &|id| {
            spells
                .as_ref()
                .and_then(|t| t.get(id).map(|s| s.name.clone()))
                .unwrap_or_else(|| format!("spell {id}"))
        },
    ))
}

/// Returns true when Close was clicked.
pub fn draw(egui: &egui::Context, v: &AppraisalView) -> bool {
    let mut close = false;
    let w = egui.viewport_rect().width();
    window(
        "appraisal",
        egui::pos2(w - 720.0, 2.0 * super::radar::RADIUS + 300.0),
        egui::vec2(440.0, 280.0),
        190,
        8,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(424.0, 264.0));
        ui.horizontal(|ui| {
            title(ui, &v.name);
            if v.failed {
                caption(ui, "(appraisal failed)");
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Close").clicked() {
                    close = true;
                }
            });
        });
        egui::ScrollArea::vertical()
            .max_height(230.0)
            .show(ui, |ui| {
                ui.set_min_width(410.0);
                for t in &v.texts {
                    ui.label(
                        egui::RichText::new(t)
                            .color(egui::Color32::from_rgb(220, 210, 180))
                            .italics(),
                    );
                }
                egui::Grid::new("appraisal_lines")
                    .num_columns(2)
                    .spacing([12.0, 2.0])
                    .show(ui, |ui| {
                        for (k, val) in &v.lines {
                            caption(ui, k);
                            ui.label(egui::RichText::new(val).color(egui::Color32::WHITE));
                            ui.end_row();
                        }
                    });
                if !v.spells.is_empty() {
                    caption(ui, "Spells");
                    for s in &v.spells {
                        ui.label(
                            egui::RichText::new(s).color(egui::Color32::from_rgb(160, 190, 255)),
                        );
                    }
                }
            });
    });
    close
}

#[derive(Default)]
pub struct AppraisalPanel {
    source: Source<AppraisalView>,
    /// The last `appraisal_seq` shown; a new one reopens the window.
    seen: u64,
    closed: bool,
}

impl AppraisalPanel {
    pub fn demo() -> Self {
        AppraisalPanel {
            source: Source::Demo(AppraisalView {
                name: "Demo Dagger".into(),
                lines: vec![
                    ("Value".into(), "150 pyreals".into()),
                    ("Burden".into(), "90 bu".into()),
                    ("Workmanship".into(), "5 (Iron)".into()),
                    ("Damage".into(), "6-12 Piercing".into()),
                    ("Speed".into(), "20".into()),
                    ("Skill".into(), "Dagger".into()),
                    ("Attack bonus".into(), "+5%".into()),
                    ("Requires".into(), "Dagger 200".into()),
                    ("Spellcraft".into(), "150".into()),
                    ("Mana".into(), "300 / 300 (0.02/s)".into()),
                ],
                texts: vec!["A well-made dagger.".into()],
                spells: vec!["Blood Drinker III".into(), "Swift Killer III".into()],
                failed: false,
            }),
            seen: 0,
            closed: false,
        }
    }
}

impl Plugin for AppraisalPanel {
    fn name(&self) -> &str {
        "appraisal"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => {
                let Some(c) = cx.try_client() else { return };
                if c.appraisal_seq != self.seen {
                    self.seen = c.appraisal_seq;
                    self.closed = false;
                }
                if self.closed {
                    return;
                }
                view(c)
            }
        };
        let Some(v) = v else { return };
        if draw(egui, &v) {
            self.closed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weapon_lines() {
        let a = Appraisal {
            guid: 1,
            success: true,
            ints: vec![
                (19, 150),
                (5, 90),
                (105, 5),
                (131, 0x3D),
                (159, 1),
                (160, 200),
            ],
            floats: vec![(29, 1.03)],
            strings: vec![(15, "A dagger.".into())],
            weapon: Some(ac_net::messages::WeaponProfile {
                damage_type: 2,
                speed: 20,
                skill: 1,
                damage: 12,
                variance: 0.5,
                damage_mod: 1.0,
                length: 0.3,
                max_velocity: 0.0,
                offense: 1.05,
                max_velocity_estimated: 0,
            }),
            spells: vec![2091],
            ..Default::default()
        };
        let v = build(
            "Dagger",
            &a,
            &ac_world::material::name,
            &|id| format!("skill {id}"),
            &|id| format!("spell {id}"),
        );
        let get = |k: &str| v.lines.iter().find(|(a, _)| a == k).map(|(_, b)| b.clone());
        assert_eq!(get("Workmanship").as_deref(), Some("5 (Iron)"));
        assert_eq!(get("Damage").as_deref(), Some("6-12 Piercing"));
        assert_eq!(get("Attack bonus").as_deref(), Some("+5%"));
        assert_eq!(get("Defense bonus").as_deref(), Some("+3%"));
        assert_eq!(get("Requires").as_deref(), Some("skill 1 200"));
        assert_eq!(v.spells, vec!["spell 2091"]);
        assert_eq!(v.texts, vec!["A dagger."]);
        assert_eq!(damage_type_name(0x11), "Slashing, Fire");
    }
}
