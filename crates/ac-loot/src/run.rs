//! Emptying one corpse, one decision at a time.

use std::time::{Duration, Instant};

use ac_agent::did::{Because, Did, Patience};

use crate::corpse::{Open, Verdict, REACH};

/// How long an item asked for and not moved is left before asking
/// again. Not the pace of the looting -- that is set by the corpse
/// answering -- only how long a lost request is left.
pub const TAKE_AGAIN: Duration = Duration::from_millis(400);
/// How long to keep at one corpse before leaving it be. A corpse
/// belongs to whoever killed it for a while, so one that will not give
/// up its contents now may well later.
pub const KEEP_AT_IT: Duration = Duration::from_secs(45);

/// The one thing the rules want done next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// Walk to it: a corpse will not open across a room.
    Approach,
    /// Ask it to open.
    Open,
    /// Ask the server what these are. Several at once: it answers each
    /// on its own, and asking one at a time was most of what made
    /// standing over a body slow.
    Ask(Vec<u32>),
    /// Take this one.
    Take(u32),
    /// Shut it. A corpse left open is one the server still has the
    /// character standing over, and the next cannot be opened until it
    /// is let go.
    Close,
}

/// What came of a turn.
#[derive(Clone, Debug, PartialEq)]
pub struct Next {
    pub act: Option<Act>,
    pub did: Did,
    pub saying: String,
}

impl Next {
    fn act(act: Act, saying: impl Into<String>) -> Next {
        Next {
            act: Some(act),
            did: Did::Acting,
            saying: saying.into(),
        }
    }

    fn done(saying: impl Into<String>) -> Next {
        Next {
            act: Some(Act::Close),
            did: Did::Done,
            saying: saying.into(),
        }
    }

    fn wait(why: &str) -> Next {
        Next {
            act: None,
            did: Did::waiting(why.to_string()),
            saying: why.to_string(),
        }
    }
}

/// Emptying one corpse.
#[derive(Debug, Default)]
pub struct Run {
    /// The item last asked for, and when.
    asked: Option<(u32, Instant)>,
    /// Items that will not come out.
    stuck: Patience<u32>,
    /// When this corpse was first stood over.
    began: Option<Instant>,
    /// How many things have been taken.
    pub taken: u32,
}

impl Run {
    pub fn new() -> Run {
        Run::default()
    }

    /// The next thing to do with this corpse, or why there is nothing.
    pub fn step(&mut self, at: &Open, now: Instant) -> Next {
        let began = *self.began.get_or_insert(now);

        if at.away > REACH {
            return Next::act(Act::Approach, format!("going to {}", at.name));
        }
        if !at.open {
            return Next::act(Act::Open, format!("opening {}", at.name));
        }

        // Nothing can be carried: leave it rather than stand here
        // asking. The body keeps for a while and will still be here
        // when the pack is not full.
        if at.slots_free == 0 {
            return Next::done("pack full, leaving the loot");
        }
        if at.carry_room == 0 {
            return Next::done("carrying as much as it means to, leaving the loot");
        }

        // Long enough. A corpse that will not give up its contents is
        // set aside, not written off.
        if now.duration_since(began) > KEEP_AT_IT {
            return Next {
                act: Some(Act::Close),
                did: Did::Blocked(Because::ours("it will not give up its contents")),
                saying: format!("leaving {} for now", at.name),
            };
        }

        // What cannot be judged without asking. Asked for in one go.
        if at.may_ask {
            let ask: Vec<u32> = at.unjudged().map(|i| i.guid).collect();
            if !ask.is_empty() {
                let n = ask.len();
                return Next::act(Act::Ask(ask), format!("looking over {n} item(s)"));
            }
            // Still waiting to hear about some of them.
            if at
                .items
                .iter()
                .any(|i| i.verdict == Verdict::MustAsk && at.asking.contains(&i.guid))
            {
                return Next::wait("waiting to hear what they are");
            }
        }

        // Take them, in the order the corpse lists them.
        //
        // Paced by the corpse, not by a clock: a take is answered by
        // the item leaving, which is what takes it off this list, so
        // when the front has changed the last one is done and the next
        // goes at once.
        // The first thing worth taking that has not been set aside.
        //
        // Set aside is not the same as waiting, and conflating the two
        // is a bug with a name: the server moves one thing at a time
        // and answers a second with "Source item not found", so an item
        // that has been asked for and has not moved yet must be waited
        // on, never stepped over. Only what will not come at all --
        // heavier than the character can carry -- is skipped.
        let next = at
            .wanted()
            .find(|i| !self.stuck.held(&i.guid, now))
            .map(|i| (i.guid, i.name.clone(), i.burden));
        if let Some((guid, name, burden)) = next {
            if burden > at.carry_room {
                self.stuck.note(
                    guid,
                    &Did::Blocked(Because::ours("too heavy to carry")),
                    now,
                );
                return Next::wait("that one is heavier than it can carry");
            }
            match self.asked {
                // Asked for and still lying there: wait for it.
                Some((last, when)) if last == guid => {
                    if now.duration_since(when) < TAKE_AGAIN {
                        return Next::wait("it has not come out yet");
                    }
                }
                _ => self.taken += 1,
            }
            self.asked = Some((guid, now));
            return Next::act(Act::Take(guid), format!("taking {name}"));
        }

        Next::done(format!("emptied {}", at.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpse::Lying;

    fn thing(guid: u32, name: &str, verdict: Verdict) -> Lying {
        Lying {
            guid,
            name: name.into(),
            burden: 10,
            verdict,
        }
    }

    fn body(items: Vec<Lying>) -> Open {
        Open {
            guid: 900,
            name: "Corpse of a Drudge".into(),
            away: 0.0,
            open: true,
            items,
            slots_free: 20,
            carry_room: 10_000,
            may_ask: true,
            asking: Vec::new(),
        }
    }

    #[test]
    fn it_walks_to_the_body_and_opens_it_before_anything_else() {
        let mut run = Run::new();
        let mut at = body(vec![thing(1, "Dagger", Verdict::Take)]);
        at.away = 8.0;
        assert_eq!(run.step(&at, Instant::now()).act, Some(Act::Approach));
        at.away = 1.0;
        at.open = false;
        assert_eq!(run.step(&at, Instant::now()).act, Some(Act::Open));
    }

    #[test]
    fn only_what_cannot_be_judged_is_asked_about_and_all_at_once() {
        // One identify per item, waited on in turn, was most of the
        // time a character spent standing over a body. And a profile
        // that can settle an item on its name and worth should cost no
        // round trip at all.
        let mut run = Run::new();
        let at = body(vec![
            thing(1, "Dagger", Verdict::Take),
            thing(2, "Pyreal", Verdict::Take),
            thing(3, "Odd Ring", Verdict::MustAsk),
            thing(4, "Odd Gem", Verdict::MustAsk),
        ]);
        match run.step(&at, Instant::now()).act {
            Some(Act::Ask(ids)) => assert_eq!(ids, vec![3, 4], "asked about the wrong things"),
            other => panic!("expected one ask for both, got {other:?}"),
        }
    }

    #[test]
    fn a_corpse_it_can_judge_alone_is_never_asked_about() {
        let mut run = Run::new();
        let at = body(vec![
            thing(1, "Dagger", Verdict::Take),
            thing(2, "Rock", Verdict::Leave),
        ]);
        assert_eq!(run.step(&at, Instant::now()).act, Some(Act::Take(1)));
    }

    #[test]
    fn the_next_thing_goes_out_as_soon_as_the_last_one_leaves() {
        // The corpse answers by the item leaving, which is what takes
        // it off the list. Waiting four hundred milliseconds between
        // every item meant ten things took four seconds.
        let now = Instant::now();
        let mut run = Run::new();
        let at = body(vec![
            thing(1, "Dagger", Verdict::Take),
            thing(2, "Shield", Verdict::Take),
        ]);
        assert_eq!(run.step(&at, now).act, Some(Act::Take(1)));
        // It came out: the same instant, the next one goes.
        let at = body(vec![thing(2, "Shield", Verdict::Take)]);
        assert_eq!(run.step(&at, now).act, Some(Act::Take(2)));
    }

    #[test]
    fn one_that_does_not_come_out_is_asked_again_then_left() {
        let now = Instant::now();
        let mut run = Run::new();
        let at = body(vec![
            thing(1, "Stuck Thing", Verdict::Take),
            thing(2, "Dagger", Verdict::Take),
        ]);
        assert_eq!(run.step(&at, now).act, Some(Act::Take(1)));
        // Still there a moment later: not asked again immediately.
        assert_eq!(run.step(&at, now).act, None);
        // After the wait, asked again.
        let later = now + TAKE_AGAIN + Duration::from_millis(1);
        assert_eq!(run.step(&at, later).act, Some(Act::Take(1)));
    }

    #[test]
    fn a_full_pack_closes_the_corpse_rather_than_standing_over_it() {
        // This one was live: the give-up path dropped the corpse
        // without telling the server, so it stayed open and the next
        // could not be opened.
        let mut run = Run::new();
        let mut at = body(vec![thing(1, "Dagger", Verdict::Take)]);
        at.slots_free = 0;
        let next = run.step(&at, Instant::now());
        assert_eq!(next.act, Some(Act::Close), "{}", next.saying);
    }

    #[test]
    fn too_laden_also_closes_it() {
        let mut run = Run::new();
        let mut at = body(vec![thing(1, "Dagger", Verdict::Take)]);
        at.carry_room = 0;
        assert_eq!(run.step(&at, Instant::now()).act, Some(Act::Close));
    }

    #[test]
    fn something_heavier_than_it_can_carry_is_left_and_the_rest_taken() {
        let now = Instant::now();
        let mut run = Run::new();
        let mut heavy = thing(1, "Anvil", Verdict::Take);
        heavy.burden = 9_000;
        let at = body(vec![heavy, thing(2, "Dagger", Verdict::Take)]);
        let mut at = at;
        at.carry_room = 100;
        // The anvil is refused, and the dagger still goes in the pack.
        assert_eq!(run.step(&at, now).act, None);
        assert_eq!(run.step(&at, now).act, Some(Act::Take(2)));
    }

    #[test]
    fn an_emptied_corpse_is_shut_behind_it() {
        let mut run = Run::new();
        let at = body(vec![thing(1, "Rock", Verdict::Leave)]);
        let next = run.step(&at, Instant::now());
        assert_eq!(next.act, Some(Act::Close));
        assert_eq!(next.did, Did::Done);
    }

    #[test]
    fn a_corpse_that_will_not_empty_is_set_aside_not_written_off() {
        // A body belongs to whoever killed it for a while, so one that
        // will not give up its contents now may well later. Blocked,
        // never Refused.
        let now = Instant::now();
        let mut run = Run::new();
        let at = body(vec![thing(1, "Stuck Thing", Verdict::Take)]);
        run.step(&at, now);
        let next = run.step(&at, now + KEEP_AT_IT + Duration::from_secs(1));
        assert_eq!(next.act, Some(Act::Close));
        assert!(
            matches!(next.did, Did::Blocked(_)),
            "written off for good: {:?}",
            next.did
        );
    }

    #[test]
    fn never_two_takes_in_the_air_at_once() {
        // The invariant with a name. The server moves one thing at a
        // time and answers a second with "Source item not found"; a
        // version that stepped over the item it was waiting on earned
        // thirty-two of those in one watched run.
        let now = Instant::now();
        let mut run = Run::new();
        let at = body(vec![
            thing(1, "First", Verdict::Take),
            thing(2, "Second", Verdict::Take),
            thing(3, "Third", Verdict::Take),
        ]);
        assert_eq!(run.step(&at, now).act, Some(Act::Take(1)));
        // Nothing has left the corpse, so nothing else is asked for,
        // however many times the rules are consulted.
        for _ in 0..10 {
            assert_eq!(run.step(&at, now).act, None, "asked for a second item");
        }
        // Only when the first is gone does the next go out.
        let at = body(vec![
            thing(2, "Second", Verdict::Take),
            thing(3, "Third", Verdict::Take),
        ]);
        assert_eq!(run.step(&at, now).act, Some(Act::Take(2)));
    }
}
