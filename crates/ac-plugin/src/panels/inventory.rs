//! Inventory: what the character wears and carries, grouped by pack,
//! searchable and sortable. I toggles it.
//!
//! * The search line matches names, materials, kinds and spell names,
//!   and compares numbers: `dmg>10`, `al>=200`, `value<100`, `ws>=5`,
//!   `wield<=100`, `spell:blood`, `type:armor`, `mat:iron`, `skill:sword`,
//!   `wielded`, `unappraised` (see `ac_client::items::Query`). Numbers
//!   come from appraisals: a search that needs them appraises the
//!   unappraised items in the background, and "Appraise all" does it by
//!   hand.
//! * Kind chips narrow the list to weapons, armor, comps and so on; the
//!   sort box orders by name, value, burden, damage, armor...
//! * Hovering an item shows its stats; a click selects and appraises it;
//!   a double-click uses it (wield, take off, read, drink...); a
//!   right-click on a stack opens the split popup.
//! * Drag an item onto a pack header to move it into that pack (the
//!   "Pack" header is the main pack); onto a stack of the same kind to
//!   merge; onto another item to apply it (a salvage bag tinkers, a mana
//!   stone charges, a key unlocks). Packs themselves hang from the main
//!   pack: picking one up equips it in a free side slot.

use super::{caption, has_sheet, item_row, title, window, Item, ItemDrag, Source};
use crate::icons::{IconCache, IconLayers};
use crate::{egui, Client, Ctx, Plugin};
use ac_client::items::{self, ItemStats, NumKey, Query, SortKey};

/// One row of the panel: the drawable item and its numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub item: Item,
    pub stats: ItemStats,
}

/// A side pack: a header the list groups under and a drop target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pack {
    pub guid: u32,
    pub name: String,
    pub count: u32,
    pub capacity: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct InventoryView {
    pub rows: Vec<Row>,
    /// Our own guid: the main pack's container id.
    pub me: u32,
    /// Items directly in the main pack (packs excluded) and its capacity.
    pub main_count: u32,
    pub main_capacity: u32,
    pub packs: Vec<Pack>,
    /// Side-pack slots used and available.
    pub pack_slots: (u32, u32),
    /// Total burden carried, and the capacity Strength gives.
    pub burden: u32,
    pub burden_capacity: u32,
    /// Foci carried ("War", "Life"...), for casters at a glance.
    pub foci: Vec<String>,
    pub unappraised: usize,
}

/// The chips above the list: a label and the kinds it keeps.
pub const KINDS: &[(&str, &[&str])] = &[
    ("All", &[]),
    ("Weapons", &["weapon", "missile", "caster"]),
    ("Armor", &["armor"]),
    ("Clothing", &["clothing"]),
    ("Jewelry", &["jewelry"]),
    ("Comps", &["comps"]),
    ("Food", &["food"]),
    ("Gems", &["gem"]),
    ("Keys", &["key", "lockpick"]),
    ("Scrolls", &["scroll"]),
    ("Salvage", &["salvage"]),
    ("Packs", &["pack"]),
    (
        "Misc",
        &["misc", "money", "healer", "manastone", "portal", "trinket"],
    ),
];

/// The sort choices: a label and the key.
pub const SORTS: &[(&str, SortKey)] = &[
    ("name", SortKey::Name),
    ("value", SortKey::Num(NumKey::Value)),
    ("burden", SortKey::Num(NumKey::Burden)),
    ("damage", SortKey::Num(NumKey::Damage)),
    ("armor", SortKey::Num(NumKey::Armor)),
    ("workmanship", SortKey::Num(NumKey::Workmanship)),
    ("wield level", SortKey::Num(NumKey::Wield)),
    ("speed", SortKey::Num(NumKey::Speed)),
    ("stack", SortKey::Num(NumKey::Stack)),
];

/// "Foci of Strife" -> "War" and the other three schools.
pub fn focus_school(name: &str) -> Option<&'static str> {
    let school = name.strip_prefix("Foci of ")?;
    Some(match school {
        "Strife" => "War",
        "Verdancy" => "Life",
        "Enchantment" => "Creature",
        "Artifice" => "Item",
        _ => return None,
    })
}

/// Build the view from the session; `None` until the sheet arrived.
pub fn view(c: &Client) -> Option<InventoryView> {
    if !has_sheet(c) {
        return None;
    }
    let me = c.world.player_guid?;
    let mut rows: Vec<Row> = c
        .item_stats()
        .into_iter()
        .filter_map(|stats| {
            let o = c.world.objects.get(&stats.guid)?;
            Some(Row {
                item: Item::of(o, stats.wielded),
                stats,
            })
        })
        .collect();
    let packs: Vec<Pack> = rows
        .iter()
        .filter(|r| r.item.container && !r.item.wielded && r.stats.container == me)
        .map(|r| {
            let o = c.world.objects.get(&r.item.guid);
            Pack {
                guid: r.item.guid,
                name: r.item.name.clone(),
                count: rows
                    .iter()
                    .filter(|x| x.stats.container == r.item.guid)
                    .count() as u32,
                capacity: o.map(|o| o.items_capacity).unwrap_or(0),
            }
        })
        .collect();
    let player = c.world.player();
    let main_count = rows
        .iter()
        .filter(|r| r.stats.container == me && !r.item.container)
        .count() as u32;
    let burden: u32 = rows.iter().map(|r| r.stats.burden).sum();
    let strength = c.world.stats.attributes[0].value();
    let mut foci: Vec<String> = rows
        .iter()
        .filter_map(|r| focus_school(&r.item.name))
        .map(String::from)
        .collect();
    foci.sort();
    foci.dedup();
    let unappraised = rows.iter().filter(|r| !r.stats.appraised).count();
    rows.sort_by(|a, b| a.item.name.cmp(&b.item.name));
    Some(InventoryView {
        rows,
        me,
        main_count,
        main_capacity: player.map(|p| p.items_capacity).unwrap_or(0),
        pack_slots: (
            packs.len() as u32,
            player.map(|p| p.containers_capacity).unwrap_or(0),
        ),
        packs,
        burden,
        burden_capacity: strength * 150,
        foci,
        unappraised,
    })
}

/// What the player did in the panel this frame.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Actions {
    /// Double-clicked items (use: wield, take off, read...).
    pub activated: Vec<u32>,
    /// Items dragged onto a pack: (item, container guid; 0 = main pack).
    pub moves: Vec<(u32, u32)>,
    /// An item dragged onto another item: (item, target), used on it.
    pub apply: Vec<(u32, u32)>,
    /// A stack dragged onto a stack of the same kind: (from, to).
    pub merge: Vec<(u32, u32)>,
    /// Right-clicked stack: open the split popup for it.
    pub split_of: Option<u32>,
    /// Single-clicked items: select and appraise.
    pub inspect: Vec<u32>,
    /// Appraise every unappraised item in the background.
    pub appraise_all: bool,
}

/// The panel's own state: search line, chip, sort, folded packs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub search: String,
    /// Index into [`KINDS`] (0 = all) and [`SORTS`] (0 = name).
    pub kind: usize,
    pub sort: usize,
    pub descending: bool,
    pub folded: Vec<u32>,
    pub worn_folded: bool,
}

impl State {
    /// Whether anything narrows the list.
    pub fn filtering(&self) -> bool {
        !self.search.trim().is_empty() || self.kind != 0
    }
}

/// The rows the state keeps, in its order, as indices into `v.rows`.
pub fn shown(v: &InventoryView, st: &State) -> Vec<usize> {
    let q = Query::parse(&st.search);
    let kinds = KINDS.get(st.kind).map(|k| k.1).unwrap_or(&[]);
    let mut stats: Vec<(usize, ItemStats)> = v
        .rows
        .iter()
        .enumerate()
        .filter(|(_, r)| kinds.is_empty() || kinds.contains(&r.stats.kind))
        .filter(|(_, r)| r.stats.matches(&q))
        .map(|(i, r)| (i, r.stats.clone()))
        .collect();
    let key = SORTS.get(st.sort).map(|s| s.1).unwrap_or(SortKey::Name);
    let mut only: Vec<ItemStats> = stats.iter().map(|(_, s)| s.clone()).collect();
    items::sort(&mut only, key, st.descending);
    // Map the sorted stats back to row indices by guid.
    stats.sort_by_key(|(_, s)| only.iter().position(|o| o.guid == s.guid));
    stats.into_iter().map(|(i, _)| i).collect()
}

fn stat_suffix(s: &ItemStats, key: SortKey) -> String {
    match key {
        SortKey::Name => {
            if s.damage_high > 0 {
                format!("{}-{}", s.damage_low, s.damage_high)
            } else if s.armor_level > 0 {
                format!("AL {}", s.armor_level)
            } else {
                String::new()
            }
        }
        SortKey::Num(k) => match (k, s.number(k)) {
            (_, None) => String::new(),
            (NumKey::Damage, Some(_)) => format!("{}-{}", s.damage_low, s.damage_high),
            (NumKey::Workmanship, Some(n)) => format!("ws {n:.0}"),
            (NumKey::Value, Some(n)) => format!("{n:.0} py"),
            (NumKey::Burden, Some(n)) => format!("{n:.0} bu"),
            (_, Some(n)) => format!("{n:.0}"),
        },
    }
}

fn tooltip(ui: &mut egui::Ui, r: &Row) {
    ui.label(egui::RichText::new(&r.item.name).strong());
    for line in r.stats.summary() {
        ui.label(line);
    }
    if !r.stats.appraised {
        caption(ui, "click to appraise");
    }
}

/// One item row with its drop handling; returns nothing, fills `actions`.
fn draw_row(
    ui: &mut egui::Ui,
    icons: &mut IconCache,
    v: &InventoryView,
    r: &Row,
    key: SortKey,
    actions: &mut Actions,
) {
    let color = if r.item.wielded {
        egui::Color32::from_rgb(180, 230, 180)
    } else if r.item.container {
        egui::Color32::from_rgb(230, 210, 150)
    } else if !r.stats.appraised {
        egui::Color32::from_gray(215)
    } else {
        egui::Color32::WHITE
    };
    let suffix = stat_suffix(&r.stats, key);
    let (resp, dropped) = ui.dnd_drop_zone::<ItemDrag, _>(egui::Frame::new(), |ui| {
        ui.horizontal(|ui| {
            let resp = item_row(ui, icons, &r.item, color);
            if !suffix.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    caption(ui, suffix);
                });
            }
            resp
        })
        .inner
    });
    let row = resp.inner.on_hover_ui(|ui| tooltip(ui, r));
    if row.clicked() {
        actions.inspect.push(r.item.guid);
    }
    if row.double_clicked() {
        actions.activated.push(r.item.guid);
    }
    if row.secondary_clicked() && r.item.stack > 1 {
        actions.split_of = Some(r.item.guid);
    }
    if let Some(p) = dropped {
        if p.0 == r.item.guid {
            return;
        }
        if r.item.container && !r.item.wielded {
            actions.moves.push((p.0, r.item.guid));
            return;
        }
        let same_kind = r.item.max_stack > 1
            && v.rows
                .iter()
                .any(|o| o.item.guid == p.0 && o.item.wcid == r.item.wcid);
        if same_kind {
            actions.merge.push((p.0, r.item.guid));
        } else {
            actions.apply.push((p.0, r.item.guid));
        }
    }
}

/// A pack header that takes drops; returns whether it was clicked (fold).
fn pack_header(ui: &mut egui::Ui, text: String, container: u32, actions: &mut Actions) -> bool {
    let (r, _) = ui.dnd_drop_zone::<ItemDrag, _>(egui::Frame::new().inner_margin(2), |ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(text)
                    .color(egui::Color32::from_rgb(230, 210, 150))
                    .small(),
            )
            .sense(egui::Sense::click()),
        )
    });
    if let Some(p) = r.response.dnd_release_payload::<ItemDrag>() {
        if p.0 != container {
            actions.moves.push((p.0, container));
        }
    }
    r.inner.clicked()
}

/// Draw the panel.
pub fn draw(
    egui: &egui::Context,
    icons: &mut IconCache,
    v: &InventoryView,
    st: &mut State,
) -> Actions {
    let mut actions = Actions::default();
    let w = egui.viewport_rect().width();
    let r = super::radar::RADIUS;
    let key = SORTS.get(st.sort).map(|s| s.1).unwrap_or(SortKey::Name);
    let order = shown(v, st);
    window(
        "inventory",
        egui::pos2(w - 348.0, 2.0 * r + 40.0),
        egui::vec2(340.0, 440.0),
        170,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(328.0, 428.0));
        ui.horizontal(|ui| {
            title(ui, "Inventory");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let burden = if v.burden_capacity > 0 {
                    format!("{} / {} bu", v.burden, v.burden_capacity)
                } else {
                    format!("{} bu", v.burden)
                };
                caption(ui, burden);
            });
        });
        ui.horizontal(|ui| {
            caption(
                ui,
                format!(
                    "pack {}/{}  side packs {}/{}",
                    v.main_count, v.main_capacity, v.pack_slots.0, v.pack_slots.1
                ),
            );
            if !v.foci.is_empty() {
                caption(ui, format!("foci: {}", v.foci.join(", ")));
            }
        });
        ui.horizontal(|ui| {
            let edit = egui::TextEdit::singleline(&mut st.search)
                .hint_text("search: name, spell, dmg>10, al>=100, type:armor")
                .desired_width(250.0);
            let resp = ui.add(edit);
            if resp.changed() && Query::parse(&st.search).needs_appraisal() && v.unappraised > 0 {
                actions.appraise_all = true;
            }
            if ui
                .add_enabled(!st.search.is_empty(), egui::Button::new("x").small())
                .clicked()
            {
                st.search.clear();
            }
        });
        ui.horizontal_wrapped(|ui| {
            for (i, (label, _)) in KINDS.iter().enumerate() {
                if ui.selectable_label(st.kind == i, *label).clicked() {
                    st.kind = i;
                }
            }
        });
        ui.horizontal(|ui| {
            caption(ui, "sort");
            egui::ComboBox::from_id_salt("inventory_sort")
                .selected_text(SORTS[st.sort].0)
                .width(110.0)
                .show_ui(ui, |ui| {
                    for (i, (label, _)) in SORTS.iter().enumerate() {
                        if ui.selectable_label(st.sort == i, *label).clicked() {
                            st.sort = i;
                            if let SortKey::Num(k) = SORTS[i].1 {
                                if k.needs_appraisal() && v.unappraised > 0 {
                                    actions.appraise_all = true;
                                }
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
                    .on_hover_text("ask the server about every item's stats, one at a time")
                    .clicked()
            {
                actions.appraise_all = true;
            }
        });
        if st.filtering() {
            caption(ui, format!("showing {} of {}", order.len(), v.rows.len()));
        }
        egui::ScrollArea::vertical()
            .max_height(300.0)
            .show(ui, |ui| {
                ui.set_min_width(320.0);
                let rows_in = |pred: &dyn Fn(&Row) -> bool| -> Vec<usize> {
                    order
                        .iter()
                        .copied()
                        .filter(|i| pred(&v.rows[*i]))
                        .collect()
                };
                // Worn.
                let worn = rows_in(&|r| r.item.wielded);
                if !worn.is_empty() {
                    let fold = if st.worn_folded { "▸" } else { "▾" };
                    let head = ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("{fold} Worn ({})", worn.len()))
                                .color(egui::Color32::from_rgb(180, 230, 180))
                                .small(),
                        )
                        .sense(egui::Sense::click()),
                    );
                    if head.clicked() {
                        st.worn_folded = !st.worn_folded;
                    }
                    if !st.worn_folded {
                        for i in worn {
                            draw_row(ui, icons, v, &v.rows[i], key, &mut actions);
                        }
                    }
                }
                // Main pack: everything directly in it except the packs.
                let main = rows_in(&|r| r.stats.container == v.me && !r.item.container);
                if !main.is_empty() || !st.filtering() {
                    pack_header(
                        ui,
                        format!("Pack ({}/{})", v.main_count, v.main_capacity),
                        0,
                        &mut actions,
                    );
                    for i in main {
                        draw_row(ui, icons, v, &v.rows[i], key, &mut actions);
                    }
                }
                // Side packs with their contents.
                for p in &v.packs {
                    let inside = rows_in(&|r| r.stats.container == p.guid);
                    if inside.is_empty() && st.filtering() {
                        continue;
                    }
                    let folded = st.folded.contains(&p.guid);
                    let fold = if folded { "▸" } else { "▾" };
                    let clicked = pack_header(
                        ui,
                        format!("{fold} {} ({}/{})", p.name, p.count, p.capacity),
                        p.guid,
                        &mut actions,
                    );
                    if clicked {
                        if folded {
                            st.folded.retain(|g| *g != p.guid);
                        } else {
                            st.folded.push(p.guid);
                        }
                    }
                    if !folded {
                        ui.indent(p.guid, |ui| {
                            for i in inside {
                                draw_row(ui, icons, v, &v.rows[i], key, &mut actions);
                            }
                        });
                    }
                }
                // Anything held by a pack we do not see as ours (a pack
                // still arriving): list it so nothing is hidden.
                let stray = rows_in(&|r| {
                    !r.item.wielded
                        && r.stats.container != v.me
                        && !v.packs.iter().any(|p| p.guid == r.stats.container)
                });
                for i in stray {
                    draw_row(ui, icons, v, &v.rows[i], key, &mut actions);
                }
            });
    });
    actions
}

pub struct Inventory {
    source: Source<InventoryView>,
    /// Open (I toggles it). Starts open.
    pub show: bool,
    pub state: State,
    /// The split popup: (stack guid, amount chosen so far).
    split: Option<(u32, u32)>,
}

/// The split popup: a slider from 1 to stack - 1 and a Split button.
/// Returns Some(amount) when confirmed, and clears `split` on cancel.
pub fn draw_split(
    egui: &egui::Context,
    item: &Item,
    split: &mut Option<(u32, u32)>,
) -> Option<u32> {
    let mut result = None;
    let rect = egui.viewport_rect();
    let (_, amount) = split.as_mut()?;
    let mut open = true;
    egui::Window::new("split_stack")
        .title_bar(false)
        .resizable(false)
        .fade_in(false)
        .frame(super::frame(220, 10))
        .fixed_pos(egui::pos2(rect.width() * 0.5 - 140.0, rect.height() * 0.4))
        .show(egui, |ui| {
            title(ui, format!("Split {} ({})", item.name, item.stack));
            ui.add(egui::Slider::new(amount, 1..=item.stack.saturating_sub(1).max(1)).text("take"));
            ui.horizontal(|ui| {
                if ui.button("Split").clicked() {
                    result = Some(*amount);
                    open = false;
                }
                if ui.button("Cancel").clicked() {
                    open = false;
                }
            });
        });
    if !open {
        *split = None;
    }
    result
}

impl Default for Inventory {
    fn default() -> Self {
        Inventory {
            source: Source::Live,
            show: true,
            state: State::default(),
            split: None,
        }
    }
}

/// A sample item with a bare icon, for demos.
pub fn demo_item(guid: u32, name: &str, stack: u32, wielded: bool, icon: u32) -> Item {
    Item {
        guid,
        name: name.to_string(),
        stack,
        wielded,
        container: false,
        icon: IconLayers::single(icon),
        wcid: 0,
        max_stack: if stack > 1 { 100 } else { 1 },
    }
}

/// A demo row: the item plus made-up numbers.
pub fn demo_row(item: Item, container: u32, stats: ItemStats) -> Row {
    let mut stats = stats;
    stats.guid = item.guid;
    stats.name = item.name.clone();
    stats.stack = item.stack;
    stats.wielded = item.wielded;
    stats.container = container;
    Row { item, stats }
}

impl Inventory {
    /// Known 32x32 icons from the portal, with sample stats.
    pub fn demo() -> Self {
        let me = 0x5000_0001;
        let pack = 0x8000_0010;
        let mut pack_item = demo_item(pack, "Pack", 1, false, 0x0600_2F40);
        pack_item.container = true;
        let rows = vec![
            demo_row(
                demo_item(1, "Leather Tunic", 1, true, 0x0600_0FAA),
                0,
                ItemStats {
                    kind: "armor",
                    appraised: true,
                    armor_level: 120,
                    value: 300,
                    burden: 500,
                    ..Default::default()
                },
            ),
            demo_row(
                demo_item(2, "Fine Sword", 1, true, 0x0600_189E),
                0,
                ItemStats {
                    kind: "weapon",
                    appraised: true,
                    damage_low: 8,
                    damage_high: 14,
                    damage_type: "Slashing".into(),
                    speed: 40,
                    spells: vec!["Blood Drinker IV".into()],
                    value: 1200,
                    burden: 300,
                    ..Default::default()
                },
            ),
            demo_row(
                demo_item(3, "Healing Kit", 1, false, 0x0600_1A8A),
                me,
                ItemStats {
                    kind: "healer",
                    value: 90,
                    burden: 50,
                    ..Default::default()
                },
            ),
            demo_row(
                demo_item(4, "Pyreal", 12, false, 0x0600_1FB7),
                me,
                ItemStats {
                    kind: "money",
                    value: 12,
                    ..Default::default()
                },
            ),
            demo_row(
                demo_item(5, "Foci of Strife", 1, false, 0x0600_261A),
                me,
                ItemStats {
                    kind: "comps",
                    burden: 50,
                    ..Default::default()
                },
            ),
            demo_row(
                pack_item,
                me,
                ItemStats {
                    kind: "pack",
                    burden: 65,
                    ..Default::default()
                },
            ),
            demo_row(
                demo_item(7, "Prismatic Taper", 3, false, 0x0600_2C0D),
                pack,
                ItemStats {
                    kind: "comps",
                    value: 3,
                    burden: 3,
                    ..Default::default()
                },
            ),
            demo_row(
                demo_item(8, "Scroll of Strength Other I", 1, false, 0x0600_321E),
                pack,
                ItemStats {
                    kind: "scroll",
                    appraised: true,
                    spells: vec!["Strength Other I".into()],
                    value: 40,
                    burden: 10,
                    ..Default::default()
                },
            ),
        ];
        let burden = rows.iter().map(|r| r.stats.burden).sum();
        Inventory {
            source: Source::Demo(InventoryView {
                rows,
                me,
                main_count: 3,
                main_capacity: 102,
                packs: vec![Pack {
                    guid: pack,
                    name: "Pack".into(),
                    count: 2,
                    capacity: 24,
                }],
                pack_slots: (1, 7),
                burden,
                burden_capacity: 15_000,
                foci: vec!["War".into()],
                unappraised: 4,
            }),
            show: true,
            state: State::default(),
            split: None,
        }
    }
}

impl Plugin for Inventory {
    fn name(&self) -> &str {
        "inventory"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if !self.show {
            return;
        }
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().and_then(|c| view(c)),
        };
        let Some(v) = v else { return };
        let actions = draw(egui, cx.icons(), &v, &mut self.state);
        if let Some(g) = actions.split_of {
            let stack = v
                .rows
                .iter()
                .find(|r| r.item.guid == g)
                .map(|r| r.item.stack)
                .unwrap_or(0);
            if stack > 1 {
                self.split = Some((g, (stack / 2).max(1)));
            }
        }
        let mut split_now = None;
        if let Some((g, _)) = self.split {
            match v.rows.iter().find(|r| r.item.guid == g) {
                Some(r) => {
                    if let Some(n) = draw_split(egui, &r.item, &mut self.split) {
                        split_now = Some((g, n));
                    }
                }
                None => self.split = None,
            }
        }
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            if let Some((g, n)) = split_now {
                c.split_stack(g, None, n);
            }
            if actions.appraise_all {
                let n = c.appraise_all();
                if n > 0 {
                    tracing::info!("appraising {n} items");
                }
            }
            for g in actions.inspect {
                c.inspect(g);
            }
            for g in actions.activated {
                c.interact(g);
            }
            for (from, to) in actions.merge {
                c.merge_stacks(from, to, None);
            }
            for (item, target) in actions.apply {
                c.use_on(item, target);
            }
            for (item, container) in actions.moves {
                let container = if container == 0 {
                    c.world.player_guid.unwrap_or(0)
                } else {
                    container
                };
                c.put_in_container(item, container);
            }
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if key == egui::Key::I && pressed {
            self.show = !self.show;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_view() -> InventoryView {
        match Inventory::demo().source {
            Source::Demo(v) => v,
            Source::Live => unreachable!(),
        }
    }

    #[test]
    fn shown_filters_and_sorts() {
        let v = demo_view();
        let mut st = State::default();
        assert_eq!(shown(&v, &st).len(), v.rows.len());
        // The Weapons chip keeps the sword only.
        st.kind = KINDS.iter().position(|k| k.0 == "Weapons").unwrap();
        let idx = shown(&v, &st);
        assert_eq!(idx.len(), 1);
        assert_eq!(v.rows[idx[0]].item.name, "Fine Sword");
        // A stat query: armor level at least 100.
        st.kind = 0;
        st.search = "al>=100".into();
        let idx = shown(&v, &st);
        assert_eq!(idx.len(), 1);
        assert_eq!(v.rows[idx[0]].item.name, "Leather Tunic");
        // A spell word.
        st.search = "strength".into();
        let names: Vec<&str> = shown(&v, &st)
            .into_iter()
            .map(|i| v.rows[i].item.name.as_str())
            .collect();
        assert_eq!(names, ["Scroll of Strength Other I"]);
        // Sorting by value, highest first, unknown values last.
        st.search.clear();
        st.sort = SORTS.iter().position(|s| s.0 == "value").unwrap();
        st.descending = true;
        let idx = shown(&v, &st);
        assert_eq!(v.rows[idx[0]].item.name, "Fine Sword");
        assert_eq!(v.rows[idx[1]].item.name, "Leather Tunic");
        assert!(!st.filtering());
    }

    #[test]
    fn foci_and_suffixes() {
        assert_eq!(focus_school("Foci of Strife"), Some("War"));
        assert_eq!(focus_school("Foci of Artifice"), Some("Item"));
        assert_eq!(focus_school("Focus Stone"), None);
        let v = demo_view();
        let sword = &v
            .rows
            .iter()
            .find(|r| r.item.name == "Fine Sword")
            .unwrap()
            .stats;
        assert_eq!(stat_suffix(sword, SortKey::Name), "8-14");
        assert_eq!(stat_suffix(sword, SortKey::Num(NumKey::Value)), "1200 py");
        let kit = &v
            .rows
            .iter()
            .find(|r| r.item.name == "Healing Kit")
            .unwrap()
            .stats;
        assert_eq!(stat_suffix(kit, SortKey::Num(NumKey::Damage)), "");
    }
}
