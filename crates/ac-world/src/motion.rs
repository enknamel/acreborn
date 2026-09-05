//! One-shot motion commands (attacks, emotes, door open/close) an object
//! has been told to play. A MovementEvent lists every command still
//! pending on the server with its motion sequence number, and the same
//! command is repeated across events until it has run, so the queue keeps
//! a short history of `(command, sequence)` pairs and only admits new ones.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

/// How many recent commands to remember for duplicate detection.
const HISTORY: usize = 16;

/// Serials are unique across every queue in the process, so a consumer
/// that remembers "the last serial I played" stays correct when the
/// object it follows is deleted and re-created.
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);

/// A motion command the server asked an object to play once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PendingCommand {
    /// Low 16 bits of the MotionCommand id, as carried on the wire.
    pub command: u16,
    /// Server motion sequence number (autonomous bit stripped).
    pub sequence: u16,
    /// Playback speed multiplier (1.0 = as authored).
    pub speed: f32,
    /// True if the object's own client started the motion.
    pub autonomous: bool,
    /// Monotonic arrival order, unique across objects.
    pub serial: u64,
}

#[derive(Debug, Clone, Default)]
pub struct CommandQueue {
    recent: VecDeque<PendingCommand>,
    /// Serial of the newest command handed out by `pop`.
    popped: u64,
}

impl CommandQueue {
    /// Record a command from a MovementEvent. Returns false if this
    /// `(command, sequence)` was already seen (a repeat, not a new play).
    pub fn push(&mut self, command: u16, packed_sequence: u16, speed: f32) -> bool {
        let sequence = packed_sequence & 0x7FFF;
        if self
            .recent
            .iter()
            .any(|c| c.command == command && c.sequence == sequence)
        {
            return false;
        }
        let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
        self.recent.push_back(PendingCommand {
            command,
            sequence,
            speed: if speed.is_finite() && speed != 0.0 {
                speed
            } else {
                1.0
            },
            autonomous: packed_sequence & 0x8000 != 0,
            serial,
        });
        while self.recent.len() > HISTORY {
            self.recent.pop_front();
        }
        true
    }

    /// The oldest command not yet taken by `pop`, in arrival order.
    pub fn pop(&mut self) -> Option<PendingCommand> {
        let c = *self.recent.iter().find(|c| c.serial > self.popped)?;
        self.popped = c.serial;
        Some(c)
    }

    /// Commands that arrived after `serial`, oldest first. Lets a consumer
    /// with only shared access track what it has played.
    pub fn since(&self, serial: u64) -> impl Iterator<Item = &PendingCommand> {
        self.recent.iter().filter(move |c| c.serial > serial)
    }

    /// Serial of the newest command, or 0 when none has arrived.
    pub fn latest_serial(&self) -> u64 {
        self.recent.back().map(|c| c.serial).unwrap_or(0)
    }

    /// True if `pop` has nothing left to hand out.
    pub fn is_empty(&self) -> bool {
        self.latest_serial() <= self.popped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ATTACK_HIGH1: u16 = 0x62;
    const WAVE: u16 = 0x87;

    #[test]
    fn repeats_are_not_replayed() {
        let mut q = CommandQueue::default();
        assert!(q.push(ATTACK_HIGH1, 5, 1.0));
        // The next MovementEvent lists the same pending action again.
        assert!(!q.push(ATTACK_HIGH1, 5, 1.0));
        // The autonomous bit is not part of the identity.
        assert!(!q.push(ATTACK_HIGH1, 5 | 0x8000, 1.0));
        // A new sequence number is a new swing.
        assert!(q.push(ATTACK_HIGH1, 6, 1.5));
        let a = q.pop().unwrap();
        let b = q.pop().unwrap();
        assert_eq!((a.command, a.sequence, a.speed), (ATTACK_HIGH1, 5, 1.0));
        assert_eq!((b.command, b.sequence, b.speed), (ATTACK_HIGH1, 6, 1.5));
        assert!(b.serial > a.serial);
        assert!(q.pop().is_none());
        assert!(q.is_empty());
    }

    #[test]
    fn since_tracks_a_shared_consumer() {
        let mut q = CommandQueue::default();
        assert_eq!(q.latest_serial(), 0);
        assert_eq!(q.since(0).count(), 0);
        q.push(WAVE, 1, 1.0);
        q.push(ATTACK_HIGH1, 2, 1.0);
        let played = {
            let new: Vec<_> = q.since(0).collect();
            assert_eq!(new.len(), 2);
            assert_eq!(new[0].command, WAVE);
            assert_eq!(new[1].command, ATTACK_HIGH1);
            new[1].serial
        };
        assert_eq!(played, q.latest_serial());
        assert_eq!(q.since(played).count(), 0);
        q.push(WAVE, 1, 1.0); // repeat
        assert_eq!(q.since(played).count(), 0);
        q.push(WAVE, 3, 1.0);
        let new: Vec<_> = q.since(played).collect();
        assert_eq!(new.len(), 1);
        assert_eq!((new[0].command, new[0].sequence), (WAVE, 3));
        assert!(!new[0].autonomous);
    }

    #[test]
    fn autonomous_bit_and_bad_speeds() {
        let mut q = CommandQueue::default();
        q.push(WAVE, 7 | 0x8000, 0.0);
        let c = q.pop().unwrap();
        assert!(c.autonomous);
        assert_eq!(c.sequence, 7);
        assert_eq!(c.speed, 1.0, "zero speed plays at normal speed");
    }

    #[test]
    fn history_is_bounded_and_serials_are_global() {
        let mut a = CommandQueue::default();
        let mut b = CommandQueue::default();
        for i in 0..(HISTORY as u16 * 2) {
            assert!(a.push(WAVE, i, 1.0));
        }
        assert!(b.push(WAVE, 0, 1.0));
        assert!(b.latest_serial() > a.latest_serial());
        // Old entries have fallen out of the history, so a very old
        // sequence looks new again (the server never re-sends one this old).
        assert!(a.push(WAVE, 0, 1.0));
        assert!(!a.push(WAVE, HISTORY as u16 * 2 - 1, 1.0));
    }
}
