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
    /// What this is worth doing at this moment, for a goal. Higher
    /// wins. See [`Step::worth`].
    worth: fn(&Client, Instant) -> f32,
    run: fn(&mut Client, Instant) -> Did,
}

/// What a goal is worth when it has no opinion of its own: its place in
/// the table, so that goals which do not score behave exactly as they
/// did when the order was all there was.
///
/// Spaced ten apart, which leaves room for a goal to say "this is worth
/// rather more than usual" without leaping the whole table.
const BY_PLACE: f32 = 10.0;

impl Step {
    pub fn run(&self, client: &mut Client, now: Instant) -> Did {
        (self.run)(client, now)
    }

    /// What this goal is worth right now, in a table where the first
    /// entry is worth most.
    ///
    /// Zero means there is nothing to do and the step is not even
    /// called, which is the one place this can be cheaper than the
    /// chain it replaces: asking "is there a corpse" is a good deal
    /// less work than the looting step deciding there is not.
    ///
    /// A goal that does not score gets its place in the table, so
    /// nothing moves until a curve is written on purpose.
    pub fn worth(&self, client: &Client, now: Instant, place: usize) -> f32 {
        let base = (STEPS.len() - place) as f32 * BY_PLACE;
        let said = (self.worth)(client, now);
        if said == UNDECIDED {
            base
        } else if said <= 0.0 {
            0.0
        } else {
            said
        }
    }
}

/// What a step's scorer returns when it has no opinion.
const UNDECIDED: f32 = f32::NEG_INFINITY;

/// The scorer for a step that has nothing to say about its own worth.
fn by_place(_: &Client, _: Instant) -> f32 {
    UNDECIDED
}

/// What looting is worth: nothing at all when there is no body, its
/// usual place while the body will keep, and more than the next fight
/// once it will not.
///
/// This is the first thing said as a curve that was already being said
/// as an exception. `autoplay_loot` has always broken off a fight for a
/// corpse about to rot and left one alone otherwise, written as a
/// comparison buried in the middle of choosing a corpse. Here it is the
/// reason looting is chosen at all, where it can be read.
fn worth_looting(client: &Client, now: Instant) -> f32 {
    use crate::autoplay::{CORPSE_LIFE, CORPSE_URGENT};
    let mut best: Option<std::time::Duration> = None;
    for o in client.world.objects.values() {
        if o.object_desc_flags & ac_world::object_desc_flags::CORPSE == 0 {
            continue;
        }
        // A corpse we never saw appear is taken as fresh, which is what
        // the looting step assumes too.
        let seen = client
            .autoplay
            .corpse_seen
            .iter()
            .find(|(g, _)| *g == o.guid)
            .map(|(_, t)| *t)
            .unwrap_or(now);
        let left = CORPSE_LIFE.saturating_sub(now.duration_since(seen));
        best = Some(best.map_or(left, |b: std::time::Duration| b.min(left)));
    }
    let Some(left) = best else {
        // No body: do not even ask the looting step, which is the one
        // place weighing is cheaper than the chain it replaces.
        return 0.0;
    };
    if left <= CORPSE_URGENT {
        // About to go. Nothing outranks this: there is no second
        // chance at it, and the fight will still be there afterwards.
        return WORTH_A_LOT;
    }
    // Otherwise it grows as the body ages and as bodies pile up.
    //
    // A character that only breaks off for a corpse about to rot never
    // goes back for the older ones: in a busy dungeon there is always
    // another fight, and the pile behind it is never cleared. So a body
    // is worth a little more every second it waits, and each one still
    // on the floor adds to it. Somewhere past the first third of its
    // life a body outranks starting another fight, which is the
    // behaviour worth having and was impossible to say when the order
    // was fixed.
    let aged = 1.0 - left.as_secs_f32() / CORPSE_LIFE.as_secs_f32();
    let waiting = client
        .world
        .objects
        .values()
        .filter(|o| o.object_desc_flags & ac_world::object_desc_flags::CORPSE != 0)
        .count()
        .min(WAITING_COUNTS) as f32;
    LOOT_AT_REST + aged * AGE_IS_WORTH + waiting * EACH_BODY_IS_WORTH
}

/// What looting is worth with a body just dropped and nothing else
/// waiting: below fighting, because the fight is what made the body.
const LOOT_AT_REST: f32 = 45.0;
/// How much a body's whole life is worth, spread across it. Enough that
/// a body past the first third of its life outranks fighting.
const AGE_IS_WORTH: f32 = 90.0;
/// What each body still on the floor adds.
const EACH_BODY_IS_WORTH: f32 = 12.0;
/// Past this many, another body says nothing new.
const WAITING_COUNTS: usize = 6;

/// More than any goal gets from its place in the table. For the few
/// things that genuinely outrank everything else for a moment.
const WORTH_A_LOT: f32 = 1_000.0;

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
        worth: by_place,
        run: claimed!(Client::autoplay_dodge),
    },
    Step {
        name: "survive",
        layer: Layer::Reflex,
        why: "heal or run before doing anything that assumes being alive",
        worth: by_place,
        run: claimed!(Client::autoplay_survive),
    },
    Step {
        name: "recover",
        layer: Layer::Reflex,
        why: "dead, or on the way back from it: nothing else until the corpse is dealt with",
        worth: by_place,
        run: claimed!(Client::autoplay_recover),
    },
    Step {
        name: "academy",
        layer: Layer::Reflex,
        why: "a new character finishes the tutorial before it is let loose on anything else",
        worth: by_place,
        run: claimed!(Client::autoplay_academy),
    },
    Step {
        name: "urgent buffs",
        layer: Layer::Reflex,
        why: "a buff about to lapse goes back up before anything else, fight or no fight",
        worth: by_place,
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
        worth: by_place,
        run: claimed!(Client::autoplay_vitals),
    },
    Step {
        name: "loot",
        layer: Layer::Goal,
        why: "a corpse keeps for five minutes and rots; the leader and the shops do not",
        worth: worth_looting,
        run: claimed!(Client::autoplay_loot),
    },
    Step {
        name: "catch up",
        layer: Layer::Goal,
        why: "a leader that has got well away is caught up with before anything else is considered",
        worth: by_place,
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
        worth: by_place,
        run: claimed!(Client::autoplay_team),
    },
    Step {
        name: "salvage",
        layer: Layer::Goal,
        why: "salvage sits between fights: left alone while anything is being fought",
        worth: by_place,
        run: claimed!(Client::autoplay_salvage),
    },
    Step {
        name: "fight",
        layer: Layer::Goal,
        why: "what the character is mostly for",
        worth: by_place,
        run: claimed!(Client::autoplay_fight),
    },
    Step {
        name: "buffs",
        layer: Layer::Goal,
        why: "the buffs that were not urgent, once the fighting is done",
        worth: by_place,
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
        worth: by_place,
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
        worth: by_place,
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
        worth: by_place,
        run: claimed!(Client::autoplay_tidy),
    },
    Step {
        name: "grow",
        layer: Layer::Goal,
        why: "with nothing else to do: spend experience, find monsters, run to town",
        worth: by_place,
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

/// The reflexes, in their fixed order.
pub fn reflexes() -> impl Iterator<Item = &'static Step> {
    STEPS.iter().filter(|s| s.layer == Layer::Reflex)
}

/// The goals, worth most first.
///
/// Scoring happens once and the order is settled there; a goal that is
/// worth nothing is not scored again and not run at all. The list is
/// small and lives on the stack, because this runs for every session on
/// every tick.
pub fn weigh(client: &Client, now: Instant) -> Weighed {
    let mut order: Vec<(usize, f32)> = Vec::with_capacity(STEPS.len());
    for (place, step) in STEPS.iter().enumerate() {
        if step.layer != Layer::Goal {
            continue;
        }
        let worth = step.worth(client, now, place);
        if worth > 0.0 {
            order.push((place, worth));
        }
    }
    // Worth most first; ties keep their place in the table, which is
    // what makes a goal with no opinion behave exactly as it did.
    order.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Weighed { order, next: 0 }
}

/// The goals of one tick, in the order they were judged worth doing.
pub struct Weighed {
    order: Vec<(usize, f32)>,
    next: usize,
}

impl Weighed {
    /// The next goal to try, and what it was worth.
    ///
    /// Takes the client because a goal that has become worthless since
    /// the weighing -- the corpse rotted while the fight was being had
    /// -- should not be run on a stale score.
    pub fn next(&mut self, client: &Client, now: Instant) -> Option<(&'static Step, f32)> {
        while let Some(&(place, worth)) = self.order.get(self.next) {
            self.next += 1;
            let step = &STEPS[place];
            if step.worth(client, now, place) > 0.0 {
                return Some((step, worth));
            }
        }
        None
    }

    /// What was weighed, worth most first, for the log and the panel.
    pub fn ranking(&self) -> Vec<(&'static str, f32)> {
        self.order
            .iter()
            .map(|&(place, worth)| (STEPS[place].name, worth))
            .collect()
    }
}

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
    fn a_goal_with_no_opinion_keeps_its_place() {
        // The whole safety of the change: scoring must reproduce the
        // table's order exactly until a curve is written on purpose.
        let places: Vec<usize> = STEPS
            .iter()
            .enumerate()
            .filter(|(_, s)| s.layer == Layer::Goal)
            .map(|(i, _)| i)
            .collect();
        let base = |place: usize| (STEPS.len() - place) as f32 * BY_PLACE;
        for pair in places.windows(2) {
            assert!(
                base(pair[0]) > base(pair[1]),
                "{} should outrank {}",
                STEPS[pair[0]].name,
                STEPS[pair[1]].name
            );
        }
        // Every place is worth something, so a goal with no opinion is
        // always considered rather than skipped.
        assert!(places.iter().all(|&p| base(p) > 0.0));
        // And a thing worth a lot beats every place in the table.
        assert!(WORTH_A_LOT > base(0));
    }

    #[test]
    fn a_body_gets_more_worth_going_back_for_the_longer_it_waits() {
        let base = |name: &str| {
            let place = STEPS.iter().position(|s| s.name == name).expect(name);
            (STEPS.len() - place) as f32 * BY_PLACE
        };
        let fight = base("fight");
        // The arithmetic of the curve, without a world to hang it on.
        let worth = |left_secs: f32, waiting: usize| {
            let aged = 1.0 - left_secs / crate::autoplay::CORPSE_LIFE.as_secs_f32();
            LOOT_AT_REST
                + aged * AGE_IS_WORTH
                + waiting.min(WAITING_COUNTS) as f32 * EACH_BODY_IS_WORTH
        };

        // Just dropped, nothing else waiting: the fight that made it
        // comes first.
        assert!(worth(300.0, 1) < fight, "a fresh body waits its turn");
        // A third of its life gone: worth going back for before
        // starting another fight. This is the point of the curve -- a
        // character that only breaks off for a rotting corpse never
        // clears the pile behind it.
        assert!(worth(200.0, 1) > fight, "an ageing body comes first");
        // And bodies piling up says the same thing sooner.
        assert!(
            worth(280.0, 5) > worth(280.0, 1),
            "a floor full of bodies is worth more than one"
        );
        assert!(worth(280.0, 6) > fight, "a pile outranks the next fight");
        // It never runs away with itself: even a heap of old bodies
        // does not outrank a corpse that is actually about to go.
        assert!(worth(80.0, 6) < WORTH_A_LOT);
    }

    #[test]
    fn a_body_about_to_rot_outranks_the_next_fight() {
        // Written as a curve rather than as an exception buried in the
        // middle of choosing a corpse: a fight will still be there
        // afterwards and the body will not.
        let at = |name: &str| STEPS.iter().position(|s| s.name == name).expect(name);
        let base = |place: usize| (STEPS.len() - place) as f32 * BY_PLACE;
        assert!(
            base(at("loot")) < WORTH_A_LOT,
            "an urgent corpse must be able to outrank its own place"
        );
        assert!(WORTH_A_LOT > base(at("fight")), "and to outrank the fight");
        // While a body will keep, looting stays where it always was:
        // after the reflexes and before the shops.
        assert!(base(at("loot")) > base(at("grow")));
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
