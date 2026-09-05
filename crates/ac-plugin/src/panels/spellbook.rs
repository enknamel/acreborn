//! Spellbook: the spells the character knows, grouped by level, with the
//! school and mana cost; hover for the description and words, double-click
//! to cast (self spells on ourselves, the rest at the selection). P
//! toggles it; it sits beside the skills panel when both are open.

use ac_formats::spell_components::SpellComponentTable;
use ac_formats::spell_table::SpellTable;

use super::{caption, has_sheet, title, window, Source};
use crate::icons::{IconCache, IconLayers};
use crate::{egui, Client, Ctx, Plugin};

/// One line of the spellbook panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpellRow {
    pub id: u32,
    pub name: String,
    /// Spell level 1..=8 (from the spell's power).
    pub level: u32,
    pub school: &'static str,
    pub mana: u32,
    /// Only castable on ourselves; other spells need a selected target.
    pub self_targeted: bool,
    /// RenderSurface (0x06) id of the spell icon.
    pub icon: u32,
    /// Shown on hover: the spell's description and its incantation.
    pub description: String,
    pub words: String,
}

/// By level, then name.
pub fn sort(rows: &mut [SpellRow]) {
    rows.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.name.cmp(&b.name)));
}

/// Spellbook rows for the given spell ids, sorted by level then name.
/// Ids missing from the table are skipped.
pub fn rows(
    table: &SpellTable,
    comps: &SpellComponentTable,
    ids: impl IntoIterator<Item = u32>,
) -> Vec<SpellRow> {
    let mut rows: Vec<SpellRow> = ids
        .into_iter()
        .filter_map(|id| {
            let s = table.get(id)?;
            Some(SpellRow {
                id,
                name: s.name.clone(),
                level: s.level(),
                school: ac_formats::spell_table::school::short_name(s.school),
                mana: s.base_mana,
                self_targeted: s.is_self_targeted(),
                icon: s.icon_id,
                description: s.description.clone(),
                words: comps.spell_words(s.formula()),
            })
        })
        .collect();
    sort(&mut rows);
    rows
}

/// The rows for this session's spellbook (`world.stats.spells`), through
/// the portal's spell tables. Empty when the tables do not load.
pub fn view(c: &Client) -> Vec<SpellRow> {
    match (c.assets.spell_table(), c.assets.spell_components()) {
        (Ok(table), Ok(comps)) => rows(&table, &comps, c.world.stats.spells.iter().copied()),
        _ => Vec::new(),
    }
}

/// Draw the panel at `x`; returns the spell ids double-clicked.
pub fn draw(egui: &egui::Context, icons: &mut IconCache, spells: &[SpellRow], x: f32) -> Vec<u32> {
    let mut casts = Vec::new();
    window(
        "spellbook",
        egui::pos2(x, 132.0),
        egui::vec2(380.0, 380.0),
        170,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(368.0, 368.0));
        title(ui, format!("Spellbook ({})", spells.len()));
        caption(ui, "double-click to cast");
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .max_height(330.0)
            .show(ui, |ui| {
                ui.set_min_width(360.0);
                if spells.is_empty() {
                    ui.label(
                        egui::RichText::new("(no spells known)")
                            .color(egui::Color32::from_gray(170)),
                    );
                }
                let mut last_level = 0;
                for sp in spells {
                    if sp.level != last_level {
                        caption(ui, format!("Level {}", sp.level));
                        last_level = sp.level;
                    }
                    let color = if sp.self_targeted {
                        egui::Color32::from_rgb(180, 230, 180)
                    } else {
                        egui::Color32::WHITE
                    };
                    let resp = ui
                        .horizontal(|ui| {
                            let icon =
                                icons.draw(ui, IconLayers::single(sp.icon), egui::Sense::click());
                            let text = ui.add(
                                egui::Label::new(egui::RichText::new(&sp.name).color(color))
                                    .sense(egui::Sense::click()),
                            );
                            let detail = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!("{}  {} mana", sp.school, sp.mana))
                                        .color(egui::Color32::from_gray(170))
                                        .small(),
                                )
                                .sense(egui::Sense::click()),
                            );
                            icon.union(text).union(detail)
                        })
                        .inner;
                    let resp = resp.on_hover_text(format!(
                        "{}\n{}\n{}",
                        sp.description,
                        sp.words,
                        if sp.self_targeted {
                            "self"
                        } else {
                            "needs a target"
                        }
                    ));
                    if resp.double_clicked() {
                        casts.push(sp.id);
                    }
                    if resp.hovered() {
                        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                    }
                }
            });
    });
    casts
}

#[derive(Default)]
pub struct Spellbook {
    source: Source<Vec<SpellRow>>,
    /// Open (P toggles it). Starts closed.
    pub show: bool,
    /// Live rows, rebuilt when the number of known spells changes.
    cache: Vec<SpellRow>,
    cached_count: Option<usize>,
}

impl Spellbook {
    /// A handful of real spells (Strength/Heal/Infuse/Invulnerability
    /// Other I, the Self variants, protections, Acid Stream III, Shock
    /// Wave II, Mind Blossom) when the tables are given, open.
    pub fn demo(tables: Option<(&SpellTable, &SpellComponentTable)>) -> Self {
        let rows = match tables {
            Some((table, comps)) => rows(table, comps, [1, 2, 5, 6, 9, 17, 20, 24, 60, 65, 2091]),
            None => Vec::new(),
        };
        Spellbook {
            source: Source::Demo(rows),
            show: true,
            cache: Vec::new(),
            cached_count: None,
        }
    }
}

impl Plugin for Spellbook {
    fn name(&self) -> &str {
        "spellbook"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if !self.show {
            return;
        }
        // Sits beside the skills panel when both are open.
        let beside_skills = cx
            .board
            .get(super::skills::OPEN_KEY)
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let x = if beside_skills { 380.0 } else { 8.0 };
        let casts = match &self.source {
            Source::Demo(rows) => draw(egui, cx.icons(), rows, x),
            Source::Live => {
                let Some(c) = cx.try_client() else { return };
                if !has_sheet(c) {
                    return;
                }
                let count = c.world.stats.spells.len();
                if self.cached_count != Some(count) {
                    self.cache = view(c);
                    self.cached_count = Some(count);
                }
                let casts = draw(egui, cx.icons(), &self.cache, x);
                let c = cx.client();
                for id in &casts {
                    c.cast(*id);
                }
                casts
            }
        };
        let _ = casts;
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if key == egui::Key::P && pressed {
            self.show = !self.show;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: u32, name: &str, level: u32) -> SpellRow {
        SpellRow {
            id,
            name: name.into(),
            level,
            school: "Life",
            mana: 10,
            self_targeted: true,
            icon: 0,
            description: String::new(),
            words: String::new(),
        }
    }

    #[test]
    fn sorted_by_level_then_name() {
        let mut rows = vec![
            row(3, "Heal Self II", 2),
            row(1, "Strength Self I", 1),
            row(2, "Heal Self I", 1),
        ];
        sort(&mut rows);
        let ids: Vec<u32> = rows.iter().map(|r| r.id).collect();
        assert_eq!(ids, [2, 1, 3]);
    }

    #[test]
    fn demo_spellbook_from_the_portal() {
        let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
            return;
        };
        let assets = ac_scene::Assets::open(dir).unwrap();
        let table = assets.spell_table().unwrap();
        let comps = assets.spell_components().unwrap();
        let Spellbook {
            source: Source::Demo(rows),
            ..
        } = Spellbook::demo(Some((&table, &comps)))
        else {
            panic!("demo() is not a demo source");
        };
        assert!(!rows.is_empty());
        assert!(rows.windows(2).all(|w| w[0].level <= w[1].level));
        assert!(rows.iter().any(|r| r.name.starts_with("Heal Self")));
    }
}
