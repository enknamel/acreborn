//! Loot profiles: what to pick up, and what to sell.
//!
//! A profile is an ordered list of rules; the first whose every
//! condition holds decides what happens to an item, and an item no rule
//! matches is left alone. That is the shape players already work in,
//! and it is the shape an editor can show honestly: a rule is a name, a
//! disposition, and a list of things that must all be true.
//!
//! Two things a rule can ask about.
//!
//! The item, which is the obvious half: its name, kind, worth,
//! workmanship, armour, the spells on it. And **the character**, which
//! is the half that makes a profile worth sharing. One profile is
//! handed to a whole group and each member reads it differently: the
//! one with the lockpick skill picks up the broken keys that only
//! lockpicking can mend, and nobody else carries them home. Without
//! that, a group needs a profile per character and no profile can be
//! passed to a friend.
//!
//! Profiles are files ([`Profile::save`]), one JSON document each, so
//! that sharing one is sending a file.
//!
//! # Deciding without an appraisal
//!
//! Half of what a rule can ask needs the server to be asked about the
//! item first, and that costs a round trip per item on every corpse.
//! So the rules are read in order and the walk stops at the first rule
//! that *could* match but cannot be judged yet ([`Verdict::NeedsId`]).
//! A profile whose early rules ask only about the cheap fields decides
//! most items without asking the server anything, which is the whole
//! reason players tune profiles this way.

use crate::autoplay::LootAction;
use crate::items::{ItemStats, NumKey, Op, Term};
use crate::weapons::Wielder;
use ac_net::messages::Appraisal;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Something a rule asks about the character holding it.
///
/// Everything here is a feature of the character: who it is and what it
/// can do. Nothing here is about the moment -- where it stands, what it
/// is fighting -- because a profile is a standing policy, not a plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Mine {
    /// The character's name contains this, case-insensitively.
    Name(String),
    /// This skill stands at least this high, buffs counted.
    Skill { skill: u32, op: Op, level: u32 },
    /// This skill is at least trained (2) or specialised (3): see
    /// `ac_world::stats::sac`.
    Trained { skill: u32, at_least: u32 },
    /// The character's level.
    Level { op: Op, level: u32 },
}

impl Mine {
    /// Whether it holds for this character.
    pub fn holds(&self, me: &Wielder, name: &str) -> bool {
        match self {
            Mine::Name(want) => {
                let want = want.trim();
                !want.is_empty() && name.to_lowercase().contains(&want.to_lowercase())
            }
            Mine::Skill { skill, op, level } => op.test(me.skill(*skill) as f64, *level as f64),
            Mine::Trained { skill, at_least } => me.advancement_of(*skill) >= *at_least,
            Mine::Level { op, level } => op.test(me.level as f64, *level as f64),
        }
    }

    /// Said in words, for the editor and the log.
    pub fn tell(&self) -> String {
        use ac_world::stats::skill_name;
        match self {
            Mine::Name(n) => format!("my name has \"{n}\""),
            Mine::Skill { skill, op, level } => {
                format!("my {} {} {level}", skill_name(*skill), op.word())
            }
            Mine::Trained { skill, at_least } => format!(
                "my {} is {}",
                skill_name(*skill),
                ac_world::stats::sac_name(*at_least)
            ),
            Mine::Level { op, level } => format!("my level {} {level}", op.word()),
        }
    }
}

/// How a piece of text is matched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextOp {
    /// Contains it, ignoring case. What most rules want.
    #[default]
    Has,
    /// Does not contain it. The plain way to say "armour, but not
    /// Covenant", which players write as a lookahead when their tool
    /// only offers a positive match.
    HasNot,
    /// Matches this regular expression.
    Like,
    /// Does not match it.
    Unlike,
}

impl TextOp {
    pub const ALL: [TextOp; 4] = [TextOp::Has, TextOp::HasNot, TextOp::Like, TextOp::Unlike];

    pub fn word(self) -> &'static str {
        match self {
            TextOp::Has => "contains",
            TextOp::HasNot => "does not contain",
            TextOp::Like => "matches",
            TextOp::Unlike => "does not match",
        }
    }

    /// Whether it is a regular expression rather than a plain word.
    pub fn is_regex(self) -> bool {
        matches!(self, TextOp::Like | TextOp::Unlike)
    }

    /// Whether `text` answers this test against `pattern`.
    ///
    /// A pattern that will not compile matches nothing rather than
    /// everything: a rule with a typo in it should take no loot, not
    /// all of it.
    pub fn holds(self, text: &str, pattern: &str) -> bool {
        match self {
            TextOp::Has => contains_fold(text, pattern),
            TextOp::HasNot => !contains_fold(text, pattern),
            TextOp::Like => regex_for(pattern).is_some_and(|r| r.is_match(text)),
            TextOp::Unlike => !regex_for(pattern).is_some_and(|r| r.is_match(text)),
        }
    }
}

fn contains_fold(text: &str, needle: &str) -> bool {
    let needle = needle.trim();
    !needle.is_empty() && text.to_lowercase().contains(&needle.to_lowercase())
}

/// The compiled form of a pattern, kept so that a rule read on every
/// item of every corpse compiles its regular expressions once.
fn regex_for(pattern: &str) -> Option<regex::Regex> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<std::collections::HashMap<String, Option<regex::Regex>>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut cache = cache.lock().ok()?;
    cache
        .entry(pattern.to_string())
        .or_insert_with(|| {
            regex::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .size_limit(1 << 20)
                .build()
                .map_err(|e| tracing::warn!("loot rule: {pattern:?} is not a pattern: {e}"))
                .ok()
        })
        .clone()
}

/// Whether a pattern will compile, for the editor to say so before the
/// rule is ever run.
pub fn pattern_error(pattern: &str) -> Option<String> {
    regex::Regex::new(pattern).err().map(|e| e.to_string())
}

/// One condition of a rule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Ask {
    /// About the item: every term the search language understands.
    Item(Term),
    /// About the character reading the profile.
    Me(Mine),
    /// About any property the server sent when it identified the item,
    /// by the server's own number: `ac_world::properties`.
    ///
    /// The friendly terms above cover what most rules want. This covers
    /// the rest, which is everything: what set a piece of armour
    /// belongs to, what a rare's icon sits on, how much of a salvage
    /// bag is left. Numbers, flags and data ids are all compared as
    /// numbers, a flag being 1 or 0.
    Prop {
        kind: PropKind,
        id: u32,
        op: Op,
        value: f64,
    },
    /// The same, for a property whose value is text.
    Text { id: u32, op: TextOp, value: String },
    /// Any spell on the item whose name answers this. `Like` with
    /// `^Legendary ` is how a rule asks for a legendary cantrip of any
    /// kind, which is a thing every profile wants and no list of spell
    /// names can keep up with.
    Spell { op: TextOp, value: String },
}

/// Which bag of an identify a property lives in. A mirror of
/// `ac_world::properties::Kind` that can be written to a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PropKind {
    Int,
    Int64,
    Bool,
    Float,
    DataId,
}

impl PropKind {
    pub const ALL: [PropKind; 5] = [
        PropKind::Int,
        PropKind::Int64,
        PropKind::Bool,
        PropKind::Float,
        PropKind::DataId,
    ];

    pub fn kind(self) -> ac_world::properties::Kind {
        use ac_world::properties::Kind;
        match self {
            PropKind::Int => Kind::Int,
            PropKind::Int64 => Kind::Int64,
            PropKind::Bool => Kind::Bool,
            PropKind::Float => Kind::Float,
            PropKind::DataId => Kind::DataId,
        }
    }

    /// The property's value on this identify, as a number.
    fn read(self, id: u32, of: &Appraisal) -> Option<f64> {
        match self {
            PropKind::Int => of
                .ints
                .iter()
                .find(|(k, _)| *k == id)
                .map(|(_, v)| *v as f64),
            PropKind::Int64 => of
                .int64s
                .iter()
                .find(|(k, _)| *k == id)
                .map(|(_, v)| *v as f64),
            PropKind::Bool => of
                .bools
                .iter()
                .find(|(k, _)| *k == id)
                .map(|(_, v)| u32::from(*v) as f64),
            PropKind::Float => of.floats.iter().find(|(k, _)| *k == id).map(|(_, v)| *v),
            PropKind::DataId => of
                .dids
                .iter()
                .find(|(k, _)| *k == id)
                .map(|(_, v)| *v as f64),
        }
    }

    /// What to call it, for the editor.
    pub fn name_of(self, id: u32) -> String {
        ac_world::properties::name_of(self.kind(), id)
            .map(str::to_string)
            .unwrap_or_else(|| format!("{} {id}", self.kind().label()))
    }
}

impl Ask {
    /// Whether answering this needs the server to be asked about the
    /// item first. Asking about the character never does, and nor do
    /// the item's name, kind, worth or workmanship -- they arrive with
    /// the item itself.
    pub fn needs_id(&self) -> bool {
        let term = match self {
            Ask::Me(_) => return false,
            // Everything the server sends on an identify needs the
            // identify, by definition.
            Ask::Prop { .. } | Ask::Text { .. } | Ask::Spell { .. } => return true,
            Ask::Item(t) => t,
        };
        match term {
            Term::Word(_)
            | Term::Kind(_)
            | Term::Material(_)
            | Term::Slot(_)
            | Term::Wielded
            | Term::Unappraised => false,
            Term::Spell(_) | Term::Skill(_) | Term::Tier(_) => true,
            Term::Num(key, _, _) => needs_id(*key),
        }
    }

    /// Said in words.
    pub fn tell(&self) -> String {
        match self {
            Ask::Item(t) => term_words(t),
            Ask::Me(m) => m.tell(),
            Ask::Prop {
                kind,
                id,
                op,
                value,
            } => format!("{} {} {value}", kind.name_of(*id), op.word()),
            Ask::Text { id, op, value } => format!(
                "{} {} \"{value}\"",
                ac_world::properties::name_of(ac_world::properties::Kind::Text, *id)
                    .unwrap_or("text"),
                op.word()
            ),
            Ask::Spell { op, value } => format!("a spell {} \"{value}\"", op.word()),
        }
    }
}

/// Whether a numeric field is only known once the item has been
/// appraised. Worth, burden, workmanship and how many are in the stack
/// come with the item; the rest are the server's to tell.
pub fn needs_id(key: NumKey) -> bool {
    !matches!(
        key,
        NumKey::Value | NumKey::Burden | NumKey::Workmanship | NumKey::Stack
    )
}

/// One rule: a name, what to do, and everything that must be true.
///
/// Every condition must hold. There is no "or" inside a rule on
/// purpose: two rules say it more plainly than one rule with a tree in
/// it, and an editor can show a list of conditions in a way it cannot
/// show a tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    /// What the rule is for, in the player's words.
    pub name: String,
    /// Turned off without being deleted.
    pub on: bool,
    /// What happens to an item this rule claims.
    pub action: LootAction,
    /// Every one of these must hold.
    pub all: Vec<Ask>,
    /// Stop once this many are carried. `None` means no limit.
    pub keep_up_to: Option<u32>,
}

impl Default for Rule {
    fn default() -> Self {
        Rule {
            name: String::new(),
            on: true,
            action: LootAction::Keep,
            all: Vec::new(),
            keep_up_to: None,
        }
    }
}

impl Rule {
    /// Whether judging this rule needs the item appraised.
    pub fn needs_id(&self) -> bool {
        self.all.iter().any(Ask::needs_id)
    }

    /// Whether every condition that can be judged without an appraisal
    /// holds. A rule that fails one of those is out whatever the
    /// server would say, which is what saves the round trip.
    fn cheap_half_holds(&self, item: &ItemStats, me: &Wielder, name: &str) -> bool {
        self.all
            .iter()
            .filter(|a| !a.needs_id())
            .all(|a| holds(a, item, None, me, name))
    }

    /// Whether the whole rule holds.
    fn holds(&self, item: &ItemStats, id: Option<&Appraisal>, me: &Wielder, name: &str) -> bool {
        self.all.iter().all(|a| holds(a, item, id, me, name))
    }

    /// The rule in words, for the editor's summary line.
    pub fn tell(&self) -> String {
        if self.all.is_empty() {
            return "anything".to_string();
        }
        self.all
            .iter()
            .map(Ask::tell)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn holds(ask: &Ask, item: &ItemStats, id: Option<&Appraisal>, me: &Wielder, name: &str) -> bool {
    match ask {
        Ask::Item(t) => item.matches_term(t),
        Ask::Me(m) => m.holds(me, name),
        Ask::Prop {
            kind,
            id: prop,
            op,
            value,
        } => id
            .and_then(|a| kind.read(*prop, a))
            .is_some_and(|x| op.test(x, *value)),
        Ask::Text {
            id: prop,
            op,
            value,
        } => {
            // A property the item does not carry is empty text, so
            // "does not contain" holds for it -- which is what a rule
            // that excludes a set name means.
            let text = id
                .and_then(|a| a.strings.iter().find(|(k, _)| k == prop))
                .map(|(_, v)| v.as_str())
                .unwrap_or("");
            op.holds(text, value)
        }
        Ask::Spell { op, value } => match op {
            // "Any spell that looks like this": true if one does.
            TextOp::Has | TextOp::Like => item.spells.iter().any(|s| op.holds(s, value)),
            // "No spell that looks like this": true only if none does,
            // so the test is turned round rather than asked of each.
            TextOp::HasNot => !item.spells.iter().any(|s| TextOp::Has.holds(s, value)),
            TextOp::Unlike => !item.spells.iter().any(|s| TextOp::Like.holds(s, value)),
        },
    }
}

/// What a profile makes of an item.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// This is what to do with it, and the rule that said so.
    Decided(LootAction, String),
    /// A rule might claim it, but not until the item has been
    /// appraised. Ask the server, then judge again.
    NeedsId(String),
    /// No rule wants it.
    None,
}

/// A named, shareable set of rules.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    /// What it is called, which is also its file name.
    pub name: String,
    /// What it is for, in the author's words.
    pub note: String,
    /// In order. The first rule that claims an item decides it.
    pub rules: Vec<Rule>,
}

impl Profile {
    /// What to do with `item`, judged for this character. `held` is how
    /// many of the thing are already carried, for the rules that cap a
    /// stack.
    ///
    /// Rules are read in order and the first that claims the item wins.
    /// A rule that would claim it but cannot be judged until the item
    /// is appraised stops the reading: what a later rule would have
    /// said does not matter, because this one comes first.
    pub fn judge(
        &self,
        item: &ItemStats,
        id: Option<&Appraisal>,
        me: &Wielder,
        name: &str,
        held: u32,
    ) -> Verdict {
        for rule in self.rules.iter().filter(|r| r.on) {
            if rule.keep_up_to.is_some_and(|cap| held >= cap) {
                continue;
            }
            if !rule.cheap_half_holds(item, me, name) {
                continue;
            }
            if rule.needs_id() && !item.appraised {
                return Verdict::NeedsId(rule.name.clone());
            }
            if rule.holds(item, id, me, name) {
                return Verdict::Decided(rule.action, rule.name.clone());
            }
        }
        Verdict::None
    }

    /// Whether any rule could ever ask for an appraisal. A profile that
    /// cannot decides every corpse without a single round trip.
    pub fn needs_id(&self) -> bool {
        self.rules.iter().filter(|r| r.on).any(Rule::needs_id)
    }

    /// Where a profile of this name lives.
    pub fn path_of(dir: &Path, name: &str) -> PathBuf {
        dir.join(format!("{}.json", tidy_name(name)))
    }

    /// The profiles in `dir`, by name, in alphabetical order.
    pub fn list(dir: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| {
                e.path()
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
            })
            .collect();
        names.sort();
        names
    }

    /// Read one. A profile that will not parse is an error rather than
    /// an empty one: silently looting nothing is worse than saying so.
    pub fn load(dir: &Path, name: &str) -> std::io::Result<Profile> {
        let path = Self::path_of(dir, name);
        let text = std::fs::read_to_string(&path)?;
        let mut p: Profile = serde_json::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if p.name.trim().is_empty() {
            p.name = name.to_string();
        }
        Ok(p)
    }

    /// Write it out, creating the directory if it is not there.
    pub fn save(&self, dir: &Path) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let path = Self::path_of(dir, &self.name);
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&path, text)?;
        Ok(path)
    }
}

impl Profile {
    /// A profile to start from: what a character would take if nobody
    /// had told it anything.
    ///
    /// Ordered the way a profile should be -- the rules that need
    /// nothing from the server first, so that most of a corpse is
    /// settled before anything is identified -- and meant to be edited
    /// rather than obeyed. Every rule in it is one a player would
    /// recognise.
    pub fn starter() -> Profile {
        let word = |w: &str| Ask::Item(Term::Word(w.into()));
        let kind = |k: &str| Ask::Item(Term::Kind(k.into()));
        let num = |k: NumKey, op: Op, v: f64| Ask::Item(Term::Num(k, op, v));
        Profile {
            name: "Starter".into(),
            note: "A place to start: money and components kept, vendor \
                   trash sold, the good things looked over."
                .into(),
            rules: vec![
                Rule {
                    name: "money".into(),
                    action: LootAction::Keep,
                    all: vec![kind("money")],
                    ..Default::default()
                },
                Rule {
                    name: "trade notes".into(),
                    action: LootAction::Keep,
                    all: vec![kind("note")],
                    ..Default::default()
                },
                Rule {
                    name: "spell components".into(),
                    action: LootAction::Keep,
                    all: vec![kind("comps")],
                    ..Default::default()
                },
                Rule {
                    name: "healing kits, a few".into(),
                    action: LootAction::Keep,
                    all: vec![word("healing kit")],
                    keep_up_to: Some(4),
                    ..Default::default()
                },
                // Peas are what a run to town is paid for with.
                Rule {
                    name: "peas to sell".into(),
                    action: LootAction::Sell,
                    all: vec![word("pea")],
                    ..Default::default()
                },
                Rule {
                    name: "gems to sell".into(),
                    action: LootAction::Sell,
                    all: vec![kind("gem"), num(NumKey::Value, Op::Ge, 100.0)],
                    ..Default::default()
                },
                // Salvage is judged on what it is made of and how well,
                // both of which come with the item.
                Rule {
                    name: "good salvage".into(),
                    action: LootAction::Salvage,
                    all: vec![kind("salvage"), num(NumKey::Workmanship, Op::Ge, 8.0)],
                    ..Default::default()
                },
                // Nothing below this is worth a slot, and saying so
                // early keeps the rules under it from asking the server
                // about junk.
                Rule {
                    name: "nothing cheap".into(),
                    action: LootAction::Skip,
                    all: vec![num(NumKey::Value, Op::Le, 250.0)],
                    ..Default::default()
                },
                // From here down the server has to be asked. Every one
                // of these is about a thing worth a round trip.
                Rule {
                    name: "anything legendary".into(),
                    action: LootAction::Keep,
                    all: vec![Ask::Spell {
                        op: TextOp::Like,
                        value: "^Legendary ".into(),
                    }],
                    ..Default::default()
                },
                Rule {
                    name: "anything epic".into(),
                    action: LootAction::Keep,
                    all: vec![Ask::Spell {
                        op: TextOp::Like,
                        value: "^Epic ".into(),
                    }],
                    ..Default::default()
                },
                Rule {
                    name: "armour worth wearing".into(),
                    action: LootAction::Keep,
                    all: vec![kind("armor"), num(NumKey::Armor, Op::Ge, 250.0)],
                    ..Default::default()
                },
                Rule {
                    name: "the rest, to the counter".into(),
                    action: LootAction::Sell,
                    all: vec![num(NumKey::Value, Op::Ge, 1_000.0)],
                    ..Default::default()
                },
            ],
        }
    }
}

/// A name safe to use as a file name: what a player types, less the
/// characters a path cannot hold.
pub fn tidy_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        "profile".to_string()
    } else {
        cleaned
    }
}

/// A term in words, for the editor.
fn term_words(t: &Term) -> String {
    match t {
        Term::Word(w) => format!("name has \"{w}\""),
        Term::Spell(s) => format!("a spell like \"{s}\""),
        Term::Kind(k) => format!("is {k}"),
        Term::Material(m) => format!("made of {m}"),
        Term::Skill(s) => format!("uses {s}"),
        Term::Slot(_) => "worn in that slot".to_string(),
        Term::Tier(t) => format!("has a {} cantrip", t.word()),
        Term::Wielded => "is wielded".to_string(),
        Term::Unappraised => "has not been looked over".to_string(),
        Term::Num(k, op, v) => format!("{} {} {v}", num_word(*k), op.word()),
    }
}

/// A numeric field's name, as the editor lists it.
pub fn num_word(k: NumKey) -> &'static str {
    match k {
        NumKey::Damage => "damage",
        NumKey::Armor => "armour",
        NumKey::Value => "value",
        NumKey::Burden => "burden",
        NumKey::Workmanship => "workmanship",
        NumKey::Speed => "speed",
        NumKey::Wield => "wield level",
        NumKey::Mana => "mana",
        NumKey::Spellcraft => "spellcraft",
        NumKey::Uses => "uses",
        NumKey::Tinks => "tinkers",
        NumKey::Stack => "stack",
        NumKey::Attack => "attack",
        NumKey::Defense => "defense",
        NumKey::Spells => "spells",
        NumKey::Cantrips => "cantrips",
        NumKey::Minors => "minors",
        NumKey::Moderates => "moderates",
        NumKey::Majors => "majors",
        NumKey::Epics => "epics",
        NumKey::Legendaries => "legendaries",
    }
}

#[cfg(test)]
impl Profile {
    /// `judge` for a test that has no raw identify to hand.
    fn judge_test(&self, item: &ItemStats, me: &Wielder, name: &str, held: u32) -> Verdict {
        self.judge(item, None, me, name, held)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::Tier;
    use ac_world::item_type;
    use ac_world::stats::{sac, skill};

    fn item(name: &str, kind: u32, value: u32) -> ItemStats {
        ItemStats {
            name: name.into(),
            item_type: kind,
            kind: crate::items::kind_name(kind),
            value,
            ..Default::default()
        }
    }

    fn me(level: u32, skills: &[(u32, u32, u32)]) -> Wielder {
        Wielder {
            level,
            skills: skills.iter().map(|(s, v, a)| (*s, *v, *v, *a)).collect(),
            ..Default::default()
        }
    }

    fn rule(name: &str, action: LootAction, all: Vec<Ask>) -> Rule {
        Rule {
            name: name.into(),
            action,
            all,
            ..Default::default()
        }
    }

    #[test]
    fn the_first_rule_that_claims_an_item_decides_it() {
        let profile = Profile {
            name: "test".into(),
            note: String::new(),
            rules: vec![
                rule(
                    "keep the good stuff",
                    LootAction::Keep,
                    vec![Ask::Item(Term::Num(NumKey::Value, Op::Ge, 5_000.0))],
                ),
                rule(
                    "sell the rest",
                    LootAction::Sell,
                    vec![Ask::Item(Term::Kind("gem".into()))],
                ),
            ],
        };
        let me = me(50, &[]);
        let dear = item("Ruby", item_type::GEM, 9_000);
        let cheap = item("Quartz", item_type::GEM, 40);
        let boots = item("Leather Boots", item_type::ARMOR, 40);
        assert_eq!(
            profile.judge_test(&dear, &me, "Aldric", 0),
            Verdict::Decided(LootAction::Keep, "keep the good stuff".into())
        );
        assert_eq!(
            profile.judge_test(&cheap, &me, "Aldric", 0),
            Verdict::Decided(LootAction::Sell, "sell the rest".into())
        );
        // Nothing claims it, so it stays on the corpse.
        assert_eq!(profile.judge_test(&boots, &me, "Aldric", 0), Verdict::None);
    }

    #[test]
    fn one_profile_reads_differently_for_each_of_a_group() {
        // The party shares a profile. Only the one who can mend a
        // broken key carries one home.
        let profile = Profile {
            name: "group".into(),
            note: String::new(),
            rules: vec![rule(
                "broken keys, if I can mend them",
                LootAction::Keep,
                vec![
                    Ask::Item(Term::Word("broken".into())),
                    Ask::Me(Mine::Skill {
                        skill: skill::LOCKPICK,
                        op: Op::Ge,
                        level: 250,
                    }),
                ],
            )],
        };
        let key = item("Broken Marble Key", item_type::KEY, 0);
        let picker = me(100, &[(skill::LOCKPICK, 300, sac::TRAINED)]);
        let mage = me(100, &[(skill::LOCKPICK, 0, sac::UNTRAINED)]);
        assert_eq!(
            profile.judge_test(&key, &picker, "Bryn", 0),
            Verdict::Decided(LootAction::Keep, "broken keys, if I can mend them".into())
        );
        assert_eq!(profile.judge_test(&key, &mage, "Aldric", 0), Verdict::None);
    }

    #[test]
    fn an_appraisal_is_asked_for_only_when_it_would_change_the_answer() {
        let profile = Profile {
            name: "test".into(),
            note: String::new(),
            rules: vec![rule(
                "armour with a major on it",
                LootAction::Keep,
                vec![
                    Ask::Item(Term::Kind("armor".into())),
                    Ask::Item(Term::Tier(Tier::Major)),
                ],
            )],
        };
        let me = me(50, &[]);
        // Not armour: judged and dismissed without asking the server,
        // because the cheap half of the rule already fails.
        let gem = item("Ruby", item_type::GEM, 9_000);
        assert_eq!(profile.judge_test(&gem, &me, "Aldric", 0), Verdict::None);
        // Armour: the rest of the rule cannot be judged until the
        // server has been asked.
        let mut plate = item("Platemail Hauberk", item_type::ARMOR, 4_000);
        assert_eq!(
            profile.judge_test(&plate, &me, "Aldric", 0),
            Verdict::NeedsId("armour with a major on it".into())
        );
        // Once appraised it is judged for real.
        plate.appraised = true;
        assert_eq!(profile.judge_test(&plate, &me, "Aldric", 0), Verdict::None);
        plate.spells = vec!["Major Armor".into()];
        assert_eq!(
            profile.judge_test(&plate, &me, "Aldric", 0),
            Verdict::Decided(LootAction::Keep, "armour with a major on it".into())
        );
    }

    #[test]
    fn a_capped_rule_stops_once_enough_are_carried() {
        let profile = Profile {
            name: "test".into(),
            note: String::new(),
            rules: vec![Rule {
                name: "two healing kits".into(),
                keep_up_to: Some(2),
                all: vec![Ask::Item(Term::Word("healing kit".into()))],
                ..Default::default()
            }],
        };
        let me = me(50, &[]);
        let kit = item("Healing Kit", item_type::MISC, 100);
        assert!(matches!(
            profile.judge_test(&kit, &me, "Aldric", 1),
            Verdict::Decided(LootAction::Keep, _)
        ));
        assert_eq!(profile.judge_test(&kit, &me, "Aldric", 2), Verdict::None);
    }

    #[test]
    fn any_property_the_server_sends_can_be_asked_about() {
        // The set a piece of armour belongs to: an int the client has
        // no field of its own for, reached by the server's own number.
        // The server calls it EquipmentSetId; the looter players are
        // used to calls it ArmorSetId, and the editor shows the
        // server's name because that is what the number means.
        const ARMOR_SET_ID: u32 = 265;
        let profile = Profile {
            name: "sets".into(),
            note: String::new(),
            rules: vec![rule(
                "any armour of a set",
                LootAction::Keep,
                vec![Ask::Prop {
                    kind: PropKind::Int,
                    id: ARMOR_SET_ID,
                    op: Op::Ge,
                    value: 1.0,
                }],
            )],
        };
        let me = me(50, &[]);
        let mut plate = item("Platemail Hauberk", item_type::ARMOR, 4_000);
        plate.appraised = true;
        // No identify to read: nothing to say yet.
        assert_eq!(profile.judge(&plate, None, &me, "Aldric", 0), Verdict::None);
        // Not part of a set.
        let plain = Appraisal {
            ints: vec![(19, 4_000)],
            ..Default::default()
        };
        assert_eq!(
            profile.judge(&plate, Some(&plain), &me, "Aldric", 0),
            Verdict::None
        );
        // Part of one.
        let set = Appraisal {
            ints: vec![(ARMOR_SET_ID, 12)],
            ..Default::default()
        };
        assert_eq!(
            profile.judge(&plate, Some(&set), &me, "Aldric", 0),
            Verdict::Decided(LootAction::Keep, "any armour of a set".into())
        );
        // The property is named for the editor rather than numbered.
        assert_eq!(PropKind::Int.name_of(ARMOR_SET_ID), "EquipmentSetId");
        assert_eq!(PropKind::Int.name_of(999_999), "number 999999");
    }

    #[test]
    fn a_pattern_catches_what_a_list_of_names_cannot() {
        // Any legendary cantrip, whatever it is of. No list of spell
        // names keeps up with that; a pattern does.
        let profile = Profile {
            name: "legendaries".into(),
            note: String::new(),
            rules: vec![rule(
                "anything with a legendary on it",
                LootAction::Keep,
                vec![Ask::Spell {
                    op: TextOp::Like,
                    value: "^Legendary ".into(),
                }],
            )],
        };
        let me = me(50, &[]);
        let mut ring = item("Gold Ring", item_type::JEWELRY, 3_000);
        ring.appraised = true;
        ring.spells = vec!["Major Strength".into(), "Legendary Focus".into()];
        assert!(matches!(
            profile.judge_test(&ring, &me, "Aldric", 0),
            Verdict::Decided(LootAction::Keep, _)
        ));
        ring.spells = vec!["Major Strength".into(), "Blood Drinker".into()];
        assert_eq!(profile.judge_test(&ring, &me, "Aldric", 0), Verdict::None);
        // "Legendary" alone would match "Legendary Armor Ineptitude"
        // too; anchoring is the point of using a pattern.
        ring.spells = vec!["Surge of Legendary Regret".into()];
        assert_eq!(profile.judge_test(&ring, &me, "Aldric", 0), Verdict::None);

        // Saying "not Covenant" plainly, rather than as a lookahead
        // that Rust's patterns do not have.
        assert!(TextOp::HasNot.holds("Platemail Hauberk", "Covenant"));
        assert!(!TextOp::HasNot.holds("Covenant Hauberk", "covenant"));
        // Case is ignored, both ways.
        assert!(TextOp::Has.holds("Platemail Hauberk", "HAUBERK"));
        assert!(TextOp::Like.holds("Legendary Focus", "^legendary"));
        // A pattern that will not compile takes nothing rather than
        // everything: a typo should not empty the corpse into the pack.
        assert!(!TextOp::Like.holds("anything", "("));
        assert!(pattern_error("(").is_some());
        assert!(pattern_error("^Legendary ").is_none());
    }

    #[test]
    fn the_starter_profile_is_one_a_player_would_recognise() {
        let p = Profile::starter();
        let me = me(50, &[]);
        let keeps = |name: &str, kind: u32, value: u32| {
            matches!(
                p.judge_test(&item(name, kind, value), &me, "Aldric", 0),
                Verdict::Decided(LootAction::Keep, _)
            )
        };
        let sells = |name: &str, kind: u32, value: u32| {
            matches!(
                p.judge_test(&item(name, kind, value), &me, "Aldric", 0),
                Verdict::Decided(LootAction::Sell, _)
            )
        };
        assert!(keeps("Pyreal", item_type::MONEY, 1_000));
        assert!(keeps(
            "Trade Note (50,000)",
            item_type::PROMISSORY_NOTE,
            50_000
        ));
        assert!(keeps("Prismatic Taper", item_type::SPELL_COMPONENTS, 5));
        assert!(sells("Copper Pea", item_type::MISC, 40));
        assert!(sells("Ruby", item_type::GEM, 9_000));
        // Junk is dismissed without the server being asked about it,
        // and the rule that dismisses it comes before every rule that
        // would have needed asking.
        let junk = item("Quartz", item_type::GEM, 20);
        assert!(matches!(
            p.judge_test(&junk, &me, "Aldric", 0),
            Verdict::Decided(LootAction::Skip, _)
        ));
        // A stack cap holds.
        let kit = item("Healing Kit", item_type::MISC, 100);
        assert!(matches!(
            p.judge_test(&kit, &me, "Aldric", 0),
            Verdict::Decided(LootAction::Keep, _)
        ));
        assert!(!matches!(
            p.judge_test(&kit, &me, "Aldric", 4),
            Verdict::Decided(LootAction::Keep, _)
        ));
        // Something dear enough to be worth a look is looked at rather
        // than guessed about.
        let plate = item("Platemail Hauberk", item_type::ARMOR, 4_000);
        assert!(matches!(
            p.judge_test(&plate, &me, "Aldric", 0),
            Verdict::NeedsId(_)
        ));
        // And the profile does need the server sometimes, which is the
        // point of the ordering rather than a fault in it.
        assert!(p.needs_id());
    }

    #[test]
    fn the_cheap_rules_settle_most_of_a_corpse_without_asking_the_server() {
        // The shape of a profile a player would actually write: the
        // dear things first, judged on worth alone, and the fussy rule
        // that needs the numbers last.
        let profile = Profile {
            name: "tuned".into(),
            note: String::new(),
            rules: vec![
                rule(
                    "peas",
                    LootAction::Sell,
                    vec![Ask::Item(Term::Word("pea".into()))],
                ),
                rule(
                    "nothing cheap",
                    LootAction::Skip,
                    vec![Ask::Item(Term::Num(NumKey::Value, Op::Le, 100.0))],
                ),
                rule(
                    "armour with a legendary on it",
                    LootAction::Keep,
                    vec![
                        Ask::Item(Term::Kind("armor".into())),
                        Ask::Spell {
                            op: TextOp::Like,
                            value: "^Legendary ".into(),
                        },
                    ],
                ),
            ],
        };
        let me = me(50, &[]);
        let corpse = [
            item("Copper Pea", item_type::MISC, 40),
            item("Quartz", item_type::GEM, 20),
            item("Platemail Hauberk", item_type::ARMOR, 4_000),
        ];
        let verdicts: Vec<Verdict> = corpse
            .iter()
            .map(|it| profile.judge_test(it, &me, "Aldric", 0))
            .collect();
        // The pea is claimed by the first rule and the quartz by the
        // second, both without the server being asked anything.
        assert!(matches!(verdicts[0], Verdict::Decided(LootAction::Sell, _)));
        assert!(matches!(verdicts[1], Verdict::Decided(LootAction::Skip, _)));
        // Only the hauberk needs looking over.
        assert!(matches!(verdicts[2], Verdict::NeedsId(_)));
        let to_look_over = verdicts
            .iter()
            .filter(|v| matches!(v, Verdict::NeedsId(_)))
            .count();
        assert_eq!(to_look_over, 1, "one round trip, not three");
    }

    #[test]
    fn an_edit_is_live_at_once_and_written_a_moment_later() {
        let dir = std::env::temp_dir().join(format!("acswarm-writes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let library = Library::default();
        library.open(&dir);

        // A profile nobody has seen is written straight away, so that
        // the file exists to be found and shared.
        let mut profile = Profile {
            name: "notes".into(),
            note: "first".into(),
            rules: Vec::new(),
        };
        library.put(profile.clone()).expect("saved");
        assert_eq!(Profile::load(&dir, "notes").expect("on disk").note, "first");

        // Typing in it is live at once...
        profile.note = "a much longer note, typed a letter at a time".into();
        library.put(profile.clone()).expect("held");
        assert_eq!(library.get("notes").expect("live").note, profile.note);
        // ...and has not gone to the disk yet, which is the point: a
        // file write per keystroke is what this avoids.
        assert_eq!(
            Profile::load(&dir, "notes").expect("still there").note,
            "first"
        );

        // It catches up.
        library.flush_all();
        assert_eq!(
            Profile::load(&dir, "notes").expect("caught up").note,
            profile.note
        );
        // And a flush with nothing waiting does nothing at all.
        library.flush();
        library.flush_all();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rule_switched_off_is_off_for_everybody_at_once() {
        let dir = std::env::temp_dir().join(format!("acswarm-library-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let library = Library::default();
        library.open(&dir);

        let mut profile = Profile {
            name: "party".into(),
            note: String::new(),
            rules: vec![rule(
                "peas",
                LootAction::Sell,
                vec![Ask::Item(Term::Word("pea".into()))],
            )],
        };
        library.put(profile.clone()).expect("saved");

        // Two characters, both reading the same profile by name.
        let me = me(50, &[]);
        let pea = item("Copper Pea", item_type::MISC, 40);
        let look = || {
            library
                .get("party")
                .map(|p| p.judge(&pea, None, &me, "anyone", 0))
        };
        assert!(matches!(
            look(),
            Some(Verdict::Decided(LootAction::Sell, _))
        ));

        // The player switches the rule off. Nobody re-reads anything
        // and nobody is told; the next item judged is judged anew.
        profile.rules[0].on = false;
        library.put(profile).expect("saved again");
        assert_eq!(look(), Some(Verdict::None));

        // And it is on the disk that way, for the next time the app
        // starts and for whoever it is sent to -- once the writing has
        // caught up with the editing.
        library.flush_all();
        assert_eq!(library.reload(), 1);
        assert_eq!(look(), Some(Verdict::None));
        assert_eq!(library.names(), vec!["party".to_string()]);

        // A character told to use no profile has none, which is not an
        // error.
        assert!(library.get("").is_none());
        assert!(library.get("no such profile").is_none());

        library.remove("party").expect("removed");
        assert!(library.get("party").is_none());
        assert!(Profile::list(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_profile_is_a_file_that_can_be_handed_to_a_friend() {
        let dir = std::env::temp_dir().join(format!("acswarm-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let profile = Profile {
            name: "Peas and rares".into(),
            note: "what the party takes".into(),
            rules: vec![rule(
                "peas",
                LootAction::Sell,
                vec![Ask::Item(Term::Word("pea".into()))],
            )],
        };
        let path = profile.save(&dir).expect("saved");
        assert!(path.exists());
        assert_eq!(Profile::list(&dir), vec!["Peas and rares".to_string()]);
        let back = Profile::load(&dir, "Peas and rares").expect("read back");
        assert_eq!(back, profile);
        // A name that would not do as a file name still makes one.
        assert_eq!(tidy_name("mage/archer"), "mage-archer");
        assert_eq!(tidy_name("   "), "profile");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Every profile in one place, shared by every character in the
/// process.
///
/// A player toggling a rule expects it to take effect everywhere at
/// once -- on the character they are watching and on the eleven others
/// working the same ground -- so nothing keeps a copy. A profile is
/// looked up by name each time it is used; saving one replaces it here
/// and the next item judged, by anybody, is judged by the new rules.
#[derive(Debug, Default)]
pub struct Library {
    dir: std::sync::RwLock<PathBuf>,
    by_name: std::sync::RwLock<std::collections::BTreeMap<String, std::sync::Arc<Profile>>>,
    /// Profiles changed but not yet written, and when each was last
    /// written. See [`Library::put`].
    unwritten: std::sync::Mutex<std::collections::BTreeMap<String, std::time::Instant>>,
}

/// How long a profile may sit changed-but-unwritten.
///
/// Being live and being saved are different things. An edit has to
/// reach every character at once, which is memory and costs nothing;
/// it does not have to reach the disk at once, and writing the file on
/// every keystroke of a rule's name would. So the shelf is updated the
/// moment anything changes and the file follows a moment later.
const WRITE_AFTER: std::time::Duration = std::time::Duration::from_millis(750);

impl Library {
    /// The one every session shares. A test that wants its own makes
    /// one with [`Library::default`].
    pub fn shared() -> std::sync::Arc<Library> {
        static SHARED: std::sync::OnceLock<std::sync::Arc<Library>> = std::sync::OnceLock::new();
        SHARED.get_or_init(Default::default).clone()
    }

    /// Where the files live. Setting it reads them.
    pub fn open(&self, dir: impl Into<PathBuf>) -> usize {
        *self.dir.write().unwrap_or_else(|e| e.into_inner()) = dir.into();
        self.reload()
    }

    pub fn dir(&self) -> PathBuf {
        self.dir.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Read the shelf, and put a starter profile on it when it is
    /// bare. A player opening the editor for the first time should
    /// find something to read and change rather than a blank page.
    pub fn open_or_start(&self, dir: impl Into<PathBuf>) -> usize {
        let n = self.open(dir);
        if n > 0 {
            return n;
        }
        match self.put(Profile::starter()) {
            Ok(()) => 1,
            Err(e) => {
                tracing::warn!("cannot write a starter loot profile: {e}");
                0
            }
        }
    }

    /// Read every profile in the directory, replacing what is held.
    /// Returns how many were read. One that will not parse is logged
    /// and left out rather than taking the rest down with it.
    pub fn reload(&self) -> usize {
        let dir = self.dir();
        let mut found = std::collections::BTreeMap::new();
        for name in Profile::list(&dir) {
            match Profile::load(&dir, &name) {
                Ok(p) => {
                    found.insert(p.name.clone(), std::sync::Arc::new(p));
                }
                Err(e) => tracing::warn!("loot profile {name}: {e}"),
            }
        }
        let n = found.len();
        *self.by_name.write().unwrap_or_else(|e| e.into_inner()) = found;
        n
    }

    /// The profile of this name, if there is one. An empty name is no
    /// profile rather than an error: that is a character told to use
    /// none.
    pub fn get(&self, name: &str) -> Option<std::sync::Arc<Profile>> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        self.by_name
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned()
    }

    /// What is on the shelf, in order.
    pub fn names(&self) -> Vec<String> {
        self.by_name
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }

    /// Put a profile in front of every character at once, and write it
    /// out shortly afterwards.
    ///
    /// This is what an editor calls when anything changes, a rule being
    /// switched off included, so it is called on every keystroke of a
    /// rule's name. The change is live immediately -- that is the point
    /// of it -- but the file is left for [`Library::flush`] a moment
    /// later, unless the profile is new, in which case it is written at
    /// once so that the file exists.
    pub fn put(&self, profile: Profile) -> std::io::Result<()> {
        let name = profile.name.clone();
        let is_new = !self
            .by_name
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&name);
        self.by_name
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.clone(), std::sync::Arc::new(profile));
        if is_new {
            return self.write_out(&name);
        }
        self.unwritten
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(name)
            .or_insert_with(std::time::Instant::now);
        Ok(())
    }

    /// Write out anything changed and left long enough. Called once a
    /// frame; almost always a lock and a look at the clock.
    pub fn flush(&self) {
        let due: Vec<String> = {
            let waiting = self.unwritten.lock().unwrap_or_else(|e| e.into_inner());
            if waiting.is_empty() {
                return;
            }
            let now = std::time::Instant::now();
            waiting
                .iter()
                .filter(|(_, since)| now.duration_since(**since) >= WRITE_AFTER)
                .map(|(name, _)| name.clone())
                .collect()
        };
        for name in due {
            if let Err(e) = self.write_out(&name) {
                tracing::warn!("cannot write loot profile {name}: {e}");
            }
        }
    }

    /// Write everything outstanding, whatever the clock says: for
    /// shutting down, where a moment later never comes.
    pub fn flush_all(&self) {
        let due: Vec<String> = self
            .unwritten
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect();
        for name in due {
            if let Err(e) = self.write_out(&name) {
                tracing::warn!("cannot write loot profile {name}: {e}");
            }
        }
    }

    fn write_out(&self, name: &str) -> std::io::Result<()> {
        let dir = self.dir();
        let profile = self
            .by_name
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
            .cloned();
        self.unwritten
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name);
        match profile {
            Some(p) => p.save(&dir).map(|_| ()),
            None => Ok(()),
        }
    }

    /// Forget one and delete its file.
    pub fn remove(&self, name: &str) -> std::io::Result<()> {
        let dir = self.dir();
        self.unwritten
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name.trim());
        self.by_name
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(name.trim());
        let path = Profile::path_of(&dir, name);
        match std::fs::remove_file(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}
