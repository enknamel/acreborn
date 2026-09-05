//! The combat bar: while in melee or missile mode, the attack height
//! (high, medium, low) and the power (melee: 50% to 150% damage, and the
//! swing animation) or accuracy (missile: speed against skill) setting
//! the next attack is sent with. Insert/PageUp step the power bar and
//! Delete/PageDown the height, the keys the spell bar uses for its tabs
//! and spells in magic mode (see docs/game/mechanics.md, section 2).

use super::{frame, Source};
use crate::{egui, Client, Ctx, Plugin};

#[derive(Clone, Debug, PartialEq)]
pub struct CombatView {
    /// Missile (accuracy) rather than melee (power).
    pub missile: bool,
    /// 1 high, 2 medium, 3 low.
    pub height: u32,
    pub power: f32,
    pub auto_repeat: bool,
    pub target: Option<String>,
}

#[derive(Debug, Default, PartialEq)]
pub struct Actions {
    pub height: Option<u32>,
    pub power: Option<f32>,
    pub auto_repeat: Option<bool>,
    pub peace: bool,
}

pub const HEIGHTS: [(u32, &str); 3] = [(1, "High"), (2, "Medium"), (3, "Low")];

pub fn view(c: &Client) -> Option<CombatView> {
    if !c.combat || c.magic {
        return None;
    }
    let auto = ac_client::options::OPTIONS
        .iter()
        .find(|o| o.id == 0x00)
        .map(|o| c.option_enabled(o))
        .unwrap_or(false);
    Some(CombatView {
        missile: c.missile,
        height: c.attack_height,
        power: c.attack_power,
        auto_repeat: auto,
        target: c
            .attack_target
            .and_then(|g| c.world.objects.get(&g))
            .map(|o| o.name.clone()),
    })
}

/// Step the height the way the keys do: Delete goes up the body,
/// PageDown down, wrapping.
pub fn step_height(height: u32, up: bool) -> u32 {
    let i = HEIGHTS.iter().position(|(h, _)| *h == height).unwrap_or(1);
    let n = HEIGHTS.len();
    let j = if up { (i + n - 1) % n } else { (i + 1) % n };
    HEIGHTS[j].0
}

/// Step the power bar by a tenth, clamped.
pub fn step_power(power: f32, up: bool) -> f32 {
    (power + if up { 0.1 } else { -0.1 }).clamp(0.0, 1.0)
}

pub fn draw(egui: &egui::Context, v: &CombatView) -> Actions {
    let mut actions = Actions::default();
    let rect = egui.viewport_rect();
    egui::Area::new(egui::Id::new("combat_bar"))
        .fade_in(false)
        .fixed_pos(egui::pos2(
            rect.width() * 0.5 - 200.0,
            rect.height() - 300.0,
        ))
        .show(egui, |ui| {
            frame(170, 6).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(if v.missile { "Missile" } else { "Melee" })
                            .color(egui::Color32::from_rgb(255, 200, 120))
                            .strong(),
                    );
                    for (h, name) in HEIGHTS {
                        if ui.selectable_label(v.height == h, name).clicked() {
                            actions.height = Some(h);
                        }
                    }
                    let mut p = v.power;
                    let label = if v.missile { "accuracy" } else { "power" };
                    if ui
                        .add(
                            egui::Slider::new(&mut p, 0.0..=1.0)
                                .text(label)
                                .show_value(false),
                        )
                        .changed()
                    {
                        actions.power = Some(p);
                    }
                    ui.label(
                        egui::RichText::new(format!("{:.0}%", v.power * 100.0))
                            .color(egui::Color32::from_gray(200)),
                    );
                    let mut auto = v.auto_repeat;
                    if ui.checkbox(&mut auto, "repeat").changed() {
                        actions.auto_repeat = Some(auto);
                    }
                    if ui.button("Peace").clicked() {
                        actions.peace = true;
                    }
                });
                if let Some(t) = &v.target {
                    ui.label(
                        egui::RichText::new(format!("attacking {t}"))
                            .color(egui::Color32::from_gray(190))
                            .small(),
                    );
                }
            });
        });
    actions
}

#[derive(Default)]
pub struct Combat {
    source: Source<CombatView>,
}

impl Combat {
    pub fn demo() -> Self {
        Combat {
            source: Source::Demo(CombatView {
                missile: false,
                height: 2,
                power: 0.5,
                auto_repeat: true,
                target: Some("Demo Drudge".into()),
            }),
        }
    }
}

impl Plugin for Combat {
    fn name(&self) -> &str {
        "combat"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().and_then(|c| view(c)),
        };
        let Some(v) = v else { return };
        let a = draw(egui, &v);
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            if let Some(h) = a.height {
                c.attack_height = h;
            }
            if let Some(p) = a.power {
                c.attack_power = p;
            }
            if let Some(on) = a.auto_repeat {
                if let Some(o) = ac_client::options::OPTIONS.iter().find(|o| o.id == 0x00) {
                    c.set_option(o, on);
                }
            }
            if a.peace {
                c.toggle_combat();
            }
        }
    }

    fn key(&mut self, cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if !pressed {
            return false;
        }
        let Some(c) = cx.try_client() else {
            return false;
        };
        if !c.combat || c.magic {
            return false;
        }
        match key {
            egui::Key::PageUp | egui::Key::Insert => {
                c.attack_power = step_power(c.attack_power, key == egui::Key::PageUp);
                true
            }
            egui::Key::PageDown | egui::Key::Delete => {
                c.attack_height = step_height(c.attack_height, key == egui::Key::Delete);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_and_power_step_and_wrap() {
        assert_eq!(step_height(2, true), 1);
        assert_eq!(step_height(1, true), 3);
        assert_eq!(step_height(3, false), 1);
        assert!((step_power(0.5, true) - 0.6).abs() < 1e-6);
        assert_eq!(step_power(0.05, false), 0.0);
        assert_eq!(step_power(0.95, true), 1.0);
    }
}
