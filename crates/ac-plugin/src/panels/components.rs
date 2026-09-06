//! Components: every spell component in the packs (`Client::components`)
//! grouped by kind (scarabs, tapers, herbs, powders, potions, talismans),
//! with its count and the quantity the character wants to keep (0..999,
//! `SetDesiredComponentLevel`, edited in place); the foci carried per
//! school; and "Fill from vendor", which buys up to the desired counts
//! while a vendor is open (`@fillcomps`). O toggles it; it sits beside the
//! skills and spellbook panels when they are open.
//!
//! Components are consumed by the server on each cast, never by the spell
//! bar (see `docs/game/mechanics.md`); this panel only shows what is
//! carried, so a component the character has none of does not appear.

use std::collections::HashMap;

use ac_formats::spell_components::{component_type, SpellComponentTable};
use ac_formats::spell_table::school;

use super::{caption, has_sheet, title, window, Source};
use crate::icons::{IconCache, IconLayers};
use crate::{egui, Client, Ctx, Plugin, Settings};

/// Whether this panel is open, on the bus: the panels that share the
/// left column (see [`super::autoplay`]) step aside for it.
pub const OPEN_KEY: &str = "panels.components_open";

/// Highest desired quantity the server accepts.
pub const MAX_DESIRED: u32 = 999;

/// One component line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentRow {
    /// Id in the SpellComponentsTable.
    pub id: u32,
    pub name: String,
    pub count: u32,
    pub desired: u32,
    /// RenderSurface (0x06) id.
    pub icon: u32,
}

/// A school's focus and whether it is carried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Focus {
    pub school: &'static str,
    pub name: &'static str,
    pub carried: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComponentsView {
    /// Kind name and its rows, in [`KIND_ORDER`].
    pub groups: Vec<(&'static str, Vec<ComponentRow>)>,
    pub foci: Vec<Focus>,
    /// A vendor is open, so "Fill from vendor" can run.
    pub vendor_open: bool,
}

/// Display order of the component kinds.
pub const KIND_ORDER: [u32; 7] = [
    component_type::SCARAB,
    component_type::TAPER,
    component_type::HERB,
    component_type::POWDER,
    component_type::POTION,
    component_type::TALISMAN,
    component_type::PEA,
];

/// The focus item of each school (see `ac_client::magic::focus_wcid`).
pub fn focus_name(school_id: u32) -> &'static str {
    match school_id {
        school::WAR => "Foci of Strife",
        school::LIFE => "Foci of Verdancy",
        school::CREATURE => "Foci of Enchantment",
        school::ITEM => "Foci of Artifice",
        school::VOID => "Foci of Shadow",
        _ => "?",
    }
}

/// The schools with a focus, in spellbook-filter order.
pub const FOCUS_SCHOOLS: [u32; 5] = [
    school::CREATURE,
    school::ITEM,
    school::LIFE,
    school::WAR,
    school::VOID,
];

/// Group `(kind, row)` pairs by kind in [`KIND_ORDER`] (unknown kinds
/// last), each group sorted by component id, which orders scarabs by
/// level.
pub fn group(rows: Vec<(u32, ComponentRow)>) -> Vec<(&'static str, Vec<ComponentRow>)> {
    let mut by_kind: HashMap<u32, Vec<ComponentRow>> = HashMap::new();
    for (kind, row) in rows {
        by_kind.entry(kind).or_default().push(row);
    }
    let mut out = Vec::new();
    let mut kinds: Vec<u32> = KIND_ORDER.to_vec();
    let mut rest: Vec<u32> = by_kind
        .keys()
        .copied()
        .filter(|k| !KIND_ORDER.contains(k))
        .collect();
    rest.sort_unstable();
    kinds.extend(rest);
    for kind in kinds {
        if let Some(mut rows) = by_kind.remove(&kind) {
            rows.sort_by_key(|r| r.id);
            out.push((component_type::name(kind), rows));
        }
    }
    out
}

/// This session's components and foci; `None` until the sheet arrived.
pub fn view(c: &Client) -> Option<ComponentsView> {
    if !has_sheet(c) {
        return None;
    }
    let table = c.assets.spell_components().ok();
    let rows = c
        .components()
        .into_iter()
        .map(|k| {
            let entry = table.as_ref().and_then(|t| t.get(k.component_id));
            (
                entry.map(|e| e.kind).unwrap_or(0),
                ComponentRow {
                    id: k.component_id,
                    name: k.name,
                    count: k.count,
                    desired: k.desired,
                    icon: entry.map(|e| e.icon_id).unwrap_or(0),
                },
            )
        })
        .collect();
    Some(ComponentsView {
        groups: group(rows),
        foci: FOCUS_SCHOOLS
            .iter()
            .map(|&s| Focus {
                school: school::short_name(s),
                name: focus_name(s),
                carried: c.has_focus(s),
            })
            .collect(),
        vendor_open: c.world.open_vendor.is_some(),
    })
}

/// What the panel asked for.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Actions {
    /// `(component id, desired quantity)`.
    pub desired: Vec<(u32, u32)>,
    pub fill: bool,
}

/// Desired quantities being edited, sent when the edit ends.
#[derive(Default, Debug)]
pub struct UiState {
    edits: HashMap<u32, u32>,
}

/// Draw the panel at `x`, in the row of the skills and spellbook panels.
pub fn draw(
    egui: &egui::Context,
    icons: &mut IconCache,
    v: &ComponentsView,
    x: f32,
    st: &mut UiState,
) -> Actions {
    let mut a = Actions::default();
    window(
        "components",
        egui::pos2(x, 132.0),
        egui::vec2(260.0, 380.0),
        170,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(248.0, 368.0));
        ui.horizontal(|ui| {
            let total: usize = v.groups.iter().map(|(_, r)| r.len()).sum();
            title(ui, format!("Components ({total})"));
            if ui
                .add_enabled(v.vendor_open, egui::Button::new("Fill from vendor"))
                .on_hover_text("buy up to the desired quantities from the open vendor")
                .on_disabled_hover_text("open a vendor first")
                .clicked()
            {
                a.fill = true;
            }
        });
        ui.horizontal_wrapped(|ui| {
            caption(ui, "Foci:");
            for f in &v.foci {
                let (color, mark) = if f.carried {
                    (egui::Color32::from_rgb(180, 230, 180), "+")
                } else {
                    (egui::Color32::from_gray(120), "-")
                };
                ui.label(
                    egui::RichText::new(format!("{}{}", f.school, mark))
                        .color(color)
                        .small(),
                )
                .on_hover_text(if f.carried {
                    format!("{} carried: prismatic formulas", f.name)
                } else {
                    format!("{} not carried: full formulas", f.name)
                });
            }
        });
        egui::ScrollArea::vertical()
            .max_height(310.0)
            .show(ui, |ui| {
                ui.set_min_width(240.0);
                if v.groups.is_empty() {
                    caption(ui, "(no components carried)");
                }
                egui::Grid::new("components_grid")
                    .num_columns(3)
                    .spacing([10.0, 2.0])
                    .show(ui, |ui| {
                        caption(ui, "Component");
                        caption(ui, "Have");
                        caption(ui, "Want");
                        ui.end_row();
                        for (kind, rows) in &v.groups {
                            caption(ui, *kind);
                            ui.end_row();
                            for r in rows {
                                ui.horizontal(|ui| {
                                    icons.draw(
                                        ui,
                                        IconLayers::single(r.icon),
                                        egui::Sense::hover(),
                                    );
                                    ui.label(
                                        egui::RichText::new(&r.name).color(egui::Color32::WHITE),
                                    );
                                });
                                let short = r.count < r.desired;
                                ui.label(
                                    egui::RichText::new(r.count.to_string())
                                        .color(if short {
                                            egui::Color32::from_rgb(255, 170, 150)
                                        } else {
                                            egui::Color32::WHITE
                                        })
                                        .strong(),
                                );
                                let mut want = st.edits.get(&r.id).copied().unwrap_or(r.desired);
                                let resp = ui.add(
                                    egui::DragValue::new(&mut want)
                                        .range(0..=MAX_DESIRED)
                                        .speed(1.0),
                                );
                                if resp.changed() {
                                    st.edits.insert(r.id, want);
                                }
                                if resp.drag_stopped() || resp.lost_focus() {
                                    if let Some(w) = st.edits.remove(&r.id) {
                                        if w != r.desired {
                                            a.desired.push((r.id, w));
                                        }
                                    }
                                }
                                ui.end_row();
                            }
                        }
                    });
            });
    });
    a
}

#[derive(Default)]
pub struct Components {
    source: Source<ComponentsView>,
    /// Open (O toggles it). Starts closed.
    pub show: bool,
    state: UiState,
}

impl Components {
    /// A few real components (named through the table when given) with
    /// counts and desired quantities, two foci, a vendor open; open.
    pub fn demo(table: Option<&SpellComponentTable>) -> Self {
        let row = |id: u32, fallback: &str, count: u32, desired: u32| {
            let entry = table.and_then(|t| t.get(id));
            (
                entry.map(|e| e.kind).unwrap_or(0),
                ComponentRow {
                    id,
                    name: entry
                        .map(|e| e.name.clone())
                        .unwrap_or_else(|| fallback.to_string()),
                    count,
                    desired,
                    icon: entry.map(|e| e.icon_id).unwrap_or(0),
                },
            )
        };
        // Any herb, powder, potion and talisman the table has.
        let first_of = |kind: u32| -> Option<u32> {
            table.and_then(|t| {
                t.components
                    .iter()
                    .find(|(_, c)| c.kind == kind)
                    .map(|(id, _)| *id)
            })
        };
        let mut rows = vec![
            row(1, "Lead Scarab", 40, 50),
            row(2, "Iron Scarab", 12, 25),
            row(3, "Copper Scarab", 3, 25),
            row(188, "Prismatic Taper", 250, 500),
        ];
        for (kind, fallback) in [
            (component_type::HERB, "Bloodrose"),
            (component_type::POWDER, "Colcothar"),
            (component_type::POTION, "Hyssop Oil"),
            (component_type::TALISMAN, "Baneful Talisman"),
        ] {
            let (id, kind_id) = match first_of(kind) {
                Some(id) => (id, kind),
                None => (1000 + kind, kind),
            };
            let mut r = row(id, fallback, 6, 10);
            r.0 = kind_id;
            rows.push(r);
        }
        let foci = FOCUS_SCHOOLS
            .iter()
            .map(|&s| Focus {
                school: school::short_name(s),
                name: focus_name(s),
                carried: matches!(s, school::LIFE | school::CREATURE),
            })
            .collect();
        Components {
            source: Source::Demo(ComponentsView {
                groups: group(rows),
                foci,
                vendor_open: true,
            }),
            show: true,
            state: UiState::default(),
        }
    }
}

impl Plugin for Components {
    fn name(&self) -> &str {
        "components"
    }

    fn load(&mut self, settings: &Settings) {
        if let Some(v) = settings.get("components.show") {
            self.show = v;
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("components.show", self.show);
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
        let Some(v) = v else { return };
        // Sits beside the skills and spellbook panels when they are open.
        let open = |key: &str| cx.board.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
        let mut x = 8.0;
        if open(super::skills::OPEN_KEY) {
            x += 372.0;
        }
        if open(super::spellbook::OPEN_KEY) {
            x += 384.0;
        }
        let a = draw(egui, cx.icons(), &v, x, &mut self.state);
        match &mut self.source {
            Source::Demo(d) => {
                for (id, want) in a.desired {
                    for row in d.groups.iter_mut().flat_map(|(_, r)| r.iter_mut()) {
                        if row.id == id {
                            row.desired = want;
                        }
                    }
                }
            }
            Source::Live => {
                let Some(c) = cx.try_client() else { return };
                for (id, want) in a.desired {
                    c.set_desired_component(id, want);
                }
                if a.fill {
                    let n = c.fill_components();
                    cx.log(format!("components: asked the vendor for {n} stacks"));
                }
            }
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if key == egui::Key::O && pressed {
            self.show = !self.show;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: u32, name: &str) -> ComponentRow {
        ComponentRow {
            id,
            name: name.into(),
            count: 1,
            desired: 0,
            icon: 0,
        }
    }

    #[test]
    fn grouped_by_kind_in_order() {
        let groups = group(vec![
            (component_type::HERB, row(20, "Bloodrose")),
            (component_type::SCARAB, row(2, "Iron Scarab")),
            (component_type::TAPER, row(188, "Prismatic Taper")),
            (component_type::SCARAB, row(1, "Lead Scarab")),
            (99, row(500, "Odd")),
        ]);
        let names: Vec<(&str, Vec<&str>)> = groups
            .iter()
            .map(|(k, rows)| (*k, rows.iter().map(|r| r.name.as_str()).collect()))
            .collect();
        assert_eq!(
            names,
            [
                ("Scarab", vec!["Lead Scarab", "Iron Scarab"]),
                ("Taper", vec!["Prismatic Taper"]),
                ("Herb", vec!["Bloodrose"]),
                ("Unknown", vec!["Odd"]),
            ]
        );
    }

    #[test]
    fn foci_named_per_school() {
        assert_eq!(focus_name(school::LIFE), "Foci of Verdancy");
        assert_eq!(focus_name(school::VOID), "Foci of Shadow");
        assert_eq!(focus_name(0), "?");
    }

    #[test]
    fn demo_has_groups_and_foci() {
        let Components {
            source: Source::Demo(v),
            ..
        } = Components::demo(None)
        else {
            panic!("demo() is not a demo source");
        };
        assert_eq!(v.foci.len(), 5);
        assert_eq!(v.foci.iter().filter(|f| f.carried).count(), 2);
        assert!(v.vendor_open, "the demo shows a vendor window too");
        let total: usize = v.groups.iter().map(|(_, r)| r.len()).sum();
        assert_eq!(total, 8);
    }
}
