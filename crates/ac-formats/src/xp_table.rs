//! XpTable (0x0E000018): the experience costs of raising an attribute, a
//! vital, a trained or a specialized skill by a number of points, the
//! total experience needed to reach each character level and the skill
//! credits each level grants. Layout cross-checked against ACE's `XpTable`
//! and the raw file: four `i32` point counts, `u32` level count, then the
//! four point lists (count + 1 `u32` each, index = points raised), the
//! level list (count + 1 `u64`, index = level) and the credit list
//! (count + 1 `u32`, index = level).

use serde::Serialize;

use crate::{expect_id, Reader, Result};

#[derive(Debug, Clone, Default, Serialize)]
pub struct XpTable {
    pub id: u32,
    /// Cumulative XP to raise an attribute by `i` points.
    pub attribute: Vec<u32>,
    /// Cumulative XP to raise a vital by `i` points.
    pub vital: Vec<u32>,
    /// Cumulative XP to raise a trained skill by `i` points.
    pub trained_skill: Vec<u32>,
    /// Cumulative XP to raise a specialized skill by `i` points.
    pub specialized_skill: Vec<u32>,
    /// Total XP a character needs to be level `i` (levels 0 and 1 cost 0).
    pub level_xp: Vec<u64>,
    /// Skill credits granted on reaching level `i`.
    pub level_credits: Vec<u32>,
}

impl XpTable {
    pub const ID: u32 = 0x0E00_0018;

    pub fn parse(id: u32, bytes: &[u8]) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let id = expect_id(&mut r, id)?;
        let attributes = r.u32()? as usize;
        let vitals = r.u32()? as usize;
        let trained = r.u32()? as usize;
        let specialized = r.u32()? as usize;
        let levels = r.u32()? as usize;
        let attribute = r.fixed(attributes + 1, &mut |r| r.u32())?;
        let vital = r.fixed(vitals + 1, &mut |r| r.u32())?;
        let trained_skill = r.fixed(trained + 1, &mut |r| r.u32())?;
        let specialized_skill = r.fixed(specialized + 1, &mut |r| r.u32())?;
        let level_xp = r.fixed(levels + 1, &mut |r| r.u64())?;
        let level_credits = r.fixed(levels + 1, &mut |r| r.u32())?;
        r.finish()?;
        Ok(XpTable {
            id,
            attribute,
            vital,
            trained_skill,
            specialized_skill,
            level_xp,
            level_credits,
        })
    }

    /// The highest level in the table.
    pub fn max_level(&self) -> u32 {
        self.level_xp.len().saturating_sub(1) as u32
    }

    /// Total XP needed to be `level`; None past the table.
    pub fn xp_for_level(&self, level: u32) -> Option<u64> {
        self.level_xp.get(level as usize).copied()
    }

    /// XP from having exactly `level` to reaching `level + 1`; None at the
    /// top of the table.
    pub fn xp_to_next_level(&self, level: u32) -> Option<u64> {
        let here = self.xp_for_level(level)?;
        let next = self.xp_for_level(level + 1)?;
        Some(next.saturating_sub(here))
    }

    /// The level a character with `total_xp` experience has reached.
    pub fn level_for_xp(&self, total_xp: u64) -> u32 {
        // The table starts 0, 0, 1000...: level 1 needs nothing.
        let past = self.level_xp.partition_point(|&xp| xp <= total_xp);
        (past.saturating_sub(1) as u32).max(1).min(self.max_level())
    }

    /// Skill credits granted on reaching `level`.
    pub fn credits_at_level(&self, level: u32) -> u32 {
        self.level_credits.get(level as usize).copied().unwrap_or(0)
    }

    /// Cumulative XP to have raised an attribute `points` times.
    pub fn xp_for_attribute_points(&self, points: u32) -> Option<u32> {
        self.attribute.get(points as usize).copied()
    }

    /// XP for the next attribute point after `points` raises.
    pub fn xp_for_attribute_point(&self, points: u32) -> Option<u32> {
        step(&self.attribute, points)
    }

    pub fn xp_for_vital_points(&self, points: u32) -> Option<u32> {
        self.vital.get(points as usize).copied()
    }

    pub fn xp_for_vital_point(&self, points: u32) -> Option<u32> {
        step(&self.vital, points)
    }

    pub fn xp_for_trained_skill_points(&self, points: u32) -> Option<u32> {
        self.trained_skill.get(points as usize).copied()
    }

    pub fn xp_for_trained_skill_point(&self, points: u32) -> Option<u32> {
        step(&self.trained_skill, points)
    }

    pub fn xp_for_specialized_skill_points(&self, points: u32) -> Option<u32> {
        self.specialized_skill.get(points as usize).copied()
    }

    pub fn xp_for_specialized_skill_point(&self, points: u32) -> Option<u32> {
        step(&self.specialized_skill, points)
    }

    /// Cumulative XP for `points` raises of a skill at an advancement
    /// class (`ac_world::stats::sac`: 2 trained, 3 specialized).
    pub fn xp_for_skill_points(&self, specialized: bool, points: u32) -> Option<u32> {
        if specialized {
            self.xp_for_specialized_skill_points(points)
        } else {
            self.xp_for_trained_skill_points(points)
        }
    }
}

/// The cost of the raise after `points` raises of a cumulative list.
fn step(list: &[u32], points: u32) -> Option<u32> {
    let here = *list.get(points as usize)?;
    let next = *list.get(points as usize + 1)?;
    Some(next.saturating_sub(here))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> XpTable {
        XpTable {
            id: XpTable::ID,
            attribute: vec![0, 110, 277, 501],
            vital: vec![0, 73, 183],
            trained_skill: vec![0, 58, 138],
            specialized_skill: vec![0, 23, 56],
            level_xp: vec![0, 0, 1000, 2777, 5697],
            level_credits: vec![0, 0, 1, 1, 1],
        }
    }

    #[test]
    fn roundtrip() {
        let t = sample();
        let mut b = Vec::new();
        b.extend_from_slice(&XpTable::ID.to_le_bytes());
        for n in [3u32, 2, 2, 2, 4] {
            b.extend_from_slice(&n.to_le_bytes());
        }
        for list in [
            &t.attribute,
            &t.vital,
            &t.trained_skill,
            &t.specialized_skill,
        ] {
            for v in list {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        for v in &t.level_xp {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for v in &t.level_credits {
            b.extend_from_slice(&v.to_le_bytes());
        }
        let p = XpTable::parse(XpTable::ID, &b).unwrap();
        assert_eq!(p.attribute, t.attribute);
        assert_eq!(p.level_xp, t.level_xp);
        assert_eq!(p.level_credits, t.level_credits);
        b.push(0);
        assert!(XpTable::parse(XpTable::ID, &b).is_err());
    }

    #[test]
    fn helpers() {
        let t = sample();
        assert_eq!(t.max_level(), 4);
        assert_eq!(t.xp_for_level(2), Some(1000));
        assert_eq!(t.xp_to_next_level(1), Some(1000));
        assert_eq!(t.xp_to_next_level(2), Some(1777));
        assert_eq!(t.xp_to_next_level(4), None);
        assert_eq!(t.level_for_xp(0), 1);
        assert_eq!(t.level_for_xp(999), 1);
        assert_eq!(t.level_for_xp(1000), 2);
        assert_eq!(t.level_for_xp(2776), 2);
        assert_eq!(t.level_for_xp(5697), 4);
        assert_eq!(t.level_for_xp(u64::MAX), 4);
        assert_eq!(t.credits_at_level(2), 1);
        assert_eq!(t.credits_at_level(99), 0);
        assert_eq!(t.xp_for_attribute_points(2), Some(277));
        assert_eq!(t.xp_for_attribute_point(0), Some(110));
        assert_eq!(t.xp_for_attribute_point(1), Some(167));
        assert_eq!(t.xp_for_attribute_point(3), None);
        assert_eq!(t.xp_for_vital_point(0), Some(73));
        assert_eq!(t.xp_for_trained_skill_point(1), Some(80));
        assert_eq!(t.xp_for_specialized_skill_point(1), Some(33));
        assert_eq!(t.xp_for_skill_points(true, 2), Some(56));
        assert_eq!(t.xp_for_skill_points(false, 2), Some(138));
    }
}
