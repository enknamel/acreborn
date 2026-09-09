//! Keeping the pack tidy: pouring loose stacks together so that slots
//! are not wasted on them.
//!
//! Slots are the scarce thing in Asheron's Call, not weight. Buy five
//! scarabs and they arrive as a stack of five, sitting in their own slot
//! next to the fifteen already carried; loot four arrows off a corpse
//! and they land beside the two hundred in the pack. Nothing warns you.
//! An afternoon of this fills a pack with change while the character
//! believes it has room.
//!
//! So the rules pour stacks together whenever two of the same thing are
//! carried loose. A stack has a maximum -- a trade note holds 250, an
//! arrow 1,000 -- so a large enough pile still needs more than one
//! stack; what it must not need is eleven.
//!
//! The server takes one merge at a time and answers in its own time,
//! so [`next_merge`] returns a single move and is asked again once that
//! move has landed.

/// One stack in the pack, as the compactor needs to see it.
#[derive(Clone, Debug, PartialEq)]
pub struct Stack {
    pub guid: u32,
    /// What it is. Two stacks merge only if these match: an Arrow and a
    /// Broadhead Arrow look alike and are not.
    pub wcid: u32,
    pub name: String,
    /// How many are in it.
    pub count: u32,
    /// How many it could hold. 1 (or 0) is something that does not
    /// stack at all.
    pub max: u32,
    /// It is in the character's hands rather than the pack: arrows in
    /// the quiver slot, say. A wielded stack can be topped up, but is
    /// never the one poured away.
    pub wielded: bool,
}

impl Stack {
    /// Whether it stacks at all.
    fn stackable(&self) -> bool {
        self.max > 1
    }

    /// Room left in it.
    fn room(&self) -> u32 {
        self.max.saturating_sub(self.count)
    }
}

/// One stack poured into another.
#[derive(Clone, Debug, PartialEq)]
pub struct Merge {
    pub from: u32,
    pub to: u32,
    /// How many to move. Never more than the source holds, and never
    /// more than the target has room for: the server refuses both.
    pub amount: u32,
    /// What is being poured, for the log.
    pub name: String,
    /// The move empties the source, freeing its slot.
    pub frees_a_slot: bool,
}

/// The next merge worth making, or `None` when the pack is as tight as
/// it goes.
///
/// Of all the merges available, the one that frees a slot outright is
/// taken first: that is the whole point of the exercise, and a move
/// that only shuffles counts between two stacks can wait. Among equals
/// the largest pour wins, so the pack settles in as few round trips to
/// the server as it can, and ties go to the lowest guid so that the
/// answer does not wander between frames.
pub fn next_merge(stacks: &[Stack]) -> Option<Merge> {
    let mut best: Option<Merge> = None;
    for from in stacks {
        // A stack in the character's hands stays there, and a full one
        // has nothing spare to give.
        if !from.stackable() || from.wielded || from.count == 0 {
            continue;
        }
        for to in stacks {
            if to.guid == from.guid || to.wcid != from.wcid || !to.stackable() {
                continue;
            }
            let room = to.room();
            if room == 0 {
                continue;
            }
            let amount = from.count.min(room);
            if amount == 0 {
                continue;
            }
            // Pour the smaller into the larger. Without this the pair
            // would swap back and forth for ever, each frame deciding
            // the other way round.
            if (to.count, to.guid) <= (from.count, from.guid) {
                continue;
            }
            let candidate = Merge {
                from: from.guid,
                to: to.guid,
                amount,
                name: from.name.clone(),
                frees_a_slot: amount == from.count,
            };
            if better(&candidate, &best) {
                best = Some(candidate);
            }
        }
    }
    best
}

/// Whether `a` is a better move than what has been found so far.
fn better(a: &Merge, best: &Option<Merge>) -> bool {
    let Some(b) = best else { return true };
    (a.frees_a_slot, a.amount, b.from, b.to).gt(&(b.frees_a_slot, b.amount, a.from, a.to))
}

/// How many slots the stacks are using, and how few they could use.
/// What the tidying is worth, for the log and the UI.
pub fn slots_wasted(stacks: &[Stack]) -> u32 {
    use std::collections::BTreeMap;
    let mut piles: BTreeMap<u32, (u32, u32, u32)> = BTreeMap::new();
    for s in stacks {
        if !s.stackable() || s.wielded {
            continue;
        }
        let e = piles.entry(s.wcid).or_insert((0, 0, s.max));
        e.0 += s.count;
        e.1 += 1;
    }
    piles
        .values()
        .map(|(total, used, max)| {
            let needed = total.div_ceil((*max).max(1));
            used.saturating_sub(needed)
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(guid: u32, wcid: u32, count: u32, max: u32) -> Stack {
        Stack {
            guid,
            wcid,
            name: format!("thing {wcid}"),
            count,
            max,
            wielded: false,
        }
    }

    #[test]
    fn five_scarabs_bought_go_into_the_fifteen_already_carried() {
        // The case that started this: a purchase arrives in its own
        // slot beside what is already there.
        let pack = [stack(1, 690, 15, 100), stack(2, 690, 5, 100)];
        let m = next_merge(&pack).expect("a merge");
        assert_eq!(m.from, 2, "poured the big stack into the small one");
        assert_eq!(m.to, 1);
        assert_eq!(m.amount, 5);
        assert!(m.frees_a_slot);
    }

    #[test]
    fn a_tidy_pack_has_nothing_to_do() {
        let pack = [stack(1, 690, 100, 100), stack(2, 693, 40, 100)];
        assert_eq!(next_merge(&pack), None);
        assert_eq!(slots_wasted(&pack), 0);
    }

    #[test]
    fn things_that_do_not_stack_are_left_alone() {
        // Two swords are two swords, however alike.
        let pack = [stack(1, 555, 1, 1), stack(2, 555, 1, 1)];
        assert_eq!(next_merge(&pack), None);
        let unstackable = [stack(1, 555, 1, 0), stack(2, 555, 1, 0)];
        assert_eq!(next_merge(&unstackable), None);
    }

    #[test]
    fn different_things_never_merge() {
        // An Arrow and a Broadhead Arrow look alike and are not.
        let pack = [stack(1, 300, 50, 1000), stack(2, 301, 50, 1000)];
        assert_eq!(next_merge(&pack), None);
    }

    #[test]
    fn a_full_stack_is_not_poured_into() {
        let pack = [stack(1, 690, 100, 100), stack(2, 690, 5, 100)];
        // The only stack with room is the small one, and pouring the
        // full one into it would free nothing.
        assert_eq!(next_merge(&pack), None);
    }

    #[test]
    fn a_pour_never_overflows_the_target() {
        // 250 to a stack: 200 already there takes 50 of the 90 offered.
        let pack = [stack(1, 2621, 200, 250), stack(2, 2621, 90, 250)];
        let m = next_merge(&pack).expect("a merge");
        assert_eq!(m.amount, 50);
        assert!(!m.frees_a_slot, "40 are left behind");
    }

    #[test]
    fn freeing_a_slot_comes_before_shuffling_counts() {
        // Two things to do: top up a nearly-full note stack (frees
        // nothing), or empty a stray taper stack (frees a slot).
        let pack = [
            stack(1, 2621, 240, 250),
            stack(2, 2621, 200, 250),
            stack(3, 20631, 300, 500),
            stack(4, 20631, 20, 500),
        ];
        let m = next_merge(&pack).expect("a merge");
        assert!(m.frees_a_slot, "{m:?}");
        assert_eq!(m.from, 4);
        assert_eq!(m.to, 3);
    }

    #[test]
    fn the_pack_settles_and_stays_settled() {
        // Run the moves out to the end: it must terminate, and the
        // result must be as few stacks as the maximum allows.
        let mut pack = vec![
            stack(1, 690, 15, 100),
            stack(2, 690, 5, 100),
            stack(3, 690, 90, 100),
            stack(4, 690, 40, 100),
            stack(5, 693, 7, 25),
            stack(6, 693, 7, 25),
            stack(7, 693, 7, 25),
            stack(8, 693, 7, 25),
        ];
        let mut moves = 0;
        while let Some(m) = next_merge(&pack) {
            moves += 1;
            assert!(moves < 100, "the pack never settles");
            let from = pack.iter().position(|s| s.guid == m.from).unwrap();
            let to = pack.iter().position(|s| s.guid == m.to).unwrap();
            assert!(pack[to].count + m.amount <= pack[to].max, "overflowed");
            pack[from].count -= m.amount;
            pack[to].count += m.amount;
            pack.retain(|s| s.count > 0);
        }
        // 150 scarabs at 100 to a stack is two; 28 tapers at 25 is two.
        assert_eq!(pack.iter().filter(|s| s.wcid == 690).count(), 2);
        assert_eq!(pack.iter().filter(|s| s.wcid == 693).count(), 2);
        assert_eq!(pack.iter().map(|s| s.count).sum::<u32>(), 150 + 28);
        assert_eq!(slots_wasted(&pack), 0);
    }

    #[test]
    fn the_answer_does_not_wander_between_frames() {
        // Asked twice about the same pack, the same move comes back;
        // and the order the stacks arrive in does not change it.
        let mut pack = vec![
            stack(1, 690, 15, 100),
            stack(2, 690, 5, 100),
            stack(3, 690, 40, 100),
        ];
        let first = next_merge(&pack).expect("a merge");
        assert_eq!(next_merge(&pack), Some(first.clone()));
        pack.reverse();
        assert_eq!(next_merge(&pack), Some(first));
    }

    #[test]
    fn arrows_in_the_quiver_are_topped_up_but_never_emptied() {
        let mut quiver = stack(1, 300, 100, 1000);
        quiver.wielded = true;
        let pack = [quiver, stack(2, 300, 50, 1000)];
        let m = next_merge(&pack).expect("a merge");
        assert_eq!(m.from, 2, "took the arrows out of the quiver");
        assert_eq!(m.to, 1);
        assert_eq!(m.amount, 50);
    }

    #[test]
    fn what_the_tidying_is_worth_is_counted() {
        // Four stacks of seven tapers where two would do.
        let pack = [
            stack(1, 693, 7, 25),
            stack(2, 693, 7, 25),
            stack(3, 693, 7, 25),
            stack(4, 693, 7, 25),
        ];
        assert_eq!(slots_wasted(&pack), 2);
        // A single stack wastes nothing, whatever its size.
        assert_eq!(slots_wasted(&[stack(1, 693, 3, 25)]), 0);
        assert_eq!(slots_wasted(&[]), 0);
    }
}
