//! Vendor: the shop we are trading with, top centre. The vendor's stock
//! on the left with Buy buttons, our sellable pack items on the right
//! with Sell buttons; Close ends the trade.
//!
//! Both lists take the inventory's search language and kind chips
//! (`dmg>10`, `type:armor`, `spell:blood`) and a sort (name, price,
//! value, burden, damage, armor). Hovering an item shows its stats;
//! "Sell all shown" sells everything the right-hand search leaves, and
//! "Appraise all" fetches the numbers for whatever is still unappraised.

use super::{caption, stats_tooltip, title, window, Filter, Source};
use crate::{egui, Client, Ctx, Plugin};
use ac_client::items::{self, ItemStats, NumKey, SortKey};

/// A vendor's stock line or one of our items offered for sale.
#[derive(Clone, Debug, PartialEq)]
pub struct TradeItem {
    pub guid: u32,
    pub name: String,
    pub price: u32,
    pub icon: u32,
    /// Unlimited supply (vendor stock) when true.
    pub unlimited: bool,
    /// What the item is, for the tooltip, the search and the sort.
    pub stats: ItemStats,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VendorView {
    pub name: String,
    pub stock: Vec<TradeItem>,
    pub selling: Vec<TradeItem>,
    /// Items on either side we have no appraisal for yet.
    pub unappraised: usize,
}

/// The sort choices over each list.
pub const SORTS: &[(&str, Option<SortKey>)] = &[
    ("price", None),
    ("name", Some(SortKey::Name)),
    ("value", Some(SortKey::Num(NumKey::Value))),
    ("burden", Some(SortKey::Num(NumKey::Burden))),
    ("damage", Some(SortKey::Num(NumKey::Damage))),
    ("armor", Some(SortKey::Num(NumKey::Armor))),
];

/// The items a filter keeps, in the chosen order (price is the panel's
/// own key; the rest come from `ac_client::items::sort`).
pub fn shown(items: &[TradeItem], f: &Filter, sort: usize, descending: bool) -> Vec<usize> {
    let mut idx = f.matching(items, |it| &it.stats);
    match SORTS.get(sort).and_then(|s| s.1) {
        None => idx.sort_by(|a, b| {
            let o = items[*a].price.cmp(&items[*b].price);
            if descending {
                o.reverse()
            } else {
                o
            }
        }),
        Some(key) => {
            let mut stats: Vec<ItemStats> = idx.iter().map(|i| items[*i].stats.clone()).collect();
            items::sort(&mut stats, key, descending);
            idx.sort_by_key(|i| {
                stats
                    .iter()
                    .position(|s| s.guid == items[*i].stats.guid)
                    .unwrap_or(usize::MAX)
            });
        }
    }
    idx
}

/// What the panel asked for.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Actions {
    pub buy: Vec<u32>,
    pub sell: Vec<u32>,
    pub close: bool,
    /// Single-clicked stock or pack items: select and appraise.
    pub inspect: Vec<u32>,
    /// Appraise everything on both sides that has no numbers yet.
    pub appraise_all: bool,
}

/// The panel's own state: a filter and a sort per side.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub stock: Filter,
    pub selling: Filter,
    pub sort: usize,
    pub descending: bool,
}

/// Vendor stock with this "-1" stack size never runs out.
pub const UNLIMITED_STACK: u32 = 0x00FF_FFFF;

/// What the vendor charges for an item of `value` at its `sell_rate`
/// (ACE's SellPrice: the rate the vendor sells at), at least 1 pyreal.
pub fn buy_price(value: u32, sell_rate: f32) -> u32 {
    ((value as f32 * sell_rate - 0.1).ceil().max(1.0)) as u32
}

/// What the vendor pays for one of our items at its `buy_rate`, at least
/// 1 pyreal.
pub fn sell_price(value: u32, buy_rate: f32) -> u32 {
    ((value as f32 * buy_rate + 0.1).floor().max(1.0)) as u32
}

/// The open vendor, if any. Only pack items with a value that are not
/// money are offered for sale.
pub fn view(c: &Client) -> Option<VendorView> {
    let v = c.world.open_vendor.as_ref()?;
    let stats_of = |guid: u32| c.stats_of(guid).unwrap_or_default();
    Some(VendorView {
        name: c
            .world
            .objects
            .get(&v.vendor)
            .map(|o| o.name.clone())
            .unwrap_or_else(|| "Vendor".into()),
        stock: v
            .items
            .iter()
            .map(|it| TradeItem {
                guid: it.guid,
                name: it.desc.name.clone(),
                price: buy_price(it.desc.value, v.sell_rate),
                icon: it.desc.icon_id,
                unlimited: it.stack == UNLIMITED_STACK,
                stats: stats_of(it.guid),
            })
            .collect(),
        selling: c
            .world
            .inventory()
            .filter(|o| o.value > 0 && o.item_type & ac_world::item_type::MONEY == 0)
            .map(|o| TradeItem {
                guid: o.guid,
                name: o.name.clone(),
                price: sell_price(o.value, v.buy_rate),
                icon: o.icon_id,
                unlimited: false,
                stats: stats_of(o.guid),
            })
            .collect(),
        unappraised: v
            .items
            .iter()
            .map(|it| it.guid)
            .chain(c.world.inventory().map(|o| o.guid))
            .filter(|g| !c.appraisals.contains_key(g))
            .count(),
    })
}

#[allow(clippy::too_many_arguments)]
fn trade_list(
    ui: &mut egui::Ui,
    salt: &str,
    header: &str,
    button: &str,
    items: &[TradeItem],
    filter: &mut Filter,
    sort: usize,
    descending: bool,
    out: &mut Vec<u32>,
    actions_inspect: &mut Vec<u32>,
) -> bool {
    let mut wants_appraisal = false;
    ui.label(egui::RichText::new(header).color(egui::Color32::from_gray(180)));
    if filter.draw(ui, salt, 150.0) && filter.needs_appraisal() {
        wants_appraisal = true;
    }
    let idx = shown(items, filter, sort, descending);
    if filter.filtering() {
        caption(ui, format!("{} of {}", idx.len(), items.len()));
    }
    egui::ScrollArea::vertical()
        .id_salt(salt.to_string())
        .max_height(280.0)
        .show(ui, |ui| {
            for i in idx {
                let it = &items[i];
                ui.horizontal(|ui| {
                    if ui.small_button(button).clicked() {
                        out.push(it.guid);
                    }
                    let color = if it.stats.appraised {
                        egui::Color32::WHITE
                    } else {
                        egui::Color32::from_gray(215)
                    };
                    let row = ui
                        .add(
                            egui::Label::new(
                                egui::RichText::new(format!("{}  {}p", it.name, it.price))
                                    .color(color),
                            )
                            .sense(egui::Sense::click()),
                        )
                        .on_hover_ui(|ui| stats_tooltip(ui, &it.name, &it.stats));
                    if row.clicked() {
                        actions_inspect.push(it.guid);
                    }
                });
            }
        });
    wants_appraisal
}

pub fn draw(egui: &egui::Context, v: &VendorView, st: &mut State) -> Actions {
    let mut actions = Actions::default();
    let w = egui.viewport_rect().width();
    window(
        "vendor",
        egui::pos2(w * 0.5 - 280.0, 60.0),
        egui::vec2(560.0, 480.0),
        200,
        8,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(544.0, 464.0));
        ui.horizontal(|ui| {
            title(ui, &v.name);
            if ui.button("Close").clicked() {
                actions.close = true;
            }
            caption(ui, "sort");
            egui::ComboBox::from_id_salt("vendor_sort")
                .selected_text(SORTS[st.sort.min(SORTS.len() - 1)].0)
                .width(90.0)
                .show_ui(ui, |ui| {
                    for (i, (label, key)) in SORTS.iter().enumerate() {
                        if ui.selectable_label(st.sort == i, *label).clicked() {
                            st.sort = i;
                            let needs = key
                                .map(|k| matches!(k, SortKey::Num(n) if n.needs_appraisal()))
                                .unwrap_or(false);
                            if needs && v.unappraised > 0 {
                                actions.appraise_all = true;
                            }
                        }
                    }
                });
            if ui
                .small_button(if st.descending { "desc" } else { "asc" })
                .clicked()
            {
                st.descending = !st.descending;
            }
            if v.unappraised > 0
                && ui
                    .small_button(format!("Appraise all ({})", v.unappraised))
                    .on_hover_text("ask the server for every item's stats, one at a time")
                    .clicked()
            {
                actions.appraise_all = true;
            }
        });
        ui.columns(2, |cols| {
            if trade_list(
                &mut cols[0],
                "stock",
                "For sale",
                "Buy",
                &v.stock,
                &mut st.stock,
                st.sort,
                st.descending,
                &mut actions.buy,
                &mut actions.inspect,
            ) {
                actions.appraise_all = true;
            }
            let ui = &mut cols[1];
            if trade_list(
                ui,
                "sell",
                "Your pack",
                "Sell",
                &v.selling,
                &mut st.selling,
                st.sort,
                st.descending,
                &mut actions.sell,
                &mut actions.inspect,
            ) {
                actions.appraise_all = true;
            }
            let shown_now = shown(&v.selling, &st.selling, st.sort, st.descending);
            let total: u32 = shown_now.iter().map(|i| v.selling[*i].price).sum();
            if !shown_now.is_empty()
                && ui
                    .button(format!("Sell all shown ({} for {total}p)", shown_now.len()))
                    .on_hover_text("sell everything the search above leaves")
                    .clicked()
            {
                actions
                    .sell
                    .extend(shown_now.iter().map(|i| v.selling[*i].guid));
            }
        });
    });
    actions
}

#[derive(Default)]
pub struct Vendor {
    source: Source<VendorView>,
    state: State,
}

impl Vendor {
    pub fn demo() -> Self {
        let item = |guid: u32, name: &str, price, unlimited, stats: ItemStats| TradeItem {
            guid,
            name: name.into(),
            price,
            icon: 0,
            unlimited,
            stats: ItemStats {
                guid,
                name: name.into(),
                ..stats
            },
        };
        let armor = |al| ItemStats {
            kind: "armor",
            appraised: true,
            armor_level: al,
            ..Default::default()
        };
        let weapon = |low, high| ItemStats {
            kind: "weapon",
            appraised: true,
            damage_low: low,
            damage_high: high,
            damage_type: "Piercing".into(),
            speed: 20,
            ..Default::default()
        };
        Vendor {
            state: State::default(),
            source: Source::Demo(VendorView {
                name: "Demo Shopkeeper".into(),
                stock: vec![
                    item(0x100, "Dagger", 15, true, weapon(4, 9)),
                    item(0x101, "Leather Cap", 22, true, armor(20)),
                    item(
                        0x102,
                        "Apple",
                        2,
                        true,
                        ItemStats {
                            kind: "food",
                            appraised: true,
                            ..Default::default()
                        },
                    ),
                ],
                selling: vec![
                    item(0x200, "Demo item 0x06001A8A", 3, false, Default::default()),
                    item(0x201, "Demo item 0x0600321E", 40, false, armor(120)),
                ],
                unappraised: 1,
            }),
        }
    }
}

impl Plugin for Vendor {
    fn name(&self) -> &str {
        "vendor"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().and_then(|c| view(c)),
        };
        let Some(v) = v else { return };
        let actions = draw(egui, &v, &mut self.state);
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            if actions.appraise_all {
                let guids: Vec<u32> = v
                    .stock
                    .iter()
                    .chain(v.selling.iter())
                    .map(|it| it.guid)
                    .collect();
                c.appraise_many(guids);
            }
            for g in actions.inspect {
                c.inspect(g);
            }
            for g in actions.buy {
                c.buy(g);
            }
            for g in actions.sell {
                c.sell(g);
            }
            if actions.close {
                c.close_vendor();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_view() -> VendorView {
        match Vendor::demo().source {
            Source::Demo(v) => v,
            Source::Live => unreachable!(),
        }
    }

    #[test]
    fn lists_filter_by_search_and_sort_by_key() {
        let v = demo_view();
        let mut f = Filter::default();
        // Cheapest first by default.
        let idx = shown(&v.stock, &f, 0, false);
        assert_eq!(v.stock[idx[0]].name, "Apple");
        let idx = shown(&v.stock, &f, 0, true);
        assert_eq!(v.stock[idx[0]].name, "Leather Cap");
        // By armor level: the cap, and the ones without it drop to the end.
        let by_armor = SORTS.iter().position(|s| s.0 == "armor").unwrap();
        let idx = shown(&v.stock, &f, by_armor, true);
        assert_eq!(v.stock[idx[0]].name, "Leather Cap");
        // A stat search keeps only what matches.
        f.search = "dmg>3".into();
        let idx = shown(&v.stock, &f, 0, false);
        assert_eq!(idx.len(), 1);
        assert_eq!(v.stock[idx[0]].name, "Dagger");
        assert!(f.filtering());
        // And a kind chip.
        f.search.clear();
        f.kind = super::super::inventory::KINDS
            .iter()
            .position(|k| k.0 == "Armor")
            .unwrap();
        let idx = shown(&v.stock, &f, 0, false);
        assert_eq!(idx.len(), 1);
        assert_eq!(v.stock[idx[0]].name, "Leather Cap");
    }

    #[test]
    fn prices_round_the_way_ace_does() {
        // Vendors sell at a markup, rounded up, never below one pyreal.
        assert_eq!(buy_price(10, 1.0), 10);
        assert_eq!(buy_price(10, 1.25), 13);
        assert_eq!(buy_price(0, 1.5), 1);
        // And buy at a discount, rounded down, never below one pyreal.
        assert_eq!(sell_price(10, 0.5), 5);
        assert_eq!(sell_price(7, 0.5), 3);
        assert_eq!(sell_price(1, 0.1), 1);
    }
}
