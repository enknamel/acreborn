//! Character creation and selection, as the game defines them (see
//! `docs/game/mechanics.md`, "Character creation"): a heritage and sex
//! with an appearance, a starting town, 330 attribute points spread over
//! six attributes of 10..=100 each, and skill credits (52; Olthoi 68)
//! spent on training or specializing skills at costs from the SkillTable,
//! overridden per heritage by the CharGen table. Templates preset the
//! attributes and skills. The server validates the same rules
//! (ACE `PlayerFactory.Create`), so a build that passes [`validate`]
//! is accepted.
//!
//! The client-side flow: after the character list arrives the client
//! either auto-enters (`Config::character` given, or `auto_enter`) or
//! emits [`Event::Characters`] and waits for [`Client::enter_world`],
//! [`Client::create_character`] or [`Client::delete_character`].

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
/// Every attribute starts in this range.
pub const ATTRIBUTE_MIN: u32 = 10;
pub const ATTRIBUTE_MAX: u32 = 100;
/// Number of skill slots on the wire (skill ids 0..55).
pub const SKILL_SLOTS: usize = 55;

/// Skills every character has trained for free (ACE `AlwaysTrained`).
pub const ALWAYS_TRAINED: [u32; 6] = [14, 15, 22, 24, 36, 40]; // ArcaneLore, MagicDefense, Jump, Run, Loyalty, Salvaging

/// What the player chose for a skill. Wire values match ACE
/// `SkillAdvancementClass`: Inactive 0, Untrained 1, Trained 2,
/// Specialized 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillChoice {
    /// Not on the sheet at all (unusable until trained).
    Unusable,
    /// Usable at its formula value, costs nothing, cannot be raised.
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
}

/// Per-skill creation costs and defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRule {
    pub skill: u32,
    pub name: String,
    /// Credits to train (0 for always-trained skills).
    pub train_cost: u32,
    /// Extra credits to specialize a trained skill.
    pub specialize_cost: u32,
    /// The state a fresh character has without spending anything.
    pub default: SkillChoice,
    /// The skill can be specialized at creation.
    pub can_specialize: bool,
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

    /// Set the attributes and skills from a heritage template.
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
        let Some(r) = rules.skills.iter().find(|r| r.skill == skill) else {
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
    /// for the skill (going over budget is allowed here and caught by
    /// [`validate`], so the UI can show a negative balance).
    pub fn set_skill(
        &mut self,
        rules: &Rules,
        skill: u32,
        choice: SkillChoice,
    ) -> Result<(), CreateError> {
        let Some(r) = rules.skills.iter().find(|r| r.skill == skill) else {
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

    /// The checks the server makes before accepting the character.
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
        let credits = self.credits_used(rules);
        if credits > rules.skill_credits {
            return Err(CreateError::TooManySkillCredits {
                used: credits,
                allowed: rules.skill_credits,
            });
        }
        for (&s, &c) in &self.skills {
            let Some(r) = rules.skills.iter().find(|r| r.skill == s) else {
                return Err(CreateError::SkillNotAllowed(s));
            };
            if c == SkillChoice::Specialized && !r.can_specialize {
                return Err(CreateError::SkillNotAllowed(s));
            }
        }
        if !valid_name(&self.name) {
            return Err(CreateError::InvalidName);
        }
        if !rules.start_areas.contains(&self.start_area) {
            return Err(CreateError::InvalidStartArea);
        }
        Ok(())
    }

    /// The wire message for this build.
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

/// Skill costs at creation: the SkillTable's, overridden by the
/// heritage's CharGen entries; always-trained skills are free and
/// trained by default. Filled in by the creation model work.
fn skill_rules(table: &SkillTable, h: &HeritageGroupCg) -> Vec<SkillRule> {
    table
        .skills
        .iter()
        .map(|(id, s)| {
            let over = h.skills.iter().find(|c| c.skill == *id);
            let always = ALWAYS_TRAINED.contains(id);
            SkillRule {
                skill: *id,
                name: s.name.clone(),
                train_cost: if always {
                    0
                } else {
                    over.map(|c| c.normal_cost).unwrap_or(s.trained_cost).max(0) as u32
                },
                specialize_cost: over
                    .map(|c| c.primary_cost)
                    .unwrap_or(s.specialized_cost)
                    .max(0) as u32,
                default: if always {
                    SkillChoice::Trained
                } else {
                    SkillChoice::Unusable
                },
                can_specialize: true,
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

/// Server name rules: 3..=32 characters, letters plus spaces, hyphens
/// and apostrophes between them.
pub fn valid_name(name: &str) -> bool {
    let n = name.trim();
    (3..=32).contains(&n.chars().count())
        && n.chars()
            .all(|c| c.is_ascii_alphabetic() || matches!(c, ' ' | '-' | '\''))
        && n.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
}

/// Server answers to CharacterCreate (0xF643).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateOutcome {
    Created {
        id: u32,
        name: String,
    },
    /// ACE `CharacterGenerationVerificationResponse` code (1 = name in
    /// use, 2 = name banned, 3 = corrupt, ... ).
    Failed(u32),
}

impl Client {
    /// Send the character to the server; on success the server answers
    /// with [`Event::CharacterCreated`] and the client enters the world
    /// with it.
    pub fn create_character(&mut self, build: &CharacterBuild) -> Result<(), CreateError> {
        let rules = rules(&self.assets, build.look.heritage)?;
        build.validate(&rules)?;
        let slot = self.characters.len() as u32;
        let msg = build.to_message(&self.config.account, slot);
        tracing::info!("creating character {}", msg.name);
        self.session
            .send_message(ac_net::messages::queue::UI, msg.encode());
        Ok(())
    }

    /// Enter the world with one of the account's characters.
    pub fn enter_world(&mut self, id: u32) {
        if self.enter_requested {
            return;
        }
        self.config.character = self
            .characters
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.name.clone());
        self.enter_requested = true;
        self.session.send_message(
            ac_net::messages::queue::UI,
            ac_net::messages::enter_world_request(),
        );
    }

    /// Ask the server to delete a character (it is kept for a while and
    /// can be restored; the list refreshes with a countdown).
    pub fn delete_character(&mut self, id: u32) {
        let mut w = ac_net::wire::Writer::new();
        w.u32(ac_net::messages::opcode::CHARACTER_DELETE)
            .string16(&self.config.account)
            .u32(self.characters.iter().position(|c| c.id == id).unwrap_or(0) as u32);
        self.session
            .send_message(ac_net::messages::queue::UI, w.finish());
    }
}
