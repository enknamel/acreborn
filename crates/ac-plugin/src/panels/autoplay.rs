//! Playing on its own (key bindable from the menu): the rules the character follows when nobody
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
//! * **Loot**: ordered rules, each a search in the inventory's own
//!   language (`value>250`, `slot:ring epics>=2`, `"epic life magic"`,
//!   `a or b`, `-c`, parentheses) and what to do with a match (keep,
//!   salvage, sell, skip; the first match decides), plus names always
//!   or never taken, and whether tagged salvage is salvaged here or
//!   carried to the team's best salvager. Each rule shows how many of
//!   the items carried right now it matches, so it can be checked before
//!   it is trusted, and what is wrong with a line that does not parse.
//!
//! The config is saved under `autoplay.config` and handed to every
//! session as it appears, so the rules survive a restart.

use std::collections::{BTreeMap, BTreeSet};

use super::{caption, title, title_bar, window, Source};
use crate::{egui, Client, Ctx, Plugin, Settings};
use ac_client::autoplay::{Buffs, Config, Fight, Loot, LootAction, LootRule, Role, Style, Survive};
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
    /// One count per loot rule, in the same order.
    pub counts: Vec<usize>,
    /// Who salvages for the team ("" when nobody carries an Ust).
    pub salvager: String,
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

/// The same for the loot rules, by their searches.
pub fn rule_counts(rules: &[LootRule], items: &[ItemStats]) -> Vec<usize> {
    let queries: Vec<String> = rules.iter().map(|r| r.query.clone()).collect();
    filter_counts(&queries, items)
}

/// Move rule `i` one step up or down. True when it moved.
pub fn move_rule(rules: &mut [LootRule], i: usize, up: bool) -> bool {
    let j = if up {
        let Some(j) = i.checked_sub(1) else {
            return false;
        };
        j
    } else {
        i + 1
    };
    if i >= rules.len() || j >= rules.len() {
        return false;
    }
    rules.swap(i, j);
    true
}

/// Add a rule unless its search is blank or already there. True when
/// it was added.
pub fn add_rule(rules: &mut Vec<LootRule>, query: &str, action: LootAction) -> bool {
    let query = query.trim();
    if query.is_empty()
        || rules
            .iter()
            .any(|r| r.query.trim().eq_ignore_ascii_case(query))
    {
        return false;
    }
    rules.push(LootRule::new(query, action));
    true
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

/// The action dropdown of a rule row.
fn action_box(ui: &mut egui::Ui, salt: &str, action: &mut LootAction) {
    egui::ComboBox::from_id_salt(salt)
        .selected_text(action.label())
        .width(72.0)
        .show_ui(ui, |ui| {
            for a in LootAction::ALL {
                ui.selectable_value(action, a, a.label())
                    .on_hover_text(match a {
                        LootAction::Keep => "take it and keep it",
                        LootAction::Salvage => "take it for the team's best salvager to salvage",
                        LootAction::Sell => "take it to sell in town",
                        LootAction::Skip => "leave it on the corpse",
                    });
            }
        });
}

/// The loot rules: a row per rule with `x`, up and down, the search,
/// its action and how many carried items it matches; a line at the
/// bottom to add one; and the search language under a fold.
fn rule_list(
    ui: &mut egui::Ui,
    key: &str,
    rules: &mut Vec<LootRule>,
    drafts: &mut Drafts,
    counts: &[usize],
) {
    let mut drop = None;
    let mut moved = None;
    let n = rules.len();
    for (i, rule) in rules.iter_mut().enumerate() {
        let problem = Query::check(&rule.query).err();
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new("x").small()).clicked() {
                drop = Some(i);
            }
            if ui
                .add_enabled(i > 0, egui::Button::new("^").small())
                .on_hover_text("Try this rule earlier")
                .clicked()
            {
                moved = Some((i, true));
            }
            if ui
                .add_enabled(i + 1 < n, egui::Button::new("v").small())
                .on_hover_text("Try this rule later")
                .clicked()
            {
                moved = Some((i, false));
            }
            let mut edit = egui::TextEdit::singleline(&mut rule.query)
                .id_salt(format!("{key}.{i}"))
                .desired_width(118.0);
            if problem.is_some() {
                edit = edit.text_color(egui::Color32::from_rgb(230, 120, 110));
            }
            let resp = ui.add(edit);
            if let Some(p) = &problem {
                resp.on_hover_text(p);
            }
            action_box(ui, &format!("{key}.{i}.action"), &mut rule.action);
            if let Some(n) = counts.get(i) {
                let colour = if *n > 0 {
                    egui::Color32::from_rgb(140, 200, 140)
                } else {
                    egui::Color32::from_gray(150)
                };
                ui.label(egui::RichText::new(format!("{n}")).small().color(colour))
                    .on_hover_text("Items carried right now that this search matches");
            }
        });
        if let Some(p) = problem {
            ui.label(
                egui::RichText::new(p)
                    .small()
                    .color(egui::Color32::from_rgb(230, 120, 110)),
            );
        }
    }
    if let Some(i) = drop {
        rules.remove(i);
    }
    if let Some((i, up)) = moved {
        move_rule(rules, i, up);
    }
    ui.horizontal(|ui| {
        let draft = drafts.get(key).clone();
        let mut text = draft;
        let entered = ui
            .add(
                egui::TextEdit::singleline(&mut text)
                    .id_salt(format!("{key}.new"))
                    .hint_text("slot:ring epics>=2")
                    .desired_width(150.0),
            )
            .lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter));
        *drafts.get(key) = text.clone();
        let action_key = format!("{key}.action");
        let mut action = LootAction::parse(drafts.get(&action_key)).unwrap_or_default();
        action_box(ui, &format!("{key}.new.action"), &mut action);
        *drafts.get(&action_key) = action.label().to_string();
        if (ui.add(egui::Button::new("add").small()).clicked() || entered)
            && add_rule(rules, &text, action)
        {
            drafts.get(key).clear();
        }
    });
    if let Some(p) = Query::check(drafts.get(key)).err() {
        ui.label(
            egui::RichText::new(p)
                .small()
                .color(egui::Color32::from_rgb(230, 120, 110)),
        );
    }
    ui.collapsing(egui::RichText::new("search language").small(), |ui| {
        egui::Grid::new(format!("{key}.help"))
            .num_columns(2)
            .spacing([8.0, 2.0])
            .show(ui, |ui| {
                for (what, means) in ac_client::items::HELP {
                    ui.label(egui::RichText::new(*what).small().monospace());
                    ui.add(egui::Label::new(egui::RichText::new(*means).small()).wrap());
                    ui.end_row();
                }
            });
        caption(
            ui,
            "first matching rule wins; \"always\" and \"never\" names come first",
        );
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
        title_bar(ui, "autoplay", "Play on its own");
        ui.checkbox(&mut cfg.enabled, egui::RichText::new("Enabled").strong())
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
                            .hint_text("strongest heal known")
                            .desired_width(180.0),
                    )
                    .on_hover_text(
                        "A spell from the spellbook, by name; blank for the strongest \
                         health boost known, whatever it is called",
                    );
                });
                ui.checkbox(&mut cfg.survive.manage_mana, "keep mana up from stamina")
                    .on_hover_text(
                        "Pour stamina into mana when mana runs low and Revitalize \
                         when stamina does, with the strongest spells known",
                    );
                percent(
                    ui,
                    "mana below",
                    &mut cfg.survive.mana_below,
                    "Pour stamina into mana when mana is under this",
                );
                percent(
                    ui,
                    "stamina below",
                    &mut cfg.survive.stamina_below,
                    "Revitalize when stamina is under this",
                );
                ui.add_space(6.0);

                title(ui, "Buffs");
                ui.checkbox(&mut cfg.buffs.auto, "buff for my training")
                    .on_hover_text(
                        "Every Life and Creature self-buff known for the skills \
                         trained, the Item auras for the way it fights, and the \
                         armour spells on each piece worn: the highest level of each",
                    );
                ui.add(
                    egui::Slider::new(&mut cfg.buffs.least_chance, 0.1..=0.95)
                        .fixed_decimals(2)
                        .text("least chance to land"),
                )
                .on_hover_text(
                    "A level that fizzles more often than this is passed over \
                     for the one below it; 0.50 is where the school's skill \
                     equals the spell's power",
                );
                caption(ui, "and these by hand");
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
                    egui::Slider::new(&mut cfg.buffs.top_up_within, 60.0..=900.0)
                        .suffix("s")
                        .fixed_decimals(0)
                        .text("top up within"),
                )
                .on_hover_text(
                    "In a quiet moment, put back anything with this long or less \
                     left. Wide, so a few are refreshed at every lull and the set \
                     never all runs out at once",
                );
                ui.add(
                    egui::Slider::new(&mut cfg.buffs.never_below, 10.0..=300.0)
                        .suffix("s")
                        .fixed_decimals(0)
                        .text("never below"),
                )
                .on_hover_text(
                    "Anything with this long or less left is put back at once, \
                     fight or no fight, swapping to a wand if need be",
                );
                ui.checkbox(
                    &mut cfg.buffs.out_of_combat_only,
                    "only top up out of combat",
                );
                ui.add(
                    egui::Slider::new(&mut cfg.buffs.keep_mana, 0.0..=0.8)
                        .fixed_decimals(2)
                        .text("mana kept back"),
                )
                .on_hover_text(
                    "Buffs never spend below this fraction of mana; it is kept for \\
                     healing and fighting",
                );
                ui.add_space(6.0);

                title(ui, "Fight");
                ui.checkbox(&mut cfg.fight.enabled, "pick fights")
                    .on_hover_text("Attack the nearest creature the rules allow");
                ui.horizontal(|ui| {
                    ui.label("with");
                    egui::ComboBox::from_id_salt("autoplay.style")
                        .selected_text(cfg.fight.style.label())
                        .show_ui(ui, |ui| {
                            for style in Style::ALL {
                                ui.selectable_value(&mut cfg.fight.style, style, style.label());
                            }
                        });
                })
                .response
                .on_hover_text(
                    "How a character fights follows what it holds: a wand \
                     casts, a bow shoots, a sword swings. This picks which \
                     of them to wield.",
                );
                ui.add(
                    egui::Slider::new(&mut cfg.fight.radius, 1.0..=60.0)
                        .suffix(" m")
                        .fixed_decimals(0)
                        .text("radius"),
                )
                .on_hover_text("How far to look for something to attack");
                ui.checkbox(&mut cfg.fight.craft_ammo, "make ammunition when out")
                    .on_hover_text(
                        "From a bundle of heads and a bundle of shafts carried, \
                         when Fletching is up to the recipe",
                    );
                ui.checkbox(&mut cfg.fight.pick_weapon, "wield the best weapon")
                    .on_hover_text(
                        "Swap to the carried weapon whose element the target \
                         takes most damage from, rending and criticals counted",
                    );
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut cfg.fight.vuln_above_health)
                            .speed(10.0)
                            .range(0..=20000),
                    );
                    ui.label("health before casting a vulnerability");
                })
                .response
                .on_hover_text(
                    "Soften anything with at least this much health with a \
                     vulnerability for the element it is weakest to. 0 never does.",
                );
                if matches!(cfg.fight.style, Style::Auto | Style::Magic) {
                    caption(ui, "attack spells, best first");
                    string_list(
                        ui,
                        "autoplay.spells",
                        &mut cfg.fight.spells,
                        drafts,
                        "e.g. Flame Bolt V",
                        None,
                    );
                }
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
                caption(ui, "rules, in order: search, then what to do with a match");
                cfg.loot.migrate();
                rule_list(ui, "autoplay.rules", &mut cfg.loot.rules, drafts, &v.counts);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut cfg.loot.salvage, "salvage")
                        .on_hover_text("Salvage tagged items when this character is the team's best salvager");
                    ui.checkbox(&mut cfg.loot.hand_off, "hand off")
                        .on_hover_text("Carry tagged items to the team's best salvager (highest Salvaging with an Ust)");
                });
                caption(
                    ui,
                    if v.salvager.is_empty() {
                        "salvager: nobody carries an Ust".to_string()
                    } else {
                        format!("salvager: {}", v.salvager)
                    },
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
                ui.add_space(6.0);

                title(ui, "Team");
                ui.checkbox(&mut cfg.team.enabled, "hunt with the others")
                    .on_hover_text(
                        "Every character being played that has this on hears the \
                         others: the same target, the debuffs landed first, spare \
                         supplies handed over, one fellowship",
                    );
                ui.horizontal(|ui| {
                    ui.label("role");
                    egui::ComboBox::from_id_salt("autoplay.role")
                        .selected_text(cfg.team.role.label())
                        .show_ui(ui, |ui| {
                            for role in [Role::Fighter, Role::Debuffer, Role::Healer] {
                                ui.selectable_value(&mut cfg.team.role, role, role.label());
                            }
                        });
                });
                ui.checkbox(&mut cfg.team.focus_fire, "fight what the leader fights")
                    .on_hover_text("The leader is the one that leads, else whoever's name sorts first");
                ui.checkbox(&mut cfg.team.lead, "lead: the others come to me and follow me about")
                    .on_hover_text(
                        "For the character played by hand. The others keep close,                          fly when it flies, and take a journey after it when it                          goes through a portal",
                    );
                ui.horizontal(|ui| {
                    ui.checkbox(&mut cfg.team.follow, "follow the leader, keeping within");
                    ui.add(
                        egui::DragValue::new(&mut cfg.team.follow_distance)
                            .speed(0.5)
                            .range(1.5..=30.0)
                            .suffix(" m"),
                    );
                    ui.label("and fighting within");
                    ui.add(
                        egui::DragValue::new(&mut cfg.team.fight_radius)
                            .speed(1.0)
                            .range(5.0..=120.0)
                            .suffix(" m of the leader"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut cfg.team.fellowship, "form a fellowship");
                    ui.add(
                        egui::TextEdit::singleline(&mut cfg.team.fellowship_name)
                            .id_salt("autoplay.fellowship_name")
                            .hint_text("name")
                            .desired_width(120.0),
                    );
                });
                ui.checkbox(&mut cfg.team.share_supplies, "hand over spare supplies")
                    .on_hover_text("Give a teammate beside us what they say they are short of");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut cfg.team.hard_fight_health)
                            .speed(10.0)
                            .range(0..=50000),
                    );
                    ui.label("health makes a hard fight");
                })
                .response
                .on_hover_text(
                    "Against anything with this much health the teammate with the \
                     highest Life Magic softens it first, with a vulnerability for \
                     its weakest element and an imperil. 0 plans nothing.",
                );
                ui.checkbox(
                    &mut cfg.team.wait_for_debuff,
                    "the rest wait for the softening",
                )
                .on_hover_text(
                    "Off, everyone opens fire at once and the softening lands \
                         over the top; the shots spent first cost next to nothing",
                );
                caption(ui, "debuffs a debuffer lands, in order");
                string_list(
                    ui,
                    "autoplay.debuffs",
                    &mut cfg.team.debuffs,
                    drafts,
                    "e.g. Imperil Other, Fire Vulnerability Other",
                    None,
                );
                caption(ui, "keep stocked: name and how many");
                let mut stocked: Vec<String> = cfg
                    .team
                    .keep_stocked
                    .iter()
                    .map(|(n, c)| format!("{n} {c}"))
                    .collect();
                string_list(
                    ui,
                    "autoplay.keep_stocked",
                    &mut stocked,
                    drafts,
                    "e.g. Prismatic Taper 50",
                    None,
                );
                cfg.team.keep_stocked = stocked
                    .iter()
                    .map(|line| {
                        let line = line.trim();
                        match line.rsplit_once(' ') {
                            Some((name, n)) => match n.parse::<u32>() {
                                Ok(n) => (name.trim().to_string(), n),
                                Err(_) => (line.to_string(), 1),
                            },
                            None => (line.to_string(), 1),
                        }
                    })
                    .collect();
            });
    });
    (cfg != v.config).then_some(cfg)
}

/// The rules, what the character is doing, and how many carried items
/// each loot search matches. The counts are worked out once here, not
/// per row.
pub fn view(c: &Client) -> AutoplayView {
    let carried = c.item_stats();
    let mut config = c.autoplay.config.clone();
    config.loot.migrate();
    AutoplayView {
        counts: rule_counts(&config.loot.rules, &carried),
        config,
        doing: c.autoplay.doing.label().to_string(),
        status: c.autoplay.status.clone(),
        salvager: c.best_salvager().map(|(n, _)| n).unwrap_or_default(),
    }
}

pub struct Autoplay {
    source: Source<AutoplayView>,
    /// Open (its key toggles it). Starts closed.
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
            team: Default::default(),
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
                rules: vec![
                    LootRule::new("slot:ring epics>=2", LootAction::Keep),
                    LootRule::new("value>250", LootAction::Keep),
                    LootRule::new("ws<6 -epics>0", LootAction::Salvage),
                ],
                always: vec!["Pyreal".into()],
                never: vec!["Rusty".into()],
                ..Default::default()
            },
            growth: Default::default(),
            academy: Default::default(),
        };
        // What the three searches would take out of the demo pack.
        let counts = vec![0, 3, 1];
        Autoplay {
            source: Source::Demo(AutoplayView {
                config: config.clone(),
                doing: "fighting".into(),
                status: "fighting Drudge Skulker".into(),
                counts,
                salvager: "Brannoc".into(),
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
        if let Some(mut c) = settings.get::<Config>("autoplay.config") {
            // A file from before the loot rules: its filters become
            // keep rules.
            c.loot.migrate();
            self.saved = c;
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("autoplay.show", self.show);
        settings.set("autoplay.config", &self.saved);
    }

    /// The sessions above the one dropped move down: what was applied
    /// to them stays applied.
    fn session_removed(&mut self, index: usize) {
        self.applied = self
            .applied
            .iter()
            .filter(|&&i| i != index)
            .map(|&i| if i > index { i - 1 } else { i })
            .collect();
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
        if let Some(ask) = super::take_open(cx.board, "autoplay") {
            self.show = ask.apply(self.show);
        }
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
        let edited = draw(egui, &v, x, &mut self.drafts);
        if super::closed("autoplay") {
            self.show = false;
        }
        let Some(edited) = edited else { return };
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
        if pressed && crate::keys::bound("autoplay", key) {
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
    fn rules_are_added_moved_and_counted() {
        let mut rules = vec![LootRule::new("value>250", LootAction::Keep)];
        assert!(add_rule(&mut rules, " ws<6 ", LootAction::Salvage));
        assert!(
            !add_rule(&mut rules, "WS<6", LootAction::Keep),
            "already there"
        );
        assert!(!add_rule(&mut rules, "  ", LootAction::Keep));
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[1].action, LootAction::Salvage);
        assert!(move_rule(&mut rules, 1, true));
        assert_eq!(rules[0].query, "ws<6");
        assert!(!move_rule(&mut rules, 0, true), "already first");
        assert!(!move_rule(&mut rules, 1, false), "already last");
        assert!(!move_rule(&mut rules, 5, true));
        let carried = vec![item("Ornate Ring", 900, 0), item("Rusty Nail", 3, 0)];
        assert_eq!(rule_counts(&rules, &carried), vec![0, 1]);
        // A settings file from before the rules still loads.
        let mut settings = Settings::new();
        settings.set(
            "autoplay.config",
            serde_json::json!({"loot": {"filters": ["value>500"], "always": ["Pyreal"]}}),
        );
        let mut p = Autoplay::default();
        p.load(&settings);
        assert!(p.saved.loot.filters.is_empty());
        assert_eq!(
            p.saved.loot.rules,
            vec![LootRule::new("value>500", LootAction::Keep)]
        );
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
        // A count for every rule, so the rows line up.
        assert_eq!(v.counts.len(), v.config.loot.rules.len());
        assert_eq!(v.salvager, "Brannoc");
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
