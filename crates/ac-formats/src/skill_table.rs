//! SkillTable (0x0E000004): one record per skill id with its name, training
//! costs and the attribute formula the client uses for the skill's base
//! value. Layout cross-checked against ACE's `SkillTable`/`SkillBase`.

use serde::Serialize;

use crate::{expect_id, Reader, Result};

/// PropertyAttribute ids used by [`SkillFormula`]: 0 none, 1 Strength,
/// 2 Endurance, 3 Quickness, 4 Coordination, 5 Focus, 6 Self.
pub mod attribute {
    pub const NONE: u32 = 0;
    pub const STRENGTH: u32 = 1;
    pub const ENDURANCE: u32 = 2;
    pub const QUICKNESS: u32 = 3;
    pub const COORDINATION: u32 = 4;
    pub const FOCUS: u32 = 5;
    pub const SELF: u32 = 6;
}

/// `base = (attr1 + attr2) / divisor`, rounded; `x == 0` disables the
/// attribute contribution entirely. `w` and `y` are unused by the client.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct SkillFormula {
    pub w: u32,
    pub x: u32,
    pub y: u32,
    /// Divisor applied to the attribute sum.
    pub divisor: u32,
    pub attr1: u32,
    pub attr2: u32,
}

impl SkillFormula {
    fn parse(r: &mut Reader) -> Result<Self> {
        Ok(SkillFormula {
            w: r.u32()?,
            x: r.u32()?,
            y: r.u32()?,
            divisor: r.u32()?,
            attr1: r.u32()?,
            attr2: r.u32()?,
        })
    }

    /// The attribute-derived part of a skill, given a lookup from attribute
    /// id to the attribute's current value.
    pub fn apply(&self, attr: impl Fn(u32) -> u32) -> u32 {
        if self.x == 0 {
            return 0;
        }
        let mut total = attr(self.attr1);
        if self.attr2 != attribute::NONE {
            total += attr(self.attr2);
        }
        if self.divisor > 1 {
            total = (total as f32 / self.divisor as f32).round() as u32;
        }
        total
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SkillBase {
    pub description: String,
    pub name: String,
    pub icon_id: u32,
    pub trained_cost: i32,
    /// Total credits to specialize, including the trained cost.
    pub specialized_cost: i32,
    /// 1 combat, 2 other, 3 magic.
    pub category: u32,
    pub chargen_use: u32,
    /// Minimum advancement class needed to use the skill: 1 usable while
    /// untrained, 2 needs training.
    pub min_level: u32,
    pub formula: SkillFormula,
    pub upper_bound: f64,
    pub lower_bound: f64,
    pub learn_mod: f64,
}

impl SkillBase {
    fn parse(r: &mut Reader) -> Result<Self> {
        let description = r.pstring16()?;
        r.align4()?;
        let name = r.pstring16()?;
        r.align4()?;
        Ok(SkillBase {
            description,
            name,
            icon_id: r.u32()?,
            trained_cost: r.i32()?,
            specialized_cost: r.i32()?,
            category: r.u32()?,
            chargen_use: r.u32()?,
            min_level: r.u32()?,
            formula: SkillFormula::parse(r)?,
            upper_bound: r.f64()?,
            lower_bound: r.f64()?,
            learn_mod: r.f64()?,
        })
    }

    /// Whether a skill at this advancement class (1 untrained, 2 trained,
    /// 3 specialized) gets its attribute-derived base at all.
    pub fn usable_at(&self, advancement_class: u32) -> bool {
        match advancement_class {
            2 | 3 => true,
            1 => self.min_level == 1,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SkillTable {
    pub id: u32,
    /// Sorted by skill id.
    pub skills: Vec<(u32, SkillBase)>,
}

impl SkillTable {
    pub const ID: u32 = 0x0E00_0004;

    pub fn parse(id: u32, bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let id = expect_id(&mut r, id)?;
        let mut skills = r.packed_hash_table(|r| r.u32(), SkillBase::parse)?;
        r.finish()?;
        skills.sort_by_key(|(k, _)| *k);
        Ok(SkillTable { id, skills })
    }

    pub fn get(&self, skill_id: u32) -> Option<&SkillBase> {
        self.skills
            .binary_search_by_key(&skill_id, |(k, _)| *k)
            .ok()
            .map(|i| &self.skills[i].1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formula_rounds_and_respects_x() {
        let f = SkillFormula {
            x: 1,
            divisor: 3,
            attr1: attribute::QUICKNESS,
            attr2: attribute::COORDINATION,
            ..Default::default()
        };
        // (100 + 55) / 3 = 51.67 -> 52
        assert_eq!(f.apply(|a| if a == 3 { 100 } else { 55 }), 52);
        let off = SkillFormula { x: 0, ..f };
        assert_eq!(off.apply(|_| 100), 0);
        let single = SkillFormula {
            attr2: attribute::NONE,
            divisor: 1,
            ..f
        };
        assert_eq!(single.apply(|_| 77), 77);
    }
}
