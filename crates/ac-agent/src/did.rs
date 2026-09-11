//! What came of trying something, and what to do about it.
//!
//! Nearly everything a character attempts can be refused. The counter
//! will not take the item, the corpse will not open, the doorway will
//! not admit it, the purse is empty, the pack is full, the quest is on
//! cooldown until tomorrow. Deciding what to do when told no is most of
//! the work.
//!
//! An action used to answer that question with `bool`: true meant it
//! did something, false meant it did not, this tick, for a reason the
//! caller could not see and therefore could not act on. So *asking
//! again immediately, for ever* was not a bug anybody wrote -- it was
//! the default that fell out of the type, and every fix was another
//! flag bolted on beside one call site. There came to be a dozen of
//! them, each encoding the same thing slightly differently.
//!
//! [`Did`] says what happened instead, and [`Patience`] is the one
//! place that decides how long to wait before asking again. See
//! `docs/agent.md`.

use std::fmt;
use std::time::{Duration, Instant};

/// Why something did not happen.
///
/// A short phrase meant to be read by a person -- in the log, in the
/// autoplay panel -- and, when the refusal came from the server, the
/// weenie error behind it, so that the retry policy can tell a
/// cooldown from a full pack without matching on English.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Because {
    /// In a few words: "the pack is full", "no vendor will take it".
    pub what: String,
    /// The server's own code, when it gave one (see
    /// [`crate::weenie_errors`]).
    pub code: Option<u32>,
}

impl Because {
    /// A reason of our own.
    pub fn ours(what: impl Into<String>) -> Because {
        Because {
            what: what.into(),
            code: None,
        }
    }

    /// A reason the server gave, named by its own error code.
    pub fn server(code: u32) -> Because {
        Because {
            what: crate::weenie_errors::text(code)
                .map(str::to_string)
                .unwrap_or_else(|| format!("the server said no ({code:#06x})")),
            code: Some(code),
        }
    }

    /// The server's code with a reason of our own for context.
    pub fn server_because(code: u32, what: impl Into<String>) -> Because {
        Because {
            what: what.into(),
            code: Some(code),
        }
    }
}

impl fmt::Display for Because {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.what)
    }
}

/// What came of trying something.
///
/// The three unhappy answers differ only in how long the wait should
/// be, and that is the whole point of telling them apart: it is the one
/// question every one of the old flags was answering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Did {
    /// It claimed the tick and is working. Nothing else runs.
    Acting,
    /// It has nothing to do, or has finished. Let something else have
    /// the tick.
    Done,
    /// It cannot go on yet, for a reason that passes by itself: a cast
    /// in progress, a walk not finished, a server that has not answered.
    /// Asking again shortly is right and costs nothing.
    Waiting(Because),
    /// It cannot go on, and asking again soon will not help: the pack is
    /// full, the purse is empty, the character is too laden. Something
    /// has to change first, so back off and let it.
    Blocked(Because),
    /// It will never work, for this thing, from now on: an item no
    /// vendor takes, a component nobody sells, a door that needs a quest
    /// this character has not done. Do not ask again.
    Refused(Because),
}

impl Did {
    /// It claimed the tick. The caller stops here.
    pub fn acting(&self) -> bool {
        matches!(self, Did::Acting)
    }

    /// Nothing went wrong; there was simply nothing to do.
    pub fn fine(&self) -> bool {
        matches!(self, Did::Acting | Did::Done)
    }

    /// Why not, when something did go wrong.
    pub fn because(&self) -> Option<&Because> {
        match self {
            Did::Acting | Did::Done => None,
            Did::Waiting(b) | Did::Blocked(b) | Did::Refused(b) => Some(b),
        }
    }

    /// Shorthand for the common unhappy answers.
    pub fn waiting(what: impl Into<String>) -> Did {
        Did::Waiting(Because::ours(what))
    }

    pub fn blocked(what: impl Into<String>) -> Did {
        Did::Blocked(Because::ours(what))
    }

    pub fn refused(what: impl Into<String>) -> Did {
        Did::Refused(Because::ours(what))
    }

    /// How long to leave it before trying this again, the first time.
    /// `None` means never.
    pub fn wait(&self) -> Option<Duration> {
        match self {
            Did::Acting | Did::Done => Some(Duration::ZERO),
            Did::Waiting(_) => Some(WAIT_AGAIN),
            Did::Blocked(_) => Some(BLOCKED_AGAIN),
            Did::Refused(_) => None,
        }
    }
}

/// How long a `Waiting` is left before asking again: a handful of
/// ticks, because the thing it waits on is usually the server
/// answering.
const WAIT_AGAIN: Duration = Duration::from_millis(250);
/// How long a `Blocked` is left the first time. Something has to change
/// before it can go on, and nothing changes in a quarter of a second.
const BLOCKED_AGAIN: Duration = Duration::from_secs(30);
/// The longest a doubling wait ever grows to. Four hours is short
/// against a daily cooldown on purpose: an ask that fails costs one
/// message, and missing a thing that came back hours ago costs the
/// thing. A session can run for days, so nothing is given up for good
/// except by [`Did::Refused`].
const AT_MOST: Duration = Duration::from_secs(4 * 60 * 60);

/// How long to wait before trying one particular thing again.
///
/// One of these stands in for a whole family of hand-written flags:
/// how many times an item has been asked for, when a vendor last
/// refused something, which corpse would not open and when. Each
/// refusal doubles the wait, to a ceiling; anything going well clears
/// it.
///
/// The waits are deliberately biased short. Asking again and being told
/// no costs one message; not asking when the answer had changed costs
/// whatever was being asked for.
#[derive(Clone, Debug, Default)]
pub struct Patience<K: Ord + Clone> {
    held: std::collections::BTreeMap<K, Held>,
}

#[derive(Clone, Copy, Debug)]
struct Held {
    /// When it was last refused.
    since: Instant,
    /// How long to leave it from then.
    wait: Duration,
    /// Refused for good.
    never: bool,
}

impl<K: Ord + Clone> Patience<K> {
    pub fn new() -> Self {
        Patience {
            held: std::collections::BTreeMap::new(),
        }
    }

    /// Whether this one is still inside its wait. A thing refused for
    /// good is always held.
    pub fn held(&self, key: &K, now: Instant) -> bool {
        self.held.get(key).is_some_and(|h| {
            h.never
                || now
                    .checked_duration_since(h.since)
                    .is_none_or(|d| d < h.wait)
        })
    }

    /// Remember what came of trying it. Going well forgets any wait;
    /// being told no sets or doubles one.
    pub fn note(&mut self, key: K, did: &Did, now: Instant) {
        match did {
            Did::Acting | Did::Done => {
                self.held.remove(&key);
            }
            Did::Refused(_) => {
                self.held.insert(
                    key,
                    Held {
                        since: now,
                        wait: AT_MOST,
                        never: true,
                    },
                );
            }
            Did::Waiting(_) | Did::Blocked(_) => {
                let first = did.wait().unwrap_or(BLOCKED_AGAIN);
                let held = self.held.entry(key).or_insert(Held {
                    since: now,
                    wait: first,
                    never: false,
                });
                // The first refusal sets the wait; every one after
                // doubles it, so a thing that keeps saying no is asked
                // less and less often without ever being abandoned.
                if held.since != now {
                    held.wait = (held.wait * 2).min(AT_MOST);
                }
                held.since = now;
            }
        }
    }

    /// How long the wait on this one has grown to, if it is being held
    /// at all.
    ///
    /// A caller that must keep asking for things in order uses this to
    /// tell "not yet" from "not coming": a wait that has doubled a few
    /// times is a thing that has been asked for a few times and has
    /// not budged.
    pub fn waited(&self, key: &K) -> Option<Duration> {
        self.held.get(key).map(|h| h.wait)
    }

    /// Hold one off for a wait the caller chooses, rather than the
    /// policy's own.
    ///
    /// For the few places that know something the policy does not: a
    /// trip to town that came to nothing is worth minutes, not the
    /// half minute a full pack is worth, because nothing about a town
    /// changes in half a minute. It doubles from there like any other.
    pub fn hold(&mut self, key: K, first: Duration, now: Instant) {
        match self.held.get_mut(&key) {
            Some(held) if !held.never => {
                held.wait = (held.wait * 2).min(AT_MOST);
                held.since = now;
            }
            Some(_) => {}
            None => {
                self.held.insert(
                    key,
                    Held {
                        since: now,
                        wait: first.min(AT_MOST),
                        never: false,
                    },
                );
            }
        }
    }

    /// Forget one, whatever was remembered about it: the thing it was
    /// waiting on has changed.
    pub fn forget(&mut self, key: &K) {
        self.held.remove(key);
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        self.held.clear();
    }

    /// Drop what is no longer worth remembering: waits that have run
    /// out and are not permanent. Keeps the table the size of what is
    /// actually being held off.
    pub fn tidy(&mut self, now: Instant) {
        self.held.retain(|_, h| {
            h.never
                || now
                    .checked_duration_since(h.since)
                    .is_none_or(|d| d < h.wait)
        });
    }

    /// How many are being held off, for the log.
    pub fn len(&self) -> usize {
        self.held.len()
    }

    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_says_how_long_to_wait() {
        assert!(Did::Acting.acting());
        assert!(Did::Acting.fine() && Did::Done.fine());
        assert!(!Did::waiting("the server has not answered").fine());
        assert_eq!(Did::Done.wait(), Some(Duration::ZERO));
        assert_eq!(Did::waiting("x").wait(), Some(WAIT_AGAIN));
        assert_eq!(Did::blocked("x").wait(), Some(BLOCKED_AGAIN));
        // The one answer that means never.
        assert_eq!(Did::refused("no vendor will take it").wait(), None);
        assert_eq!(
            Did::blocked("the pack is full")
                .because()
                .map(|b| b.what.as_str()),
            Some("the pack is full")
        );
        assert_eq!(Did::Done.because(), None);
    }

    #[test]
    fn a_reason_from_the_server_keeps_its_code() {
        // YouHaveSolvedThisQuestTooRecently: the retry policy can tell
        // a cooldown from a full pack without reading English.
        let b = Because::server(0x043E);
        assert_eq!(b.code, Some(0x043E));
        assert_eq!(b.what, "You have solved this quest too recently");
        // One nobody has a name for still says something.
        let odd = Because::server(0xDEAD);
        assert_eq!(odd.code, Some(0xDEAD));
        assert!(odd.what.contains("0xdead"), "{}", odd.what);
        assert_eq!(Because::ours("the pack is full").code, None);
    }

    #[test]
    fn a_refusal_doubles_the_wait_and_going_well_forgets_it() {
        let t0 = Instant::now();
        let mut p: Patience<u32> = Patience::new();
        assert!(!p.held(&1, t0));

        // Blocked: held for the first wait, free again after it.
        p.note(1, &Did::blocked("the pack is full"), t0);
        assert!(p.held(&1, t0));
        assert!(p.held(&1, t0 + BLOCKED_AGAIN - Duration::from_secs(1)));
        assert!(!p.held(&1, t0 + BLOCKED_AGAIN));

        // Refused again: the wait doubles.
        let t1 = t0 + BLOCKED_AGAIN;
        p.note(1, &Did::blocked("the pack is full"), t1);
        assert!(p.held(&1, t1 + BLOCKED_AGAIN));
        assert!(!p.held(&1, t1 + BLOCKED_AGAIN * 2));

        // It goes through: everything remembered about it is forgotten.
        p.note(1, &Did::Done, t1 + BLOCKED_AGAIN * 2);
        assert!(!p.held(&1, t1 + BLOCKED_AGAIN * 2));
        assert!(p.is_empty());
    }

    #[test]
    fn a_caller_may_choose_its_own_first_wait() {
        let t0 = Instant::now();
        let mut p: Patience<u32> = Patience::new();
        let five = Duration::from_secs(5 * 60);
        // A trip to town that came to nothing is worth minutes, not
        // the half minute a full pack is worth.
        p.hold(1, five, t0);
        assert!(p.held(&1, t0 + five - Duration::from_secs(1)));
        assert!(!p.held(&1, t0 + five));
        // And it doubles from there like anything else.
        p.hold(1, five, t0 + five);
        assert!(p.held(&1, t0 + five * 2));
        assert!(!p.held(&1, t0 + five * 3));
        // A chosen wait never beats the ceiling either.
        let mut q: Patience<u32> = Patience::new();
        q.hold(1, Duration::from_secs(365 * 24 * 60 * 60), t0);
        assert_eq!(q.waited(&1), Some(AT_MOST));
        // And it does not disturb something already given up on.
        q.note(2, &Did::refused("never"), t0);
        q.hold(2, five, t0);
        assert!(q.held(&2, t0 + Duration::from_secs(24 * 60 * 60)));
    }

    #[test]
    fn only_a_refusal_is_for_ever() {
        let t0 = Instant::now();
        let mut p: Patience<u32> = Patience::new();
        p.note(7, &Did::refused("no vendor will take it"), t0);
        assert!(p.held(&7, t0));
        // Not even a day lifts it.
        assert!(p.held(&7, t0 + Duration::from_secs(24 * 60 * 60)));
        // A wait, however long, does lift.
        p.note(8, &Did::blocked("too heavy"), t0);
        assert!(!p.held(&8, t0 + AT_MOST));
        // Tidying keeps the permanent one and drops the lapsed one.
        p.tidy(t0 + AT_MOST);
        assert_eq!(p.len(), 1);
        assert!(p.held(&7, t0 + AT_MOST));
        // And a thing can be forgiven on purpose, when what it was
        // waiting on has changed.
        p.forget(&7);
        assert!(p.is_empty());
    }

    #[test]
    fn a_wait_never_grows_past_the_ceiling() {
        let t0 = Instant::now();
        let mut p: Patience<u32> = Patience::new();
        let mut at = t0;
        for _ in 0..20 {
            p.note(1, &Did::blocked("still no"), at);
            at += AT_MOST;
        }
        // Twenty refusals later it is still asked about every four
        // hours, not once a fortnight.
        p.note(1, &Did::blocked("still no"), at);
        assert!(!p.held(&1, at + AT_MOST));
        assert!(AT_MOST < Duration::from_secs(24 * 60 * 60));
    }
}
