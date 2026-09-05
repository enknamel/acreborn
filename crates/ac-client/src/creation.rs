//! Character creation and selection, as the game defines them (see
//! `docs/game/mechanics.md`, "Character creation and selection"): a
//! heritage and sex with an appearance, a starting town, 330 attribute
//! points spread over six attributes of 10..=100 each, and skill credits
//! (52; Olthoi 68) spent on training or specializing skills at costs from
//! the SkillTable, overridden per heritage by the CharGen table. Templates
//! preset the attributes and skills. The server validates the same rules
//! (ACE `PlayerFactory.Create`), so a build that passes
//! [`CharacterBuild::validate`] is accepted.
//!
//! Costs, as the server spends them (ACE `PlayerFactory.Create` calling
//! `Player.TrainSkill` then `Player.SpecializeSkill`): training costs the
//! SkillTable's `trained_cost`; specializing a trained skill costs the
//! extra `specialized_cost - trained_cost` (the table's second number is
//! the total). A CharGen heritage entry replaces both numbers directly:
//! `normal_cost` to train and `primary_cost` extra to specialize. An extra
//! of 999 or more (Salvaging, the tinkering skills) means the skill cannot
//! be specialized at creation. The always-trained skills (Arcane Lore,
//! Magic Defense, Jump, Run, Loyalty, Salvaging) cost 0 to train and are
//! sent Trained; they can still be specialized at their extra cost.
//!
//! The client-side flow: after the character list arrives the client
//! either auto-enters (`Config::auto_enter`, or `Config::character`
//! given) or emits [`crate::Event::Characters`] and waits for
//! [`Client::enter_world`], [`Client::create_character`],
//! [`Client::delete_character`] or [`Client::restore_character`].

use std::collections::BTreeMap;

use ac_formats::chargen::{CharGen, HeritageGroupCg, TemplateCg};
use ac_formats::skill_table::SkillTable;
use ac_net::messages::{Appearance, CharacterCreate};
use ac_scene::chargen::Look;
use ac_scene::Assets;

use crate::Client;

/// Attribute indices in [`CharacterBuild::attributes`].
pub const STRENGTH: usize = 0;
pub const ENDURANCE: usize = 1;
pub const COORDINATION: usize = 2;
pub const QUICKNESS: usize = 3;
pub const FOCUS: usize = 4;
pub const SELF: usize = 5;
pub const ATTRIBUTE_NAMES: [&str; 6] = [
    "Strength",
    "Endurance",
    "Coordination",
    "Quickness",
    "Focus",
    "Self",
];
/// Every attribute starts in this range (ACE `ValidateAttributeCredits`).
pub const ATTRIBUTE_MIN: u32 = 10;
pub const ATTRIBUTE_MAX: u32 = 100;
/// Number of skill slots on the wire (skill ids 0..55); the server
/// terminates the session on any other count.
pub const SKILL_SLOTS: usize = 55;

/// Skills every character has trained for free (ACE `AlwaysTrained`):
/// Arcane Lore, Magic Defense, Jump, Run, Loyalty, Salvaging.
pub const ALWAYS_TRAINED: [u32; 6] = [14, 15, 22, 24, 36, 40];

/// A specialize cost at or above this means "cannot be specialized at
/// creation" (Salvaging 999, the tinkering skills 999 extra); the
/// tinkering skills are specialized by augmentation later instead.
pub const NO_SPECIALIZE_COST: u32 = 999;

/// Heritage ids (ACE `HeritageGroup`).
pub const HERITAGE_ALUVIAN: u32 = 1;
pub const HERITAGE_OLTHOI: u32 = 12;
pub const HERITAGE_OLTHOI_ACID: u32 = 13;

/// What the player chose for a skill. Wire values match ACE
/// `SkillAdvancementClass`: Inactive 0, Untrained 1, Trained 2,
/// Specialized 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillChoice {
    /// Not on the sheet at all (the server leaves the skill as the
    /// player weenie has it).
    Unusable,
    /// Usable at its formula value when the skill allows it (see
    /// [`SkillRule::usable_untrained`]), costs nothing, cannot be raised.
    Untrained,
    Trained,
    Specialized,
}

impl SkillChoice {
    pub fn wire(self) -> u32 {
        match self {
            SkillChoice::Unusable => 0,
            SkillChoice::Untrained => 1,
            SkillChoice::Trained => 2,
            SkillChoice::Specialized => 3,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SkillChoice::Unusable => "unusable",
            SkillChoice::Untrained => "untrained",
            SkillChoice::Trained => "trained",
            SkillChoice::Specialized => "specialized",
        }
    }
}

/// Per-skill creation costs and defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRule {
    pub skill: u32,
    pub name: String,
    /// Credits to train (0 for always-trained skills).
    pub train_cost: u32,
    /// Extra credits to specialize a trained skill; a specialized skill
    /// costs `train_cost + specialize_cost` in total.
    pub specialize_cost: u32,
    /// The state a fresh character has without spending anything.
    pub default: SkillChoice,
    /// The skill can be specialized at creation.
    pub can_specialize: bool,
    /// The skill works at its formula value while untrained (SkillTable
    /// `min_level` 1); the others (magic schools, Healing, Lockpick,
    /// Fletching, Alchemy, Cooking, ...) need training to be used.
    pub usable_untrained: bool,
}

/// The rules for one heritage, from the CharGen and Skill tables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    pub heritage: u32,
    pub heritage_name: String,
    /// Total attribute points (330; Olthoi 60).
    pub attribute_credits: u32,
    /// Skill credits to spend (52; Olthoi 68).
    pub skill_credits: u32,
    /// Skills that may be changed, by id, with their costs.
    pub skills: Vec<SkillRule>,
    /// Starting town indices into [`CharGen::starter_areas`]: the
    /// heritage's home first, then the others it may pick.
    pub start_areas: Vec<usize>,
    pub start_area_names: Vec<String>,
    /// Template names, in CharGen order.
    pub templates: Vec<String>,
}

impl Rules {
    pub fn skill(&self, skill: u32) -> Option<&SkillRule> {
        self.skills.iter().find(|r| r.skill == skill)
    }

    /// Olthoi characters take their attributes and skills from the
    /// server's weenie; the numbers here are only what the table says.
    pub fn is_olthoi(&self) -> bool {
        matches!(self.heritage, HERITAGE_OLTHOI | HERITAGE_OLTHOI_ACID)
    }

    /// Template index from a name (case-insensitive prefix) or a number.
    pub fn template_index(&self, name_or_index: &str) -> Option<usize> {
        if let Ok(i) = name_or_index.parse::<usize>() {
            return (i < self.templates.len()).then_some(i);
        }
        let want = name_or_index.trim().to_ascii_lowercase();
        self.templates
            .iter()
            .position(|t| t.to_ascii_lowercase().starts_with(&want))
    }

    /// Starter-area index (into `CharGen::starter_areas`) from a town name
    /// (case-insensitive prefix) or the index itself.
    pub fn start_area_index(&self, name_or_index: &str) -> Option<usize> {
        if let Ok(i) = name_or_index.parse::<usize>() {
            return self.start_areas.contains(&i).then_some(i);
        }
        let want = name_or_index.trim().to_ascii_lowercase();
        self.start_area_names
            .iter()
            .position(|n| n.to_ascii_lowercase().starts_with(&want))
            .and_then(|p| self.start_areas.get(p).copied())
    }

    /// A readable summary: credits, towns, templates and the skill costs.
    pub fn summary(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{} (heritage {}): {} attribute points over {} attributes of {}..={}, {} skill credits",
            self.heritage_name,
            self.heritage,
            self.attribute_credits,
            ATTRIBUTE_NAMES.len(),
            ATTRIBUTE_MIN,
            ATTRIBUTE_MAX,
            self.skill_credits
        );
        let _ = writeln!(
            s,
            "start areas: {}",
            self.start_areas
                .iter()
                .zip(&self.start_area_names)
                .map(|(i, n)| format!("{n} ({i})"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let _ = writeln!(
            s,
            "templates: {}",
            self.templates
                .iter()
                .enumerate()
                .map(|(i, n)| format!("{n} ({i})"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let _ = writeln!(s, "skills (id name: train / specialize extra):");
        for r in &self.skills {
            let _ = writeln!(
                s,
                "  {:>2} {:<22} {:>2} / {}{}{}",
                r.skill,
                r.name,
                r.train_cost,
                if r.can_specialize {
                    r.specialize_cost.to_string()
                } else {
                    "-".to_string()
                },
                if r.default == SkillChoice::Trained {
                    "  always trained"
                } else {
                    ""
                },
                if r.usable_untrained {
                    ""
                } else {
                    "  needs training to use"
                },
            );
        }
        s
    }
}

/// Why a build cannot be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateError {
    /// An attribute is outside 10..=100.
    AttributeOutOfRange(usize),
    /// More than the heritage's attribute credits are spent.
    TooManyAttributePoints {
        used: u32,
        allowed: u32,
    },
    TooManySkillCredits {
        used: u32,
        allowed: u32,
    },
    /// The skill cannot take that choice (not on the sheet, cannot be
    /// specialized, always trained).
    SkillNotAllowed(u32),
    /// Name empty, too long, or with characters the server rejects.
    InvalidName,
    InvalidStartArea,
    UnknownHeritage,
}

impl std::fmt::Display for CreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CreateError::AttributeOutOfRange(i) => write!(
                f,
                "{} must be between {ATTRIBUTE_MIN} and {ATTRIBUTE_MAX}",
                ATTRIBUTE_NAMES.get(*i).copied().unwrap_or("attribute")
            ),
            CreateError::TooManyAttributePoints { used, allowed } => {
                write!(f, "{used} attribute points spent, {allowed} allowed")
            }
            CreateError::TooManySkillCredits { used, allowed } => {
                write!(f, "{used} skill credits spent, {allowed} allowed")
            }
            CreateError::SkillNotAllowed(s) => write!(f, "skill {s} cannot take that choice"),
            CreateError::InvalidName => write!(
                f,
                "name must be {NAME_MIN}..={NAME_MAX} letters, with spaces, hyphens or apostrophes between them"
            ),
            CreateError::InvalidStartArea => write!(f, "that town is not open to this heritage"),
            CreateError::UnknownHeritage => write!(f, "unknown heritage"),
        }
    }
}

impl std::error::Error for CreateError {}

/// Everything the create message needs, kept editable.
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterBuild {
    pub name: String,
    pub look: Look,
    /// Clothing choices: (style, color, hue) for headgear (style
    /// `u32::MAX` = none), shirt, pants, footwear.
    pub headgear: (u32, u32, f64),
    pub shirt: (u32, u32, f64),
    pub pants: (u32, u32, f64),
    pub footwear: (u32, u32, f64),
    /// Template index the attributes and skills were last set from.
    pub template: usize,
    /// Strength, Endurance, Coordination, Quickness, Focus, Self.
    pub attributes: [u32; 6],
    /// Choice per skill id (skills absent here are Unusable).
    pub skills: BTreeMap<u32, SkillChoice>,
    /// Index into [`CharGen::starter_areas`].
    pub start_area: usize,
}

impl CharacterBuild {
    /// A build for `heritage` and `gender` (1 male, 2 female) from the
    /// heritage's first template, in its home town, unnamed.
    pub fn new(assets: &Assets, heritage: u32, gender: u32) -> Result<Self, CreateError> {
        let cg = assets.chargen().map_err(|_| CreateError::UnknownHeritage)?;
        let rules = rules(assets, heritage)?;
        let mut b = CharacterBuild {
            name: String::new(),
            look: Look {
                heritage,
                gender,
                hair_style: 0,
                hair_color: 0,
                hair_shade: 0.5,
                eyes: 0,
                eye_color: 0,
                nose: 0,
                mouth: 0,
                skin_shade: 0.5,
            },
            headgear: (u32::MAX, 0, 0.0),
            shirt: (0, 0, 0.0),
            pants: (0, 0, 0.0),
            footwear: (0, 0, 0.0),
            template: 0,
            attributes: [ATTRIBUTE_MIN; 6],
            skills: BTreeMap::new(),
            start_area: rules.start_areas.first().copied().unwrap_or(0),
        };
        b.apply_template(&cg, &rules, 0);
        Ok(b)
    }

    /// A named build from command-line style choices: heritage by name
    /// or id, gender `m`/`f` (or `male`/`female`, `1`/`2`), template by
    /// name or index, start area by town name or index. Missing choices
    /// fall back to Aluvian, male, the first template and the home town.
    pub fn from_options(
        assets: &Assets,
        name: &str,
        heritage: Option<&str>,
        gender: Option<&str>,
        template: Option<&str>,
        start_area: Option<&str>,
    ) -> Result<(Self, Rules), String> {
        let cg = assets
            .chargen()
            .map_err(|e| format!("reading the CharGen table: {e}"))?;
        let heritage_id = match heritage {
            Some(h) => ac_scene::chargen::heritage_id(&cg, h)
                .ok_or_else(|| format!("unknown heritage {h:?}"))?,
            None => HERITAGE_ALUVIAN,
        };
        let gender_id = match gender {
            Some(g) => gender_id(g).ok_or_else(|| format!("gender must be m or f, not {g:?}"))?,
            None => 1,
        };
        let rules = rules(assets, heritage_id).map_err(|e| e.to_string())?;
        let mut build =
            CharacterBuild::new(assets, heritage_id, gender_id).map_err(|e| e.to_string())?;
        build.name = name.trim().to_string();
        if let Some(t) = template {
            let i = rules
                .template_index(t)
                .ok_or_else(|| format!("unknown template {t:?}; have {:?}", rules.templates))?;
            build.apply_template(&cg, &rules, i);
        }
        if let Some(a) = start_area {
            build.start_area = rules.start_area_index(a).ok_or_else(|| {
                format!(
                    "unknown start area {a:?}; have {:?}",
                    rules.start_area_names
                )
            })?;
        }
        Ok((build, rules))
    }

    /// Set the attributes and skills from a heritage template: the
    /// template's attributes, every listed skill at its default (always
    /// trained ones Trained, the rest Untrained), the template's normal
    /// skills Trained and its primary skills Specialized.
    pub fn apply_template(&mut self, cg: &CharGen, rules: &Rules, index: usize) {
        let Some(t) = heritage(cg, rules.heritage).and_then(|h| h.templates.get(index)) else {
            return;
        };
        self.template = index;
        self.attributes = template_attributes(t);
        self.skills.clear();
        for r in &rules.skills {
            if r.default != SkillChoice::Unusable {
                self.skills.insert(r.skill, r.default);
            }
        }
        for &s in &t.normal_skills {
            self.skills.insert(s, SkillChoice::Trained);
        }
        for &s in &t.primary_skills {
            self.skills.insert(s, SkillChoice::Specialized);
        }
    }

    pub fn attribute_points_used(&self) -> u32 {
        self.attributes.iter().sum()
    }

    /// Points still to spend (negative when over).
    pub fn attribute_points_left(&self, rules: &Rules) -> i64 {
        rules.attribute_credits as i64 - self.attribute_points_used() as i64
    }

    /// Set an attribute, clamped to 10..=100. Returns the value stored.
    pub fn set_attribute(&mut self, index: usize, value: u32) -> u32 {
        let v = value.clamp(ATTRIBUTE_MIN, ATTRIBUTE_MAX);
        if let Some(a) = self.attributes.get_mut(index) {
            *a = v;
        }
        v
    }

    pub fn skill(&self, skill: u32) -> SkillChoice {
        self.skills
            .get(&skill)
            .copied()
            .unwrap_or(SkillChoice::Unusable)
    }

    /// Credits a skill choice costs under `rules`.
    pub fn skill_cost(rules: &Rules, skill: u32, choice: SkillChoice) -> u32 {
        let Some(r) = rules.skill(skill) else {
            return 0;
        };
        match choice {
            SkillChoice::Unusable | SkillChoice::Untrained => 0,
            SkillChoice::Trained => r.train_cost,
            SkillChoice::Specialized => r.train_cost + r.specialize_cost,
        }
    }

    pub fn credits_used(&self, rules: &Rules) -> u32 {
        self.skills
            .iter()
            .map(|(&s, &c)| Self::skill_cost(rules, s, c))
            .sum()
    }

    /// Credits still to spend (negative when over).
    pub fn credits_left(&self, rules: &Rules) -> i64 {
        rules.skill_credits as i64 - self.credits_used(rules) as i64
    }

    /// Change a skill; refused when the rules do not allow that choice
    /// for the skill (not on the sheet, below its default, specializing
    /// one that cannot be). Going over budget is allowed here and caught
    /// by [`CharacterBuild::validate`], so a UI can show a negative
    /// balance.
    pub fn set_skill(
        &mut self,
        rules: &Rules,
        skill: u32,
        choice: SkillChoice,
    ) -> Result<(), CreateError> {
        let Some(r) = rules.skill(skill) else {
            return Err(CreateError::SkillNotAllowed(skill));
        };
        if choice == SkillChoice::Specialized && !r.can_specialize {
            return Err(CreateError::SkillNotAllowed(skill));
        }
        if ALWAYS_TRAINED.contains(&skill) && choice < SkillChoice::Trained {
            return Err(CreateError::SkillNotAllowed(skill));
        }
        if choice < r.default {
            return Err(CreateError::SkillNotAllowed(skill));
        }
        if choice == SkillChoice::Unusable {
            self.skills.remove(&skill);
        } else {
            self.skills.insert(skill, choice);
        }
        Ok(())
    }

    /// The checks the server makes before accepting the character
    /// (ACE `ValidateAttributeCredits` and the skill loop of
    /// `PlayerFactory.Create`), plus the client-side name rule.
    pub fn validate(&self, rules: &Rules) -> Result<(), CreateError> {
        for (i, &a) in self.attributes.iter().enumerate() {
            if !(ATTRIBUTE_MIN..=ATTRIBUTE_MAX).contains(&a) {
                return Err(CreateError::AttributeOutOfRange(i));
            }
        }
        let used = self.attribute_points_used();
        if used > rules.attribute_credits {
            return Err(CreateError::TooManyAttributePoints {
                used,
                allowed: rules.attribute_credits,
            });
        }
        for (&s, &c) in &self.skills {
            let Some(r) = rules.skill(s) else {
                return Err(CreateError::SkillNotAllowed(s));
            };
            if c == SkillChoice::Specialized && !r.can_specialize {
                return Err(CreateError::SkillNotAllowed(s));
            }
            if c < r.default {
                return Err(CreateError::SkillNotAllowed(s));
            }
        }
        let credits = self.credits_used(rules);
        if credits > rules.skill_credits {
            return Err(CreateError::TooManySkillCredits {
                used: credits,
                allowed: rules.skill_credits,
            });
        }
        if !valid_name(&self.name) {
            return Err(CreateError::InvalidName);
        }
        if !rules.start_areas.contains(&self.start_area) {
            return Err(CreateError::InvalidStartArea);
        }
        Ok(())
    }

    /// The wire message for this build: 55 skill slots with the wire
    /// value of each choice, 0 (inactive) for skills not on the sheet.
    pub fn to_message(&self, account: &str, slot: u32) -> CharacterCreate {
        let mut skills = vec![0u32; SKILL_SLOTS];
        for (&s, &c) in &self.skills {
            if let Some(slot) = skills.get_mut(s as usize) {
                *slot = c.wire();
            }
        }
        let l = &self.look;
        CharacterCreate {
            account: account.to_string(),
            name: self.name.trim().to_string(),
            heritage: l.heritage,
            gender: l.gender,
            appearance: Appearance {
                eyes: l.eyes as u32,
                nose: l.nose as u32,
                mouth: l.mouth as u32,
                hair_color: l.hair_color as u32,
                eye_color: l.eye_color as u32,
                hair_style: l.hair_style as u32,
                headgear_style: self.headgear.0,
                headgear_color: self.headgear.1,
                shirt_style: self.shirt.0,
                shirt_color: self.shirt.1,
                pants_style: self.pants.0,
                pants_color: self.pants.1,
                footwear_style: self.footwear.0,
                footwear_color: self.footwear.1,
                skin_hue: l.skin_shade as f64,
                hair_hue: l.hair_shade as f64,
                headgear_hue: self.headgear.2,
                shirt_hue: self.shirt.2,
                pants_hue: self.pants.2,
                footwear_hue: self.footwear.2,
            },
            template: self.template as i32,
            strength: self.attributes[STRENGTH],
            endurance: self.attributes[ENDURANCE],
            coordination: self.attributes[COORDINATION],
            quickness: self.attributes[QUICKNESS],
            focus: self.attributes[FOCUS],
            self_: self.attributes[SELF],
            slot,
            skills,
            start_area: self.start_area as u32,
        }
    }
}

/// The creation rules for a heritage.
pub fn rules(assets: &Assets, heritage_id: u32) -> Result<Rules, CreateError> {
    let cg = assets.chargen().map_err(|_| CreateError::UnknownHeritage)?;
    let table = assets
        .skill_table()
        .map_err(|_| CreateError::UnknownHeritage)?;
    let h = heritage(&cg, heritage_id).ok_or(CreateError::UnknownHeritage)?;
    let skills = skill_rules(&table, h);
    let start_areas: Vec<usize> = h
        .primary_start_areas
        .iter()
        .chain(h.secondary_start_areas.iter())
        .filter(|&&i| i >= 0)
        .map(|&i| i as usize)
        .collect();
    Ok(Rules {
        heritage: heritage_id,
        heritage_name: h.name.clone(),
        attribute_credits: h.attribute_credits,
        skill_credits: h.skill_credits,
        skills,
        start_area_names: start_areas
            .iter()
            .filter_map(|&i| cg.starter_areas.get(i).map(|a| a.name.clone()))
            .collect(),
        start_areas,
        templates: h.templates.iter().map(|t| t.name.clone()).collect(),
    })
}

/// Skill costs at creation, as ACE `PlayerFactory.Create` spends them:
/// train at the SkillTable's `trained_cost`, specialize for the extra
/// `specialized_cost - trained_cost`; a heritage's CharGen entry replaces
/// both (`normal_cost`, `primary_cost` as the extra). Always-trained
/// skills are free and Trained by default; every other listed skill
/// starts Untrained. An extra of [`NO_SPECIALIZE_COST`] or more cannot be
/// bought.
pub fn skill_rules(table: &SkillTable, h: &HeritageGroupCg) -> Vec<SkillRule> {
    table
        .skills
        .iter()
        .map(|(id, s)| {
            let over = h.skills.iter().find(|c| c.skill == *id);
            let (train, extra) = match over {
                Some(c) => (c.normal_cost, c.primary_cost),
                None => (s.trained_cost, s.specialized_cost - s.trained_cost),
            };
            let always = ALWAYS_TRAINED.contains(id);
            let specialize_cost = extra.max(0) as u32;
            SkillRule {
                skill: *id,
                name: s.name.clone(),
                train_cost: if always { 0 } else { train.max(0) as u32 },
                specialize_cost,
                default: if always {
                    SkillChoice::Trained
                } else {
                    SkillChoice::Untrained
                },
                can_specialize: specialize_cost < NO_SPECIALIZE_COST,
                usable_untrained: s.min_level <= 1,
            }
        })
        .collect()
}

pub fn heritage(cg: &CharGen, id: u32) -> Option<&HeritageGroupCg> {
    cg.heritage_groups
        .iter()
        .find(|(hid, _)| *hid == id)
        .map(|(_, h)| h)
}

/// Gender id from `m`/`male`/`1` or `f`/`female`/`2` (case-insensitive).
pub fn gender_id(s: &str) -> Option<u32> {
    match s.trim().to_ascii_lowercase().as_str() {
        "m" | "male" | "1" => Some(1),
        "f" | "female" | "2" => Some(2),
        _ => None,
    }
}

fn template_attributes(t: &TemplateCg) -> [u32; 6] {
    [
        t.strength,
        t.endurance,
        t.coordination,
        t.quickness,
        t.focus,
        t.self_,
    ]
}

pub const NAME_MIN: usize = 3;
pub const NAME_MAX: usize = 32;

/// The client-side name rule, as the retail creation screen enforced it:
/// 3..=32 characters, letters plus single spaces, hyphens and apostrophes
/// between letters. ACE itself checks no format: only the taboo table
/// (name banned), creature names (banned) and uniqueness (name in use).
pub fn valid_name(name: &str) -> bool {
    let n = name.trim();
    if !(NAME_MIN..=NAME_MAX).contains(&n.chars().count()) {
        return false;
    }
    let mut prev_sep = true; // no separator may lead, follow another, or trail
    for c in n.chars() {
        if c.is_ascii_alphabetic() {
            prev_sep = false;
        } else if matches!(c, ' ' | '-' | '\'') {
            if prev_sep {
                return false;
            }
            prev_sep = true;
        } else {
            return false;
        }
    }
    !prev_sep
}

/// Server answers to CharacterCreate (0xF643).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateOutcome {
    Created {
        id: u32,
        name: String,
    },
    /// ACE `CharacterGenerationVerificationResponse` code; see
    /// [`create_failure_message`].
    Failed(u32),
}

/// What a CharacterCreateResponse (0xF643) code means (ACE
/// `CharacterGenerationVerificationResponse`). The same message answers a
/// CharacterRestore, so "name in use" there means the deleted
/// character's name was taken meanwhile.
pub fn create_failure_message(code: u32) -> &'static str {
    match code {
        0 => "undefined response",
        1 => "character created",
        2 => "creation pending (Olthoi play is disabled on this server)",
        3 => "that name is already in use",
        4 => "that name is not allowed",
        5 => "the server rejected the character (attributes or skills out of bounds, or a corrupt build)",
        6 => "the character database is down; try again later",
        7 => "admin privilege denied",
        _ => "unknown creation response",
    }
}

/// A request the next CharacterCreateResponse answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    Create,
    Restore(u32),
}

impl Client {
    /// Send the character to the server; on success the server answers
    /// with [`crate::Event::CharacterCreated`] and the client enters the
    /// world with it, on failure with
    /// [`crate::Event::CharacterCreateFailed`].
    pub fn create_character(&mut self, build: &CharacterBuild) -> Result<(), CreateError> {
        let rules = rules(&self.assets, build.look.heritage)?;
        build.validate(&rules)?;
        let slot = self.characters.len() as u32;
        let msg = build.to_message(&self.config.account, slot);
        tracing::info!(
            "creating character {} ({} {}, template {}, {} attribute points, {} credits)",
            msg.name,
            rules.heritage_name,
            if build.look.gender == 2 {
                "female"
            } else {
                "male"
            },
            rules
                .templates
                .get(build.template)
                .map(String::as_str)
                .unwrap_or("?"),
            build.attribute_points_used(),
            build.credits_used(&rules)
        );
        self.pending_create = Some(Pending::Create);
        self.session
            .send_message(ac_net::messages::queue::UI, msg.encode());
        Ok(())
    }

    /// Enter the world with one of the account's characters: the same
    /// request / ServerReady / enter sequence the automatic path uses.
    /// Ignored while an enter is already in flight; before the login
    /// handshake (DDD exchange and character list) is done the enter is
    /// held until it is.
    pub fn enter_world(&mut self, id: u32) {
        if self.enter_requested {
            return;
        }
        if let Some(c) = self.characters.iter().find(|c| c.id == id) {
            self.config.character = Some(c.name.clone());
        }
        self.entering = Some(id);
        if self.ddd_done && self.characters_known {
            self.send_enter_request();
        } else {
            tracing::debug!("enter world with {id:#010x} held until the login handshake is done");
        }
    }

    pub(crate) fn send_enter_request(&mut self) {
        let name = self
            .entering
            .and_then(|id| self.characters.iter().find(|c| c.id == id))
            .map(|c| c.name.clone())
            .unwrap_or_default();
        tracing::info!("entering world as {name}");
        self.enter_requested = true;
        self.session.send_message(
            ac_net::messages::queue::UI,
            ac_net::messages::enter_world_request(),
        );
    }

    /// Ask the server to delete a character. ACE keeps it for
    /// `char_delete_time` (an hour by default) and answers with a fresh
    /// character list ([`crate::Event::Characters`]) in which the
    /// character carries `seconds_until_deleted > 0`; until then
    /// [`Client::restore_character`] brings it back.
    pub fn delete_character(&mut self, id: u32) {
        let Some(slot) = self.characters.iter().position(|c| c.id == id) else {
            tracing::warn!("delete: no character {id:#010x} on this account");
            return;
        };
        tracing::info!(
            "deleting character {} (slot {slot})",
            self.characters[slot].name
        );
        self.session.send_message(
            ac_net::messages::queue::UI,
            ac_net::messages::character_delete(&self.config.account, slot as u32),
        );
    }

    /// Undo a pending deletion (0xF7D9). The server answers with a
    /// CharacterCreateResponse-shaped message: on success the list is
    /// updated and re-emitted, on failure (name taken meanwhile, or the
    /// deletion already went through) [`crate::Event::CharacterCreateFailed`].
    pub fn restore_character(&mut self, id: u32) {
        if !self.characters.iter().any(|c| c.id == id) {
            tracing::warn!("restore: no character {id:#010x} on this account");
            return;
        }
        tracing::info!("restoring character {id:#010x}");
        self.pending_create = Some(Pending::Restore(id));
        self.session.send_message(
            ac_net::messages::queue::UI,
            ac_net::messages::character_restore(id),
        );
    }

    /// Handle a CharacterCreateResponse (0xF643), which answers both a
    /// create and a restore.
    pub(crate) fn create_response(&mut self, r: ac_net::messages::CharacterCreateResponse) {
        let pending = self.pending_create.take();
        match (pending, r.response) {
            (Some(Pending::Restore(id)), 1) => {
                if let Some(c) = self.characters.iter_mut().find(|c| c.id == id) {
                    c.seconds_until_deleted = 0;
                    if !r.name.is_empty() {
                        c.name = r.name.clone();
                    }
                }
                tracing::info!("character {} restored", r.name);
                self.events
                    .push(crate::Event::Characters(self.characters.clone()));
            }
            (_, 1) => {
                tracing::info!("character {} created as {:#010x}", r.name, r.guid);
                self.characters.push(ac_net::messages::CharacterEntry {
                    id: r.guid,
                    name: r.name.clone(),
                    seconds_until_deleted: 0,
                });
                self.events.push(crate::Event::CharacterCreated {
                    id: r.guid,
                    name: r.name,
                });
                self.enter_world(r.guid);
            }
            (_, code) => {
                tracing::error!(
                    "character {} failed: {} (code {code})",
                    if matches!(pending, Some(Pending::Restore(_))) {
                        "restore"
                    } else {
                        "creation"
                    },
                    create_failure_message(code)
                );
                self.events.push(crate::Event::CharacterCreateFailed(code));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(skill: u32, name: &str, train: u32, extra: u32) -> SkillRule {
        let always = ALWAYS_TRAINED.contains(&skill);
        SkillRule {
            skill,
            name: name.into(),
            train_cost: if always { 0 } else { train },
            specialize_cost: extra,
            default: if always {
                SkillChoice::Trained
            } else {
                SkillChoice::Untrained
            },
            can_specialize: extra < NO_SPECIALIZE_COST,
            usable_untrained: true,
        }
    }

    /// A hand-made Aluvian: the real table's numbers for a few skills.
    fn aluvian() -> Rules {
        Rules {
            heritage: 1,
            heritage_name: "Aluvian".into(),
            attribute_credits: 330,
            skill_credits: 52,
            skills: vec![
                rule(6, "Melee Defense", 10, 10),
                rule(14, "Arcane Lore", 0, 2),
                rule(24, "Run", 0, 4),
                rule(34, "War Magic", 16, 12),
                rule(40, "Salvaging", 0, 999),
                rule(18, "Item Tinkering", 2, 999),
                rule(44, "Heavy Weapons", 6, 6),
            ],
            start_areas: vec![0, 1, 2, 3],
            start_area_names: vec![
                "Holtburg".into(),
                "Shoushi".into(),
                "Yaraq".into(),
                "Sanamar".into(),
            ],
            templates: vec!["Adventurer".into(), "Bow Hunter".into()],
        }
    }

    fn blank(rules: &Rules) -> CharacterBuild {
        let mut b = CharacterBuild {
            name: "Test Char".into(),
            look: Look::default(),
            headgear: (u32::MAX, 0, 0.0),
            shirt: (0, 0, 0.0),
            pants: (0, 0, 0.0),
            footwear: (0, 0, 0.0),
            template: 0,
            attributes: [ATTRIBUTE_MIN; 6],
            skills: BTreeMap::new(),
            start_area: 0,
        };
        for r in &rules.skills {
            b.skills.insert(r.skill, r.default);
        }
        b
    }

    #[test]
    fn attribute_clamp_and_pool() {
        let rules = aluvian();
        let mut b = blank(&rules);
        assert_eq!(b.attribute_points_used(), 60);
        assert_eq!(b.attribute_points_left(&rules), 270);
        assert_eq!(b.set_attribute(STRENGTH, 5), 10);
        assert_eq!(b.set_attribute(STRENGTH, 500), 100);
        assert_eq!(
            b.set_attribute(99, 50),
            50,
            "out of range index stores nothing"
        );
        assert_eq!(b.attributes, [100, 10, 10, 10, 10, 10]);
        // 100/100/100/10/10/10 is exactly 330; one more point is over.
        b.set_attribute(ENDURANCE, 100);
        b.set_attribute(COORDINATION, 100);
        assert_eq!(b.attribute_points_left(&rules), 0);
        assert_eq!(b.validate(&rules), Ok(()));
        b.set_attribute(QUICKNESS, 20);
        assert_eq!(b.attribute_points_left(&rules), -10);
        assert_eq!(
            b.validate(&rules),
            Err(CreateError::TooManyAttributePoints {
                used: 340,
                allowed: 330
            })
        );
        b.set_attribute(QUICKNESS, 10);
        assert_eq!(b.validate(&rules), Ok(()));
        // A 6 x 100 build is rejected; 10 is the floor.
        b.attributes = [100; 6];
        assert!(matches!(
            b.validate(&rules),
            Err(CreateError::TooManyAttributePoints { used: 600, .. })
        ));
        b.attributes = [10, 10, 10, 10, 10, 9];
        assert_eq!(
            b.validate(&rules),
            Err(CreateError::AttributeOutOfRange(SELF))
        );
    }

    #[test]
    fn skill_credit_arithmetic() {
        let rules = aluvian();
        let mut b = blank(&rules);
        // Always-trained skills are on the sheet for free.
        assert_eq!(b.skill(14), SkillChoice::Trained);
        assert_eq!(b.skill(24), SkillChoice::Trained);
        assert_eq!(b.skill(6), SkillChoice::Untrained);
        assert_eq!(b.skill(99), SkillChoice::Unusable);
        assert_eq!(b.credits_used(&rules), 0);
        assert_eq!(b.credits_left(&rules), 52);
        b.set_skill(&rules, 6, SkillChoice::Trained).unwrap();
        assert_eq!(b.credits_used(&rules), 10);
        b.set_skill(&rules, 6, SkillChoice::Specialized).unwrap();
        assert_eq!(b.credits_used(&rules), 20, "train 10 + specialize 10");
        b.set_skill(&rules, 34, SkillChoice::Specialized).unwrap();
        assert_eq!(b.credits_used(&rules), 48, "war magic 16 + 12");
        b.set_skill(&rules, 14, SkillChoice::Specialized).unwrap();
        assert_eq!(b.credits_used(&rules), 50, "arcane lore 0 + 2");
        assert_eq!(b.validate(&rules), Ok(()));
        // Over budget is stored but refused by validate.
        b.set_skill(&rules, 44, SkillChoice::Trained).unwrap();
        assert_eq!(b.credits_left(&rules), -4);
        assert_eq!(
            b.validate(&rules),
            Err(CreateError::TooManySkillCredits {
                used: 56,
                allowed: 52
            })
        );
        b.set_skill(&rules, 44, SkillChoice::Untrained).unwrap();
        assert_eq!(b.validate(&rules), Ok(()));
        assert_eq!(
            CharacterBuild::skill_cost(&rules, 99, SkillChoice::Specialized),
            0
        );
    }

    #[test]
    fn set_skill_refusals() {
        let rules = aluvian();
        let mut b = blank(&rules);
        // Not on the sheet.
        assert_eq!(
            b.set_skill(&rules, 99, SkillChoice::Trained),
            Err(CreateError::SkillNotAllowed(99))
        );
        // Cannot specialize Salvaging or a tinkering skill, but can train.
        assert_eq!(
            b.set_skill(&rules, 40, SkillChoice::Specialized),
            Err(CreateError::SkillNotAllowed(40))
        );
        assert_eq!(
            b.set_skill(&rules, 18, SkillChoice::Specialized),
            Err(CreateError::SkillNotAllowed(18))
        );
        b.set_skill(&rules, 18, SkillChoice::Trained).unwrap();
        // Always-trained skills cannot drop below Trained.
        assert_eq!(
            b.set_skill(&rules, 24, SkillChoice::Untrained),
            Err(CreateError::SkillNotAllowed(24))
        );
        assert_eq!(
            b.set_skill(&rules, 24, SkillChoice::Unusable),
            Err(CreateError::SkillNotAllowed(24))
        );
        // A listed skill cannot be taken off the sheet either.
        assert_eq!(
            b.set_skill(&rules, 6, SkillChoice::Unusable),
            Err(CreateError::SkillNotAllowed(6))
        );
        assert_eq!(b.skill(6), SkillChoice::Untrained);
        // validate catches the same on a hand-edited map.
        b.skills.insert(40, SkillChoice::Specialized);
        assert_eq!(b.validate(&rules), Err(CreateError::SkillNotAllowed(40)));
        b.skills.insert(40, SkillChoice::Trained);
        b.skills.insert(14, SkillChoice::Untrained);
        assert_eq!(b.validate(&rules), Err(CreateError::SkillNotAllowed(14)));
        b.skills.insert(14, SkillChoice::Trained);
        b.skills.insert(77, SkillChoice::Trained);
        assert_eq!(b.validate(&rules), Err(CreateError::SkillNotAllowed(77)));
    }

    #[test]
    fn name_rules() {
        assert!(valid_name("Bob"));
        assert!(valid_name("  Bob  "), "trimmed");
        assert!(valid_name("Mary-Jane O'Neil"));
        assert!(valid_name(&"a".repeat(32)));
        assert!(!valid_name(&"a".repeat(33)));
        assert!(!valid_name("Bo"));
        assert!(!valid_name(""));
        assert!(!valid_name("Bob1"));
        assert!(!valid_name("Bob_"));
        assert!(!valid_name("-Bob"));
        assert!(!valid_name("Bob-"));
        assert!(!valid_name("Bob  Smith"), "no double separators");
        assert!(!valid_name("Bob '-Smith"));
        assert!(!valid_name("Björn"), "ASCII letters only");
        let rules = aluvian();
        let mut b = blank(&rules);
        b.name = "x".into();
        assert_eq!(b.validate(&rules), Err(CreateError::InvalidName));
        b.name = "Valid Name".into();
        b.start_area = 4;
        assert_eq!(b.validate(&rules), Err(CreateError::InvalidStartArea));
    }

    #[test]
    fn helpers() {
        let rules = aluvian();
        assert_eq!(rules.template_index("bow"), Some(1));
        assert_eq!(rules.template_index("1"), Some(1));
        assert_eq!(rules.template_index("7"), None);
        assert_eq!(rules.template_index("wizard"), None);
        assert_eq!(rules.start_area_index("yar"), Some(2));
        assert_eq!(rules.start_area_index("3"), Some(3));
        assert_eq!(rules.start_area_index("9"), None);
        assert_eq!(gender_id("M"), Some(1));
        assert_eq!(gender_id("female"), Some(2));
        assert_eq!(gender_id("x"), None);
        assert!(!rules.is_olthoi());
        assert!(rules.summary().contains("Melee Defense"));
        assert_eq!(create_failure_message(3), "that name is already in use");
    }

    #[test]
    fn message_wire_layout() {
        let rules = aluvian();
        let mut b = blank(&rules);
        b.name = "Bob".into();
        b.look.heritage = 2;
        b.look.gender = 2;
        b.look.hair_style = 3;
        b.look.eyes = 1;
        b.look.skin_shade = 0.25;
        b.template = 1;
        b.attributes = [40, 30, 100, 100, 50, 10];
        b.start_area = 2;
        b.set_skill(&rules, 6, SkillChoice::Specialized).unwrap();
        b.set_skill(&rules, 44, SkillChoice::Trained).unwrap();
        let m = b.to_message("acct", 1);
        assert_eq!(m.skills.len(), SKILL_SLOTS);
        assert_eq!(m.skills[6], 3);
        assert_eq!(m.skills[44], 2);
        assert_eq!(m.skills[14], 2, "always trained");
        assert_eq!(m.skills[34], 1, "listed, untrained");
        assert_eq!(m.skills[0], 0, "not on the sheet");
        let bytes = m.encode();

        // The same message by hand (ACE CharacterCreateInfo.Unpack order).
        let mut w = ac_net::wire::Writer::new();
        w.u32(0xF656);
        w.u16(4).bytes(b"acct").u16(0); // string16 "acct" padded to 4
        w.u32(1).u32(2).u32(2);
        for v in [1u32, 0, 0, 0, 0, 3, u32::MAX, 0, 0, 0, 0, 0, 0, 0] {
            w.u32(v); // eyes nose mouth hair_color eye_color hair_style, then gear style/color x4
        }
        for v in [0.25f64, 0.5, 0.0, 0.0, 0.0, 0.0] {
            w.f64(v); // skin hair headgear shirt pants footwear hues
        }
        w.i32(1);
        for v in [40u32, 30, 100, 100, 50, 10] {
            w.u32(v);
        }
        w.u32(1).u32(0); // slot, class id
        w.u32(55);
        let mut slots = [0u32; 55];
        for r in &rules.skills {
            slots[r.skill as usize] = r.default.wire();
        }
        slots[6] = 3;
        slots[44] = 2;
        for v in slots {
            w.u32(v);
        }
        w.u16(3).bytes(b"Bob").u8(0).u8(0).u8(0); // string16 "Bob" padded to 4
        w.u32(2).u32(0).u32(0);
        let expect = w.finish();
        assert_eq!(
            bytes.len(),
            4 + 8 + 12 + 56 + 48 + 4 + 24 + 8 + 4 + 220 + 8 + 12
        );
        assert_eq!(bytes, expect);
    }

    #[test]
    fn rules_and_templates_over_the_archives() {
        let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
            eprintln!("AC_DATA_DIR unset; skipping");
            return;
        };
        let assets = ac_scene::Assets::open(dir).unwrap();
        let cg = assets.chargen().unwrap();
        let r = rules(&assets, HERITAGE_ALUVIAN).unwrap();
        assert_eq!((r.attribute_credits, r.skill_credits), (330, 52));
        assert_eq!(r.start_area_names[0], "Holtburg");
        assert_eq!(r.start_areas, vec![0, 1, 2, 3]);
        assert_eq!(r.skills.len(), 38, "every SkillTable entry is on the sheet");
        let md = r.skill(6).unwrap();
        assert_eq!(
            (md.train_cost, md.specialize_cost, md.can_specialize),
            (10, 10, true)
        );
        assert_eq!(md.default, SkillChoice::Untrained);
        assert!(md.usable_untrained);
        // Heritage override: Arcane Lore free, 2 to specialize.
        let al = r.skill(14).unwrap();
        assert_eq!(
            (al.train_cost, al.specialize_cost, al.default),
            (0, 2, SkillChoice::Trained)
        );
        assert!(al.can_specialize);
        // War Magic 16 / 28 total: extra 12; needs training to use.
        let wm = r.skill(34).unwrap();
        assert_eq!((wm.train_cost, wm.specialize_cost), (16, 12));
        assert!(!wm.usable_untrained);
        for id in ALWAYS_TRAINED {
            let s = r.skill(id).unwrap();
            assert_eq!(
                (s.train_cost, s.default),
                (0, SkillChoice::Trained),
                "{}",
                s.name
            );
        }
        for id in [40, 18, 28, 29, 30] {
            assert!(!r.skill(id).unwrap().can_specialize, "skill {id}");
        }
        assert!(
            r.skill(15).unwrap().can_specialize,
            "magic defense specializes for 12"
        );
        assert_eq!(r.skill(15).unwrap().specialize_cost, 12);

        // Every template of every heritage validates, and the built-in
        // ones spend the whole budget exactly.
        for (hid, h) in &cg.heritage_groups {
            let r = rules(&assets, *hid).unwrap();
            for (i, t) in h.templates.iter().enumerate() {
                let mut b = CharacterBuild::new(&assets, *hid, 1).unwrap();
                b.name = "Template Test".into();
                b.apply_template(&cg, &r, i);
                assert_eq!(b.template, i);
                assert_eq!(b.validate(&r), Ok(()), "{} {}", h.name, t.name);
                let used = b.credits_used(&r);
                if t.normal_skills.is_empty() && t.primary_skills.is_empty() {
                    assert_eq!(used, 0, "{} {}", h.name, t.name);
                    assert_eq!(b.attribute_points_used(), 60);
                } else {
                    assert_eq!(used, r.skill_credits, "{} {}", h.name, t.name);
                    assert_eq!(b.attribute_points_used(), r.attribute_credits);
                }
                let m = b.to_message("acct", 0);
                assert_eq!(m.skills.len(), SKILL_SLOTS);
                assert_eq!(m.skills[24], 2, "Run always trained on the wire");
            }
        }
        // Olthoi: 60 attribute points, 68 credits, its own lair.
        let o = rules(&assets, HERITAGE_OLTHOI).unwrap();
        assert!(o.is_olthoi());
        assert_eq!((o.attribute_credits, o.skill_credits), (60, 68));
        assert_eq!(o.start_area_names, vec!["OlthoiLair".to_string()]);

        // from_options resolves names.
        let (b, r) = CharacterBuild::from_options(
            &assets,
            "Bow Tester",
            Some("sho"),
            Some("f"),
            Some("bow"),
            Some("yaraq"),
        )
        .unwrap();
        assert_eq!((b.look.heritage, b.look.gender), (3, 2));
        assert_eq!(r.templates[b.template], "Bow Hunter");
        assert_eq!(b.start_area, 2);
        assert_eq!(b.validate(&r), Ok(()));
        assert!(
            CharacterBuild::from_options(&assets, "X", Some("klingon"), None, None, None).is_err()
        );
        assert!(
            CharacterBuild::from_options(&assets, "X", None, None, Some("wizard"), None).is_err()
        );
        assert!(
            CharacterBuild::from_options(&assets, "X", None, None, None, Some("olthoi")).is_err()
        );
    }
}
