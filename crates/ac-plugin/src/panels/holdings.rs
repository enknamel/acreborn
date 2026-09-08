//! Items across characters: one window that searches every character's
//! inventory on every account, in this process, in every other process
//! on the bus, and in the snapshots left behind by characters that are
//! not logged in, so "which character has a Hauberk with Epic Life
//! Magic Aptitude" is one search away.
//!
//! The plugin has two halves. As a **publisher** it watches every
//! session of this process: when a character's items change (added,
//! removed, moved, wielded, appraised) it takes a snapshot
//! (`Client::holdings_snapshot`), at most once every 2 s, and hands it
//! to the process-wide `ac_client::holdings::store`, writes it to
//! `<cache dir>/holdings/<account>/<character>.json`, and `set`s it on
//! the blackboard under `holdings.<account>/<character>`, so every other
//! process on the bus gets it now and on joining later. It says so
//! again every 30 s while the session lives, and once more with
//! `online: false` when the session ends. As a **reader** it merges
//! every `holdings.*` value the bus delivers into the store, newest
//! snapshot per character winning, and reads the snapshot files once at
//! start so offline characters are there from the first frame.
//!
//! The window ("Items", menu → Items (all characters); no key out of the
//! box, bindable in the menu; a button in the Fleet window) searches the
//! store with the inventory's own query language and lists the hits by
//! character, account, item, where it sits, value, spells and when the
//! snapshot was taken. Refresh publishes this process's snapshots again
//! and posts `holdings.request`, on which every session appraises what
//! it has not yet and publishes once the answers are in.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::{caption, title_bar, window, Source};
use crate::{egui, Ctx, Event, Plugin, Settings};
use ac_client::holdings::{
    self, unix_now, CharacterHoldings, Hit, HoldingRecord, HoldingsStore, BUS_PREFIX, REQUEST_TOPIC,
};
use ac_client::items::{ItemStats, Query};

/// A changed inventory is published at most this often.
pub const PUBLISH_EVERY: Duration = Duration::from_secs(2);
/// An unchanged one is said again this often, so the snapshot's
/// `taken_at` says the session lives (see `holdings::ONLINE_FOR`).
pub const HEARTBEAT: Duration = Duration::from_secs(30);
/// How often the bus values are looked through for new snapshots.
pub const MERGE_EVERY: Duration = Duration::from_millis(500);
/// Refresh, and answering a `holdings.request`, at most this often.
pub const REFRESH_EVERY: Duration = Duration::from_secs(10);

/// The window's action id (its key and open requests) and window name.
pub const ID: &str = "holdings";

/// The columns, in order; each sorts the table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Column {
    Character,
    Account,
    #[default]
    Item,
    Place,
    Value,
    Spells,
    Updated,
}

impl Column {
    pub const ALL: [Column; 7] = [
        Column::Character,
        Column::Account,
        Column::Item,
        Column::Place,
        Column::Value,
        Column::Spells,
        Column::Updated,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Column::Character => "Character",
            Column::Account => "Account",
            Column::Item => "Item",
            Column::Place => "Where",
            Column::Value => "Value",
            Column::Spells => "Spells",
            Column::Updated => "Updated",
        }
    }

    /// The width the column is drawn at.
    fn width(self) -> f32 {
        match self {
            Column::Character => 120.0,
            Column::Account => 90.0,
            Column::Item => 220.0,
            Column::Place => 90.0,
            Column::Value => 64.0,
            Column::Spells => 260.0,
            Column::Updated => 80.0,
        }
    }

    fn cmp(self, a: &Hit, b: &Hit) -> std::cmp::Ordering {
        let lower = |s: &str| s.to_lowercase();
        match self {
            Column::Character => lower(&a.character).cmp(&lower(&b.character)),
            Column::Account => lower(&a.account).cmp(&lower(&b.account)),
            Column::Item => lower(&a.stats.name).cmp(&lower(&b.stats.name)),
            Column::Place => lower(&a.place).cmp(&lower(&b.place)),
            Column::Value => a.stats.value.cmp(&b.stats.value),
            Column::Spells => {
                lower(&a.stats.spells.join(", ")).cmp(&lower(&b.stats.spells.join(", ")))
            }
            // Online first, then the freshest.
            Column::Updated => b.online.cmp(&a.online).then(b.taken_at.cmp(&a.taken_at)),
        }
    }
}

/// Sort hits by a column; ties by character, then item name.
pub fn sort_hits(hits: &mut [Hit], by: Column, descending: bool) {
    hits.sort_by(|a, b| {
        let o = by.cmp(a, b);
        let o = if descending { o.reverse() } else { o };
        o.then_with(|| a.character.cmp(&b.character))
            .then_with(|| a.stats.name.cmp(&b.stats.name))
    });
}

/// `taken_at` as "online", "just now", "5m ago", "2d ago".
pub fn ago(online: bool, taken_at: u64, now: u64) -> String {
    if online {
        return "online".into();
    }
    let s = now.saturating_sub(taken_at);
    if s < 60 {
        "just now".into()
    } else if s < 3600 {
        format!("{}m ago", s / 60)
    } else if s < 86_400 {
        format!("{}h ago", s / 3600)
    } else {
        format!("{}d ago", s / 86_400)
    }
}

/// The search language, for the help popover.
pub const HELP: &[(&str, &str)] = &[
    (
        "words",
        "match the name, material, kind, a spell or a slot; \"epic life magic\" in quotes is one phrase",
    ),
    (
        "spell:blood  type:armor  mat:iron  skill:sword  slot:ring  tier:epic",
        "one field; slots: head, chest, hands, feet, neck, wrist, ring, trinket, cloak, wand, shield...",
    ),
    ("wielded  unappraised", "worn right now; not yet appraised"),
    (
        "dmg  al  value  burden  ws  speed  wield  mana  sc  uses  tinks  stack  atk  def",
        "numbers, with < <= > >= =: dmg>10, al>=200, value<100, ws>=8",
    ),
    (
        "spells  cantrips  minors  moderates  majors  epics  legendaries",
        "how many the item carries: epics>=2, cantrips>0",
    ),
    (
        "or  not  ( )",
        "a space is and; \"ring or bracelet\", \"not bonded\" (or -bonded), \"(sword or axe) dmg>20\"",
    ),
];

/// The panel's own state; the search is not kept across restarts.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct State {
    #[serde(skip)]
    pub search: String,
    pub sort: Column,
    pub descending: bool,
    /// The row shown in full: (account, character, item guid).
    #[serde(skip)]
    pub selected: Option<(String, String, u32)>,
}

/// What the window draws.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HoldingsView {
    pub rows: Vec<Hit>,
    pub characters: usize,
    pub items: usize,
    pub unappraised: usize,
    /// Unappraised items on characters that are online, which Refresh
    /// can do something about.
    pub online_unappraised: usize,
    /// The search compares numbers or spells only appraisal brings.
    pub needs_appraisal: bool,
    /// What is wrong with the search line, if anything.
    pub problem: Option<String>,
    /// Unix seconds when the view was built.
    pub now: u64,
    pub on_bus: bool,
    /// Seconds until Refresh may be pressed again, 0 when it may.
    pub refresh_in: f32,
}

/// Build the view from a store.
pub fn view(store: &HoldingsStore, st: &State, on_bus: bool, refresh_in: f32) -> HoldingsView {
    let now = unix_now();
    let q = Query::parse(&st.search);
    let mut rows = store.search(&q, now);
    sort_hits(&mut rows, st.sort, st.descending);
    let (characters, items, unappraised) = store.counts();
    let online_unappraised = store
        .iter()
        .filter(|h| h.online_at(now))
        .map(|h| h.unappraised())
        .sum();
    HoldingsView {
        rows,
        characters,
        items,
        unappraised,
        online_unappraised,
        needs_appraisal: q.needs_appraisal(),
        problem: Query::check(&st.search).err(),
        now,
        on_bus,
        refresh_in,
    }
}

/// What the player did in the window this frame.
#[derive(Debug, Default, PartialEq)]
pub struct Actions {
    pub refresh: bool,
    /// A column header was clicked.
    pub sort: Option<Column>,
    /// A row was clicked: (account, character, item guid).
    pub select: Option<(String, String, u32)>,
}

/// A cell of fixed width, cut with an ellipsis, the whole on hover.
fn cell(ui: &mut egui::Ui, width: f32, text: egui::RichText, hover: &str) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 16.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            let r = ui.add(
                egui::Label::new(text)
                    .truncate()
                    .sense(egui::Sense::click()),
            );
            if hover.is_empty() {
                r
            } else {
                r.on_hover_text(hover)
            }
        },
    )
    .inner
}

fn header_row(ui: &mut egui::Ui, st: &State, a: &mut Actions) {
    for col in Column::ALL {
        let mark = if st.sort == col {
            if st.descending {
                " ▾"
            } else {
                " ▴"
            }
        } else {
            ""
        };
        let text = egui::RichText::new(format!("{}{mark}", col.label()))
            .color(egui::Color32::from_gray(170))
            .small();
        if cell(ui, col.width(), text, "Click to sort").clicked() {
            a.sort = Some(col);
        }
    }
    ui.end_row();
}

fn row(ui: &mut egui::Ui, v: &HoldingsView, hit: &Hit, selected: bool, a: &mut Actions) {
    let who = if hit.online {
        egui::Color32::from_rgb(180, 230, 180)
    } else {
        egui::Color32::from_gray(190)
    };
    let item_color = if selected {
        egui::Color32::from_rgb(255, 220, 120)
    } else if !hit.stats.appraised {
        egui::Color32::from_gray(215)
    } else {
        egui::Color32::WHITE
    };
    let name = super::item_label(&hit.stats.name, hit.stats.stack);
    let spells = hit.stats.spells.join(", ");
    let updated = ago(hit.online, hit.taken_at, v.now);
    let mut clicked = false;
    clicked |= cell(
        ui,
        Column::Character.width(),
        egui::RichText::new(&hit.character).color(who),
        "",
    )
    .clicked();
    clicked |= cell(
        ui,
        Column::Account.width(),
        egui::RichText::new(&hit.account).small(),
        "",
    )
    .clicked();
    let item = cell(
        ui,
        Column::Item.width(),
        egui::RichText::new(&name).color(item_color),
        "",
    )
    .on_hover_ui(|ui| super::stats_tooltip(ui, &name, &hit.stats));
    clicked |= item.clicked();
    clicked |= cell(
        ui,
        Column::Place.width(),
        egui::RichText::new(&hit.place).small(),
        "",
    )
    .clicked();
    let value = if hit.stats.value > 0 {
        format!("{} py", hit.stats.value)
    } else {
        String::new()
    };
    clicked |= cell(
        ui,
        Column::Value.width(),
        egui::RichText::new(value).small(),
        "",
    )
    .clicked();
    clicked |= cell(
        ui,
        Column::Spells.width(),
        egui::RichText::new(&spells).small(),
        &spells,
    )
    .clicked();
    clicked |= cell(
        ui,
        Column::Updated.width(),
        egui::RichText::new(updated).small().color(who),
        "",
    )
    .clicked();
    ui.end_row();
    if clicked {
        a.select = Some((hit.account.clone(), hit.character.clone(), hit.stats.guid));
    }
}

/// The selected item in full, under the table.
fn detail(ui: &mut egui::Ui, hit: &Hit) {
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&hit.stats.name).strong());
        caption(
            ui,
            format!(
                "{} on {} ({}), {}",
                hit.place,
                hit.character,
                hit.account,
                ago(hit.online, hit.taken_at, unix_now())
            ),
        );
    });
    for line in hit.stats.summary() {
        ui.label(egui::RichText::new(line).small());
    }
}

/// Draw the window.
pub fn draw(egui: &egui::Context, v: &HoldingsView, st: &mut State) -> Actions {
    let mut a = Actions::default();
    let w = egui.viewport_rect().width();
    let h = egui.viewport_rect().height();
    let width = 980.0_f32.min(w - 16.0);
    let height = 560.0_f32.min(h - 16.0);
    window(
        ID,
        egui::pos2((w - width) * 0.5, 30.0),
        egui::vec2(width, height),
        245,
        8,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(width - 16.0, height - 16.0));
        title_bar(ui, ID, "Items");
        ui.horizontal(|ui| {
            let edit = egui::TextEdit::singleline(&mut st.search)
                .id_salt("holdings.search")
                .hint_text("search every character: hauberk \"epic life\", slot:ring epics>=2, al>=300 or dmg>40")
                .desired_width(width - 240.0);
            ui.add(edit);
            if ui
                .add_enabled(!st.search.is_empty(), egui::Button::new("x").small())
                .clicked()
            {
                st.search.clear();
            }
            ui.add(egui::Label::new(egui::RichText::new("?").strong()).sense(egui::Sense::hover()))
                .on_hover_ui(|ui| {
                    ui.set_max_width(520.0);
                    ui.label(egui::RichText::new("Search language").strong());
                    for (keys, what) in HELP {
                        ui.horizontal_wrapped(|ui| {
                            ui.monospace(*keys);
                        });
                        ui.label(egui::RichText::new(*what).small());
                    }
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if v.refresh_in > 0.0 {
                    format!("Refresh ({:.0}s)", v.refresh_in.ceil())
                } else {
                    "Refresh".to_string()
                };
                if ui
                    .add_enabled(v.refresh_in <= 0.0, egui::Button::new(label).small())
                    .on_hover_text(
                        "Publish this process's items again and ask every online character to appraise what it has not",
                    )
                    .clicked()
                {
                    a.refresh = true;
                }
            });
        });
        ui.horizontal(|ui| {
            caption(
                ui,
                format!(
                    "{} character{}, {} item{}, {} not appraised",
                    v.characters,
                    if v.characters == 1 { "" } else { "s" },
                    v.items,
                    if v.items == 1 { "" } else { "s" },
                    v.unappraised,
                ),
            );
            if !st.search.trim().is_empty() {
                caption(ui, format!("· {} shown", v.rows.len()));
            }
            if !v.on_bus {
                caption(ui, "· this process and the files only (no --bus)");
            }
        });
        if let Some(p) = &v.problem {
            ui.colored_label(egui::Color32::from_rgb(255, 160, 120), p);
        } else if v.needs_appraisal && v.unappraised > 0 {
            let hint = if v.online_unappraised > 0 {
                format!(
                    "The search needs appraised items; {} are not, {} of them on characters online (Refresh asks them to appraise).",
                    v.unappraised, v.online_unappraised
                )
            } else {
                format!(
                    "The search needs appraised items; {} are not, none of them on a character online, so they cannot match until they are.",
                    v.unappraised
                )
            };
            ui.colored_label(egui::Color32::from_rgb(255, 220, 120), hint);
        }
        let selected = st.selected.clone();
        let detail_h = if selected.is_some() { 120.0 } else { 0.0 };
        egui::ScrollArea::both()
            .max_height(height - 110.0 - detail_h)
            .show(ui, |ui| {
                egui::Grid::new("holdings.rows")
                    .striped(true)
                    .spacing([8.0, 3.0])
                    .min_col_width(20.0)
                    .show(ui, |ui| {
                        header_row(ui, st, &mut a);
                        if v.rows.is_empty() {
                            ui.weak(if v.items == 0 {
                                "No items known yet: a character has to be in the world once."
                            } else {
                                "Nothing matches."
                            });
                            ui.end_row();
                        }
                        for hit in &v.rows {
                            let is_selected = selected.as_ref().is_some_and(|(acc, ch, g)| {
                                acc.eq_ignore_ascii_case(&hit.account)
                                    && ch == &hit.character
                                    && *g == hit.stats.guid
                            });
                            row(ui, v, hit, is_selected, &mut a);
                        }
                    });
            });
        if let Some((acc, ch, g)) = &selected {
            if let Some(hit) = v.rows.iter().find(|h| {
                acc.eq_ignore_ascii_case(&h.account) && ch == &h.character && *g == h.stats.guid
            }) {
                detail(ui, hit);
            }
        }
    });
    a
}

/// What a session last published.
#[derive(Debug, Clone)]
struct Published {
    fingerprint: u64,
    at: Instant,
    account: String,
    character: String,
}

/// The plugin: publisher for this process's sessions, reader of the bus,
/// and the window.
pub struct Holdings {
    source: Source<HoldingsStore>,
    pub show: bool,
    pub state: State,
    /// The snapshot files were read into the store.
    loaded: bool,
    last_merge: Option<Instant>,
    /// Bus keys merged and the `taken_at` seen, so a value is parsed
    /// once.
    seen: BTreeMap<String, u64>,
    /// Per session: what it last said.
    published: BTreeMap<usize, Published>,
    /// Characters whose session ended, to be said offline next tick.
    pending_offline: Vec<(String, String)>,
    /// Every session that last published before this publishes again
    /// (Refresh, or an appraisal request).
    force_at: Option<Instant>,
    /// Where the snapshot files live; `None` is the default cache
    /// directory.
    dir: Option<std::path::PathBuf>,
    last_refresh: Option<Instant>,
    last_request: Option<Instant>,
}

impl Default for Holdings {
    fn default() -> Self {
        Holdings {
            source: Source::Live,
            show: false,
            state: State::default(),
            loaded: false,
            last_merge: None,
            seen: BTreeMap::new(),
            published: BTreeMap::new(),
            pending_offline: Vec::new(),
            force_at: None,
            dir: None,
            last_refresh: None,
            last_request: None,
        }
    }
}

impl Holdings {
    /// Three sample characters on two accounts, one of them offline.
    pub fn demo() -> Self {
        let now = unix_now();
        let record = |guid: u32, stats: ItemStats| {
            let mut s = stats;
            s.guid = guid;
            HoldingRecord::from(&s)
        };
        let mut store = HoldingsStore::new();
        store.put(CharacterHoldings {
            account: "fleetbot1".into(),
            character: "Fleetbot One".into(),
            guid: 0x5000_0001,
            taken_at: now,
            online: true,
            items: vec![
                record(
                    0x8000_0001,
                    ItemStats {
                        name: "Chainmail Hauberk".into(),
                        kind: "armor",
                        wielded: true,
                        appraised: true,
                        armor_level: 320,
                        value: 4200,
                        burden: 900,
                        material: "Steel",
                        workmanship: 8.0,
                        spells: vec![
                            "Epic Life Magic Aptitude".into(),
                            "Major Impregnability".into(),
                        ],
                        ..Default::default()
                    },
                ),
                record(
                    0x8000_0002,
                    ItemStats {
                        name: "Prismatic Taper".into(),
                        kind: "comps",
                        stack: 120,
                        container: 0x5000_0001,
                        value: 120,
                        burden: 120,
                        ..Default::default()
                    },
                ),
                record(
                    0x8000_0003,
                    ItemStats {
                        name: "Mystery Wand".into(),
                        kind: "caster",
                        container: 0x5000_0001,
                        value: 50,
                        burden: 20,
                        ..Default::default()
                    },
                ),
            ],
        });
        store.put(CharacterHoldings {
            account: "fleetbot1".into(),
            character: "Fleetbot Two".into(),
            guid: 0x5000_0002,
            taken_at: now - 3 * 86_400,
            online: false,
            items: vec![
                record(
                    0x8000_0011,
                    ItemStats {
                        name: "Pack".into(),
                        kind: "pack",
                        container: 0x5000_0002,
                        burden: 65,
                        ..Default::default()
                    },
                ),
                record(
                    0x8000_0012,
                    ItemStats {
                        name: "Gold Ring".into(),
                        kind: "jewelry",
                        container: 0x8000_0011,
                        appraised: true,
                        value: 9000,
                        burden: 10,
                        valid_locations: ac_client::items::slot::FINGER,
                        spells: vec![
                            "Epic Strength".into(),
                            "Epic Endurance".into(),
                            "Major Coordination".into(),
                        ],
                        ..Default::default()
                    },
                ),
            ],
        });
        store.put(CharacterHoldings {
            account: "academy".into(),
            character: "Academy Bot One".into(),
            guid: 0x5000_0003,
            taken_at: now - 1800,
            online: false,
            items: vec![record(
                0x8000_0021,
                ItemStats {
                    name: "Fine Sword".into(),
                    kind: "weapon",
                    wielded: true,
                    appraised: true,
                    damage_low: 8,
                    damage_high: 14,
                    damage_type: "Slashing".into(),
                    speed: 40,
                    value: 1200,
                    burden: 300,
                    spells: vec!["Blood Drinker IV".into()],
                    ..Default::default()
                },
            )],
        });
        Holdings {
            source: Source::Demo(store),
            show: true,
            ..Default::default()
        }
    }

    /// A plugin whose snapshot files live under `dir` (tests).
    pub fn with_dir(dir: std::path::PathBuf) -> Self {
        Holdings {
            dir: Some(dir),
            ..Default::default()
        }
    }

    /// Whether Refresh may be pressed, as seconds still to wait.
    fn refresh_in(&self, now: Instant) -> f32 {
        match self.last_refresh {
            Some(t) => (REFRESH_EVERY.saturating_sub(now.duration_since(t))).as_secs_f32(),
            None => 0.0,
        }
    }

    /// Put a snapshot in the store, the file and on the blackboard (the
    /// bus with it).
    fn publish(cx: &mut Ctx, h: CharacterHoldings) {
        let key = h.bus_key();
        let value = match serde_json::to_value(&h) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("holdings: cannot serialize {key}: {e}");
                return;
            }
        };
        {
            let store = holdings::store();
            if let Err(e) = store.save(&h) {
                tracing::warn!("holdings: cannot write {}: {e}", h.character);
            }
            drop(store);
        }
        holdings::store().put(h);
        cx.board.set(key, value);
    }

    /// Say a character has gone offline: its last snapshot, marked so.
    fn say_offline(cx: &mut Ctx, account: &str, character: &str) {
        let h = holdings::store().set_offline(account, character);
        if let Some(h) = h {
            Self::publish(cx, h);
        }
    }

    /// Once per frame (on session 0's tick): the files at first, then
    /// what the bus delivered, farewells, and appraisal requests.
    fn housekeeping(&mut self, cx: &mut Ctx) {
        let now = cx.now;
        if !self.loaded {
            self.loaded = true;
            let dir = self.dir.clone().unwrap_or_else(HoldingsStore::default_dir);
            let mut store = holdings::store();
            store.set_dir(Some(dir.clone()));
            let n = store.load_dir(&dir);
            tracing::info!(dir = %dir.display(), characters = n, "holdings loaded");
        }
        if self
            .last_merge
            .is_none_or(|t| now.duration_since(t) >= MERGE_EVERY)
        {
            self.last_merge = Some(now);
            let fresh: Vec<(String, serde_json::Value)> = cx
                .board
                .values
                .iter()
                .filter(|(k, _)| k.starts_with(BUS_PREFIX))
                .filter(|(k, v)| {
                    let taken = v.get("taken_at").and_then(|t| t.as_u64()).unwrap_or(0);
                    self.seen.get(*k).is_none_or(|s| taken > *s)
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if !fresh.is_empty() {
                let mut store = holdings::store();
                for (k, v) in fresh {
                    let taken = v.get("taken_at").and_then(|t| t.as_u64()).unwrap_or(0);
                    self.seen.insert(k, taken);
                    store.merge_value(&v);
                }
            }
        }
        for (account, character) in std::mem::take(&mut self.pending_offline) {
            Self::say_offline(cx, &account, &character);
        }
        // Appraisal requests, from this process's window or another's.
        let asked: Vec<serde_json::Value> = cx
            .board
            .messages_on(REQUEST_TOPIC)
            .map(|m| m.value.clone())
            .collect();
        if !asked.is_empty()
            && self
                .last_request
                .is_none_or(|t| now.duration_since(t) >= REFRESH_EVERY)
        {
            self.last_request = Some(now);
            let wants = |v: &serde_json::Value, account: &str, character: &str| {
                let acc = v.get("account").and_then(|a| a.as_str());
                let ch = v.get("character").and_then(|c| c.as_str());
                match (acc, ch) {
                    (None, None) => true,
                    (a, c) => {
                        a.is_none_or(|a| a.eq_ignore_ascii_case(account))
                            && c.is_none_or(|c| c == character)
                    }
                }
            };
            let mut queued = 0;
            for c in cx.clients.iter_mut() {
                let account = c.config.account.clone();
                let character = c.world.stats.name.clone();
                if character.is_empty() || !asked.iter().any(|v| wants(v, &account, &character)) {
                    continue;
                }
                queued += c.appraise_all();
            }
            if queued > 0 {
                tracing::info!(queued, "holdings: appraising on request");
            }
            self.force_at = Some(now);
        }
    }

    /// This session's turn: publish when its items changed, or to keep
    /// the heartbeat.
    fn publish_session(&mut self, cx: &mut Ctx) {
        let now = cx.now;
        let i = cx.index;
        let force_at = self.force_at;
        let Some(client) = cx.try_client() else {
            return;
        };
        if client.world.stats.name.is_empty() {
            return;
        }
        let fingerprint = client.holdings_fingerprint();
        let account = client.config.account.clone();
        let character = client.world.stats.name.clone();
        let due = match self.published.get(&i) {
            None => true,
            Some(p) => {
                let since = now.duration_since(p.at);
                let forced = force_at.is_some_and(|f| p.at < f);
                (p.character != character || p.account != account)
                    || ((forced || fingerprint != p.fingerprint) && since >= PUBLISH_EVERY)
                    || since >= HEARTBEAT
            }
        };
        if !due {
            return;
        }
        let Some(snapshot) = client.holdings_snapshot(true) else {
            return;
        };
        // A relog as someone else: the one before has gone.
        if let Some(p) = self.published.get(&i) {
            if p.character != character || p.account != account {
                self.pending_offline
                    .push((p.account.clone(), p.character.clone()));
            }
        }
        self.published.insert(
            i,
            Published {
                fingerprint,
                at: now,
                account,
                character,
            },
        );
        Self::publish(cx, snapshot);
    }
}

impl Plugin for Holdings {
    fn name(&self) -> &str {
        ID
    }

    fn load(&mut self, settings: &Settings) {
        if let Some(v) = settings.get("holdings.show") {
            self.show = v;
        }
        if let Some(v) = settings.get::<State>("holdings.state") {
            self.state = v;
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("holdings.show", self.show);
        settings.set("holdings.state", &self.state);
    }

    fn session_removed(&mut self, index: usize) {
        if let Some(p) = self.published.get(&index) {
            self.pending_offline
                .push((p.account.clone(), p.character.clone()));
        }
        crate::shift_removed(&mut self.published, index);
    }

    fn on_event(&mut self, cx: &mut Ctx, ev: &Event) {
        if !matches!(self.source, Source::Live) {
            return;
        }
        if matches!(ev, Event::Terminated(_) | Event::Refused(_)) {
            if let Some(p) = self.published.remove(&cx.index) {
                self.pending_offline.push((p.account, p.character));
            }
        }
    }

    fn tick(&mut self, cx: &mut Ctx) {
        if !matches!(self.source, Source::Live) {
            return;
        }
        if cx.index == 0 {
            self.housekeeping(cx);
        }
        self.publish_session(cx);
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if let Some(ask) = super::take_open(cx.board, ID) {
            self.show = ask.apply(self.show);
        }
        if !self.show {
            return;
        }
        let refresh_in = self.refresh_in(cx.now);
        let on_bus = cx.board.bus_name().is_some();
        let v = match &self.source {
            Source::Demo(store) => view(store, &self.state, true, refresh_in),
            Source::Live => {
                let store = holdings::store();
                view(&store, &self.state, on_bus, refresh_in)
            }
        };
        let a = draw(egui, &v, &mut self.state);
        if super::closed(ID) {
            self.show = false;
        }
        if let Some(col) = a.sort {
            if self.state.sort == col {
                self.state.descending = !self.state.descending;
            } else {
                self.state.sort = col;
                self.state.descending = false;
            }
        }
        if let Some(sel) = a.select {
            self.state.selected = if self.state.selected.as_ref() == Some(&sel) {
                None
            } else {
                Some(sel)
            };
        }
        if a.refresh && matches!(self.source, Source::Live) && refresh_in <= 0.0 {
            self.last_refresh = Some(cx.now);
            self.force_at = Some(cx.now);
            cx.post(REQUEST_TOPIC, serde_json::json!({ "all": true }));
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if pressed && crate::keys::bound(ID, key) {
            self.show = !self.show;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_store() -> HoldingsStore {
        match Holdings::demo().source {
            Source::Demo(s) => s,
            Source::Live => unreachable!(),
        }
    }

    #[test]
    fn the_view_searches_sorts_and_counts() {
        let store = demo_store();
        let mut st = State::default();
        let v = view(&store, &st, true, 0.0);
        assert_eq!((v.characters, v.items, v.unappraised), (3, 6, 3));
        assert_eq!(v.online_unappraised, 2);
        assert_eq!(v.rows.len(), 6);
        assert!(!v.needs_appraisal);
        assert!(v.problem.is_none());
        // Sorted by item name out of the box.
        assert_eq!(v.rows[0].stats.name, "Chainmail Hauberk");
        // An epic on a ring, wherever it is: the offline character's pack.
        st.search = "slot:ring epics>=2".into();
        let v = view(&store, &st, true, 0.0);
        assert!(v.needs_appraisal);
        assert_eq!(v.rows.len(), 1);
        assert_eq!(v.rows[0].character, "Fleetbot Two");
        assert_eq!(v.rows[0].place, "Pack");
        assert!(!v.rows[0].online);
        // Worn things, by value, highest first.
        st.search = "wielded".into();
        st.sort = Column::Value;
        st.descending = true;
        let v = view(&store, &st, true, 0.0);
        let names: Vec<&str> = v.rows.iter().map(|r| r.stats.name.as_str()).collect();
        assert_eq!(names, ["Chainmail Hauberk", "Fine Sword"]);
        assert_eq!(v.rows[0].place, "worn");
        assert!(v.rows[0].online);
        // Online first under Updated.
        st.search.clear();
        st.sort = Column::Updated;
        st.descending = false;
        let v = view(&store, &st, true, 0.0);
        assert!(v.rows[..3].iter().all(|r| r.online));
        assert_eq!(
            v.rows[3].character, "Academy Bot One",
            "the fresher offline one next"
        );
        // A broken line is reported, not crashed on.
        st.search = "(sword".into();
        let v = view(&store, &st, true, 0.0);
        assert!(v.problem.is_some());
    }

    #[test]
    fn bus_values_reach_the_store_and_files_are_read_at_start() {
        let dir =
            std::env::temp_dir().join(format!("acswarm-holdings-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let now = unix_now();
        // A file left by an earlier run: an offline character.
        let filed = CharacterHoldings {
            account: "old".into(),
            character: "Filed One".into(),
            guid: 1,
            taken_at: now - 600,
            online: false,
            items: vec![HoldingRecord {
                guid: 0x8000_0101,
                name: "Filed Dagger".into(),
                ..Default::default()
            }],
        };
        HoldingsStore::save_to(&dir, &filed).unwrap();
        let mut host = crate::Host::new();
        host.register(Box::new(Holdings::with_dir(dir.clone())));
        // A value another process set, as the bus would deliver it.
        let heard = CharacterHoldings {
            account: "Remote".into(),
            character: "Heard One".into(),
            guid: 2,
            taken_at: now,
            online: true,
            items: vec![HoldingRecord {
                guid: 0x8000_0202,
                name: "Heard Sword".into(),
                spells: vec!["Epic Strength".into()],
                appraised: true,
                ..Default::default()
            }],
        };
        host.board
            .set(heard.bus_key(), serde_json::to_value(&heard).unwrap());
        // A request with nobody to appraise is harmless.
        host.board.set_local("unrelated", 1);
        host.board
            .post(0, REQUEST_TOPIC, serde_json::json!({"all": true}));
        host.end_frame();
        host.frame(Vec::new(), 0, &[], 0.05, Instant::now());
        let store = holdings::store();
        assert!(store.get("old", "Filed One").is_some(), "the file was read");
        let h = store
            .get("remote", "Heard One")
            .expect("the bus value was merged");
        assert!(h.online_at(now));
        let hits = store.search(&Query::parse("epic"), now);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].character, "Heard One");
        assert_eq!(store.dir(), Some(dir.as_path()));
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ages_read_well() {
        let now = 1_000_000;
        assert_eq!(ago(true, now - 5000, now), "online");
        assert_eq!(ago(false, now - 5, now), "just now");
        assert_eq!(ago(false, now - 300, now), "5m ago");
        assert_eq!(ago(false, now - 7200, now), "2h ago");
        assert_eq!(ago(false, now - 3 * 86_400, now), "3d ago");
    }

    #[test]
    fn state_keeps_the_sort_but_not_the_search() {
        let st = State {
            search: "sword".into(),
            sort: Column::Value,
            descending: true,
            selected: Some(("a".into(), "b".into(), 1)),
        };
        let json = serde_json::to_value(&st).unwrap();
        let back: State = serde_json::from_value(json).unwrap();
        assert_eq!(back.sort, Column::Value);
        assert!(back.descending);
        assert!(back.search.is_empty());
        assert!(back.selected.is_none());
        // An older settings file without the state parses too.
        let old: State = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(old.sort, Column::Item);
    }
}
