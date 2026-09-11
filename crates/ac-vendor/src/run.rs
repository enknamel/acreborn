//! One trip to a counter, one decision at a time.

use std::time::Instant;

use ac_agent::did::{Did, Patience};
use ac_agent::pack;

use crate::counter::{Item, Snapshot};

/// The one thing the rules want done next.
///
/// At most one per turn, and every one of them is something a player
/// could do by hand. The server takes one at a time and answers in its
/// own time, so asking for two is how a run ends up selling a stack it
/// has already merged away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Act {
    /// Walk to the counter: it will not trade across a room.
    Approach { guid: u32 },
    /// Ask it to open its window.
    Open { guid: u32 },
    /// Pour one stack into another to free a slot.
    Merge { from: u32, to: u32, amount: u32 },
    /// Cut a piece off a stack worth more than the counter will look at.
    Split { guid: u32, amount: u32 },
    /// Hand these over, all in one go.
    ///
    /// A counter takes a whole armful at once -- the sell action has
    /// always carried a list, and sending them one at a time was a
    /// round trip per dagger. What bounds an armful is not the counter
    /// but the pack: the takings come back as coin, and coin needs
    /// slots.
    Sell { items: Vec<u32> },
    /// Buy this many of something on the shelf.
    Buy { wcid: u32, count: u32 },
    /// Turn trade notes back into coin to pay a bill.
    ///
    /// A counter takes notes for what it sells, but not as change: a
    /// purse of notes and no coin cannot buy a handful of tapers. The
    /// notes are sold back first, which costs their markup and is
    /// still the only way to spend them on something small.
    Cash { face: u32, count: u32 },
    /// Shut the window; the trip is over.
    Close,
}

/// Where the trip has got to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// Not at the counter yet.
    #[default]
    Walking,
    /// Waiting for the window.
    Opening,
    /// Compressing, selling and making notes, round after round.
    Selling,
    /// Spending what the selling raised.
    Buying,
    /// Nothing left to do here.
    Done,
}

/// What came of a turn: what to do, what it means, and how to say it.
#[derive(Clone, Debug, PartialEq)]
pub struct Next {
    pub act: Option<Act>,
    pub did: Did,
    /// One line for the log and the panel, in the character's voice.
    pub saying: String,
}

impl Next {
    fn acting(act: Act, saying: impl Into<String>) -> Next {
        Next {
            act: Some(act),
            did: Did::Acting,
            saying: saying.into(),
        }
    }

    fn nothing(did: Did, saying: impl Into<String>) -> Next {
        Next {
            act: None,
            did,
            saying: saying.into(),
        }
    }
}

/// A trip to one counter.
///
/// It holds only what cannot be seen in a snapshot: which phase the
/// trip is in, what has been handed over and not yet answered for, and
/// what has been refused and should not be asked about again yet.
#[derive(Debug, Default)]
pub struct Run {
    pub phase: Phase,
    /// Handed to the counter and not yet gone from the pack.
    offered: Vec<u32>,
    /// Items no counter will take.
    refused: Patience<u32>,
    /// Pairs of stacks that would not join.
    wont_merge: Patience<(u32, u32)>,
    /// How many things have been sold this trip.
    pub sold: u32,
}

impl Run {
    pub fn new() -> Run {
        Run::default()
    }

    /// What a person would see: where the trip is and what is waiting.
    pub fn waiting_on(&self) -> usize {
        self.offered.len()
    }

    /// The next thing to do, or why there is nothing.
    pub fn step(&mut self, snap: &Snapshot, now: Instant) -> Next {
        let Some(counter) = snap.counter.as_ref() else {
            self.phase = Phase::Done;
            return Next::nothing(Did::blocked("there is no counter here"), "no counter");
        };

        // Stand at it first. A counter will not trade with somebody
        // across the room: the server answers by telling us to walk
        // there, and a client that does not walk waits for a window
        // that never opens.
        if counter.away > snap.rules.reach {
            self.phase = Phase::Walking;
            return Next::acting(
                Act::Approach { guid: counter.guid },
                format!("walking up to {}", counter.name),
            );
        }
        if !counter.open {
            self.phase = Phase::Opening;
            return Next::acting(
                Act::Open { guid: counter.guid },
                format!("talking to {}", counter.name),
            );
        }

        // Anything handed over that is still in the pack has not been
        // taken. The kind and the worth were checked before it was
        // offered, so this is the item refusing for itself.
        if !self.offered.is_empty() {
            let still: Vec<u32> = self
                .offered
                .iter()
                .copied()
                .filter(|g| snap.item(*g).is_some())
                .collect();
            self.sold += (self.offered.len() - still.len()) as u32;
            self.offered.clear();
            for guid in still {
                self.refused
                    .note(guid, &Did::refused("no counter will take it"), now);
            }
        }

        if matches!(self.phase, Phase::Walking | Phase::Opening) {
            self.phase = Phase::Selling;
        }

        if self.phase == Phase::Selling {
            // 1. Compress. Before any of it is sold, and again every
            //    round: a sale makes the character lighter, and the
            //    server weighs a pour against what is carried.
            if let Some(next) = self.compress(snap, now) {
                return next;
            }
            // 2. Make notes once the pack is low on room.
            if snap.slots_free <= snap.rules.keep_slots {
                if let Some(next) = self.make_notes(snap) {
                    return next;
                }
            }
            // 3. Sell one thing.
            if let Some(next) = self.sell_one(snap, now) {
                return next;
            }
            self.phase = Phase::Buying;
        }

        if self.phase == Phase::Buying {
            if let Some(next) = self.buy_one(snap) {
                return next;
            }
            self.phase = Phase::Done;
        }

        Next {
            act: Some(Act::Close),
            did: Did::Done,
            saying: format!("sold {} item(s) to {}", self.sold, counter.name),
        }
    }

    /// Pour one pair of stacks together, if any pair is worth pouring
    /// and light enough for the server to allow it.
    fn compress(&mut self, snap: &Snapshot, now: Instant) -> Option<Next> {
        let stacks: Vec<pack::Stack> = snap
            .items
            .iter()
            .map(|i| pack::Stack {
                guid: i.guid,
                wcid: i.wcid,
                name: i.name.clone(),
                count: i.stack.max(1),
                max: i.max_stack,
                wielded: i.wielded,
                burden: i.burden,
            })
            .collect();
        let room = snap.burden_room();
        // Never pour two stacks into one the counter will not look at.
        //
        // Selling cuts a stack down when it is worth more than the
        // ceiling; compressing would pour the piece straight back, and
        // the two would take it in turns for ever -- the same argument
        // the quartermaster and the tidier once had over a counted-out
        // purse. Whichever ran first would look like it was working.
        let ceiling = snap
            .counter
            .as_ref()
            .map(|c| c.max_value)
            .filter(|c| *c > 0);
        let worth = |guid: u32| snap.item(guid).map(|i| i.value).unwrap_or(0);
        let held = |from: u32, to: u32| {
            if self.wont_merge.held(&(from, to), now) {
                return true;
            }
            match ceiling {
                Some(max) => worth(from).saturating_add(worth(to)) > max,
                None => false,
            }
        };
        match pack::next_merge_unless(&stacks, room, held) {
            Some(m) => {
                self.wont_merge.note(
                    (m.from, m.to),
                    &Did::waiting("the stacks have not joined yet"),
                    now,
                );
                Some(Next::acting(
                    Act::Merge {
                        from: m.from,
                        to: m.to,
                        amount: m.amount,
                    },
                    format!("putting {} {} with the rest", m.amount, m.name),
                ))
            }
            None => None,
        }
    }

    /// Turn the takings into trade notes, so the selling can go on.
    fn make_notes(&mut self, snap: &Snapshot) -> Option<Next> {
        let counter = snap.counter.as_ref()?;
        let face = counter.note_face.filter(|f| *f > 0)?;
        // A note costs more than its face -- fifteen per cent, whatever
        // the denomination -- so this always loses a little. It is
        // worth it: a slot of Mayoi notes carries sixty two million and
        // a slot of coin carries twenty five thousand.
        let each = crate::errand::note_cost(face).max(1);
        let count = snap.coin.saturating_sub(snap.rules.float) / each;
        if count == 0 {
            return None;
        }
        let wcid = face_wcid(counter, face)?;
        Some(Next::acting(
            Act::Buy { wcid, count },
            format!("packing the takings into {count} note(s)"),
        ))
    }

    /// Hand over an armful, cutting a stack down first when it is worth
    /// more than the counter will look at.
    ///
    /// As many as the pack can take the money for. Twenty-five thousand
    /// pyreals fill a slot, so a big enough armful pays for itself in
    /// slots and then starts costing them; the armful stops at the
    /// reserve, and the next round turns the coin into notes and goes
    /// on.
    fn sell_one(&mut self, snap: &Snapshot, now: Instant) -> Option<Next> {
        let counter = snap.counter.as_ref()?;
        let mut offer: Vec<&Item> = Vec::new();
        for it in &snap.items {
            if it.keep.forbidden() || it.wielded {
                continue;
            }
            if self.refused.held(&it.guid, now) {
                continue;
            }
            if counter.buys != 0 && it.item_type & counter.buys == 0 {
                continue;
            }
            if it.value < snap.rules.floor_value || it.value == 0 {
                continue;
            }
            // Not what the character came here to buy. Selling the
            // tapers out of the pack and buying them back a moment
            // later is a round trip that costs the markup and gains
            // nothing.
            if snap.wants.iter().any(|w| w.wcid == it.wcid && w.short > 0) {
                continue;
            }
            offer.push(it);
        }
        // The dearest first: they are the ones most worth the slot they
        // sit in, and the ones the armful should certainly include.
        offer.sort_by(|a, b| b.value.cmp(&a.value));

        // Anything too dear for this counter is cut down first, on its
        // own: the piece is a new object and the armful would be naming
        // a guid that does not exist yet.
        if let Some(it) = offer.first().copied() {
            if counter.max_value > 0 && it.value > counter.max_value {
                let each = it.each().max(1);
                let take = (counter.max_value / each).max(1).min(it.stack);
                if take < it.stack {
                    return Some(Next::acting(
                        Act::Split {
                            guid: it.guid,
                            amount: take,
                        },
                        format!("cutting {take} off the {}", it.name),
                    ));
                }
                self.refused.note(
                    it.guid,
                    &Did::refused("worth more than this counter will look at"),
                    now,
                );
                return Some(Next::nothing(
                    Did::waiting("that one is too dear for this counter"),
                    format!("{} is worth more than {} will take", it.name, counter.name),
                ));
            }
        }

        let mut items: Vec<u32> = Vec::new();
        let mut takings = snap.coin;
        let before = coin_slots(snap.coin);
        for it in offer {
            let after = takings.saturating_add(it.value);
            // What the pack looks like once this one is gone and its
            // money is in: one slot back for the item, and however many
            // the coin has grown into.
            let freed = items.len() as u32 + 1;
            let cost = coin_slots(after).saturating_sub(before);
            let left = snap
                .slots_free
                .saturating_add(freed)
                .saturating_sub(cost);
            if left < snap.rules.keep_slots && !items.is_empty() {
                break;
            }
            items.push(it.guid);
            takings = after;
        }
        if items.is_empty() {
            return None;
        }
        self.offered.extend(items.iter().copied());
        let saying = match items.len() {
            1 => format!("selling {} item", items.len()),
            n => format!("selling {n} items"),
        };
        Some(Next::acting(Act::Sell { items }, saying))
    }

    /// Buy one thing the character came for, within the purse and what
    /// it can still lift.
    fn buy_one(&mut self, snap: &Snapshot) -> Option<Next> {
        let counter = snap.counter.as_ref()?;
        let purse = snap.purse();
        for want in &snap.wants {
            if want.short == 0 {
                continue;
            }
            let Some(ware) = counter.wares.iter().find(|w| w.wcid == want.wcid) else {
                continue;
            };
            if ware.price == 0 || ware.price > purse {
                continue;
            }
            let afford = purse / ware.price;
            // And what it can carry home. Nothing known about the
            // weight is not a reason to refuse: buy, and let the server
            // answer for it.
            let liftable = match ware.burden {
                0 => u32::MAX,
                each => snap.burden_room() / each,
            };
            let count = want
                .short
                .min(afford)
                .min(liftable)
                .min(ware.stock.unwrap_or(u32::MAX));
            if count == 0 {
                continue;
            }
            let bill = ware.price.saturating_mul(count);
            // Coin first. A purse whose worth is all in notes cannot
            // pay a small bill, so enough of them are turned back into
            // coin -- the smallest note that covers it, so that a
            // fortune is not broken to buy a handful of tapers.
            if bill > snap.coin {
                let short = bill - snap.coin;
                if let Some((face, have)) = snap
                    .notes
                    .iter()
                    .find(|(face, n)| **n > 0 && **face >= short)
                    .or_else(|| snap.notes.iter().rev().find(|(_, n)| **n > 0))
                {
                    let need = short.div_ceil((*face).max(1)).min(*have);
                    if need > 0 {
                        return Some(Next::acting(
                            Act::Cash {
                                face: *face,
                                count: need,
                            },
                            format!("cashing {need} note(s) to pay for {}", want.name),
                        ));
                    }
                }
                continue;
            }
            return Some(Next::acting(
                Act::Buy {
                    wcid: want.wcid,
                    count,
                },
                format!("buying {count} {}", want.name),
            ));
        }
        None
    }
}

/// How many slots a pile of coin takes up. A pyreal stack holds
/// twenty-five thousand and then wants another slot.
fn coin_slots(coin: u32) -> u32 {
    coin.div_ceil(crate::errand::COIN_STACK)
}

/// The weenie of the note a counter deals in, found on its own shelf.
fn face_wcid(counter: &crate::counter::Counter, face: u32) -> Option<u32> {
    counter
        .wares
        .iter()
        .find(|w| w.price >= face)
        .map(|w| w.wcid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::counter::{Counter, Keep, Rules, Want, Ware};
    use std::collections::BTreeMap;

    const COIN: u32 = 273; // the pyreal's own weenie
    const NOTE: u32 = 20630; // Mayoi trade note, face 250,000

    fn item(guid: u32, name: &str, value: u32, stack: u32, max: u32) -> Item {
        Item {
            guid,
            wcid: guid,
            name: name.into(),
            value,
            burden: 10,
            stack,
            max_stack: max,
            item_type: 1,
            pack: None,
            wielded: false,
            keep: Keep::default(),
        }
    }

    fn counter() -> Counter {
        Counter {
            guid: 900,
            name: "Ulgrim".into(),
            open: true,
            buys: 0,
            max_value: 0,
            wares: vec![Ware {
                wcid: NOTE,
                name: "Mayoi Trade Note".into(),
                price: 287_500,
                stock: None,
                burden: 1,
            }],
            note_face: Some(250_000),
            away: 0.0,
        }
    }

    fn snap(items: Vec<Item>) -> Snapshot {
        Snapshot {
            items,
            coin: 0,
            notes: BTreeMap::new(),
            carried: 1_000,
            capacity: 30_000,
            slots_free: 20,
            counter: Some(counter()),
            wants: Vec::new(),
            rules: Rules::default(),
        }
    }

    #[test]
    fn it_stands_at_the_counter_before_asking_it_to_trade() {
        let mut run = Run::new();
        let mut s = snap(vec![item(1, "Dagger", 500, 1, 1)]);
        s.counter.as_mut().unwrap().away = 12.0;
        assert_eq!(
            run.step(&s, Instant::now()).act,
            Some(Act::Approach { guid: 900 })
        );
        // Near enough, but the window is shut.
        s.counter.as_mut().unwrap().away = 1.0;
        s.counter.as_mut().unwrap().open = false;
        assert_eq!(run.step(&s, Instant::now()).act, Some(Act::Open { guid: 900 }));
    }

    #[test]
    fn the_pack_is_compressed_before_anything_is_sold() {
        let mut run = Run::new();
        // Two part stacks of the same thing and one thing worth
        // selling. The stacks go together first: slots are what a sale
        // runs out of.
        let mut a = item(1, "Taper", 100, 37, 1000);
        let mut b = item(2, "Taper", 100, 41, 1000);
        a.wcid = 20631;
        b.wcid = 20631;
        let s = snap(vec![a, b, item(3, "Dagger", 500, 1, 1)]);
        assert_eq!(
            run.step(&s, Instant::now()).act,
            Some(Act::Merge {
                from: 1,
                to: 2,
                amount: 37
            })
        );
    }

    #[test]
    fn a_pour_heavier_than_the_room_is_not_asked_for() {
        let mut run = Run::new();
        let mut a = item(1, "Taper", 100, 37, 1000);
        let mut b = item(2, "Taper", 100, 41, 1000);
        a.wcid = 20631;
        b.wcid = 20631;
        a.burden = 456;
        b.burden = 500;
        let mut s = snap(vec![a, b, item(3, "Dagger", 500, 1, 1)]);
        // Seventeen units under the ceiling, as a real character was:
        // the server weighs a merge as though the source were being
        // picked up afresh, and refuses it without a word.
        s.carried = s.capacity * 3 - 17;
        // It does not try to pour them together; it gets on with the
        // selling, which is what makes the character light enough to
        // pour them together later.
        let next = run.step(&s, Instant::now());
        assert!(
            matches!(&next.act, Some(Act::Sell { .. })),
            "expected selling, got {:?} -- {}",
            next.act,
            next.saying
        );
    }

    #[test]
    fn selling_stops_to_make_notes_when_the_pack_is_low_on_room() {
        let mut run = Run::new();
        let mut s = snap(vec![item(3, "Dagger", 500, 1, 1)]);
        s.coin = 1_000_000;
        s.slots_free = 3; // at the reserve, not out of slots
        let next = run.step(&s, Instant::now());
        assert_eq!(
            next.act,
            Some(Act::Buy {
                wcid: NOTE,
                count: 3
            }),
            "{}",
            next.saying
        );
        // With room to spare it gets on with the selling instead.
        s.slots_free = 20;
        assert_eq!(run.step(&s, Instant::now()).act, Some(Act::Sell { items: vec![3] }));
    }

    #[test]
    fn what_must_never_be_sold_is_never_offered() {
        let now = Instant::now();
        for (what, mut keep) in [
            ("tinkered", Keep::default()),
            ("inscribed", Keep::default()),
            ("equipped", Keep::default()),
            ("retained", Keep::default()),
            ("unsellable", Keep::default()),
        ] {
            match what {
                "tinkered" => keep.tinkered = true,
                "inscribed" => keep.inscribed = true,
                "equipped" => keep.equipped = true,
                "retained" => keep.retained = true,
                _ => keep.unsellable = true,
            }
            let mut it = item(3, "Sword", 50_000, 1, 1);
            it.keep = keep;
            let mut run = Run::new();
            let next = run.step(&snap(vec![it]), now);
            assert_ne!(
                next.act,
                Some(Act::Sell { items: vec![3] }),
                "offered a {what} item: {}",
                next.saying
            );
        }
    }

    #[test]
    fn a_stack_too_dear_for_the_counter_is_cut_down_rather_than_given_up() {
        let mut run = Run::new();
        // A hundred peas worth five million, at a counter that will not
        // look at anything over a million.
        let mut s = snap(vec![item(4, "Pyreal Pea", 5_000_000, 100, 100)]);
        s.counter.as_mut().unwrap().max_value = 1_000_000;
        assert_eq!(
            run.step(&s, Instant::now()).act,
            Some(Act::Split {
                guid: 4,
                amount: 20
            })
        );
    }

    #[test]
    fn an_item_the_counter_would_not_take_is_not_offered_twice() {
        let now = Instant::now();
        let mut run = Run::new();
        let s = snap(vec![item(3, "Dagger", 500, 1, 1)]);
        assert_eq!(run.step(&s, now).act, Some(Act::Sell { items: vec![3] }));
        // It is still in the pack a moment later: the counter would not
        // have it. Asking again is how a run spends an afternoon.
        let next = run.step(&s, now);
        assert!(
            !matches!(&next.act, Some(Act::Sell { items }) if items.contains(&3)),
            "{}",
            next.saying
        );
    }

    #[test]
    fn what_the_character_came_to_buy_is_not_sold_first() {
        let mut run = Run::new();
        let mut taper = item(1, "Taper", 2_000, 78, 1000);
        taper.wcid = 20631;
        let mut s = snap(vec![taper, item(3, "Dagger", 500, 1, 1)]);
        s.wants = vec![Want {
            wcid: 20631,
            name: "Prismatic Taper".into(),
            short: 500,
            urgent: true,
        }];
        // The dagger, not the tapers, however much they are worth.
        let next = run.step(&s, Instant::now());
        assert_eq!(next.act, Some(Act::Sell { items: vec![3] }), "{}", next.saying);
    }

    #[test]
    fn a_purse_of_notes_is_cashed_before_a_small_bill_is_paid() {
        let mut run = Run::new();
        let mut s = snap(Vec::new());
        s.coin = 0;
        s.notes.insert(250_000, 4);
        s.wants = vec![Want {
            wcid: 20631,
            name: "Prismatic Taper".into(),
            short: 100,
            urgent: true,
        }];
        s.counter.as_mut().unwrap().wares.push(Ware {
            wcid: 20631,
            name: "Prismatic Taper".into(),
            price: 26,
            stock: None,
            burden: 6,
        });
        // 2,600 of bill and not a coin to pay it with.
        let next = run.step(&s, Instant::now());
        assert_eq!(
            next.act,
            Some(Act::Cash {
                face: 250_000,
                count: 1
            }),
            "{}",
            next.saying
        );
    }

    #[test]
    fn a_laden_character_buys_only_what_it_can_carry_home() {
        let mut run = Run::new();
        let mut s = snap(Vec::new());
        s.coin = 10_000_000;
        s.rules.float = 10_000_000;
        // Sixty units of room, and a taper weighs six.
        s.carried = s.capacity * 3 - 60;
        s.wants = vec![Want {
            wcid: 20631,
            name: "Prismatic Taper".into(),
            short: 500,
            urgent: true,
        }];
        s.counter.as_mut().unwrap().wares.push(Ware {
            wcid: 20631,
            name: "Prismatic Taper".into(),
            price: 26,
            stock: None,
            burden: 6,
        });
        let next = run.step(&s, Instant::now());
        assert_eq!(
            next.act,
            Some(Act::Buy {
                wcid: 20631,
                count: 10
            }),
            "{}",
            next.saying
        );
    }

    #[test]
    fn a_counter_is_handed_an_armful_not_one_thing_at_a_time() {
        let mut run = Run::new();
        let s = snap(vec![
            item(1, "Dagger", 500, 1, 1),
            item(2, "Shield", 900, 1, 1),
            item(3, "Helm", 700, 1, 1),
        ]);
        match run.step(&s, Instant::now()).act {
            Some(Act::Sell { items }) => {
                // Dearest first, and all three in one go.
                assert_eq!(items, vec![2, 3, 1]);
            }
            other => panic!("expected an armful, got {other:?}"),
        }
    }

    #[test]
    fn the_armful_stops_where_the_coin_would_fill_the_pack() {
        let mut run = Run::new();
        // Each of these turns into two slots of coin, and there are
        // only a few slots to put it in.
        let mut s = snap(
            (1..=8)
                .map(|g| item(g, "Ingot", 50_000, 1, 1))
                .collect(),
        );
        s.slots_free = 5;
        s.rules.keep_slots = 3;
        match run.step(&s, Instant::now()).act {
            Some(Act::Sell { items }) => {
                assert!(
                    items.len() < 8,
                    "sold the lot with nowhere to put the money: {items:?}"
                );
                assert!(!items.is_empty());
            }
            other => panic!("expected an armful, got {other:?}"),
        }
    }

    #[test]
    fn buying_comes_after_selling_and_stays_inside_the_purse() {
        let mut run = Run::new();
        let mut s = snap(Vec::new());
        s.coin = 600_000;
        s.rules.float = 600_000; // nothing spare to turn into notes
        s.wants = vec![Want {
            wcid: NOTE,
            name: "Mayoi Trade Note".into(),
            short: 10,
            urgent: false,
        }];
        // Nothing to sell, so it buys -- but only what it can pay for:
        // 600,000 buys two at 287,500, not the ten asked for.
        let next = run.step(&s, Instant::now());
        assert_eq!(
            next.act,
            Some(Act::Buy {
                wcid: NOTE,
                count: 2
            }),
            "{}",
            next.saying
        );
        let _ = COIN;
    }
}
