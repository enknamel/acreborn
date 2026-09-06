//! Playing on its own (J): the rules the character follows when nobody
//! is at the keyboard, and a line saying what it is doing right now.
//!
//! The rules themselves live in `ac_client::autoplay`; this panel only
//! edits the [`Config`] on the session and reads back `doing` and
//! `status`. Four sections, in the order the engine considers them:
//!
//! * **Stay alive**: the health fractions at which it heals and at which
//!   it breaks off, whether it may spend a healing kit, and the spell it
//!   heals with.
//! * **Buffs**: the spells to keep up, how close to running out they may
//!   get, and whether to bother mid-fight.
//! * **Fight**: how far it looks for something to attack, and the names
//!   it will and will not take on.
//! * **Loot**: searches in the inventory's own language (`value>250`,
//!   `type:armor al>=200`, `spell:blood`), plus names always or never
//!   taken. Each search shows how many of the items carried right now it
//!   matches, so a rule can be checked before it is trusted.
//!
//! The config is saved under `autoplay.config` and handed to every
//! session as it appears, so the rules survive a restart.

use std::collections::{BTreeMap, BTreeSet};

use super::{caption, title, window, Source};
use crate::{egui, Client, Ctx, Plugin, Settings};
use ac_client::autoplay::{Buffs, Config, Fight, Loot, Survive};
use ac_client::items::{ItemStats, Query};

/// What the panel draws: the rules, what the character is doing, and how
/// many carried items each loot search matches.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutoplayView {
    pub config: Config,
    /// `autoplay.doing.label()`: "waiting", "fighting", "looting"...
    pub doing: String,
    /// `autoplay.status`: "fighting Drudge Skulker".
    pub status: String,
    /// One count per loot filter, in the same order.
    pub counts: Vec<usize>,
}

/// How many of `items` each search matches. A blank or meaningless
/// search matches nothing, the way the engine treats it.
pub fn filter_counts(filters: &[String], items: &[ItemStats]) -> Vec<usize> {
    filters
        .iter()
        .map(|f| {
            let q = Query::parse(f);
            if q.is_empty() {
                return 0;
            }
            items.iter().filter(|s| s.matches(&q)).count()
        })
        .collect()
}

/// The line under the checkbox: what it is doing and the engine's own
/// words for it. The status usually already begins with the label
/// ("fighting" / "fighting Drudge Skulker"), and then it is not said
/// twice; with no status yet, the label stands alone.
pub fn status_line(doing: &str, status: &str) -> String {
    let status = status.trim();
    if status.is_empty() {
        doing.to_string()
    } else if status.to_lowercase().starts_with(&doing.to_lowercase()) {
        status.to_string()
    } else {
        format!("{doing}: {status}")
    }
}

/// Add `text` to `list` unless it is blank or already there (names and
/// searches are matched case-insensitively). True when it was added.
pub fn add_entry(list: &mut Vec<String>, text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || list.iter().any(|e| e.trim().eq_ignore_ascii_case(text)) {
        return false;
    }
    list.push(text.to_string());
    true
}

/// Drop entry `i`. True when there was one.
pub fn remove_entry(list: &mut Vec<String>, i: usize) -> bool {
    if i >= list.len() {
        return false;
    }
    list.remove(i);
    true
}

/// The half-typed line under each editable list, kept by list name so
/// the text survives between frames.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drafts(BTreeMap<String, String>);

impl Drafts {
    fn get(&mut self, key: &str) -> &mut String {
        self.0.entry(key.to_string()).or_default()
    }
}

/// One editable list of strings: a row per entry with an `x` to drop it,
/// and a line at the bottom to add one. `counts` (the loot searches) puts
/// the number of matching carried items beside each row.
fn string_list(
    ui: &mut egui::Ui,
    key: &str,
    list: &mut Vec<String>,
    drafts: &mut Drafts,
    hint: &str,
    counts: Option<&[usize]>,
) {
    let mut drop = None;
    for (i, entry) in list.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new("x").small()).clicked() {
                drop = Some(i);
            }
            let width = if counts.is_some() { 174.0 } else { 226.0 };
            ui.add(
                egui::TextEdit::singleline(entry)
                    .id_salt(format!("{key}.{i}"))
                    .desired_width(width),
            );
            if let Some(n) = counts.and_then(|c| c.get(i)) {
                let text = format!("{n} carried");
                let colour = if *n > 0 {
                    egui::Color32::from_rgb(140, 200, 140)
                } else {
                    egui::Color32::from_gray(150)
                };
                ui.label(egui::RichText::new(text).small().color(colour))
                    .on_hover_text("Items carried right now that this search matches");
            }
        });
    }
    if let Some(i) = drop {
        remove_entry(list, i);
    }
    ui.horizontal(|ui| {
        let draft = drafts.get(key);
        let entered = ui
            .add(
                egui::TextEdit::singleline(draft)
                    .id_salt(format!("{key}.new"))
                    .hint_text(hint)
                    .desired_width(200.0),
            )
            .lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.add(egui::Button::new("add").small()).clicked() || entered {
            let text = draft.clone();
            if add_entry(list, &text) {
                drafts.get(key).clear();
            }
        }
    });
}

/// A fraction of health as a percentage slider.
fn percent(ui: &mut egui::Ui, label: &str, value: &mut f32, hint: &str) {
    let mut pct = (*value * 100.0).round();
    let resp = ui.add(
        egui::Slider::new(&mut pct, 0.0..=100.0)
            .suffix("%")
            .fixed_decimals(0)
            .text(label),
    );
    if resp.changed() {
        *value = pct / 100.0;
    }
    resp.on_hover_text(hint);
}

/// Draw the panel at `x` (it shares the left column with the skills,
/// spellbook and components panels and steps aside for the open ones).
/// Returns the rules when they were edited this frame, so the caller can
/// put them back on the session.
pub fn draw(egui: &egui::Context, v: &AutoplayView, x: f32, drafts: &mut Drafts) -> Option<Config> {
    let mut cfg = v.config.clone();
    window(
        "autoplay",
        egui::pos2(x, 132.0),
        egui::vec2(320.0, 404.0),
        // Nearly opaque: it is read while everything else is open.
        245,
        8,
    )
    .show(egui, |ui| {
        ui.set_min_width(300.0);
        ui.checkbox(
            &mut cfg.enabled,
            egui::RichText::new("Play on its own").strong().size(16.0),
        )
        .on_hover_text("Let the character heal, fight, loot and buff by itself");
        let line = status_line(&v.doing, &v.status);
        let colour = if cfg.enabled {
            egui::Color32::from_rgb(150, 210, 150)
        } else {
            egui::Color32::from_gray(150)
        };
        ui.label(egui::RichText::new(line).color(colour).italics())
            .on_hover_text("What the rules are doing this moment");
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                title(ui, "Stay alive");
                percent(
                    ui,
                    "heal below",
                    &mut cfg.survive.heal_below,
                    "Heal when health falls below this much of its maximum",
                );
                percent(
                    ui,
                    "break off",
                    &mut cfg.survive.flee_below,
                    "Stop fighting below this much health; 0 to keep fighting",
                );
                ui.checkbox(&mut cfg.survive.use_kits, "use healing kits")
                    .on_hover_text("Spend a carried healing kit before casting");
                ui.horizontal(|ui| {
                    caption(ui, "heal spell");
                    ui.add(
                        egui::TextEdit::singleline(&mut cfg.survive.heal_spell)
                            .id_salt("autoplay.heal_spell")
                            .hint_text("Heal Self")
                            .desired_width(180.0),
                    )
                    .on_hover_text("A spell from the spellbook, by name; blank for none");
                });
                ui.add_space(6.0);

                title(ui, "Buffs");
                caption(ui, "spells to keep up");
                string_list(
                    ui,
                    "autoplay.buffs",
                    &mut cfg.buffs.spells,
                    drafts,
                    "spell name, e.g. Strength Self",
                    None,
                );
                ui.add(
                    egui::Slider::new(&mut cfg.buffs.recast_within, 0.0..=300.0)
                        .suffix("s")
                        .fixed_decimals(0)
                        .text("recast within"),
                )
                .on_hover_text("Cast again once this many seconds or fewer are left");
                ui.checkbox(&mut cfg.buffs.out_of_combat_only, "only out of combat");
                ui.add_space(6.0);

                title(ui, "Fight");
                ui.checkbox(&mut cfg.fight.enabled, "pick fights")
                    .on_hover_text("Attack the nearest creature the rules allow");
                ui.add(
                    egui::Slider::new(&mut cfg.fight.radius, 1.0..=60.0)
                        .suffix(" m")
                        .fixed_decimals(0)
                        .text("radius"),
                )
                .on_hover_text("How far to look for something to attack");
                caption(ui, "only these (blank: anything)");
                string_list(
                    ui,
                    "autoplay.only",
                    &mut cfg.fight.only,
                    drafts,
                    "name contains, e.g. Drudge",
                    None,
                );
                caption(ui, "never these");
                string_list(
                    ui,
                    "autoplay.avoid",
                    &mut cfg.fight.avoid,
                    drafts,
                    "name contains, e.g. Olthoi",
                    None,
                );
                ui.add_space(6.0);

                title(ui, "Loot");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut cfg.loot.enabled, "empty corpses");
                    ui.checkbox(&mut cfg.loot.appraise, "appraise first")
                        .on_hover_text("Ask the server for the numbers before deciding");
                });
                caption(ui, "take what matches (inventory search)");
                string_list(
                    ui,
                    "autoplay.filters",
                    &mut cfg.loot.filters,
                    drafts,
                    "value>250, type:armor al>=200",
                    Some(&v.counts),
                );
                caption(ui, "always take");
                string_list(
                    ui,
                    "autoplay.always",
                    &mut cfg.loot.always,
                    drafts,
                    "name contains, e.g. Pyreal",
                    None,
                );
                caption(ui, "never take");
                string_list(
                    ui,
                    "autoplay.never",
                    &mut cfg.loot.never,
                    drafts,
                    "name contains, e.g. Rusty",
                    None,
                );
            });
    });
    (cfg != v.config).then_some(cfg)
}

/// The rules, what the character is doing, and how many carried items
/// each loot search matches. The counts are worked out once here, not
/// per row.
pub fn view(c: &Client) -> AutoplayView {
    let carried = c.item_stats();
    let config = c.autoplay.config.clone();
    AutoplayView {
        counts: filter_counts(&config.loot.filters, &carried),
        config,
        doing: c.autoplay.doing.label().to_string(),
        status: c.autoplay.status.clone(),
    }
}

pub struct Autoplay {
    source: Source<AutoplayView>,
    /// Open (J toggles it). Starts closed.
    pub show: bool,
    /// The rules as last edited or read from the settings file; handed
    /// to each session as it appears and written back on exit.
    saved: Config,
    /// Sessions already given the saved rules, by index.
    applied: BTreeSet<usize>,
    drafts: Drafts,
}

impl Default for Autoplay {
    fn default() -> Self {
        Autoplay {
            source: Source::Live,
            show: false,
            saved: Config::default(),
            applied: BTreeSet::new(),
            drafts: Drafts::default(),
        }
    }
}

impl Autoplay {
    /// A character in the middle of a fight, with rules filled in.
    pub fn demo() -> Self {
        let config = Config {
            enabled: true,
            survive: Survive {
                heal_below: 0.65,
                flee_below: 0.3,
                ..Default::default()
            },
            buffs: Buffs {
                spells: vec!["Strength Self".into(), "Armor Self".into()],
                ..Default::default()
            },
            fight: Fight {
                only: vec!["Drudge".into()],
                avoid: vec!["Olthoi".into()],
                radius: 30.0,
                ..Default::default()
            },
            loot: Loot {
                filters: vec!["value>250".into(), "type:armor al>=200".into()],
                always: vec!["Pyreal".into()],
                never: vec!["Rusty".into()],
                ..Default::default()
            },
        };
        // What the two searches would take out of the demo pack.
        let counts = vec![3, 1];
        Autoplay {
            source: Source::Demo(AutoplayView {
                config: config.clone(),
                doing: "fighting".into(),
                status: "fighting Drudge Skulker".into(),
                counts,
            }),
            show: true,
            saved: config,
            applied: BTreeSet::new(),
            drafts: Drafts::default(),
        }
    }
}

impl Plugin for Autoplay {
    fn name(&self) -> &str {
        "autoplay"
    }

    fn load(&mut self, settings: &Settings) {
        if let Some(v) = settings.get("autoplay.show") {
            self.show = v;
        }
        if let Some(c) = settings.get::<Config>("autoplay.config") {
            self.saved = c;
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("autoplay.show", self.show);
        settings.set("autoplay.config", &self.saved);
    }

    /// Give a session the saved rules the first time it is seen.
    fn tick(&mut self, cx: &mut Ctx) {
        if !matches!(self.source, Source::Live) {
            return;
        }
        let saved = self.saved.clone();
        let i = cx.index;
        if !self.applied.insert(i) {
            return;
        }
        if let Some(c) = cx.try_client() {
            c.autoplay.config = saved;
        }
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if !self.show {
            return;
        }
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().map(|c| view(c)),
        };
        let Some(v) = v else { return };
        // Sits beside the other left-column panels when they are open.
        let open = |key: &str| cx.board.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
        let mut x = 8.0;
        if open(super::skills::OPEN_KEY) {
            x += 372.0;
        }
        if open(super::spellbook::OPEN_KEY) {
            x += 384.0;
        }
        if open(super::components::OPEN_KEY) {
            x += 268.0;
        }
        let Some(edited) = draw(egui, &v, x, &mut self.drafts) else {
            return;
        };
        match &mut self.source {
            // The demo panel is live enough to click through offline.
            Source::Demo(d) => {
                d.config = edited;
            }
            Source::Live => {
                self.saved = edited.clone();
                if let Some(c) = cx.try_client() {
                    c.autoplay.config = edited;
                }
            }
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if key == egui::Key::J && pressed {
            self.show = !self.show;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_view() -> AutoplayView {
        match Autoplay::demo().source {
            Source::Demo(v) => v,
            Source::Live => unreachable!(),
        }
    }

    fn item(name: &str, value: u32, armor: u32) -> ItemStats {
        ItemStats {
            name: name.into(),
            value,
            armor_level: armor,
            appraised: true,
            kind: if armor > 0 { "armor" } else { "misc" },
            ..Default::default()
        }
    }

    #[test]
    fn entries_are_added_once_and_dropped_by_row() {
        let mut list = Vec::new();
        assert!(add_entry(&mut list, "Drudge"));
        assert_eq!(list, vec!["Drudge".to_string()]);
        // Blank lines and repeats (in any case, with stray spaces) do
        // nothing.
        assert!(!add_entry(&mut list, "   "));
        assert!(!add_entry(&mut list, ""));
        assert!(!add_entry(&mut list, "drudge"));
        assert!(!add_entry(&mut list, "  Drudge  "));
        assert_eq!(list.len(), 1);
        assert!(add_entry(&mut list, " Olthoi "));
        assert_eq!(list, vec!["Drudge".to_string(), "Olthoi".to_string()]);
        // The x on a row drops that row and no other.
        assert!(remove_entry(&mut list, 0));
        assert_eq!(list, vec!["Olthoi".to_string()]);
        assert!(!remove_entry(&mut list, 1));
        assert!(remove_entry(&mut list, 0));
        assert!(list.is_empty());
        assert!(!remove_entry(&mut list, 0));
    }

    #[test]
    fn filters_count_what_they_would_take() {
        let carried = vec![
            item("Ornate Ring", 900, 0),
            item("Silver Chain", 400, 0),
            item("Platemail Girth", 100, 240),
            item("Rusty Nail", 3, 0),
        ];
        let filters = vec![
            "value>250".to_string(),
            "type:armor al>=200".to_string(),
            "value>100000".to_string(),
            // A blank search takes nothing, so it counts nothing.
            "  ".to_string(),
        ];
        assert_eq!(filter_counts(&filters, &carried), vec![2, 1, 0, 0]);
        assert!(filter_counts(&[], &carried).is_empty());
        assert_eq!(filter_counts(&filters, &[]), vec![0, 0, 0, 0]);
    }

    #[test]
    fn the_status_line_falls_back_to_the_label() {
        assert_eq!(
            status_line("fighting", "fighting Drudge Skulker"),
            "fighting Drudge Skulker"
        );
        assert_eq!(status_line("waiting", ""), "waiting");
        assert_eq!(status_line("waiting", "   "), "waiting");
        // When the two say different things, both are shown.
        assert_eq!(
            status_line("looting", "took 3 item(s)"),
            "looting: took 3 item(s)"
        );
    }

    #[test]
    fn the_demo_shows_a_character_in_a_fight() {
        let v = demo_view();
        assert!(v.config.enabled);
        assert_eq!(v.status, "fighting Drudge Skulker");
        assert_eq!(v.doing, "fighting");
        // A count for every filter, so the rows line up.
        assert_eq!(v.counts.len(), v.config.loot.filters.len());
        assert!(!v.config.buffs.spells.is_empty());
    }

    #[test]
    fn the_rules_survive_a_restart() {
        let mut p = Autoplay::default();
        let cfg = Config {
            enabled: true,
            fight: Fight {
                avoid: vec!["Olthoi".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        p.saved = cfg.clone();
        p.show = true;
        let mut settings = Settings::new();
        p.save(&mut settings);

        let mut back = Autoplay::default();
        assert!(!back.show);
        assert_eq!(back.saved, Config::default());
        back.load(&settings);
        assert!(back.show);
        assert_eq!(back.saved, cfg);
    }

    #[test]
    fn a_settings_file_without_rules_leaves_the_defaults() {
        // Untouched rules are still written, so the file always says
        // what the character would do.
        let p = Autoplay::default();
        let mut settings = Settings::new();
        p.save(&mut settings);
        assert!(settings.contains("autoplay.config"));
        assert_eq!(
            settings.get::<Config>("autoplay.config"),
            Some(Config::default())
        );
        // Nothing stored at all: the defaults stand.
        let mut back = Autoplay::default();
        back.load(&Settings::new());
        assert_eq!(back.saved, Config::default());
        assert!(!back.saved.enabled, "it never starts playing by itself");
    }
}
