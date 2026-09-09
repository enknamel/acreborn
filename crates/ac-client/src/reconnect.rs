//! Getting back in after a drop.
//!
//! A session can end three ways. The player quits, which is a clean
//! [`Client::disconnect`] and the end of it. The server refuses us for a
//! reason waiting will not fix (banned, wrong password, a character that
//! is not ours), which is also the end of it. Or the connection dies:
//! the router hiccups, the server restarts, a packet storm eats the
//! session. That last one is worth recovering from, because leaving the
//! character standing in the world until the server's own timeout is
//! worse than being logged out, and because everything since the last
//! server-side save is rolled back when the character finally does drop.
//!
//! [`Reconnect`] is the rule for the third case: a state machine fed the
//! ending and the clock, answering with [`Action::Connect`] when it is
//! time to try again. It touches no client state and opens no sockets,
//! so the schedule, the attempt cap and the cooldown are tested without
//! a server. The caller (the viewer's session loop) does the connecting
//! and hands back what happened.
//!
//! The one subtlety is the login cooldown. A session that ends without a
//! clean disconnect leaves the account logged in server-side for a
//! minute or so; a retry inside that window is refused with
//! `CharacterError` 1 (`Logon`) rather than let in. Those refusals are
//! not the client's fault and do not spend an attempt: the machine waits
//! out [`Policy::cooldown`] and tries again, up to
//! [`Policy::cooldown_waits`] times.

use std::time::{Duration, Instant};

/// The opcodes a refusal arrives on (see `ac_net::messages::opcode`).
const CHARACTER_ERROR: u32 = 0xF659;
const ACCOUNT_BOOT: u32 = 0xF7DC;
const ACCOUNT_BANNED: u32 = 0xF7DD;

/// Why a session ended, and so whether to come back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    /// The player meant it: the menu's Quit, the fleet panel's stop, a
    /// headless run's `--fleet-stop-after`. Never reconnected.
    Quit,
    /// The connection died or the server dropped us. Reconnect on the
    /// backoff schedule.
    Dropped(String),
    /// The server refused the login because the account is still logged
    /// in from the session that just ended. Wait out the cooldown and
    /// try again, without spending an attempt.
    StillLoggedIn,
    /// The server refused us for a reason waiting will not fix. Give up
    /// and say so.
    Fatal(String),
}

impl Ending {
    /// A line for the chat log describing what happened.
    pub fn describe(&self) -> String {
        match self {
            Ending::Quit => "Disconnected".into(),
            Ending::Dropped(why) => format!("Connection lost ({why})"),
            Ending::StillLoggedIn => "The server still has the account logged in".into(),
            Ending::Fatal(why) => format!("The server refused the login: {why}"),
        }
    }
}

/// What an `ac_client::Event::Refused` code means for coming back.
///
/// `opcode` is the message it arrived on and `code` the ACE
/// `CharacterError` in its body (see `docs/game/mechanics.md`). Codes
/// that mean "the character you asked for is still in the world" are
/// [`Ending::StillLoggedIn`]; codes that mean "the world is down or
/// busy" are an ordinary drop, worth retrying on the backoff; the rest
/// are fatal, since retrying a bad password or a ban only makes it
/// worse.
pub fn classify_refusal(opcode: u32, code: u32) -> Ending {
    match opcode {
        ACCOUNT_BOOT => return Ending::Fatal("booted by a GM".into()),
        ACCOUNT_BANNED => return Ending::Fatal("the account is banned".into()),
        _ => {}
    }
    if opcode != CHARACTER_ERROR {
        return Ending::Dropped(format!("refused ({opcode:#06x}/{code:#x})"));
    }
    match code {
        // Logon, Logoff, CharacterInWorld, CharacterInWorldServer: the
        // character we asked for has not left the world yet.
        0x1 | 0x5 | 0xD | 0x10 => Ending::StillLoggedIn,
        // ServerCrash, EnterGameGeneric, OldCharacter, StartServerDown,
        // CouldntPlaceCharacter, LogonServerFull: the world is down,
        // restarting or full. Worth trying again later.
        0x4 | 0x8 | 0xB | 0x11 | 0x13 | 0x14 | 0x15 => {
            Ending::Dropped(format!("the world refused us (code {code:#x})"))
        }
        0x3 => Ending::Fatal("the account or password was not accepted".into()),
        0x9 | 0xA | 0xE => Ending::Fatal("no such account".into()),
        0xF => Ending::Fatal("the character is not on this account".into()),
        0x12 => Ending::Fatal("the character is corrupt".into()),
        0x17 => Ending::Fatal("the character is locked".into()),
        0x18 => Ending::Fatal("the subscription has expired".into()),
        _ => Ending::Fatal(format!("code {code:#x}")),
    }
}

/// Why a session that has ended ended, from what the client saw.
///
/// `quitting` is set by `Client::disconnect`, so a clean exit is never
/// reconnected whatever else happened. `refusal` is the last
/// `CharacterError` / `AccountBoot` / `AccountBanned` seen, and `why`
/// the reason on `Event::Terminated`. A refusal outranks the reason: a
/// session refused at login is also terminated, and the refusal says
/// far more about what to do next.
pub fn classify(quitting: bool, refusal: Option<(u32, u32)>, why: Option<&str>) -> Ending {
    if quitting {
        return Ending::Quit;
    }
    if let Some((op, code)) = refusal {
        return classify_refusal(op, code);
    }
    Ending::Dropped(why.unwrap_or("the connection ended").to_string())
}

/// How hard and how often to try.
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    /// Wait before the first attempt.
    pub first_delay: Duration,
    /// Each further wait is the last one times this...
    pub factor: f32,
    /// ...capped here.
    pub max_delay: Duration,
    /// Attempts before giving up. 0 turns reconnection off.
    pub tries: u32,
    /// How long the server keeps the account logged in after a session
    /// ends badly. A `StillLoggedIn` refusal waits this long.
    pub cooldown: Duration,
    /// How many times in a row a `StillLoggedIn` refusal may be waited
    /// out before it counts as a failure like any other.
    pub cooldown_waits: u32,
    /// An attempt that has neither placed the character nor failed
    /// within this long counts as failed.
    pub attempt_timeout: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            first_delay: Duration::from_secs(3),
            factor: 2.0,
            max_delay: Duration::from_secs(60),
            tries: 6,
            // The project's notes put the server-side logout at 60-75 s;
            // wait the long end of that plus a little.
            cooldown: Duration::from_secs(80),
            cooldown_waits: 4,
            attempt_timeout: Duration::from_secs(45),
        }
    }
}

impl Policy {
    /// How long to wait before attempt `n` (1 for the first).
    pub fn delay(&self, n: u32) -> Duration {
        if n <= 1 {
            return self.first_delay.min(self.max_delay);
        }
        let mut d = self.first_delay.as_secs_f32();
        for _ in 1..n {
            d *= self.factor.max(1.0);
            if d >= self.max_delay.as_secs_f32() {
                return self.max_delay;
            }
        }
        Duration::from_secs_f32(d).min(self.max_delay)
    }
}

/// Why the machine stopped trying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stopped {
    /// The player quit.
    Quit,
    /// Out of attempts.
    Exhausted,
    /// The server said something waiting will not fix.
    Fatal(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// Connected, or at least not known to be dropped.
    Live,
    /// Nothing to do until `at`, when attempt `attempt` goes out.
    Waiting {
        at: Instant,
        attempt: u32,
    },
    /// Attempt `attempt` is in flight; it started at `since`.
    Trying {
        attempt: u32,
        since: Instant,
    },
    Stopped(Stopped),
}

/// What the caller should do this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Nothing.
    Idle,
    /// Open a fresh connection for this session now, then call
    /// [`Reconnect::placed`] when the character is in the world or
    /// [`Reconnect::ended`] when the attempt fails.
    Connect,
}

/// The rule: when to try again after a session ends.
#[derive(Debug, Clone)]
pub struct Reconnect {
    pub policy: Policy,
    state: State,
    /// Consecutive cooldown waits since the last real attempt.
    waits: u32,
    /// Lines for the chat log, taken by [`Reconnect::take_notice`].
    notices: Vec<String>,
}

impl Default for Reconnect {
    fn default() -> Self {
        Reconnect::new(Policy::default())
    }
}

impl Reconnect {
    pub fn new(policy: Policy) -> Self {
        Reconnect {
            policy,
            state: State::Live,
            waits: 0,
            notices: Vec::new(),
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// Coming back is under way (or about to be).
    pub fn busy(&self) -> bool {
        matches!(self.state, State::Waiting { .. } | State::Trying { .. })
    }

    /// It has given up, and why.
    pub fn stopped(&self) -> Option<&Stopped> {
        match &self.state {
            State::Stopped(s) => Some(s),
            _ => None,
        }
    }

    /// When the next attempt goes out, while waiting for one.
    pub fn next_attempt(&self) -> Option<Instant> {
        match self.state {
            State::Waiting { at, .. } => Some(at),
            _ => None,
        }
    }

    /// A line for the chat log, if there is one.
    pub fn take_notice(&mut self) -> Option<String> {
        if self.notices.is_empty() {
            None
        } else {
            Some(self.notices.remove(0))
        }
    }

    /// The session ended (or an attempt at it failed). Schedules the
    /// next attempt, or stops.
    pub fn ended(&mut self, ending: Ending, now: Instant) {
        // A session that already gave up stays given up until it is
        // reset; a live one that ends starts at attempt 1.
        let attempt = match self.state {
            State::Trying { attempt, .. } => attempt,
            State::Waiting { attempt, .. } => attempt.saturating_sub(1),
            State::Live => 0,
            State::Stopped(_) => return,
        };
        match ending {
            Ending::Quit => {
                self.state = State::Stopped(Stopped::Quit);
            }
            Ending::Fatal(why) => {
                self.notices
                    .push(format!("Cannot reconnect: {why}. Log in again by hand."));
                self.state = State::Stopped(Stopped::Fatal(why));
            }
            Ending::StillLoggedIn => {
                // Not our fault and not a failure: the account has not
                // finished logging out. Wait it out at the same attempt
                // number, so a slow logout cannot burn the budget.
                if self.waits >= self.policy.cooldown_waits {
                    self.fail(attempt, now, "the account never finished logging out");
                    return;
                }
                self.waits += 1;
                let at = now + self.policy.cooldown;
                self.notices.push(format!(
                    "The server still has the account logged in; trying again in {} s",
                    self.policy.cooldown.as_secs()
                ));
                self.state = State::Waiting {
                    at,
                    attempt: attempt.max(1),
                };
            }
            Ending::Dropped(why) => {
                self.waits = 0;
                if self.policy.tries == 0 {
                    // Reconnection is off; the session stays down.
                    self.state = State::Stopped(Stopped::Exhausted);
                    return;
                }
                let next = attempt + 1;
                if next > self.policy.tries {
                    self.notices.push(format!(
                        "Could not reconnect after {} tries. Log in again by hand.",
                        self.policy.tries
                    ));
                    self.state = State::Stopped(Stopped::Exhausted);
                    return;
                }
                let delay = self.policy.delay(next);
                self.notices.push(if attempt == 0 {
                    format!(
                        "Connection lost ({why}); reconnecting in {} s",
                        delay.as_secs()
                    )
                } else {
                    format!(
                        "Reconnect {} of {} failed; trying again in {} s",
                        attempt,
                        self.policy.tries,
                        delay.as_secs()
                    )
                });
                self.state = State::Waiting {
                    at: now + delay,
                    attempt: next,
                };
            }
        }
    }

    /// Out of patience with the cooldown: treat it as an ordinary failure.
    fn fail(&mut self, attempt: u32, now: Instant, why: &str) {
        self.waits = 0;
        self.state = State::Trying {
            attempt,
            since: now,
        };
        self.ended(Ending::Dropped(why.into()), now);
    }

    /// Time passed. Answers [`Action::Connect`] once per attempt, when
    /// its wait is up; an attempt that hangs is failed here too.
    pub fn poll(&mut self, now: Instant) -> Action {
        match self.state {
            State::Waiting { at, attempt } if now >= at => {
                self.notices.push(format!(
                    "Reconnecting ({} of {})...",
                    attempt, self.policy.tries
                ));
                self.state = State::Trying {
                    attempt,
                    since: now,
                };
                Action::Connect
            }
            State::Trying { since, .. }
                if now.duration_since(since) >= self.policy.attempt_timeout =>
            {
                self.ended(Ending::Dropped("no answer from the server".into()), now);
                Action::Idle
            }
            _ => Action::Idle,
        }
    }

    /// The character is in the world again: the attempt worked.
    pub fn placed(&mut self) {
        if self.busy() {
            self.notices.push("Reconnected.".into());
        }
        self.waits = 0;
        self.state = State::Live;
    }

    /// Forget everything and start listening for drops again (a session
    /// the player logged back in by hand, say).
    pub fn reset(&mut self) {
        self.waits = 0;
        self.notices.clear();
        self.state = State::Live;
    }
}

/// The bits of a dropped session worth carrying into the one that
/// replaces it.
///
/// A reconnect is a brand new [`Client`](crate::Client): a new socket, a
/// new handshake, and a world the server describes again from scratch.
/// Almost none of the old client should survive that, because almost all
/// of it describes a world that is about to be replaced. What should
/// survive is what the *player* set rather than what the server said:
/// the autoplay rules, the movement tweaks, and which character to be.
///
/// What deliberately does not survive: the world and its objects, the
/// character sheet, the physics body, combat mode, the current target,
/// selections and appraisals, the loot and appraise queues, the planned
/// route, and the walk back to a corpse. Every one of those is either
/// resent by the server or names an object by a guid that means nothing
/// until the server sends it again.
#[derive(Debug, Clone)]
pub struct Carry {
    /// The character to come back as, so the session lands where it was
    /// rather than at the character-select screen.
    pub character: Option<String>,
    /// Everything the character does on its own.
    pub autoplay: crate::autoplay::Config,
    pub speed_boost: f32,
    pub jump_height: f32,
    pub require_components: bool,
}

impl Carry {
    /// Read what is worth keeping off a session that has ended.
    ///
    /// The character is the one actually in the world (the server told
    /// us its name), falling back to the one the config asked for. A
    /// session dropped at the character-select screen has neither, and
    /// comes back to the select screen.
    pub fn of(client: &crate::Client) -> Self {
        let in_world = client.world.stats.name.trim();
        let character = if in_world.is_empty() {
            client.config.character.clone()
        } else {
            Some(in_world.to_string())
        };
        Carry {
            character,
            autoplay: client.autoplay.config.clone(),
            speed_boost: client.speed_boost,
            jump_height: client.jump_height,
            require_components: client.require_components,
        }
    }

    /// Put it back onto the fresh session.
    pub fn apply(self, client: &mut crate::Client) {
        client.autoplay.config = self.autoplay;
        client.speed_boost = self.speed_boost;
        client.jump_height = self.jump_height;
        client.require_components = self.require_components;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn a_clean_quit_is_never_reconnected() {
        assert_eq!(classify(true, None, Some("disconnect")), Ending::Quit);
        // Even a refusal on the way out stays a quit.
        assert_eq!(classify(true, Some((0xF659, 0x1)), None), Ending::Quit);
        let mut r = Reconnect::default();
        let now = Instant::now();
        r.ended(Ending::Quit, now);
        assert_eq!(r.stopped(), Some(&Stopped::Quit));
        assert_eq!(r.poll(now + secs(600)), Action::Idle);
    }

    #[test]
    fn a_silent_drop_is_a_drop() {
        let e = classify(false, None, Some("net error 0x1/0x2"));
        assert!(matches!(e, Ending::Dropped(_)));
        let e = classify(false, None, None);
        assert!(matches!(e, Ending::Dropped(_)));
    }

    #[test]
    fn a_refusal_outranks_the_reason_it_terminated_with() {
        // The net session raises both `Terminated` (with the error's
        // text) and `Refused` for a CharacterError. The code is what
        // says whether coming back is worth trying, so it wins.
        assert_eq!(
            classify(
                false,
                Some((0xF659, 0x1)),
                Some("this character is already in the world (character error 0x1)"),
            ),
            Ending::StillLoggedIn
        );
        assert!(matches!(
            classify(false, Some((0xF659, 0xF)), Some("terminated")),
            Ending::Fatal(_)
        ));
    }

    #[test]
    fn still_logged_in_codes_are_the_cooldown() {
        for code in [0x1, 0x5, 0xD, 0x10] {
            assert_eq!(
                classify_refusal(0xF659, code),
                Ending::StillLoggedIn,
                "code {code:#x}"
            );
        }
    }

    #[test]
    fn world_down_codes_are_retried_and_the_rest_are_fatal() {
        for code in [0x4, 0x8, 0xB, 0x11, 0x13, 0x14, 0x15] {
            assert!(
                matches!(classify_refusal(0xF659, code), Ending::Dropped(_)),
                "code {code:#x} should be retried"
            );
        }
        for code in [0x3, 0x9, 0xA, 0xE, 0xF, 0x12, 0x17, 0x18] {
            assert!(
                matches!(classify_refusal(0xF659, code), Ending::Fatal(_)),
                "code {code:#x} should be fatal"
            );
        }
        assert!(matches!(classify_refusal(0xF7DC, 0), Ending::Fatal(_)));
        assert!(matches!(classify_refusal(0xF7DD, 0), Ending::Fatal(_)));
    }

    #[test]
    fn the_backoff_grows_and_is_capped() {
        let p = Policy {
            first_delay: secs(3),
            factor: 2.0,
            max_delay: secs(60),
            ..Policy::default()
        };
        assert_eq!(p.delay(1), secs(3));
        assert_eq!(p.delay(2), secs(6));
        assert_eq!(p.delay(3), secs(12));
        assert_eq!(p.delay(4), secs(24));
        assert_eq!(p.delay(5), secs(48));
        assert_eq!(p.delay(6), secs(60));
        assert_eq!(p.delay(20), secs(60));
    }

    #[test]
    fn a_drop_waits_then_asks_to_connect() {
        let mut r = Reconnect::default();
        let t0 = Instant::now();
        r.ended(Ending::Dropped("net error".into()), t0);
        assert_eq!(r.next_attempt(), Some(t0 + secs(3)));
        assert_eq!(r.poll(t0 + secs(1)), Action::Idle);
        assert_eq!(r.poll(t0 + secs(3)), Action::Connect);
        // Only once: the attempt is in flight now.
        assert_eq!(r.poll(t0 + secs(4)), Action::Idle);
        r.placed();
        assert_eq!(*r.state(), State::Live);
    }

    #[test]
    fn a_successful_reconnect_resets_the_budget() {
        let mut r = Reconnect::default();
        let mut t = Instant::now();
        for _ in 0..3 {
            r.ended(Ending::Dropped("net error".into()), t);
            t += secs(120);
            assert_eq!(r.poll(t), Action::Connect);
            r.placed();
            // Back to attempt 1's delay each time.
            r.ended(Ending::Dropped("net error".into()), t);
            assert_eq!(r.next_attempt(), Some(t + secs(3)));
            r.placed();
        }
    }

    #[test]
    fn attempts_are_capped_and_then_it_gives_up() {
        let mut r = Reconnect::new(Policy {
            tries: 3,
            ..Policy::default()
        });
        let mut t = Instant::now();
        r.ended(Ending::Dropped("net error".into()), t);
        for n in 1..=3 {
            t = r.next_attempt().expect("scheduled");
            assert_eq!(r.poll(t), Action::Connect, "attempt {n}");
            r.ended(Ending::Dropped("refused".into()), t);
        }
        assert_eq!(r.stopped(), Some(&Stopped::Exhausted));
        assert_eq!(r.next_attempt(), None);
        assert_eq!(r.poll(t + secs(3600)), Action::Idle);
        let said: Vec<String> = std::iter::from_fn(|| r.take_notice()).collect();
        assert!(
            said.iter()
                .any(|l| l.contains("Could not reconnect after 3")),
            "{said:?}"
        );
    }

    #[test]
    fn the_login_cooldown_does_not_spend_an_attempt() {
        let mut r = Reconnect::new(Policy {
            tries: 2,
            cooldown: secs(80),
            ..Policy::default()
        });
        let t0 = Instant::now();
        r.ended(Ending::Dropped("net error".into()), t0);
        let t1 = r.next_attempt().unwrap();
        assert_eq!(r.poll(t1), Action::Connect);
        // The server says the account is still logged in, three times.
        let mut t = t1;
        for _ in 0..3 {
            r.ended(Ending::StillLoggedIn, t);
            let next = r.next_attempt().expect("waits out the cooldown");
            assert_eq!(next, t + secs(80));
            t = next;
            assert_eq!(r.poll(t), Action::Connect);
        }
        // Still on attempt 1: the fourth try can still fail twice.
        assert!(r.stopped().is_none());
        r.placed();
        assert_eq!(*r.state(), State::Live);
    }

    #[test]
    fn an_endless_cooldown_eventually_counts_as_a_failure() {
        let mut r = Reconnect::new(Policy {
            tries: 1,
            cooldown: secs(80),
            cooldown_waits: 2,
            ..Policy::default()
        });
        let mut t = Instant::now();
        r.ended(Ending::Dropped("net error".into()), t);
        t = r.next_attempt().unwrap();
        assert_eq!(r.poll(t), Action::Connect);
        for _ in 0..2 {
            r.ended(Ending::StillLoggedIn, t);
            t = r.next_attempt().unwrap();
            assert_eq!(r.poll(t), Action::Connect);
        }
        // The third refusal is out of patience, and the single attempt
        // is spent, so it gives up.
        r.ended(Ending::StillLoggedIn, t);
        assert_eq!(r.stopped(), Some(&Stopped::Exhausted));
    }

    #[test]
    fn a_fatal_refusal_stops_at_once() {
        let mut r = Reconnect::default();
        let t = Instant::now();
        r.ended(Ending::Dropped("net error".into()), t);
        let t1 = r.next_attempt().unwrap();
        assert_eq!(r.poll(t1), Action::Connect);
        r.ended(classify_refusal(0xF659, 0xF), t1);
        assert!(matches!(r.stopped(), Some(Stopped::Fatal(_))));
        assert_eq!(r.poll(t1 + secs(3600)), Action::Idle);
    }

    #[test]
    fn zero_tries_turns_it_off() {
        let mut r = Reconnect::new(Policy {
            tries: 0,
            ..Policy::default()
        });
        let t = Instant::now();
        r.ended(Ending::Dropped("net error".into()), t);
        assert_eq!(r.stopped(), Some(&Stopped::Exhausted));
    }

    #[test]
    fn an_attempt_that_hangs_is_failed() {
        let mut r = Reconnect::new(Policy {
            attempt_timeout: secs(45),
            ..Policy::default()
        });
        let t0 = Instant::now();
        r.ended(Ending::Dropped("net error".into()), t0);
        let t1 = r.next_attempt().unwrap();
        assert_eq!(r.poll(t1), Action::Connect);
        assert_eq!(r.poll(t1 + secs(44)), Action::Idle);
        assert!(r.next_attempt().is_none());
        assert_eq!(r.poll(t1 + secs(45)), Action::Idle);
        // Failed and rescheduled as attempt 2.
        assert_eq!(r.next_attempt(), Some(t1 + secs(45) + secs(6)));
    }

    #[test]
    fn notices_read_as_a_story() {
        let mut r = Reconnect::default();
        let t0 = Instant::now();
        r.ended(Ending::Dropped("net error 0x1/0x2".into()), t0);
        let t1 = r.next_attempt().unwrap();
        r.poll(t1);
        r.placed();
        let said: Vec<String> = std::iter::from_fn(|| r.take_notice()).collect();
        assert_eq!(
            said,
            vec![
                "Connection lost (net error 0x1/0x2); reconnecting in 3 s".to_string(),
                "Reconnecting (1 of 6)...".to_string(),
                "Reconnected.".to_string(),
            ]
        );
    }
}
