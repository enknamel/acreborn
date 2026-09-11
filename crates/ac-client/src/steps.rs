//! Everything a character might do, in the order it is considered.
//!
//! This was a chain of twenty `if this(now) { return }` lines. The
//! order was doing a great deal of work -- it silently settles a
//! hundred questions, such as whether a dying character heals or flees,
//! whether loot is taken before a leader is followed, whether the pack
//! is tidied before anything decides it is full -- and none of that was
//! readable, loggable or testable, because it was control flow.
//!
//! It is a table now. The same order, the same behaviour, but the order
//! is a thing the program has rather than a thing it is: a panel can
//! show it, the log can name the step that claimed a tick, and a test
//! can assert that healing comes before hunting.
//!
//! Two kinds of step, because they are not the same sort of thing (see
//! `docs/agent.md`):
//!
//! - [`Layer::Reflex`] is what must happen before anything is decided.
//!   A spell in the air, health at forty per cent. Fixed order, no
//!   scoring, and the cost is paid by every session on every tick.
//! - [`Layer::Goal`] is what the character does with its time when
//!   nothing is on fire. These are the ones whose fixed order will
//!   become utility scoring, and they are marked so that the change can
//!   be made to exactly them.
//!
//! [`Housekeeping`] is neither: a handful of things that run every tick
//! and never claim it.

use crate::did::Did;
use crate::Client;
use std::time::Instant;

/// Which sort of step this is, and so what becomes of it later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// Runs before anything is chosen, in this order, always.
    Reflex,
    /// What to do with the time. Ordered by hand today; scored later.
    Goal,
}

/// One thing a character might do.
pub struct Step {
    /// What it is called, in the log and in the panel.
    pub name: &'static str,
    pub layer: Layer,
    /// Why it sits where it does. The reason the order matters, kept
    /// beside the order rather than in a comment above one branch of a
    /// chain.
    pub why: &'static str,
    run: fn(&mut Client, Instant) -> Did,
}

impl Step {
    pub fn run(&self, client: &mut Client, now: Instant) -> Did {
        (self.run)(client, now)
    }
}

/// Wrap one of the old `-> bool` steps: true meant it claimed the tick.
///
/// Every step will say more than this in time (see `crate::did`); until
/// it does, "it acted" and "it did not" is exactly what the old chain
/// knew, so nothing is lost or invented in the meantime.
macro_rules! claimed {
    ($f:expr) => {
        |client: &mut Client, now: Instant| {
            if $f(client, now) {
                Did::Acting
            } else {
                Did::Done
            }
        }
    };
}

/// The order. Reading down this table is reading what the character
/// cares about, most pressing first.
pub const STEPS: &[Step] = &[
    Step {
        name: "dodge",
        layer: Layer::Reflex,
        why: "a spell already in the air is stepped out of before anything else, healing included",
        run: claimed!(Client::autoplay_dodge),
    },
    Step {
        name: "survive",
        layer: Layer::Reflex,
        why: "heal or run before doing anything that assumes being alive",
        run: claimed!(Client::autoplay_survive),
    },
    Step {
        name: "recover",
        layer: Layer::Reflex,
        why: "dead, or on the way back from it: nothing else until the corpse is dealt with",
        run: claimed!(Client::autoplay_recover),
    },
    Step {
        name: "academy",
        layer: Layer::Reflex,
        why: "a new character finishes the tutorial before it is let loose on anything else",
        run: claimed!(Client::autoplay_academy),
    },
    Step {
        name: "urgent buffs",
        layer: Layer::Reflex,
        why: "a buff about to lapse goes back up before anything else, fight or no fight",
        run: |c, now| {
            if c.autoplay_buff(now, true) {
                Did::Acting
            } else {
                Did::Done
            }
        },
    },
    Step {
        name: "vitals",
        layer: Layer::Reflex,
        why: "mana and stamina are kept up between everything else",
        run: claimed!(Client::autoplay_vitals),
    },
    Step {
        name: "loot",
        layer: Layer::Goal,
        why: "a corpse keeps for five minutes and rots; the leader and the shops do not",
        run: claimed!(Client::autoplay_loot),
    },
    Step {
        name: "catch up",
        layer: Layer::Goal,
        why: "a leader that has got well away is caught up with before anything else is considered",
        run: |c, now| {
            if c.autoplay_follow(now, true) {
                Did::Acting
            } else {
                Did::Done
            }
        },
    },
    Step {
        name: "team",
        layer: Layer::Goal,
        why: "debuff the party's target, hand a teammate what it is short of, heal whoever is worst",
        run: claimed!(Client::autoplay_team),
    },
    Step {
        name: "salvage",
        layer: Layer::Goal,
        why: "salvage sits between fights: left alone while anything is being fought",
        run: claimed!(Client::autoplay_salvage),
    },
    Step {
        name: "fight",
        layer: Layer::Goal,
        why: "what the character is mostly for",
        run: claimed!(Client::autoplay_fight),
    },
    Step {
        name: "buffs",
        layer: Layer::Goal,
        why: "the buffs that were not urgent, once the fighting is done",
        run: |c, now| {
            if c.autoplay_buff(now, false) {
                Did::Acting
            } else {
                Did::Done
            }
        },
    },
    Step {
        name: "follow",
        layer: Layer::Goal,
        why: "a leader near at hand is followed once the fighting is done",
        run: |c, now| {
            if c.autoplay_follow(now, false) {
                Did::Acting
            } else {
                Did::Done
            }
        },
    },
    Step {
        name: "resume the journey",
        layer: Layer::Goal,
        why: "a walk broken off by a fight is picked up again",
        run: |c, _| {
            if c.autoplay_resume_journey() {
                Did::Acting
            } else {
                Did::Done
            }
        },
    },
    Step {
        name: "tidy",
        layer: Layer::Goal,
        why: "before anything decides the pack is full: a pack full of change does not need emptying in town",
        run: claimed!(Client::autoplay_tidy),
    },
    Step {
        name: "grow",
        layer: Layer::Goal,
        why: "with nothing else to do: spend experience, find monsters, run to town",
        run: claimed!(Client::autoplay_grow),
    },
];

/// The things that run every tick and never claim it: they put a
/// weapon in an empty hand, raise a shield, restock a quiver from the
/// pack. None of them is a reason to stop considering anything else.
pub struct Housekeeping {
    pub name: &'static str,
    run: fn(&mut Client, Instant),
}

impl Housekeeping {
    pub fn run(&self, client: &mut Client, now: Instant) {
        (self.run)(client, now)
    }
}

pub const HOUSEKEEPING: &[Housekeeping] = &[
    Housekeeping {
        name: "take up a weapon",
        run: |c, _| c.autoplay_pending_wield(),
    },
    Housekeeping {
        name: "shield",
        run: Client::autoplay_shield,
    },
    Housekeeping {
        name: "rearm",
        run: |c, _| c.autoplay_rearm(),
    },
    Housekeeping {
        name: "restock from the pack",
        run: |c, _| c.autoplay_stock(),
    },
];

/// The step of this name, for a log line or a panel row.
pub fn named(name: &str) -> Option<&'static Step> {
    STEPS.iter().find(|s| s.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_order_says_what_the_character_cares_about() {
        let at = |name: &str| {
            STEPS
                .iter()
                .position(|s| s.name == name)
                .unwrap_or_else(|| panic!("no step called {name}"))
        };

        // Staying alive comes before everything. These are the ones
        // that cost a character its life when they are wrong.
        assert!(at("dodge") < at("survive"), "step out before healing");
        assert!(at("survive") < at("fight"), "heal before fighting");
        assert!(at("survive") < at("loot"), "heal before looting");
        assert!(at("recover") < at("fight"), "deal with the corpse first");

        // A buff about to lapse goes up before the fight; the rest wait
        // until after it.
        assert!(at("urgent buffs") < at("fight"));
        assert!(at("fight") < at("buffs"));

        // A corpse rots and a shop does not.
        assert!(at("loot") < at("grow"), "loot before shopping");
        assert!(at("loot") < at("salvage"));

        // Tidying comes before anything that decides the pack is full.
        assert!(at("tidy") < at("grow"));

        // And the last word is the one that finds something to do.
        assert_eq!(STEPS.last().map(|s| s.name), Some("grow"));
    }

    #[test]
    fn every_step_says_what_it_is_and_why_it_is_there() {
        assert!(!STEPS.is_empty());
        for s in STEPS {
            assert!(!s.name.is_empty());
            assert!(
                s.why.len() > 20,
                "{} should say why it sits where it does",
                s.name
            );
        }
        // Names are unique, or naming one in a log is ambiguous.
        let mut names: Vec<&str> = STEPS.iter().map(|s| s.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "two steps share a name");
        assert!(named("fight").is_some());
        assert!(named("nonsense").is_none());
    }

    #[test]
    fn the_reflexes_come_first_and_are_few() {
        // Once the goals start, no reflex follows: a reflex that ran
        // after a goal had claimed the tick would never run at all.
        let first_goal = STEPS
            .iter()
            .position(|s| s.layer == Layer::Goal)
            .expect("some goals");
        assert!(
            STEPS[first_goal..].iter().all(|s| s.layer == Layer::Goal),
            "a reflex is sitting below a goal"
        );
        // They are the per-tick cost every session pays, so there
        // should not be many.
        let reflexes = STEPS.iter().filter(|s| s.layer == Layer::Reflex).count();
        assert!(reflexes <= 8, "{reflexes} reflexes is a lot to pay a tick");
    }
}
