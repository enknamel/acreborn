//! Loot: the corpse or chest we are looking into, mid screen. Double-click
//! an item to take it, "Take all" for everything, "Close" to stop looking.
//!
//! * Hovering an item shows its stats (see `ItemStats::summary`); a
//!   single click selects and appraises it; "Appraise all" asks about
//!   every unappraised item in the background.
//! * A container holding more than [`SEARCH_MIN`] items gets the
//!   inventory's search line and kind chips; while they narrow the list
//!   "Take all" becomes "Take matching", taking only what is shown.
//! * Drop carried items on the window to store them in the container.

use super::{
    caption, inventory::Row, item_row, stats_tooltip, title, window, Filter, ItemDrag, Source,
};
use crate::icons::IconCache;
use crate::{egui, Client, Ctx, Plugin};

/// The open container's name and contents, each with its stats.
#[derive(Clone, Debug, PartialEq)]
pub struct LootView {
    pub name: String,
    pub rows: Vec<Row>,
}

impl LootView {
    /// Whether the list is long enough to want the search line.
    pub fn searchable(&self) -> bool {
        self.rows.len() > SEARCH_MIN
    }

    /// The guids of the rows a filter keeps, in list order.
    pub fn shown(&self, f: &Filter) -> Vec<u32> {
        f.matching(&self.rows, |r| &r.stats)
            .into_iter()
            .map(|i| self.rows[i].item.guid)
            .collect()
    }

    pub fn unappraised(&self) -> usize {
        self.rows.iter().filter(|r| !r.stats.appraised).count()
    }
}

/// Containers with more items than this get the search line and chips.
pub const SEARCH_MIN: usize = 8;

/// What the panel asked for.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Actions {
    pub take: Vec<u32>,
    pub close: bool,
    /// Carried items dragged onto the window: put into the container.
    pub store: Vec<u32>,
    /// Single-clicked items: select and appraise.
    pub inspect: Vec<u32>,
    /// Appraise every unappraised item of the container in the background.
    pub appraise_all: bool,
}

/// The open container, if any. Items the server has not described yet
/// are skipped.
pub fn view(c: &Client) -> Option<LootView> {
    let (guid, items) = c.world.open_container.as_ref()?;
    // Our own side packs list their contents the same way at login; they
    // belong in the inventory panel, not here.
    let me = c.world.player_guid;
    if me.is_some() && c.world.objects.get(guid).is_some_and(|o| o.container == me) {
        return None;
    }
    Some(LootView {
        name: c
            .world
            .objects
            .get(guid)
            .map(|o| o.name.clone())
            .unwrap_or_else(|| "Container".into()),
        rows: items
            .iter()
            .filter_map(|g| {
                let o = c.world.objects.get(g)?;
                Some(Row {
                    item: Item::of(o, false),
                    stats: c.stats_of(*g)?,
                })
            })
            .collect(),
    })
}

use super::Item;

/// The short number after a row: the damage range or the armor level.
fn suffix(r: &Row) -> String {
    if r.stats.damage_high > 0 {
        format!("{}-{}", r.stats.damage_low, r.stats.damage_high)
    } else if r.stats.armor_level > 0 {
        format!("AL {}", r.stats.armor_level)
    } else {
        String::new()
    }
}

pub fn draw(egui: &egui::Context, icons: &mut IconCache, v: &LootView, f: &mut Filter) -> Actions {
    let mut actions = Actions::default();
    let rect = egui.viewport_rect();
    let (w, h) = (rect.width(), rect.height());
    let searchable = v.searchable();
    let size = if searchable {
        egui::vec2(300.0, 330.0)
    } else {
        egui::vec2(280.0, 250.0)
    };
    let shown = if searchable {
        v.shown(f)
    } else {
        v.rows.iter().map(|r| r.item.guid).collect()
    };
    let filtering = searchable && f.filtering();
    window(
        "loot",
        egui::pos2(w * 0.5 - size.x * 0.5, h * 0.5 - size.y * 0.5),
        size,
        190,
        8,
    )
    .show(egui, |ui| {
        ui.set_min_size(size - egui::vec2(16.0, 16.0));
        ui.horizontal(|ui| {
            title(ui, &v.name);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if filtering {
                    caption(ui, format!("{} of {}", shown.len(), v.rows.len()));
                } else {
                    caption(ui, format!("{} items", v.rows.len()));
                }
            });
        });
        if searchable {
            let changed = f.draw(ui, "loot_search", size.x - 60.0);
            if changed && f.needs_appraisal() && v.unappraised() > 0 {
                actions.appraise_all = true;
            }
        }
        egui::ScrollArea::vertical()
            .max_height(size.y - if searchable { 150.0 } else { 80.0 })
            .show(ui, |ui| {
                ui.set_min_width(size.x - 30.0);
                if v.rows.is_empty() {
                    ui.label(egui::RichText::new("(empty)").color(egui::Color32::from_gray(170)));
                } else if shown.is_empty() {
                    ui.label(
                        egui::RichText::new("(nothing matches)")
                            .color(egui::Color32::from_gray(170)),
                    );
                }
                for r in v.rows.iter().filter(|r| shown.contains(&r.item.guid)) {
                    let color = if r.stats.appraised {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_gray(215)
                    };
                    let suffix = suffix(r);
                    let resp = ui
                        .horizontal(|ui| {
                            let resp = item_row(ui, icons, &r.item, color);
                            if !suffix.is_empty() {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| caption(ui, suffix),
                                );
                            }
                            resp
                        })
                        .inner
                        .on_hover_ui(|ui| stats_tooltip(ui, &r.item.name, &r.stats));
                    if resp.clicked() {
                        actions.inspect.push(r.item.guid);
                    }
                    if resp.double_clicked() {
                        actions.take.push(r.item.guid);
                    }
                }
            });
        ui.horizontal(|ui| {
            let take = if filtering {
                format!("Take matching ({})", shown.len())
            } else {
                "Take all".to_string()
            };
            if ui
                .add_enabled(!shown.is_empty(), egui::Button::new(take))
                .on_hover_text("take every item the list shows")
                .clicked()
            {
                actions.take.extend(shown.iter().copied());
            }
            let unappraised = v.unappraised();
            if unappraised > 0
                && ui
                    .small_button(format!("Appraise all ({unappraised})"))
                    .on_hover_text("ask the server about every item's stats, one at a time")
                    .clicked()
            {
                actions.appraise_all = true;
            }
            if ui.button("Close").clicked() {
                actions.close = true;
            }
        });
        let (r, _) = ui.dnd_drop_zone::<ItemDrag, _>(egui::Frame::new().inner_margin(2), |ui| {
            ui.label(
                egui::RichText::new("drop items here to store")
                    .color(egui::Color32::from_gray(170)),
            );
        });
        if let Some(p) = r.response.dnd_release_payload::<ItemDrag>() {
            actions.store.push(p.0);
        }
    });
    actions
}

#[derive(Default)]
pub struct Loot {
    source: Source<LootView>,
    /// The search line and chip; kept while the window is open.
    pub filter: Filter,
}

impl Loot {
    /// Known icons, one with an overlay, to check the layering; enough
    /// items for the search line, with sample stats for the tooltips.
    pub fn demo() -> Self {
        use super::inventory::{demo_item, demo_row};
        use ac_client::items::ItemStats;
        let mut layered = demo_item(12, "0x06001A8A + 0x06006A21", 1, false, 0x0600_1A8A);
        layered.icon.overlay = 0x0600_6A21;
        let plain = |guid, name: &str, stack, icon, kind, value| {
            demo_row(
                demo_item(guid, name, stack, false, icon),
                0,
                ItemStats {
                    kind,
                    value,
                    burden: 50,
                    ..Default::default()
                },
            )
        };
        Loot {
            source: Source::Demo(LootView {
                name: "Demo corpse".into(),
                rows: vec![
                    demo_row(
                        demo_item(9, "Loot 0x06002C0D", 1, false, 0x0600_2C0D),
                        0,
                        ItemStats {
                            kind: "weapon",
                            appraised: true,
                            damage_low: 6,
                            damage_high: 12,
                            damage_type: "Piercing".into(),
                            speed: 20,
                            spells: vec!["Blood Drinker III".into()],
                            wield_skill: "Dagger".into(),
                            wield_level: 200,
                            value: 800,
                            burden: 90,
                            material: "Iron",
                            workmanship: 5.0,
                            ..Default::default()
                        },
                    ),
                    plain(10, "Loot 0x0600601C", 5, 0x0600_601C, "comps", 15),
                    demo_row(
                        demo_item(11, "Loot 0x06006A21", 1, false, 0x0600_6A21),
                        0,
                        ItemStats {
                            kind: "armor",
                            appraised: true,
                            armor_level: 140,
                            value: 450,
                            burden: 600,
                            ..Default::default()
                        },
                    ),
                    demo_row(
                        layered,
                        0,
                        ItemStats {
                            kind: "healer",
                            value: 90,
                            ..Default::default()
                        },
                    ),
                    plain(13, "Pyreal", 220, 0x0600_1FB7, "money", 220),
                    plain(14, "Hyssop", 3, 0x0600_1DE9, "comps", 3),
                    plain(
                        15,
                        "Scroll of Strength Other I",
                        1,
                        0x0600_321E,
                        "scroll",
                        40,
                    ),
                    plain(16, "Leather Cap", 1, 0x0600_0FAA, "armor", 22),
                    plain(17, "Apple", 2, 0x0600_2F40, "food", 2),
                ],
            }),
            filter: Filter::default(),
        }
    }
}

impl Plugin for Loot {
    fn name(&self) -> &str {
        "loot"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().and_then(|c| view(c)),
        };
        let Some(v) = v else {
            self.filter = Filter::default();
            return;
        };
        let actions = draw(egui, cx.icons(), &v, &mut self.filter);
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            if actions.appraise_all {
                let n = c.appraise_many(v.rows.iter().map(|r| r.item.guid));
                if n > 0 {
                    tracing::info!("appraising {n} items in {}", v.name);
                }
            }
            for g in actions.inspect {
                c.inspect(g);
            }
            for g in actions.take {
                c.take(g);
            }
            if let Some(container) = c.world.open_container.as_ref().map(|(g, _)| *g) {
                for item in actions.store {
                    c.store_in(item, container);
                }
            }
            if actions.close {
                c.close_container();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_view() -> LootView {
        match Loot::demo().source {
            Source::Demo(v) => v,
            Source::Live => unreachable!(),
        }
    }

    #[test]
    fn demo_has_a_layered_icon() {
        let v = demo_view();
        assert_eq!(v.name, "Demo corpse");
        assert!(v.rows.iter().any(|r| r.item.icon.overlay != 0));
        assert!(v.rows.iter().all(|r| !r.item.wielded));
        assert!(v.searchable());
        assert!(v.unappraised() > 0);
    }

    #[test]
    fn shown_follows_the_filter() {
        let v = demo_view();
        let mut f = Filter::default();
        assert_eq!(v.shown(&f).len(), v.rows.len());
        // A chip.
        f.kind = super::super::inventory::KINDS
            .iter()
            .position(|k| k.0 == "Armor")
            .unwrap();
        let names: Vec<&str> = v
            .shown(&f)
            .into_iter()
            .map(|g| {
                v.rows
                    .iter()
                    .find(|r| r.item.guid == g)
                    .unwrap()
                    .item
                    .name
                    .as_str()
            })
            .collect();
        assert_eq!(names, ["Loot 0x06006A21", "Leather Cap"]);
        // A stat query only appraised items answer.
        f.kind = 0;
        f.search = "dmg>5".into();
        assert_eq!(v.shown(&f), vec![9]);
        // A word.
        f.search = "hyssop".into();
        assert_eq!(v.shown(&f), vec![14]);
        f.search = "nothing here".into();
        assert!(v.shown(&f).is_empty());
        // Short containers keep the plain window.
        let mut short = v.clone();
        short.rows.truncate(SEARCH_MIN);
        assert!(!short.searchable());
    }
}
