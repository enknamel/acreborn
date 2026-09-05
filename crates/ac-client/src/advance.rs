//! Character advancement (see `docs/game/mechanics.md`, section 3):
//! unassigned experience is spent on attributes, vitals and trained or
//! specialized skills one rank at a time, at costs from the XpTable
//! (0x0E000018), and skill credits train new skills at the SkillTable's
//! price. The server (ACE `Player_Skills`, `Player_Attributes`,
//! `Player_Vitals`) takes an XP amount and adds it to the stat; sending
//! exactly the cost of the next rank raises it by one. It answers with
//! the updated record and the new unassigned total.

use crate::Client;

/// Attribute indices in `stats.attributes`, also the wire ids minus one
/// (PropertyAttribute: Strength 1, Endurance 2, Quickness 3,
/// Coordination 4, Focus 5, Self 6).
pub const ATTRIBUTE_NAMES: [&str; 6] = [
    "Strength",
    "Endurance",
    "Quickness",
    "Coordination",
    "Focus",
    "Self",
];
/// Vital indices in `stats.vitals`; wire ids are MaxHealth 1,
/// MaxStamina 3, MaxMana 5.
pub const VITAL_NAMES: [&str; 3] = ["Health", "Stamina", "Mana"];
const VITAL_WIRE: [u32; 3] = [1, 3, 5];

/// What one more rank costs, or why it cannot be bought.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaiseCost {
    /// XP for the next rank (the caller checks the unassigned pool).
    Xp(u32),
    /// At the table's last rank.
    Maxed,
    /// Not trained (skills), or unknown.
    Unavailable,
}

impl RaiseCost {
    pub fn xp(self) -> Option<u32> {
        match self {
            RaiseCost::Xp(x) => Some(x),
            _ => None,
        }
    }
}

fn next_cost(table: &[u32], ranks: u32, spent: u32) -> RaiseCost {
    match table.get(ranks as usize + 1) {
        Some(&total) => RaiseCost::Xp(total.saturating_sub(spent).max(1)),
        None => RaiseCost::Maxed,
    }
}

impl Client {
    /// XP for the next point of an attribute (index into
    /// [`ATTRIBUTE_NAMES`]).
    pub fn attribute_raise_cost(&self, index: usize) -> RaiseCost {
        let (Ok(xp), Some(a)) = (
            self.assets.xp_table(),
            self.world.stats.attributes.get(index),
        ) else {
            return RaiseCost::Unavailable;
        };
        next_cost(&xp.attribute, a.ranks, a.xp)
    }

    /// XP for the next point of a vital (index into [`VITAL_NAMES`]).
    pub fn vital_raise_cost(&self, index: usize) -> RaiseCost {
        let (Ok(xp), Some(v)) = (self.assets.xp_table(), self.world.stats.vitals.get(index)) else {
            return RaiseCost::Unavailable;
        };
        next_cost(&xp.vital, v.ranks, v.xp)
    }

    /// XP for the next rank of a trained or specialized skill.
    pub fn skill_raise_cost(&self, skill: u32) -> RaiseCost {
        use ac_world::stats::sac;
        let (Ok(xp), Some(s)) = (self.assets.xp_table(), self.world.stats.skill(skill)) else {
            return RaiseCost::Unavailable;
        };
        let table = match s.advancement {
            sac::TRAINED => &xp.trained_skill,
            sac::SPECIALIZED => &xp.specialized_skill,
            _ => return RaiseCost::Unavailable,
        };
        next_cost(table, s.ranks as u32, s.xp)
    }

    /// Skill credits to train an untrained skill; None when it is
    /// already trained or not on the sheet.
    pub fn skill_train_cost(&self, skill: u32) -> Option<u32> {
        use ac_world::stats::sac;
        // A skill the sheet lacks a record for is untrained (the server
        // creates the record when it is trained).
        if self
            .world
            .stats
            .skill(skill)
            .is_some_and(|s| s.advancement >= sac::TRAINED)
        {
            return None;
        }
        let table = self.assets.skill_table().ok()?;
        let base = table.get(skill)?;
        Some(base.trained_cost.max(0) as u32)
    }

    fn can_afford(&self, cost: RaiseCost) -> Option<u32> {
        let xp = cost.xp()?;
        (self.world.stats.available_xp >= xp as i64).then_some(xp)
    }

    /// Spend unassigned XP on one attribute point (RaiseAttribute 0x0045).
    /// False when unaffordable or maxed.
    pub fn raise_attribute(&mut self, index: usize) -> bool {
        use ac_net::messages::action;
        let Some(xp) = self.can_afford(self.attribute_raise_cost(index)) else {
            return false;
        };
        tracing::info!("raise {} for {xp} xp", ATTRIBUTE_NAMES[index.min(5)]);
        let mut w = ac_net::wire::Writer::new();
        w.u32(index as u32 + 1).u32(xp);
        self.session
            .send_action(action::RAISE_ATTRIBUTE, &w.finish());
        true
    }

    /// Spend unassigned XP on one vital point (RaiseVital 0x0044).
    pub fn raise_vital(&mut self, index: usize) -> bool {
        use ac_net::messages::action;
        let (Some(xp), Some(&wire)) = (
            self.can_afford(self.vital_raise_cost(index)),
            VITAL_WIRE.get(index),
        ) else {
            return false;
        };
        tracing::info!("raise {} for {xp} xp", VITAL_NAMES[index.min(2)]);
        let mut w = ac_net::wire::Writer::new();
        w.u32(wire).u32(xp);
        self.session.send_action(action::RAISE_VITAL, &w.finish());
        true
    }

    /// Spend unassigned XP on one skill rank (RaiseSkill 0x0046).
    pub fn raise_skill(&mut self, skill: u32) -> bool {
        use ac_net::messages::action;
        let Some(xp) = self.can_afford(self.skill_raise_cost(skill)) else {
            return false;
        };
        tracing::info!("raise {} for {xp} xp", ac_world::stats::skill_name(skill));
        let mut w = ac_net::wire::Writer::new();
        w.u32(skill).u32(xp);
        self.session.send_action(action::RAISE_SKILL, &w.finish());
        true
    }

    /// Train an untrained skill with skill credits (TrainSkill 0x0047).
    pub fn train_skill(&mut self, skill: u32) -> bool {
        use ac_net::messages::action;
        let Some(credits) = self.skill_train_cost(skill) else {
            return false;
        };
        if self.world.stats.skill_credits < credits as i32 {
            return false;
        }
        tracing::info!(
            "train {} for {credits} credits",
            ac_world::stats::skill_name(skill)
        );
        let mut w = ac_net::wire::Writer::new();
        w.u32(skill).u32(credits);
        self.session.send_action(action::TRAIN_SKILL, &w.finish());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_rank_is_the_table_step_minus_what_was_spent() {
        let table = [0u32, 23, 56, 97];
        assert_eq!(next_cost(&table, 0, 0), RaiseCost::Xp(23));
        assert_eq!(next_cost(&table, 1, 23), RaiseCost::Xp(33));
        // Partly paid ranks cost the remainder.
        assert_eq!(next_cost(&table, 1, 40), RaiseCost::Xp(16));
        assert_eq!(next_cost(&table, 3, 97), RaiseCost::Maxed);
    }
}
