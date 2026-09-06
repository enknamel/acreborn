//! Spellbook: the spells the character knows (`Client::known_spell_ids`),
//! in the spell table's display order, filtered by school and level with
//! the server-side filter bits (`Client::spellbook_filters`, one toggle
//! button each). Double-click a spell to put it on the shown spell bar,
//! or drag it onto the bar or one of its tabs; right-click (or the `i`
//! button) for details: school, level, mana, duration, description, the
//! current formula with each component's presence in the packs, a Cast
//! button and Delete (with confirmation, RemoveSpellC2S). P toggles it; it
//! sits beside the skills panel when both are open, and publishes whether
//! it is open as [`OPEN_KEY`] so the spell bar shows with it.

use std::collections::HashMap;

use ac_formats::spell_components::SpellComponentTable;
use ac_formats::spell_table::{school, Spell, SpellTable};

use super::{caption, fmt_seconds, has_sheet, title, window, Source, SpellDrag};
use crate::icons::{IconCache, IconLayers};
use crate::{egui, Client, Ctx, Plugin, Settings};

/// Blackboard key: `true` while the spellbook is open.
pub const OPEN_KEY: &str = "panels.spellbook_open";

/// The spellbook filter bits as the server stores them (SpellbookFilter
/// 0x0286): a set bit shows that school or level.
pub mod filter {
    use ac_formats::spell_table::school;

    pub const CREATURE: u32 = 0x1;
    pub const ITEM: u32 = 0x2;
    pub const LIFE: u32 = 0x4;
    pub const WAR: u32 = 0x8;
    pub const VOID: u32 = 0x2000;
    /// Level1..Level9.
    pub const LEVELS: u32 = 0x1FF0;
    pub const SCHOOLS: u32 = CREATURE | ITEM | LIFE | WAR | VOID;
    pub const ALL: u32 = SCHOOLS | LEVELS;

    /// The schools with a toggle, in the client's order.
    pub const SCHOOL_TOGGLES: [(&str, u32); 5] = [
        ("Creature", CREATURE),
        ("Item", ITEM),
        ("Life", LIFE),
        ("War", WAR),
        ("Void", VOID),
    ];

    /// The bit for spell level 1..=9; 0 otherwise.
    pub fn level(level: u32) -> u32 {
        if (1..=9).contains(&level) {
            0x10 << (level - 1)
        } else {
            0
        }
    }

    /// The bit for a `spell_table::school` id; 0 for none.
    pub fn school_bit(school_id: u32) -> u32 {
        match school_id {
            school::CREATURE => CREATURE,
            school::ITEM => ITEM,
            school::LIFE => LIFE,
            school::WAR => WAR,
            school::VOID => VOID,
            _ => 0,
        }
    }

    /// Flip one bit. A zero bitfield (the server sent no filters) means
    /// everything is shown, so the first toggle starts from [`ALL`].
    pub fn toggled(bits: u32, bit: u32) -> u32 {
        let base = if bits == 0 { ALL } else { bits };
        base ^ bit
    }

    /// Whether a spell of this school and level is shown. Spells without a
    /// school or level bit (quest spells) are always shown.
    pub fn passes(bits: u32, school_id: u32, level: u32) -> bool {
        if bits == 0 {
            return true;
        }
        let s = school_bit(school_id);
        let l = self::level(level);
        (s == 0 || bits & s != 0) && (l == 0 || bits & l != 0)
    }
}

/// `1` -> `I` ... `8` -> `VIII`.
pub fn roman(level: u32) -> &'static str {
    match level {
        1 => "I",
        2 => "II",
        3 => "III",
        4 => "IV",
        5 => "V",
        6 => "VI",
        7 => "VII",
        8 => "VIII",
        9 => "IX",
        _ => "?",
    }
}

/// One line of the spellbook panel.
#[derive(Clone, Debug, PartialEq)]
pub struct SpellRow {
    pub id: u32,
    pub name: String,
    /// Spell level 1..=8 (from the spell's power; the scarab's level when
    /// the power gives none).
    pub level: u32,
    pub school_id: u32,
    pub school: &'static str,
    pub mana: u32,
    /// Only castable on ourselves; other spells need a selected target.
    pub self_targeted: bool,
    /// RenderSurface (0x06) id of the spell icon.
    pub icon: u32,
    pub description: String,
    /// The incantation.
    pub words: String,
    /// The spell table's display order.
    pub display_order: u32,
    /// Enchantment duration in seconds, if the spell has one.
    pub duration: Option<f64>,
    /// The components one cast needs right now (prismatic when the
    /// school's focus is carried), by name, and whether each is carried.
    pub formula: Vec<(String, bool)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpellbookView {
    pub filters: u32,
    pub rows: Vec<SpellRow>,
}

/// By display order, then level, then name.
pub fn sort(rows: &mut [SpellRow]) {
    rows.sort_by(|a, b| {
        a.display_order
            .cmp(&b.display_order)
            .then_with(|| a.level.cmp(&b.level))
            .then_with(|| a.name.cmp(&b.name))
    });
}

fn spell_level(s: &Spell) -> u32 {
    match s.level() {
        0 => s.scarab_level(),
        l => l,
    }
}

/// Spellbook rows for the given spell ids, sorted. `formula_of` gives the
/// components a cast needs right now and `carried` whether a component is
/// in the packs. Ids missing from the table are skipped.
pub fn rows(
    table: &SpellTable,
    comps: &SpellComponentTable,
    ids: impl IntoIterator<Item = u32>,
    formula_of: &dyn Fn(u32, &Spell) -> Vec<u32>,
    carried: &dyn Fn(u32) -> bool,
) -> Vec<SpellRow> {
    let mut rows: Vec<SpellRow> = ids
        .into_iter()
        .filter_map(|id| {
            let s = table.get(id)?;
            let formula = formula_of(id, s)
                .into_iter()
                .map(|c| {
                    (
                        comps
                            .get(c)
                            .map(|e| e.name.clone())
                            .unwrap_or_else(|| format!("component {c}")),
                        carried(c),
                    )
                })
                .collect();
            Some(SpellRow {
                id,
                name: s.name.clone(),
                level: spell_level(s),
                school_id: s.school,
                school: school::short_name(s.school),
                mana: s.base_mana,
                self_targeted: s.is_self_targeted(),
                icon: s.icon_id,
                description: s.description.clone(),
                words: comps.spell_words(s.formula()),
                display_order: s.display_order,
                duration: s.duration(),
                formula,
            })
        })
        .collect();
    sort(&mut rows);
    rows
}

/// This session's spellbook with its filters. Rows are empty when the
/// tables do not load.
pub fn view(c: &Client) -> SpellbookView {
    let rows = match (c.assets.spell_table(), c.assets.spell_components()) {
        (Ok(table), Ok(comps)) => {
            let counts: HashMap<u32, u32> = c
                .components()
                .into_iter()
                .map(|k| (k.component_id, k.count))
                .collect();
            rows(
                &table,
                &comps,
                c.known_spell_ids(),
                &|id, _| c.current_formula(id),
                &|comp| counts.get(&comp).is_some_and(|&n| n > 0),
            )
        }
        _ => Vec::new(),
    };
    SpellbookView {
        filters: c.spellbook_filters(),
        rows,
    }
}

/// What the panel asked for.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Actions {
    /// Filter bit to flip.
    pub toggle: Option<u32>,
    /// Spells to put on the shown spell bar.
    pub add_to_bar: Vec<u32>,
    pub cast: Vec<u32>,
    pub forget: Option<u32>,
}

/// Which spell's details are open and which delete awaits confirmation.
#[derive(Default, Debug)]
pub struct UiState {
    pub info: Option<u32>,
    pub confirm: Option<u32>,
}

fn toggle_button(ui: &mut egui::Ui, on: bool, label: &str) -> bool {
    let text = egui::RichText::new(label).small().color(if on {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_gray(130)
    });
    ui.selectable_label(on, text).clicked()
}

fn details(ui: &mut egui::Ui, sp: &SpellRow, st: &mut UiState, a: &mut Actions) {
    let dim = egui::Color32::from_gray(200);
    ui.indent(("spell_details", sp.id), |ui| {
        ui.label(
            egui::RichText::new(format!(
                "{}  level {}  {} mana  {}",
                school::name(sp.school_id),
                roman(sp.level),
                sp.mana,
                match sp.duration {
                    Some(d) if d > 0.0 => format!("lasts {}", fmt_seconds(d)),
                    _ => "instant".to_string(),
                }
            ))
            .color(dim)
            .small(),
        );
        ui.label(egui::RichText::new(&sp.description).color(dim).small());
        ui.label(
            egui::RichText::new(format!("\"{}\"", sp.words))
                .color(dim)
                .italics()
                .small(),
        );
        ui.horizontal_wrapped(|ui| {
            caption(ui, "Formula:");
            if sp.formula.is_empty() {
                caption(ui, "(none)");
            }
            for (name, have) in &sp.formula {
                let color = if *have {
                    egui::Color32::from_rgb(180, 230, 180)
                } else {
                    egui::Color32::from_rgb(255, 150, 120)
                };
                ui.label(egui::RichText::new(name).color(color).small())
                    .on_hover_text(if *have { "carried" } else { "not carried" });
            }
        });
        ui.horizontal(|ui| {
            if ui.small_button("Cast").clicked() {
                a.cast.push(sp.id);
            }
            if ui.small_button("To bar").clicked() {
                a.add_to_bar.push(sp.id);
            }
            if st.confirm == Some(sp.id) {
                ui.label(
                    egui::RichText::new("Forget this spell?")
                        .color(egui::Color32::from_rgb(255, 150, 120))
                        .small(),
                );
                if ui.small_button("Yes").clicked() {
                    a.forget = Some(sp.id);
                    st.confirm = None;
                    st.info = None;
                }
                if ui.small_button("No").clicked() {
                    st.confirm = None;
                }
            } else if ui
                .small_button("Delete")
                .on_hover_text("remove the spell from the spellbook (the server confirms)")
                .clicked()
            {
                st.confirm = Some(sp.id);
            }
        });
    });
}

/// Draw the panel at `x`.
pub fn draw(
    egui: &egui::Context,
    icons: &mut IconCache,
    v: &SpellbookView,
    x: f32,
    shown_bar: usize,
    st: &mut UiState,
) -> Actions {
    let mut a = Actions::default();
    let shown: Vec<&SpellRow> = v
        .rows
        .iter()
        .filter(|r| filter::passes(v.filters, r.school_id, r.level))
        .collect();
    window(
        "spellbook",
        egui::pos2(x, 132.0),
        egui::vec2(380.0, 380.0),
        170,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(368.0, 368.0));
        title(
            ui,
            format!("Spellbook ({} of {})", shown.len(), v.rows.len()),
        );
        let bits = if v.filters == 0 {
            filter::ALL
        } else {
            v.filters
        };
        ui.horizontal(|ui| {
            for (label, bit) in filter::SCHOOL_TOGGLES {
                if toggle_button(ui, bits & bit != 0, label) {
                    a.toggle = Some(bit);
                }
            }
        });
        ui.horizontal(|ui| {
            for level in 1..=ac_formats::spell_table::MAX_LEVEL {
                let bit = filter::level(level);
                if toggle_button(ui, bits & bit != 0, roman(level)) {
                    a.toggle = Some(bit);
                }
            }
        });
        caption(
            ui,
            format!(
                "double-click or drag: add to spell bar tab {}; right-click: details",
                shown_bar + 1
            ),
        );
        ui.add_space(2.0);
        egui::ScrollArea::vertical()
            .max_height(300.0)
            .show(ui, |ui| {
                ui.set_min_width(360.0);
                if shown.is_empty() {
                    ui.label(
                        egui::RichText::new(if v.rows.is_empty() {
                            "(no spells known)"
                        } else {
                            "(every known spell is filtered out)"
                        })
                        .color(egui::Color32::from_gray(170)),
                    );
                }
                for sp in shown {
                    let color = if sp.self_targeted {
                        egui::Color32::from_rgb(180, 230, 180)
                    } else {
                        egui::Color32::WHITE
                    };
                    let resp = ui
                        .horizontal(|ui| {
                            let icon = icons.draw(
                                ui,
                                IconLayers::single(sp.icon),
                                egui::Sense::click_and_drag(),
                            );
                            let text = ui.add(
                                egui::Label::new(egui::RichText::new(&sp.name).color(color))
                                    .sense(egui::Sense::click_and_drag()),
                            );
                            let detail = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "{}  {}  {} mana",
                                        sp.school,
                                        roman(sp.level),
                                        sp.mana
                                    ))
                                    .color(egui::Color32::from_gray(170))
                                    .small(),
                                )
                                .sense(egui::Sense::click_and_drag()),
                            );
                            let row = icon.union(text).union(detail);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("i").on_hover_text("details").clicked() {
                                        st.info = if st.info == Some(sp.id) {
                                            None
                                        } else {
                                            Some(sp.id)
                                        };
                                    }
                                },
                            );
                            row
                        })
                        .inner;
                    resp.dnd_set_drag_payload(SpellDrag(sp.id));
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
                    if resp.hovered() {
                        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                    }
                    if resp.double_clicked() {
                        a.add_to_bar.push(sp.id);
                    }
                    if resp.secondary_clicked() {
                        st.info = if st.info == Some(sp.id) {
                            None
                        } else {
                            Some(sp.id)
                        };
                    }
                    if st.info == Some(sp.id) {
                        details(ui, sp, st, &mut a);
                    }
                }
            });
    });
    // The dragged spell's name follows the pointer.
    if let Some(p) = egui::DragAndDrop::payload::<SpellDrag>(egui) {
        if let (Some(name), Some(pos)) = (
            v.rows.iter().find(|r| r.id == p.0).map(|r| r.name.clone()),
            egui.pointer_interact_pos(),
        ) {
            egui::Area::new(egui::Id::new("spell_drag_ghost"))
                .order(egui::Order::Tooltip)
                .fixed_pos(pos + egui::vec2(12.0, 12.0))
                .interactable(false)
                .show(egui, |ui| {
                    super::frame(200, 4).show(ui, |ui| {
                        ui.label(egui::RichText::new(name).color(egui::Color32::WHITE));
                    });
                });
        }
    }
    a
}

#[derive(Default)]
pub struct Spellbook {
    source: Source<SpellbookView>,
    /// Open (P toggles it). Starts closed.
    pub show: bool,
    state: UiState,
}

impl Spellbook {
    /// A handful of real spells across the schools (Strength/Heal/Infuse/
    /// Invulnerability Other I, the Self variants, protections, Acid
    /// Stream III, Shock Wave II, Mind Blossom) when the tables are given,
    /// scarabs and tapers carried, every filter on, open.
    pub fn demo(tables: Option<(&SpellTable, &SpellComponentTable)>) -> Self {
        let rows = match tables {
            Some((table, comps)) => rows(
                table,
                comps,
                [1, 2, 5, 6, 9, 17, 20, 24, 60, 65, 2091],
                &|_, s| s.formula().collect(),
                &|c| ac_client::magic::is_scarab(c) || c == ac_client::magic::PRISMATIC_TAPER,
            ),
            None => Vec::new(),
        };
        Spellbook {
            source: Source::Demo(SpellbookView {
                filters: filter::ALL,
                rows,
            }),
            show: true,
            state: UiState::default(),
        }
    }
}

impl Plugin for Spellbook {
    fn name(&self) -> &str {
        "spellbook"
    }

    fn load(&mut self, settings: &Settings) {
        if let Some(v) = settings.get("spellbook.show") {
            self.show = v;
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("spellbook.show", self.show);
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        cx.board.set(OPEN_KEY, self.show);
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
        let shown_bar = cx
            .board
            .get(super::spellbar::SHOWN_KEY)
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => match cx.try_client() {
                Some(c) if has_sheet(c) => Some(view(c)),
                _ => None,
            },
        };
        let Some(v) = v else { return };
        let a = draw(egui, cx.icons(), &v, x, shown_bar, &mut self.state);
        match &mut self.source {
            Source::Demo(d) => {
                if let Some(bit) = a.toggle {
                    d.filters = filter::toggled(d.filters, bit);
                }
            }
            Source::Live => {
                let Some(c) = cx.try_client() else { return };
                if let Some(bit) = a.toggle {
                    c.set_spellbook_filters(filter::toggled(v.filters, bit));
                }
                for id in &a.cast {
                    c.cast(*id);
                }
                for id in &a.add_to_bar {
                    c.add_to_spell_bar(shown_bar, usize::MAX, *id);
                }
                if let Some(id) = a.forget {
                    c.forget_spell(id);
                }
                let names: Vec<String> = a
                    .add_to_bar
                    .iter()
                    .filter_map(|id| v.rows.iter().find(|r| r.id == *id))
                    .map(|r| r.name.clone())
                    .collect();
                for n in names {
                    cx.log(format!(
                        "spellbook: {n} added to spell bar tab {}",
                        shown_bar + 1
                    ));
                }
                if let Some(n) = a
                    .forget
                    .and_then(|id| v.rows.iter().find(|r| r.id == id))
                    .map(|r| r.name.clone())
                {
                    cx.log(format!("spellbook: forgetting {n}"));
                }
            }
        }
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

    fn row(id: u32, name: &str, level: u32, order: u32) -> SpellRow {
        SpellRow {
            id,
            name: name.into(),
            level,
            school_id: school::LIFE,
            school: "Life",
            mana: 10,
            self_targeted: true,
            icon: 0,
            description: String::new(),
            words: String::new(),
            display_order: order,
            duration: None,
            formula: Vec::new(),
        }
    }

    #[test]
    fn sorted_by_display_order_then_level_then_name() {
        let mut rows = vec![
            row(3, "Heal Self II", 2, 5),
            row(1, "Strength Self I", 1, 9),
            row(2, "Heal Self I", 1, 5),
            row(4, "Armor Self I", 1, 5),
        ];
        sort(&mut rows);
        let ids: Vec<u32> = rows.iter().map(|r| r.id).collect();
        assert_eq!(ids, [4, 2, 3, 1]);
    }

    #[test]
    fn filter_bits_toggle_and_pass() {
        use filter::*;
        assert_eq!(level(1), 0x10);
        assert_eq!(level(8), 0x800);
        assert_eq!(level(9), 0x1000);
        assert_eq!(level(0), 0);
        assert_eq!(school_bit(school::VOID), VOID);
        assert_eq!(school_bit(school::NONE), 0);
        // Nothing set shows everything; the first toggle starts from ALL.
        assert!(passes(0, school::WAR, 8));
        assert_eq!(toggled(0, WAR), ALL & !WAR);
        assert_eq!(toggled(ALL & !WAR, WAR), ALL);
        // A school or a level off hides its spells, the rest still show.
        let bits = toggled(ALL, LIFE);
        assert!(!passes(bits, school::LIFE, 1));
        assert!(passes(bits, school::WAR, 1));
        let bits = toggled(ALL, level(3));
        assert!(!passes(bits, school::WAR, 3));
        assert!(passes(bits, school::WAR, 4));
        // Quest spells without a school or level are always shown.
        assert!(passes(bits, school::NONE, 0));
    }

    #[test]
    fn roman_levels() {
        assert_eq!(roman(1), "I");
        assert_eq!(roman(4), "IV");
        assert_eq!(roman(8), "VIII");
        assert_eq!(roman(0), "?");
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
            source: Source::Demo(v),
            ..
        } = Spellbook::demo(Some((&table, &comps)))
        else {
            panic!("demo() is not a demo source");
        };
        assert!(!v.rows.is_empty());
        assert!(v
            .rows
            .windows(2)
            .all(|w| w[0].display_order <= w[1].display_order));
        let heal = v
            .rows
            .iter()
            .find(|r| r.name.starts_with("Heal Self"))
            .expect("Heal Self in the demo");
        assert!(!heal.formula.is_empty());
        assert!(heal.formula[0].1, "the scarab is carried in the demo");
        assert!(v.rows.iter().any(|r| r.duration.is_some_and(|d| d > 0.0)));
    }
}
