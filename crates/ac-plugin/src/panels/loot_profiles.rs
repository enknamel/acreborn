//! Loot profiles (bindable from the menu): the shelf of rule sets every
//! character in the process reads, and an editor for the one open.
//!
//! A profile is an ordered list of rules, and a rule is a name, what to
//! do with what it claims, and conditions that must all hold (see
//! `ac_client::profile`). The first rule that claims an item decides
//! it, so the order of the list *is* the policy and most of the panel
//! is about reading and reordering it.
//!
//! Three things the panel takes trouble over.
//!
//! * **Every change is saved as it is made.** There is no Apply button:
//!   a rule switched off here is off for the eleven other characters
//!   working the same ground before the next corpse is opened, which is
//!   what [`Library::put`] does.
//! * **What costs a round trip is marked.** Half of what a rule can ask
//!   needs the server to identify the item first, which is a message
//!   per item on every corpse. A rule or condition that needs one
//!   carries an `id` marker, so a profile can be tuned until it decides
//!   most items without asking anything.
//! * **A rule can be tried before it is trusted.** The list at the
//!   bottom judges what the character is carrying right now by the
//!   profile open, so the verdicts are read here rather than guessed at
//!   over a corpse.
//!
//! Sharing a profile is sending a file. They all live in one folder,
//! named at the top with a button that opens it in the file manager and
//! another that reads it again, so a profile from a friend is dropped
//! in and reloaded.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{caption, title, title_bar, window, Source};
use crate::{egui, Client, Ctx, Plugin, Settings};
use ac_client::autoplay::LootAction;
use ac_client::items::{NumKey, Op, Term, Tier};
use ac_client::profile::{self, Ask, Library, Mine, Profile, PropKind, Rule, TextOp, Verdict};
use ac_client::weapons::Wielder;
use ac_world::properties;
use ac_world::stats::{sac, sac_name, skill_name, SKILL_NAMES};

/// Something the rules would take, and the verdicts that say so.
const GOOD: egui::Color32 = egui::Color32::from_rgb(140, 200, 140);
/// Worth knowing before it bites: a round trip, a name already taken.
const WARN: egui::Color32 = egui::Color32::from_rgb(220, 190, 120);
/// Wrong: a pattern that will not compile, a file that would not save.
const BAD: egui::Color32 = egui::Color32::from_rgb(230, 120, 110);
/// Said quietly.
const DIM: egui::Color32 = egui::Color32::from_gray(150);

/// The comparisons a number can be held to, most used first.
const OPS: [Op; 5] = [Op::Ge, Op::Gt, Op::Eq, Op::Le, Op::Lt];

/// Every numeric field of an item a rule can name.
const NUM_KEYS: [NumKey; 21] = [
    NumKey::Value,
    NumKey::Burden,
    NumKey::Workmanship,
    NumKey::Stack,
    NumKey::Damage,
    NumKey::Armor,
    NumKey::Speed,
    NumKey::Wield,
    NumKey::Mana,
    NumKey::Spellcraft,
    NumKey::Uses,
    NumKey::Tinks,
    NumKey::Attack,
    NumKey::Defense,
    NumKey::Spells,
    NumKey::Cantrips,
    NumKey::Minors,
    NumKey::Moderates,
    NumKey::Majors,
    NumKey::Epics,
    NumKey::Legendaries,
];

/// Where the profiles live when nobody has said otherwise:
/// `~/.config/acswarm/profiles`, beside the UI settings.
pub fn default_dir() -> PathBuf {
    Settings::config_dir().join("profiles")
}

/// The library's folder, opened at [`default_dir`] the first time it is
/// wanted. Nothing else in the client sets it, so the editor is what
/// decides where a shared profile is dropped.
fn dir_of(library: &Library) -> PathBuf {
    let dir = library.dir();
    if !dir.as_os_str().is_empty() {
        return dir;
    }
    let dir = default_dir();
    library.open(dir.clone());
    dir
}

/// One carried item and what the profile makes of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Trial {
    pub item: String,
    pub verdict: Verdict,
}

/// What the panel draws: the shelf, the profile open for editing, and
/// what this character carries judged by it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfilesView {
    /// Every profile on the shelf, in order.
    pub names: Vec<String>,
    /// The one being edited, when it is still there.
    pub profile: Option<Profile>,
    /// Where the files live, for sharing one.
    pub dir: PathBuf,
    /// The character the trial list is judged for.
    pub who: String,
    /// Each carried item and the verdict on it.
    pub trial: Vec<Trial>,
}

/// The shelf, and what the character carries judged by `chosen`.
pub fn view(c: &Client, chosen: &str) -> ProfilesView {
    let library = &c.profiles;
    let dir = dir_of(library);
    let profile = library.get(chosen).map(|p| (*p).clone());
    let me = c.wielder();
    let who = c.world.stats.name.clone();
    let trial = profile
        .as_ref()
        .map(|p| judge_carried(c, p, &me, &who))
        .unwrap_or_default();
    ProfilesView {
        names: library.names(),
        profile,
        dir,
        who,
        trial,
    }
}

/// Every carried item judged, in the order the pack lists it. `held`
/// counts what is already carried of the same kind, which is what a
/// capped rule reads and what makes a "keep two" rule stop at two.
fn judge_carried(c: &Client, p: &Profile, me: &Wielder, who: &str) -> Vec<Trial> {
    c.item_stats()
        .into_iter()
        .map(|s| {
            let held: u32 = c
                .world
                .inventory()
                .filter(|o| o.weenie_class_id == s.wcid)
                .map(|o| o.stack_size.max(1))
                .sum();
            let verdict = p.judge(&s, c.appraisals.get(&s.guid), me, who, held);
            Trial {
                item: super::item_label(&s.name, s.stack),
                verdict,
            }
        })
        .collect()
}

/// A verdict in words, and the colour it is said in.
pub fn verdict_words(v: &Verdict) -> (String, egui::Color32) {
    match v {
        Verdict::Decided(action, rule) => {
            let colour = match action {
                LootAction::Keep => GOOD,
                LootAction::Salvage | LootAction::Sell => egui::Color32::from_rgb(170, 200, 230),
                LootAction::Skip => DIM,
            };
            (format!("{} — {rule}", action.label()), colour)
        }
        Verdict::NeedsId(rule) => (format!("identify first — {rule}"), WARN),
        Verdict::None => ("left alone".to_string(), DIM),
    }
}

/// The regular expression a condition carries, when it has one that
/// will not compile. Said in the editor rather than found out on a
/// corpse, where a pattern with a typo in it quietly matches nothing.
pub fn ask_pattern_error(ask: &Ask) -> Option<String> {
    match ask {
        Ask::Text { op, value, .. } | Ask::Spell { op, value } if op.is_regex() => {
            profile::pattern_error(value)
        }
        _ => None,
    }
}

/// A name no profile on the shelf has: `Rares`, then `Rares 2`.
pub fn unused_name(names: &[String], want: &str) -> String {
    let want = want.trim();
    let want = if want.is_empty() { "profile" } else { want };
    let taken = |n: &str| names.iter().any(|x| x.eq_ignore_ascii_case(n));
    if !taken(want) {
        return want.to_string();
    }
    (2..)
        .map(|i| format!("{want} {i}"))
        .find(|n| !taken(n))
        .unwrap_or_else(|| want.to_string())
}

/// Move entry `i` of a list one step up or down. True when it moved.
pub fn move_entry<T>(list: &mut [T], i: usize, up: bool) -> bool {
    let Some(j) = (if up { i.checked_sub(1) } else { Some(i + 1) }) else {
        return false;
    };
    if i >= list.len() || j >= list.len() {
        return false;
    }
    list.swap(i, j);
    true
}

/// Every word `ItemStats::kind` can hold, for the kind dropdown: the
/// inventory's own chips flattened, so the two lists cannot drift.
pub fn item_kinds() -> Vec<&'static str> {
    let mut kinds: Vec<&'static str> = super::inventory::KINDS
        .iter()
        .flat_map(|(_, words)| words.iter().copied())
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    kinds
}

/// The places a rule can name, one per slot: the search language lists
/// a few of them twice ("ring" and "finger" are the same place) and a
/// dropdown wants each place once.
pub fn slots() -> Vec<(&'static str, u32)> {
    let mut out: Vec<(&'static str, u32)> = Vec::new();
    for (word, mask) in ac_client::items::SLOTS {
        if !out.iter().any(|(_, m)| *m == mask) {
            out.push((word, mask));
        }
    }
    out
}

/// What sort of thing a condition asks about: the choice at the head of
/// every condition row, and what the "add" dropdown offers.
///
/// The model's `Ask::Item` holds a whole search term, which is one
/// choice to the compiler and nine to a player; splitting it here is
/// what lets each sort of question show only the fields it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskKind {
    Name,
    Kind,
    Material,
    Slot,
    WeaponSkill,
    SpellNamed,
    Tier,
    Wielded,
    Unappraised,
    Number,
    Me,
    Prop,
    TextProp,
    Spell,
}

impl AskKind {
    pub const ALL: [AskKind; 14] = [
        AskKind::Name,
        AskKind::Kind,
        AskKind::Material,
        AskKind::Slot,
        AskKind::WeaponSkill,
        AskKind::SpellNamed,
        AskKind::Tier,
        AskKind::Wielded,
        AskKind::Unappraised,
        AskKind::Number,
        AskKind::Me,
        AskKind::Prop,
        AskKind::TextProp,
        AskKind::Spell,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AskKind::Name => "item name",
            AskKind::Kind => "item kind",
            AskKind::Material => "material",
            AskKind::Slot => "worn in",
            AskKind::WeaponSkill => "weapon skill",
            AskKind::SpellNamed => "spell named",
            AskKind::Tier => "cantrip tier",
            AskKind::Wielded => "is wielded",
            AskKind::Unappraised => "not appraised",
            AskKind::Number => "a number",
            AskKind::Me => "about me",
            AskKind::Prop => "any property",
            AskKind::TextProp => "text property",
            AskKind::Spell => "a spell matching",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            AskKind::Name => "The item's name holds this word",
            AskKind::Kind => "What the item is: armour, a gem, a weapon",
            AskKind::Material => "What it is made of",
            AskKind::Slot => "Where it is worn or held",
            AskKind::WeaponSkill => "The skill it is used with, or needed to wield it",
            AskKind::SpellNamed => "One of its spells holds this word",
            AskKind::Tier => "It carries at least one cantrip of this tier",
            AskKind::Wielded => "It is being worn or held right now",
            AskKind::Unappraised => "The server has not been asked about it yet",
            AskKind::Number => "Its worth, burden, armour, damage, how many cantrips",
            AskKind::Me => "About the character reading the profile, not the item",
            AskKind::Prop => "Any number, flag or data id the server sent, by its own name",
            AskKind::TextProp => {
                "Any of the server's text properties, such as the set it belongs to"
            }
            AskKind::Spell => "Any spell on it whose name answers this, patterns included",
        }
    }

    /// Which sort of question this condition is.
    pub fn of(ask: &Ask) -> AskKind {
        match ask {
            Ask::Item(Term::Word(_)) => AskKind::Name,
            Ask::Item(Term::Kind(_)) => AskKind::Kind,
            Ask::Item(Term::Material(_)) => AskKind::Material,
            Ask::Item(Term::Slot(_)) => AskKind::Slot,
            Ask::Item(Term::Skill(_)) => AskKind::WeaponSkill,
            Ask::Item(Term::Spell(_)) => AskKind::SpellNamed,
            Ask::Item(Term::Tier(_)) => AskKind::Tier,
            Ask::Item(Term::Wielded) => AskKind::Wielded,
            Ask::Item(Term::Unappraised) => AskKind::Unappraised,
            Ask::Item(Term::Num(..)) => AskKind::Number,
            Ask::Me(_) => AskKind::Me,
            Ask::Prop { .. } => AskKind::Prop,
            Ask::Text { .. } => AskKind::TextProp,
            Ask::Spell { .. } => AskKind::Spell,
        }
    }

    /// A blank condition of this sort, for the row that has just been
    /// added or had its sort changed. The defaults are the common case:
    /// value at least a thousand, level at least one.
    pub fn blank(self) -> Ask {
        match self {
            AskKind::Name => Ask::Item(Term::Word(String::new())),
            AskKind::Kind => Ask::Item(Term::Kind("armor".into())),
            AskKind::Material => Ask::Item(Term::Material(String::new())),
            AskKind::Slot => Ask::Item(Term::Slot(slots().first().map(|(_, m)| *m).unwrap_or(0))),
            AskKind::WeaponSkill => Ask::Item(Term::Skill(String::new())),
            AskKind::SpellNamed => Ask::Item(Term::Spell(String::new())),
            AskKind::Tier => Ask::Item(Term::Tier(Tier::Epic)),
            AskKind::Wielded => Ask::Item(Term::Wielded),
            AskKind::Unappraised => Ask::Item(Term::Unappraised),
            AskKind::Number => Ask::Item(Term::Num(NumKey::Value, Op::Ge, 1_000.0)),
            AskKind::Me => Ask::Me(Mine::Level {
                op: Op::Ge,
                level: 1,
            }),
            AskKind::Prop => Ask::Prop {
                kind: PropKind::Int,
                id: 0,
                op: Op::Ge,
                value: 1.0,
            },
            AskKind::TextProp => Ask::Text {
                id: 0,
                op: TextOp::Has,
                value: String::new(),
            },
            AskKind::Spell => Ask::Spell {
                op: TextOp::Like,
                value: String::new(),
            },
        }
    }
}

/// What the editor holds between frames: which rule is open, the names
/// being typed, the search line inside each property picker, and what
/// went wrong with the last write.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Editor {
    /// The rule whose conditions are shown, by position in the list.
    pub open_rule: Option<usize>,
    /// The name being typed for a new profile, while "New" waits for one.
    pub new_name: Option<String>,
    /// The profile being renamed, and the name being typed for it.
    pub rename: Option<(String, String)>,
    /// Whether Delete has been asked once and is waiting to be meant.
    pub confirm_delete: bool,
    /// What went wrong last, said under the panel until it goes right.
    pub status: String,
    /// Search lines inside the pickers, by the widget's salt.
    searches: BTreeMap<String, String>,
}

impl Editor {
    fn search(&mut self, salt: &str) -> &mut String {
        self.searches.entry(salt.to_string()).or_default()
    }
}

/// What the panel asked for. Everything that changes a profile goes
/// through the library, so it reaches every character at once.
#[derive(Default, Debug, PartialEq)]
pub struct Actions {
    /// Profiles to write, in order.
    pub put: Vec<Profile>,
    /// The profile to have open after this frame.
    pub select: Option<String>,
    pub delete: Option<String>,
    /// `(from, to)`: written under the new name, the old file deleted.
    pub rename: Option<(String, String)>,
    /// Read the folder again, for a profile dropped in from outside.
    pub reload: bool,
    /// Show the folder in the file manager.
    pub reveal: bool,
    pub close: bool,
}

/// The `id` marker: this cannot be judged until the server has been
/// asked about the item, which is a message per item on every corpse.
fn id_marker(ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("id").small().color(WARN))
        .on_hover_text(
            "Judging this needs the server to identify the item first: a \
             round trip for every item on every corpse. Rules that ask \
             only about the name, kind, worth and workmanship decide \
             without one.",
        );
}

/// A skill chosen by name.
fn skill_picker(ui: &mut egui::Ui, salt: &str, skill: &mut u32) {
    egui::ComboBox::from_id_salt(salt.to_string())
        .selected_text(skill_name(*skill))
        .width(150.0)
        .show_ui(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    for (id, name) in SKILL_NAMES.iter().enumerate().skip(1) {
                        ui.selectable_value(skill, id as u32, *name);
                    }
                });
        });
}

/// A property chosen by name. The server has nearly nine hundred of
/// them, so the list is narrowed by what is typed rather than scrolled;
/// a property nobody has a name for is still usable by its number.
fn prop_picker(
    ui: &mut egui::Ui,
    salt: &str,
    kind: properties::Kind,
    id: &mut u32,
    editor: &mut Editor,
) {
    let chosen = properties::name_of(kind, *id)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{} {id}", kind.label()));
    egui::ComboBox::from_id_salt(salt.to_string())
        .selected_text(chosen)
        .width(180.0)
        .show_ui(ui, |ui| {
            let search_salt = format!("{salt}.search");
            let needle = {
                let text = editor.search(&search_salt);
                ui.add(
                    egui::TextEdit::singleline(text)
                        .id_salt(search_salt.clone())
                        .hint_text("search, e.g. set")
                        .desired_width(170.0),
                );
                text.trim().to_lowercase()
            };
            let mut found = properties::of_kind(kind);
            found.sort_unstable_by_key(|(_, name)| *name);
            let matching: Vec<(u32, &'static str)> = found
                .into_iter()
                .filter(|(_, name)| needle.is_empty() || name.to_lowercase().contains(&needle))
                .collect();
            if matching.is_empty() {
                caption(ui, "nothing of that name");
            }
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    for (pid, name) in matching.into_iter().take(200) {
                        ui.selectable_value(id, pid, name)
                            .on_hover_text(format!("{} {pid}", kind.label()));
                    }
                });
        });
}

/// The comparison of a number.
fn op_box(ui: &mut egui::Ui, salt: &str, op: &mut Op) {
    egui::ComboBox::from_id_salt(salt.to_string())
        .selected_text(op.word())
        .width(88.0)
        .show_ui(ui, |ui| {
            for o in OPS {
                ui.selectable_value(op, o, o.word());
            }
        });
}

/// How a piece of text is matched. The two pattern choices are named as
/// such: a player who picks one is owed the warning that it is a
/// regular expression and not a word.
fn text_op_box(ui: &mut egui::Ui, salt: &str, op: &mut TextOp) {
    egui::ComboBox::from_id_salt(salt.to_string())
        .selected_text(op.word())
        .width(126.0)
        .show_ui(ui, |ui| {
            for o in TextOp::ALL {
                ui.selectable_value(op, o, o.word())
                    .on_hover_text(if o.is_regex() {
                        "A regular expression: ^Legendary takes a legendary \
                         cantrip of any kind"
                    } else {
                        "A plain word, matched anywhere in the text"
                    });
            }
        });
}

/// What to do with an item the rule claims.
fn action_box(ui: &mut egui::Ui, salt: &str, action: &mut LootAction) {
    egui::ComboBox::from_id_salt(salt.to_string())
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

/// A one-line text field with a hint.
fn text_field(ui: &mut egui::Ui, salt: &str, text: &mut String, hint: &str, width: f32) {
    ui.add(
        egui::TextEdit::singleline(text)
            .id_salt(salt.to_string())
            .hint_text(hint)
            .desired_width(width),
    );
}

/// The fields a question about the character needs.
fn mine_fields(ui: &mut egui::Ui, salt: &str, mine: &mut Mine) {
    let choices = [
        (0usize, "name has"),
        (1, "skill stands at"),
        (2, "skill is trained"),
        (3, "level"),
    ];
    let mut which = match mine {
        Mine::Name(_) => 0usize,
        Mine::Skill { .. } => 1,
        Mine::Trained { .. } => 2,
        Mine::Level { .. } => 3,
    };
    let was = which;
    egui::ComboBox::from_id_salt(format!("{salt}.mine"))
        .selected_text(choices[which].1)
        .width(120.0)
        .show_ui(ui, |ui| {
            for (i, label) in choices {
                ui.selectable_value(&mut which, i, label);
            }
        });
    if which != was {
        *mine = match which {
            0 => Mine::Name(String::new()),
            1 => Mine::Skill {
                skill: 1,
                op: Op::Ge,
                level: 200,
            },
            2 => Mine::Trained {
                skill: 1,
                at_least: sac::TRAINED,
            },
            _ => Mine::Level {
                op: Op::Ge,
                level: 1,
            },
        };
    }
    match mine {
        Mine::Name(name) => text_field(ui, &format!("{salt}.name"), name, "part of my name", 150.0),
        Mine::Skill { skill, op, level } => {
            skill_picker(ui, &format!("{salt}.skill"), skill);
            op_box(ui, &format!("{salt}.op"), op);
            ui.add(egui::DragValue::new(level).speed(1.0).range(0..=1000));
        }
        Mine::Trained { skill, at_least } => {
            skill_picker(ui, &format!("{salt}.skill"), skill);
            egui::ComboBox::from_id_salt(format!("{salt}.sac"))
                .selected_text(sac_name(*at_least))
                .width(110.0)
                .show_ui(ui, |ui| {
                    for level in [sac::TRAINED, sac::SPECIALIZED] {
                        ui.selectable_value(at_least, level, sac_name(level));
                    }
                });
        }
        Mine::Level { op, level } => {
            op_box(ui, &format!("{salt}.op"), op);
            ui.add(egui::DragValue::new(level).speed(1.0).range(0..=999));
        }
    }
}

/// The fields a question about the item needs.
fn term_fields(ui: &mut egui::Ui, salt: &str, term: &mut Term) {
    match term {
        Term::Word(w) => text_field(ui, &format!("{salt}.word"), w, "part of the name", 180.0),
        Term::Material(m) => text_field(ui, &format!("{salt}.mat"), m, "e.g. iron", 150.0),
        Term::Skill(s) => text_field(ui, &format!("{salt}.skill"), s, "e.g. sword", 150.0),
        Term::Spell(s) => text_field(ui, &format!("{salt}.spell"), s, "e.g. blood drinker", 180.0),
        Term::Kind(k) => {
            egui::ComboBox::from_id_salt(format!("{salt}.kind"))
                .selected_text(k.clone())
                .width(120.0)
                .show_ui(ui, |ui| {
                    for word in item_kinds() {
                        if ui.selectable_label(k == word, word).clicked() {
                            *k = word.to_string();
                        }
                    }
                });
        }
        Term::Slot(mask) => {
            let places = slots();
            let chosen = places
                .iter()
                .find(|(_, m)| m == mask)
                .map(|(w, _)| *w)
                .unwrap_or("anywhere");
            egui::ComboBox::from_id_salt(format!("{salt}.slot"))
                .selected_text(chosen)
                .width(120.0)
                .show_ui(ui, |ui| {
                    for (word, m) in places {
                        ui.selectable_value(mask, m, word);
                    }
                });
        }
        Term::Tier(tier) => {
            egui::ComboBox::from_id_salt(format!("{salt}.tier"))
                .selected_text(tier.word())
                .width(110.0)
                .show_ui(ui, |ui| {
                    for t in Tier::ALL {
                        ui.selectable_value(tier, t, t.word());
                    }
                });
        }
        Term::Wielded | Term::Unappraised => {}
        Term::Num(key, op, value) => {
            egui::ComboBox::from_id_salt(format!("{salt}.num"))
                .selected_text(profile::num_word(*key))
                .width(120.0)
                .show_ui(ui, |ui| {
                    for k in NUM_KEYS {
                        ui.selectable_value(key, k, profile::num_word(k));
                    }
                });
            op_box(ui, &format!("{salt}.op"), op);
            ui.add(egui::DragValue::new(value).speed(10.0));
        }
    }
}

/// One condition: what it asks about, the fields that question needs,
/// and the marker when answering it costs a round trip. A pattern that
/// will not compile is said underneath.
fn ask_row(ui: &mut egui::Ui, salt: &str, ask: &mut Ask, editor: &mut Editor) {
    ui.horizontal_wrapped(|ui| {
        let mut kind = AskKind::of(ask);
        egui::ComboBox::from_id_salt(format!("{salt}.ask"))
            .selected_text(kind.label())
            .width(116.0)
            .show_ui(ui, |ui| {
                for k in AskKind::ALL {
                    ui.selectable_value(&mut kind, k, k.label())
                        .on_hover_text(k.help());
                }
            });
        // A different question asks for different fields, so the old
        // ones are dropped rather than half-carried over.
        if kind != AskKind::of(ask) {
            *ask = kind.blank();
        }
        match ask {
            Ask::Item(term) => term_fields(ui, salt, term),
            Ask::Me(mine) => mine_fields(ui, salt, mine),
            Ask::Prop {
                kind,
                id,
                op,
                value,
            } => {
                egui::ComboBox::from_id_salt(format!("{salt}.propkind"))
                    .selected_text(kind.kind().label())
                    .width(92.0)
                    .show_ui(ui, |ui| {
                        for k in PropKind::ALL {
                            ui.selectable_value(kind, k, k.kind().label());
                        }
                    });
                prop_picker(ui, &format!("{salt}.prop"), kind.kind(), id, editor);
                op_box(ui, &format!("{salt}.op"), op);
                ui.add(egui::DragValue::new(value).speed(1.0));
            }
            Ask::Text { id, op, value } => {
                prop_picker(
                    ui,
                    &format!("{salt}.text"),
                    properties::Kind::Text,
                    id,
                    editor,
                );
                text_op_box(ui, &format!("{salt}.textop"), op);
                text_field(ui, &format!("{salt}.value"), value, "e.g. Sedgemail", 150.0);
            }
            Ask::Spell { op, value } => {
                text_op_box(ui, &format!("{salt}.spellop"), op);
                text_field(
                    ui,
                    &format!("{salt}.spellvalue"),
                    value,
                    "e.g. ^Legendary ",
                    180.0,
                );
            }
        }
        if ask.needs_id() {
            id_marker(ui);
        }
    });
    if let Some(problem) = ask_pattern_error(ask) {
        ui.label(egui::RichText::new(problem).small().color(BAD));
    }
}

/// The conditions of the rule open, with a line to add one.
fn rule_editor(ui: &mut egui::Ui, salt: &str, rule: &mut Rule, editor: &mut Editor) {
    ui.horizontal(|ui| {
        caption(ui, "name");
        text_field(
            ui,
            &format!("{salt}.name"),
            &mut rule.name,
            "what this rule is for",
            170.0,
        );
        action_box(ui, &format!("{salt}.action"), &mut rule.action);
    });
    ui.horizontal(|ui| {
        let mut capped = rule.keep_up_to.is_some();
        if ui
            .checkbox(&mut capped, "stop once I carry")
            .on_hover_text("The rule stands aside once this many are already carried")
            .changed()
        {
            rule.keep_up_to = capped.then_some(1);
        }
        if let Some(cap) = &mut rule.keep_up_to {
            ui.add(egui::DragValue::new(cap).speed(1.0).range(1..=10_000));
        }
    });
    caption(ui, "every one of these must hold");
    let mut drop = None;
    for i in 0..rule.all.len() {
        ui.horizontal_top(|ui| {
            if ui
                .add(egui::Button::new("x").small())
                .on_hover_text("Drop this condition")
                .clicked()
            {
                drop = Some(i);
            }
            ui.vertical(|ui| {
                ask_row(ui, &format!("{salt}.ask{i}"), &mut rule.all[i], editor);
            });
        });
    }
    if let Some(i) = drop {
        rule.all.remove(i);
    }
    if rule.all.is_empty() {
        ui.label(
            egui::RichText::new("nothing asked: this rule claims every item")
                .small()
                .color(WARN),
        );
    }
    ui.horizontal(|ui| {
        let mut adding: Option<AskKind> = None;
        egui::ComboBox::from_id_salt(format!("{salt}.add"))
            .selected_text("add a condition")
            .width(140.0)
            .show_ui(ui, |ui| {
                for k in AskKind::ALL {
                    if ui
                        .selectable_label(false, k.label())
                        .on_hover_text(k.help())
                        .clicked()
                    {
                        adding = Some(k);
                    }
                }
            });
        if let Some(k) = adding {
            rule.all.push(k.blank());
        }
    });
}

/// The shelf on the left: the profiles, and the buttons that make,
/// copy, rename and delete one.
fn shelf(ui: &mut egui::Ui, v: &ProfilesView, editor: &mut Editor, a: &mut Actions) {
    caption(ui, "profiles");
    egui::ScrollArea::vertical()
        .id_salt("loot_profiles.shelf")
        .max_height(300.0)
        .show(ui, |ui| {
            ui.set_min_width(140.0);
            if v.names.is_empty() {
                caption(ui, "(none yet)");
            }
            for name in &v.names {
                let open = v.profile.as_ref().is_some_and(|p| &p.name == name);
                if ui.selectable_label(open, name).clicked() {
                    a.select = Some(name.clone());
                    editor.open_rule = None;
                    editor.confirm_delete = false;
                }
            }
        });
    ui.separator();
    ui.horizontal(|ui| {
        if ui
            .add(egui::Button::new("New").small())
            .on_hover_text("Start an empty profile")
            .clicked()
        {
            editor.new_name = Some(unused_name(&v.names, "profile"));
        }
        if ui
            .add_enabled(v.profile.is_some(), egui::Button::new("Copy").small())
            .on_hover_text("A second profile with the same rules, to change freely")
            .clicked()
        {
            if let Some(p) = &v.profile {
                let mut copy = p.clone();
                copy.name = unused_name(&v.names, &format!("{} copy", p.name));
                a.select = Some(copy.name.clone());
                a.put.push(copy);
            }
        }
    });
    ui.horizontal(|ui| {
        if ui
            .add_enabled(v.profile.is_some(), egui::Button::new("Rename").small())
            .clicked()
        {
            if let Some(p) = &v.profile {
                editor.rename = Some((p.name.clone(), p.name.clone()));
            }
        }
        let delete = if editor.confirm_delete {
            egui::Button::new(egui::RichText::new("Really?").color(BAD)).small()
        } else {
            egui::Button::new("Delete").small()
        };
        if ui
            .add_enabled(v.profile.is_some(), delete)
            .on_hover_text("Delete the profile and its file")
            .clicked()
        {
            match (editor.confirm_delete, &v.profile) {
                (true, Some(p)) => {
                    a.delete = Some(p.name.clone());
                    editor.confirm_delete = false;
                }
                _ => editor.confirm_delete = true,
            }
        }
    });
    // Naming a new profile, or renaming the one open: the same shape,
    // and both refuse a name already on the shelf rather than quietly
    // writing over it.
    if let Some(name) = &mut editor.new_name {
        ui.horizontal(|ui| {
            text_field(ui, "loot_profiles.new", name, "name", 100.0);
            let taken = v.names.iter().any(|n| n.eq_ignore_ascii_case(name.trim()));
            if ui
                .add_enabled(
                    !taken && !name.trim().is_empty(),
                    egui::Button::new("make").small(),
                )
                .clicked()
            {
                let made = Profile {
                    name: name.trim().to_string(),
                    ..Default::default()
                };
                a.select = Some(made.name.clone());
                a.put.push(made);
            }
            if taken {
                ui.label(egui::RichText::new("taken").small().color(WARN));
            }
        });
    }
    if editor.new_name.is_some() && a.select.is_some() {
        editor.new_name = None;
    }
    if let Some((from, to)) = &mut editor.rename {
        ui.horizontal(|ui| {
            text_field(ui, "loot_profiles.rename", to, "new name", 100.0);
            let taken = v
                .names
                .iter()
                .any(|n| n.eq_ignore_ascii_case(to.trim()) && !n.eq_ignore_ascii_case(from));
            if ui
                .add_enabled(
                    !taken && !to.trim().is_empty(),
                    egui::Button::new("rename").small(),
                )
                .clicked()
            {
                a.rename = Some((from.clone(), to.trim().to_string()));
            }
            if taken {
                ui.label(egui::RichText::new("taken").small().color(WARN));
            }
        });
    }
    if a.rename.is_some() {
        editor.rename = None;
    }
}

/// The rules of the profile open, in the order they are read.
fn rules(ui: &mut egui::Ui, p: &mut Profile, editor: &mut Editor) {
    let mut drop = None;
    let mut moved = None;
    let mut copy = None;
    let n = p.rules.len();
    for i in 0..n {
        let open = editor.open_rule == Some(i);
        ui.horizontal(|ui| {
            // Straight to the library, so a rule switched off here is
            // off everywhere before the next corpse.
            ui.checkbox(&mut p.rules[i].on, "")
                .on_hover_text("Off without being deleted");
            let name = if p.rules[i].name.trim().is_empty() {
                format!("rule {}", i + 1)
            } else {
                p.rules[i].name.clone()
            };
            if ui
                .selectable_label(open, name)
                .on_hover_text("Open its conditions")
                .clicked()
            {
                editor.open_rule = if open { None } else { Some(i) };
            }
            action_box(
                ui,
                &format!("loot_profiles.rule{i}.act"),
                &mut p.rules[i].action,
            );
            if p.rules[i].needs_id() {
                id_marker(ui);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new("x").small())
                    .on_hover_text("Delete this rule")
                    .clicked()
                {
                    drop = Some(i);
                }
                if ui
                    .add(egui::Button::new("+").small())
                    .on_hover_text("A copy of this rule, under it")
                    .clicked()
                {
                    copy = Some(i);
                }
                if ui
                    .add_enabled(i + 1 < n, egui::Button::new("v").small())
                    .on_hover_text("Read this rule later")
                    .clicked()
                {
                    moved = Some((i, false));
                }
                if ui
                    .add_enabled(i > 0, egui::Button::new("^").small())
                    .on_hover_text("Read this rule earlier")
                    .clicked()
                {
                    moved = Some((i, true));
                }
            });
        });
        let told = p.rules[i].tell();
        ui.add(egui::Label::new(egui::RichText::new(told).small().color(DIM)).wrap());
        if open {
            egui::Frame::new()
                .inner_margin(4)
                .fill(egui::Color32::from_black_alpha(60))
                .show(ui, |ui| {
                    rule_editor(
                        ui,
                        &format!("loot_profiles.rule{i}"),
                        &mut p.rules[i],
                        editor,
                    );
                });
        }
        ui.add_space(2.0);
    }
    if let Some(i) = drop {
        p.rules.remove(i);
        editor.open_rule = None;
    }
    if let Some(i) = copy {
        let mut made = p.rules[i].clone();
        made.name = format!("{} copy", made.name);
        p.rules.insert(i + 1, made);
        editor.open_rule = Some(i + 1);
    }
    if let Some((i, up)) = moved {
        if move_entry(&mut p.rules, i, up) {
            // The editor follows the rule it had open rather than the
            // place it used to sit in.
            if editor.open_rule == Some(i) {
                editor.open_rule = Some(if up { i - 1 } else { i + 1 });
            }
        }
    }
}

/// The panel. Returns what was asked for; every edit to the profile
/// open comes back as a `put`, so nothing waits on an Apply button.
pub fn draw(egui: &egui::Context, v: &ProfilesView, editor: &mut Editor) -> Actions {
    let mut a = Actions::default();
    let mut edited = v.profile.clone();
    let size = egui::vec2(640.0, 580.0);
    let rect = egui.viewport_rect();
    window(
        "loot_profiles",
        egui::pos2(rect.width() * 0.5 - size.x * 0.5, 60.0),
        size,
        240,
        8,
    )
    .show(egui, |ui| {
        ui.set_min_size(size - egui::vec2(16.0, 16.0));
        title_bar(ui, "loot_profiles", "Loot profiles");
        ui.horizontal(|ui| {
            caption(ui, "every profile is a file in");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new("Reload").small())
                    .on_hover_text("Read the folder again, for a profile dropped in")
                    .clicked()
                {
                    a.reload = true;
                }
                if ui
                    .add(egui::Button::new("Open folder").small())
                    .on_hover_text(
                        "Show the folder: a profile is a file, and sharing one is sending it",
                    )
                    .clicked()
                {
                    a.reveal = true;
                }
            });
        });
        // On its own line and wrapped: a long path would otherwise run
        // under the buttons.
        ui.add(
            egui::Label::new(
                egui::RichText::new(v.dir.display().to_string())
                    .small()
                    .color(DIM),
            )
            .wrap(),
        );
        ui.separator();
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(150.0);
                shelf(ui, v, editor, &mut a);
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.set_width(456.0);
                let Some(p) = &mut edited else {
                    ui.add_space(40.0);
                    ui.label(
                        egui::RichText::new(
                            "No profile open. Pick one on the left, or start a new one.",
                        )
                        .color(DIM),
                    );
                    return;
                };
                ui.horizontal(|ui| {
                    title(ui, p.name.clone());
                    if p.needs_id() {
                        id_marker(ui);
                    }
                });
                ui.add(
                    egui::TextEdit::multiline(&mut p.note)
                        .id_salt("loot_profiles.note")
                        .hint_text("what this profile is for")
                        .desired_rows(2)
                        .desired_width(440.0),
                );
                ui.add_space(4.0);
                caption(
                    ui,
                    "rules, in order: the first that claims an item decides it",
                );
                egui::ScrollArea::vertical()
                    .id_salt("loot_profiles.rules")
                    .max_height(240.0)
                    .show(ui, |ui| {
                        ui.set_min_width(440.0);
                        if p.rules.is_empty() {
                            caption(ui, "(no rules: nothing is taken)");
                        }
                        rules(ui, p, editor);
                    });
                if ui
                    .add(egui::Button::new("Add a rule").small())
                    .on_hover_text("A new rule at the end, read last")
                    .clicked()
                {
                    p.rules.push(Rule {
                        name: format!("rule {}", p.rules.len() + 1),
                        ..Default::default()
                    });
                    editor.open_rule = Some(p.rules.len() - 1);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    caption(ui, "what would this do?");
                    if !v.who.is_empty() {
                        caption(ui, format!("carried by {}", v.who));
                    }
                });
                egui::ScrollArea::vertical()
                    .id_salt("loot_profiles.trial")
                    .max_height(120.0)
                    .show(ui, |ui| {
                        ui.set_min_width(440.0);
                        if v.trial.is_empty() {
                            caption(ui, "(nothing carried)");
                        }
                        egui::Grid::new("loot_profiles.trial.grid")
                            .num_columns(2)
                            .spacing([10.0, 2.0])
                            .show(ui, |ui| {
                                for t in &v.trial {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(t.item.clone()).small(),
                                        )
                                        .wrap(),
                                    );
                                    let (words, colour) = verdict_words(&t.verdict);
                                    ui.label(egui::RichText::new(words).small().color(colour));
                                    ui.end_row();
                                }
                            });
                    });
            });
        });
        if !editor.status.is_empty() {
            ui.label(
                egui::RichText::new(editor.status.clone())
                    .small()
                    .color(BAD),
            );
        }
    });
    if super::closed("loot_profiles") {
        a.close = true;
    }
    // Anything touched this frame goes to the library at once: that is
    // what makes an edit reach every character before the next corpse.
    if let (Some(edited), Some(was)) = (edited, &v.profile) {
        if &edited != was {
            a.put.push(edited);
        }
    }
    a
}

/// Show the folder in the file manager, so a profile can be copied out
/// or dropped in without a file dialog.
fn reveal(dir: &std::path::Path) -> std::io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    // The folder is only made when the first profile is saved, so it
    // may not be there yet; a folder that cannot be opened is nothing
    // to look at.
    std::fs::create_dir_all(dir)?;
    std::process::Command::new(opener).arg(dir).spawn()?;
    Ok(())
}

#[derive(Default)]
pub struct LootProfiles {
    source: Source<ProfilesView>,
    /// Open (bindable from the menu). Starts closed.
    pub show: bool,
    /// The profile being edited, kept between runs.
    chosen: String,
    editor: Editor,
}

impl LootProfiles {
    /// A shelf with one profile on it, filled in enough to show what
    /// the panel is for: a cheap rule that decides without an
    /// appraisal, and one that cannot.
    pub fn demo() -> Self {
        let profile = Profile {
            name: "Rares and rings".into(),
            note: "what the party takes on a drudge run".into(),
            rules: vec![
                Rule {
                    name: "anything dear".into(),
                    action: LootAction::Keep,
                    all: vec![Ask::Item(Term::Num(NumKey::Value, Op::Ge, 5_000.0))],
                    ..Default::default()
                },
                Rule {
                    name: "rings with two epics".into(),
                    action: LootAction::Keep,
                    all: vec![
                        Ask::Item(Term::Slot(ac_client::items::slot::FINGER)),
                        Ask::Item(Term::Num(NumKey::Epics, Op::Ge, 2.0)),
                    ],
                    ..Default::default()
                },
                Rule {
                    name: "broken keys, if I can mend them".into(),
                    action: LootAction::Keep,
                    keep_up_to: Some(2),
                    all: vec![
                        Ask::Item(Term::Word("broken".into())),
                        Ask::Me(Mine::Skill {
                            skill: ac_world::stats::skill::LOCKPICK,
                            op: Op::Ge,
                            level: 250,
                        }),
                    ],
                    ..Default::default()
                },
                Rule {
                    name: "poor armour for salvage".into(),
                    action: LootAction::Salvage,
                    all: vec![
                        Ask::Item(Term::Kind("armor".into())),
                        Ask::Item(Term::Num(NumKey::Workmanship, Op::Lt, 6.0)),
                    ],
                    ..Default::default()
                },
            ],
        };
        LootProfiles {
            source: Source::Demo(ProfilesView {
                names: vec!["Rares and rings".into(), "Sell everything".into()],
                profile: Some(profile),
                dir: default_dir(),
                who: "Aldric".into(),
                trial: vec![
                    Trial {
                        item: "Ornate Ring".into(),
                        verdict: Verdict::Decided(LootAction::Keep, "anything dear".into()),
                    },
                    Trial {
                        item: "Platemail Girth".into(),
                        verdict: Verdict::NeedsId("poor armour for salvage".into()),
                    },
                    Trial {
                        item: "Broken Marble Key (2)".into(),
                        verdict: Verdict::Decided(
                            LootAction::Keep,
                            "broken keys, if I can mend them".into(),
                        ),
                    },
                    Trial {
                        item: "Rusty Nail".into(),
                        verdict: Verdict::None,
                    },
                ],
            }),
            show: true,
            chosen: "Rares and rings".into(),
            editor: Editor::default(),
        }
    }

    /// The library this session reads. Every session in the process
    /// shares one, so an edit here is an edit for all of them.
    fn library(cx: &mut Ctx) -> std::sync::Arc<Library> {
        cx.try_client()
            .map(|c| c.profiles.clone())
            .unwrap_or_else(Library::shared)
    }

    /// Write a profile and put it in front of every character at once.
    fn put(&mut self, library: &Library, p: Profile) {
        if let Err(e) = library.put(p) {
            tracing::warn!("loot profiles: {e}");
            self.editor.status = format!("could not save: {e}");
        } else {
            self.editor.status.clear();
        }
    }

    /// Apply what the panel asked for. The demo has no library behind
    /// it, so there only the profile shown changes; the buttons are
    /// there to be looked at.
    fn apply(&mut self, cx: &mut Ctx, a: Actions) {
        if a.close {
            self.show = false;
        }
        if let Source::Demo(d) = &mut self.source {
            if let Some(p) = a
                .put
                .into_iter()
                .find(|p| Some(&p.name) == d.profile.as_ref().map(|c| &c.name))
            {
                d.profile = Some(p);
            }
            return;
        }
        let library = Self::library(cx);
        if a.reveal {
            if let Err(e) = reveal(&dir_of(&library)) {
                tracing::warn!("loot profiles: cannot open the folder: {e}");
                self.editor.status = format!("could not open the folder: {e}");
            }
        }
        if a.reload {
            let n = library.reload();
            self.editor.status.clear();
            tracing::info!("loot profiles: {n} read from {}", library.dir().display());
        }
        for p in a.put {
            self.put(&library, p);
        }
        if let Some((from, to)) = a.rename {
            // Written under the new name first: a rename that fails
            // half way leaves the profile where it was rather than
            // nowhere.
            if let Some(p) = library.get(&from) {
                let mut moved = (*p).clone();
                moved.name = to.clone();
                self.put(&library, moved);
                if self.editor.status.is_empty() {
                    if let Err(e) = library.remove(&from) {
                        tracing::warn!("loot profiles: {e}");
                        self.editor.status = format!("renamed, but the old file stayed: {e}");
                    }
                    self.chosen = to;
                    self.editor.open_rule = None;
                }
            }
        }
        if let Some(name) = a.delete {
            if let Err(e) = library.remove(&name) {
                tracing::warn!("loot profiles: {e}");
                self.editor.status = format!("could not delete: {e}");
            }
            if self.chosen == name {
                self.chosen.clear();
                self.editor.open_rule = None;
            }
        }
        if let Some(name) = a.select {
            self.chosen = name;
            self.editor.open_rule = None;
        }
    }
}

impl Plugin for LootProfiles {
    fn name(&self) -> &str {
        "loot_profiles"
    }

    fn load(&mut self, settings: &Settings) {
        if let Some(v) = settings.get("loot_profiles.show") {
            self.show = v;
        }
        if let Some(v) = settings.get::<String>("loot_profiles.chosen") {
            self.chosen = v;
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("loot_profiles.show", self.show);
        settings.set("loot_profiles.chosen", &self.chosen);
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if let Some(ask) = super::take_open(cx.board, "loot_profiles") {
            self.show = ask.apply(self.show);
        }
        if !self.show {
            return;
        }
        let chosen = self.chosen.clone();
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().map(|c| view(c, &chosen)),
        };
        let Some(v) = v else { return };
        let actions = draw(egui, &v, &mut self.editor);
        self.apply(cx, actions);
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if pressed && crate::keys::bound("loot_profiles", key) {
            self.show = !self.show;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_profile_gets_a_name_nobody_has() {
        let names = vec!["Rares".to_string(), "Rares 2".to_string()];
        assert_eq!(unused_name(&names, "Peas"), "Peas");
        // A name already taken is matched however it is typed, so two
        // profiles cannot end up in one file on a case-insensitive
        // disk; the case the player typed is what they get back.
        assert_eq!(unused_name(&names, "rares"), "rares 3");
        assert_eq!(unused_name(&names, "Rares"), "Rares 3");
        assert_eq!(unused_name(&[], "  "), "profile");
    }

    #[test]
    fn rules_move_one_step_and_stop_at_the_ends() {
        let mut list = vec![1, 2, 3];
        assert!(!move_entry(&mut list, 0, true));
        assert!(!move_entry(&mut list, 2, false));
        assert!(!move_entry(&mut list, 9, true));
        assert!(move_entry(&mut list, 0, false));
        assert_eq!(list, vec![2, 1, 3]);
        assert!(move_entry(&mut list, 2, true));
        assert_eq!(list, vec![2, 3, 1]);
    }

    #[test]
    fn changing_what_a_condition_asks_gives_it_the_right_fields() {
        // Every sort of question round-trips: the blank one it makes is
        // read back as the same sort, so the dropdown cannot land on a
        // choice that then shows as another.
        for kind in AskKind::ALL {
            assert_eq!(AskKind::of(&kind.blank()), kind, "{}", kind.label());
        }
        // And the ones that cost a round trip are the ones the model
        // says do.
        assert!(!AskKind::Name.blank().needs_id());
        assert!(!AskKind::Me.blank().needs_id());
        assert!(AskKind::Prop.blank().needs_id());
        assert!(AskKind::Spell.blank().needs_id());
    }

    #[test]
    fn a_pattern_that_will_not_compile_is_named_before_it_is_run() {
        let bad = Ask::Spell {
            op: TextOp::Like,
            value: "^Legendary (".into(),
        };
        assert!(ask_pattern_error(&bad).is_some());
        let good = Ask::Spell {
            op: TextOp::Like,
            value: "^Legendary ".into(),
        };
        assert!(ask_pattern_error(&good).is_none());
        // A plain word is never a pattern, however it is punctuated.
        let word = Ask::Spell {
            op: TextOp::Has,
            value: "blood drinker (".into(),
        };
        assert!(ask_pattern_error(&word).is_none());
        assert!(ask_pattern_error(&Ask::Item(Term::Word("(".into()))).is_none());
    }

    #[test]
    fn every_place_is_offered_once_and_every_kind_at_all() {
        let places = slots();
        for (i, (_, mask)) in places.iter().enumerate() {
            assert!(
                !places[i + 1..].iter().any(|(_, m)| m == mask),
                "slot {mask:#x} offered twice"
            );
        }
        assert!(places.iter().any(|(w, _)| *w == "ring"));
        let kinds = item_kinds();
        assert!(kinds.contains(&"armor"));
        assert!(kinds.contains(&"weapon"));
        assert!(kinds.contains(&"gem"));
    }

    #[test]
    fn a_verdict_reads_as_what_would_happen() {
        let (words, _) = verdict_words(&Verdict::Decided(LootAction::Keep, "rares".into()));
        assert_eq!(words, "keep — rares");
        let (words, colour) = verdict_words(&Verdict::NeedsId("armour sets".into()));
        assert_eq!(words, "identify first — armour sets");
        assert_eq!(colour, WARN);
        let (words, _) = verdict_words(&Verdict::None);
        assert_eq!(words, "left alone");
    }

    /// One frame of the panel with no renderer behind it: egui hands
    /// back texture changes it expects somebody to apply, and drops
    /// loudly if nobody does.
    fn frame(ctx: &egui::Context, view: &ProfilesView, editor: &mut Editor) -> Actions {
        let mut asked = Actions::default();
        let mut out = ctx.run_ui(Default::default(), |ui| {
            asked = draw(ui.ctx(), view, editor);
        });
        out.textures_delta.clear();
        asked
    }

    fn demo_view() -> ProfilesView {
        match LootProfiles::demo().source {
            Source::Demo(v) => v,
            Source::Live => unreachable!(),
        }
    }

    #[test]
    fn the_demo_shows_a_profile_worth_reading() {
        let d = demo_view();
        let p = d.profile.clone().expect("a profile is open");
        assert_eq!(p.name, "Rares and rings");
        // The first rule decides without a round trip; the profile as a
        // whole needs one, which is what the marker says.
        assert!(!p.rules[0].needs_id());
        assert!(p.needs_id());
        assert!(d
            .trial
            .iter()
            .any(|t| matches!(t.verdict, Verdict::NeedsId(_))));
    }

    #[test]
    fn a_frame_that_changes_nothing_asks_for_nothing() {
        // Every edit is saved as it is made, so a frame in which
        // nothing was touched must ask for nothing: otherwise the file
        // would be written, and every character handed new rules, on
        // every frame the panel is open.
        let view = demo_view();
        let mut editor = Editor::default();
        let ctx = egui::Context::default();
        assert_eq!(frame(&ctx, &view, &mut editor), Actions::default());
        // Again with a rule's conditions open, which is where nearly
        // every widget of the panel lives.
        editor.open_rule = Some(2);
        assert_eq!(frame(&ctx, &view, &mut editor), Actions::default());
        assert_eq!(editor.open_rule, Some(2));
    }
}
