//! What a trip to a counter will actually do, worked out before it is
//! done.
//!
//! A trip to town is the one goal with interacting preconditions over a
//! shared pool. Money buys components; selling makes money; selling also
//! frees slots and lifts weight; trade notes turn money into something
//! weightless but cost thirteen per cent to make and unmake; and every
//! purchase is held back by whichever of purse, weight and slots runs
//! out first.
//!
//! Every bug this subsystem has had came from that. Buying without
//! counting the weight it would add. Turning the money into notes and
//! then being unable to pay. Shopping again and again at a counter with
//! nothing affordable on it. Each was fixed where it was found, which
//! is to say each was fixed once, in one order, for one case.
//!
//! So the order is not written out any more. [`plan`] walks the
//! resources forward -- sell, cash, buy, convert -- and each act takes
//! from the pool what it costs and gives back what it yields. "Cannot
//! buy because too laden, therefore sell first" is not a rule here: it
//! is what falls out of selling happening before buying and of weight
//! being counted. And because the whole thing is arithmetic over a
//! plain state, it can be run before the character leaves, which is how
//! a trip is known to be worth making.
//!
//! See `docs/agent.md`.

use ac_agent::did::Because;

/// What a character has to spend on a trip, and what it has room for.
///
/// Coin and notes are kept apart because a counter takes coin: a purse
/// of notes is a rich character who cannot pay for anything until it
/// has cashed some.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Means {
    /// Spendable at the counter, now.
    pub coin: u32,
    /// Face value of the trade notes carried.
    pub notes: u32,
    /// Burden units the character can still be handed.
    pub room: u32,
    /// Pack slots free.
    pub slots: u32,
}

impl Means {
    /// Everything the purse is worth, cashed or not.
    pub fn purse(&self) -> u32 {
        self.coin.saturating_add(self.notes)
    }
}

/// Something in the pack that this counter would buy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForSale {
    pub guid: u32,
    /// What the counter pays for the whole stack.
    pub pays: u32,
    /// What carrying it costs, which selling gives back.
    pub weighs: u32,
}

/// A note in the pack, and what cashing it is worth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub guid: u32,
    /// A note pays its face value back; only making one costs.
    pub face: u32,
}

/// A line on the shelf that answers something the character wants.
#[derive(Clone, Debug, PartialEq)]
pub struct Wanted {
    /// The vendor's stock line to buy from.
    pub line: u32,
    pub name: String,
    /// How many are wanted.
    pub want: u32,
    /// What one costs and what one weighs.
    pub each: u32,
    pub weighs: u32,
    /// How many the shelf has; `None` for an endless supply.
    pub stock: Option<u32>,
}

/// One thing the trip does, in the order it does it.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// Hand these over. Always first: it is what pays for everything
    /// after it, and what makes room to carry what is bought.
    Sell { items: Vec<u32>, takings: u32 },
    /// Turn these notes back into coin, because the counter takes coin.
    Cash { notes: Vec<u32>, coin: u32 },
    /// Buy this many off this line.
    Buy {
        line: u32,
        name: String,
        amount: u32,
        cost: u32,
    },
    /// Turn coin into trade notes: `count` of them at this face,
    /// costing `spend`.
    ///
    /// Not only a tidy-up at the end. A sale of any size has to stop
    /// part way and do this, because the coin it has taken in is
    /// filling the pack (see [`COIN_STACK`]).
    Keep { face: u32, count: u32, spend: u32 },
}

/// What the trip will do, what it will leave behind, and what it will
/// not manage.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub acts: Vec<Act>,
    /// The means as the trip would leave them.
    pub left: Means,
    /// What was wanted and will not be had, and why not.
    pub unmet: Vec<(String, u32, Because)>,
}

impl Plan {
    /// Whether the trip does anything at all. A plan that neither sells
    /// nor buys is a walk to town for its own sake.
    pub fn worth_going(&self) -> bool {
        self.acts
            .iter()
            .any(|a| !matches!(a, Act::Keep { .. } | Act::Cash { .. }))
    }

    /// What it will spend.
    pub fn cost(&self) -> u32 {
        self.acts
            .iter()
            .map(|a| match a {
                Act::Buy { cost, .. } => *cost,
                Act::Keep { spend, .. } => *spend,
                _ => 0,
            })
            .sum()
    }

    /// What it will take in.
    pub fn takings(&self) -> u32 {
        self.acts
            .iter()
            .map(|a| match a {
                Act::Sell { takings, .. } => *takings,
                _ => 0,
            })
            .sum()
    }

    /// Said in a line, for the log.
    pub fn tell(&self) -> String {
        let bought: u32 = self
            .acts
            .iter()
            .filter_map(|a| match a {
                Act::Buy { amount, .. } => Some(*amount),
                _ => None,
            })
            .sum();
        let sold = self
            .acts
            .iter()
            .filter_map(|a| match a {
                Act::Sell { items, .. } => Some(items.len()),
                _ => None,
            })
            .sum::<usize>();
        let mut out = format!("sell {sold}, buy {bought}");
        if self.takings() > 0 {
            out.push_str(&format!(", take {}", self.takings()));
        }
        if self.cost() > 0 {
            out.push_str(&format!(", spend {}", self.cost()));
        }
        if !self.unmet.is_empty() {
            out.push_str(&format!("; {} line(s) short", self.unmet.len()));
        }
        out
    }
}

/// How much coin to keep back rather than turn into notes. Notes cost
/// to make and unmake, so a character keeps enough to shop with.
pub const FLOAT: u32 = 2_000;

/// How many pyreals go in one pack slot, and how many trade notes do.
///
/// This is the whole reason selling is not one act. Coin weighs
/// nothing, so weight never stops a sale -- but twenty-five thousand
/// pyreals fill a slot, and a note of a quarter of a million stacks two
/// hundred and fifty deep. One slot of notes holds sixty-two and a half
/// million; one slot of coin holds twenty-five thousand. Sell a serious
/// pile of loot without converting and the pack is full of change
/// before half of it has gone over the counter.
pub const COIN_STACK: u32 = 25_000;
pub const NOTE_STACK: u32 = 250;

/// What a counter charges to make a trade note, as a multiple of its
/// face. The server fixes this and pays back only face, so a purse
/// turned into notes and back is thirteen per cent lighter.
pub const NOTE_MARKUP: f32 = 1.15;

/// How many slots this much coin takes up.
pub fn coin_slots(coin: u32) -> u32 {
    coin.div_ceil(COIN_STACK)
}

/// What one note of this face costs to make.
pub fn note_cost(face: u32) -> u32 {
    (face as f32 * NOTE_MARKUP).ceil() as u32
}

/// Work out what the trip does.
///
/// The order is the argument. Selling comes first because it is what
/// pays for the buying and what makes room to carry it; cashing comes
/// next because a counter takes coin and not notes; buying is held to
/// whichever of purse, weight and slots runs out first; and only what
/// is left over after all of that becomes notes to carry home.
///
/// Selling is not one act. Coin fills slots as it comes in, so a sale
/// of any size runs out of pack before it runs out of loot, and has to
/// stop and turn what it has taken into notes before it can go on.
/// That is a round; a big pile of loot takes several. `note_face` is
/// the face of the Mayoi note this counter makes, if it makes one --
/// without it the sale simply stops when the pack is full, which is
/// what it should do rather than offer things it cannot be paid for.
pub fn plan(
    means: Means,
    sale: &[ForSale],
    notes: &[Note],
    wanted: &[Wanted],
    float: u32,
    note_face: Option<u32>,
) -> Plan {
    let mut left = means;
    let mut plan = Plan::default();

    // 1. Sell, in rounds, converting the takings whenever they have
    //    filled the pack.
    let mut to_sell: Vec<&ForSale> = sale.iter().collect();
    while !to_sell.is_empty() {
        let before = to_sell.len();
        let mut batch = Vec::new();
        let mut takings = 0;
        let mut lighter = 0;
        // Room for the coin, in slots. Each item sold frees its own
        // slot as it goes, which is why this is worked out as it goes
        // rather than up front.
        let mut slots = left.slots;
        let mut coin = left.coin;
        while let Some(item) = to_sell.first() {
            let would = coin.saturating_add(item.pays);
            // Selling frees the item's own slot and gives back the
            // slots its coin was in; the coin it pays takes some. The
            // question is whether what comes back covers what goes out.
            let have = slots + 1 + coin_slots(coin);
            let needs = coin_slots(would);
            if have < needs {
                break;
            }
            let after = have - needs;
            batch.push(item.guid);
            takings += item.pays;
            lighter += item.weighs;
            coin = would;
            slots = after;
            to_sell.remove(0);
        }
        if !batch.is_empty() {
            left.coin = coin;
            left.slots = slots;
            left.room = left.room.saturating_add(lighter);
            plan.acts.push(Act::Sell {
                items: batch,
                takings,
            });
        }
        if to_sell.is_empty() {
            break;
        }
        // The pack is full of change and there is still loot to sell.
        // Convert, which is the only thing that makes room.
        if !convert(&mut left, &mut plan, note_face, float) || to_sell.len() == before {
            // Nothing to convert with, or converting freed nothing: the
            // rest of the loot goes home again.
            let short = to_sell.len() as u32;
            plan.unmet.push((
                "loot to sell".to_string(),
                short,
                Because::ours("no room for the money it would make"),
            ));
            break;
        }
    }

    // 2. What the shopping will cost, as far as it can be afforded at
    //    all. Cashing notes is worth doing only for what will be spent.
    let bill = affordable_bill(&left, wanted);
    if bill > left.coin && !notes.is_empty() {
        let short = bill - left.coin;
        let mut cashed = Vec::new();
        let mut coin = 0;
        // Smallest first, so that no more of the fortune is unpacked
        // than the shopping needs.
        let mut by_size: Vec<&Note> = notes.iter().collect();
        by_size.sort_by_key(|n| n.face);
        for note in by_size {
            if coin >= short {
                break;
            }
            coin += note.face;
            cashed.push(note.guid);
        }
        if !cashed.is_empty() {
            left.coin = left.coin.saturating_add(coin);
            left.notes = left.notes.saturating_sub(coin);
            plan.acts.push(Act::Cash {
                notes: cashed,
                coin,
            });
        }
    }

    // 3. Buy, in the order asked, each line held to whatever runs out
    //    first. This is where "too laden to buy" stops being a rule
    //    somebody has to remember to write.
    for want in wanted {
        if want.want == 0 {
            continue;
        }
        let mut amount = want.want;
        let mut stopped: Option<Because> = None;
        if let Some(on_shelf) = want.stock {
            if on_shelf < amount {
                amount = on_shelf;
                stopped = Some(Because::ours("the shelf has no more"));
            }
        }
        // A free line is held back by nothing; a weightless one by
        // nothing either. Both happen -- a trade note weighs nothing,
        // which is the whole reason a fortune travels as notes.
        if let Some(afford) = left.coin.checked_div(want.each) {
            if afford < amount {
                amount = afford;
                stopped = Some(Because::ours("not enough money"));
            }
        }
        if let Some(carry) = left.room.checked_div(want.weighs) {
            if carry < amount {
                amount = carry;
                stopped = Some(Because::ours("too laden to carry any more"));
            }
        }
        if amount == 0 {
            plan.unmet.push((
                want.name.clone(),
                want.want,
                stopped.unwrap_or_else(|| Because::ours("nothing on the shelf")),
            ));
            continue;
        }
        let cost = amount.saturating_mul(want.each);
        left.coin = left.coin.saturating_sub(cost);
        left.room = left.room.saturating_sub(amount.saturating_mul(want.weighs));
        plan.acts.push(Act::Buy {
            line: want.line,
            name: want.name.clone(),
            amount,
            cost,
        });
        if amount < want.want {
            plan.unmet.push((
                want.name.clone(),
                want.want - amount,
                stopped.unwrap_or_else(|| Because::ours("not all of it was to be had")),
            ));
        }
    }

    // 4. Whatever is left above the float travels home as notes. This
    //    is last on purpose: the money the shopping needed has already
    //    been spent, so nothing here can leave the character unable to
    //    pay -- which is exactly what it used to do.
    convert(&mut left, &mut plan, note_face, float);

    plan.left = left;
    plan
}

/// Turn the coin above the float into trade notes, and say whether any
/// were made.
///
/// A note costs more than its face, so this always loses a little; it
/// is worth it because a slot of notes holds two and a half thousand
/// times what a slot of coin does.
fn convert(left: &mut Means, plan: &mut Plan, face: Option<u32>, float: u32) -> bool {
    // One denomination, and it is the largest: the Mayoi note of a
    // quarter of a million. No smaller one is worth making. Every note
    // costs fifteen per cent of its face whatever it is worth, so the
    // small ones pay the same toll to carry a fraction as much, and a
    // purse turned into hundred-notes is a purse that has paid to fill
    // its own pack.
    let Some(face) = face.filter(|f| *f > 0) else {
        return false;
    };
    let each = note_cost(face).max(1);
    let count = left.coin.saturating_sub(float) / each;
    if count == 0 {
        return false;
    }
    // And only when it frees a slot, which the Mayoi always does: a
    // quarter of a million in coin is ten slots and the note it becomes
    // is a fraction of one.
    let spend = count * each;
    if spend < COIN_STACK {
        return false;
    }
    let before = coin_slots(left.coin);
    left.coin -= spend;
    left.notes = left.notes.saturating_add(count * face);
    // Notes take a slot for every two hundred and fifty of them; the
    // coin gives back every slot it was sitting in.
    left.slots = left.slots.saturating_sub(count.div_ceil(NOTE_STACK));
    left.slots = left.slots + before - coin_slots(left.coin);
    plan.acts.push(Act::Keep { face, count, spend });
    true
}

/// What the wanted lines would cost if bought as far as the purse and
/// the weight allow. Used to decide how many notes to cash, so that a
/// fortune is not unpacked for a bill the character cannot pay anyway.
fn affordable_bill(means: &Means, wanted: &[Wanted]) -> u32 {
    let mut room = means.room;
    let mut bill: u32 = 0;
    for want in wanted {
        let mut amount = want.want;
        if let Some(on_shelf) = want.stock {
            amount = amount.min(on_shelf);
        }
        if let Some(carry) = room.checked_div(want.weighs) {
            amount = amount.min(carry);
        }
        room = room.saturating_sub(amount.saturating_mul(want.weighs));
        bill = bill.saturating_add(amount.saturating_mul(want.each));
    }
    bill
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The only note worth making: the Mayoi, a quarter of a million,
    /// two hundred and fifty to a stack.
    const MMD: Option<u32> = Some(250_000);

    fn means(coin: u32, notes: u32, room: u32) -> Means {
        Means {
            coin,
            notes,
            room,
            slots: 20,
        }
    }

    fn want(line: u32, name: &str, n: u32, each: u32, weighs: u32) -> Wanted {
        Wanted {
            line,
            name: name.into(),
            want: n,
            each,
            weighs,
            stock: None,
        }
    }

    fn bought(plan: &Plan, name: &str) -> u32 {
        plan.acts
            .iter()
            .filter_map(|a| match a {
                Act::Buy {
                    name: n, amount, ..
                } if n == name => Some(*amount),
                _ => None,
            })
            .sum()
    }

    #[test]
    fn selling_comes_first_because_it_pays_for_the_rest() {
        // Not a penny, but a pack of loot. The old code asked whether
        // it could afford the shopping before it had sold anything and
        // concluded it could not.
        let loot = [
            ForSale {
                guid: 1,
                pays: 5_000,
                weighs: 300,
            },
            ForSale {
                guid: 2,
                pays: 3_000,
                weighs: 200,
            },
        ];
        let p = plan(
            means(0, 0, 100),
            &loot,
            &[],
            &[want(9, "Lead Scarab", 100, 50, 10)],
            FLOAT,
            MMD,
        );
        assert!(matches!(p.acts.first(), Some(Act::Sell { .. })));
        assert_eq!(p.takings(), 8_000);
        // And the weight the loot was taking is weight the scarabs can
        // use: a hundred at ten each needs a thousand, and only five
        // hundred of that came free, so it buys what it can carry.
        assert_eq!(bought(&p, "Lead Scarab"), 60);
        assert!(p
            .unmet
            .iter()
            .any(|(n, _, b)| n == "Lead Scarab" && b.what.contains("laden")));
    }

    #[test]
    fn weight_holds_an_order_back_as_surely_as_money_does() {
        // Rich and full. This is the character that asked for scarabs,
        // was refused, and asked again on the next trip for ever.
        let p = plan(
            means(1_000_000, 0, 0),
            &[],
            &[],
            &[want(9, "Lead Scarab", 100, 5, 10)],
            FLOAT,
            MMD,
        );
        assert_eq!(bought(&p, "Lead Scarab"), 0);
        assert!(!p.worth_going(), "nothing to sell and nothing it can lift");
        assert_eq!(
            p.unmet.first().map(|(_, n, b)| (*n, b.what.as_str())),
            Some((100, "too laden to carry any more"))
        );

        // Room for a quarter of it buys a quarter of it.
        let p = plan(
            means(1_000_000, 0, 250),
            &[],
            &[],
            &[want(9, "Lead Scarab", 100, 5, 10)],
            FLOAT,
            MMD,
        );
        assert_eq!(bought(&p, "Lead Scarab"), 25);
    }

    #[test]
    fn the_money_the_shopping_needs_is_not_turned_into_notes() {
        // The bug this whole module exists to make impossible: the
        // trip turned its coin into trade notes and then could not pay
        // the bill.
        let p = plan(
            means(10_000, 0, 100_000),
            &[],
            &[],
            &[want(9, "Prismatic Taper", 500, 15, 1)],
            FLOAT,
            MMD,
        );
        assert_eq!(bought(&p, "Prismatic Taper"), 500);
        // Seven and a half thousand spent, two and a half left. The
        // float keeps two thousand, and the five hundred over it is
        // left as change: it would buy four hundred-notes, lose sixty
        // doing it, and free no slot at all, since five hundred coins
        // and four notes both sit in one.
        assert_eq!(p.cost(), 500 * 15);
        assert!(!p.acts.iter().any(|a| matches!(a, Act::Keep { .. })));
        assert_eq!(p.left.coin, 2_500);
        // Buying comes before converting, always. With a fortune in
        // hand rather than pocket change, both happen and in that
        // order.
        let rich = plan(
            means(1_000_000, 0, 100_000),
            &[],
            &[],
            &[want(9, "Prismatic Taper", 500, 15, 1)],
            FLOAT,
            MMD,
        );
        let buy_at = rich
            .acts
            .iter()
            .position(|a| matches!(a, Act::Buy { .. }))
            .expect("bought something");
        let keep_at = rich
            .acts
            .iter()
            .position(|a| matches!(a, Act::Keep { .. }))
            .expect("kept something");
        assert!(buy_at < keep_at, "the tapers are paid for first");
        // And the tapers were all bought before any of it was packed
        // away, which is the bug this guards.
        assert_eq!(bought(&rich, "Prismatic Taper"), 500);
    }

    #[test]
    fn a_big_sale_stops_to_convert_and_goes_on_selling() {
        // Forty pieces of loot at fifty thousand each: two million
        // pyreals, which is eighty slots of coin. The character has
        // ten slots. Selling it in one go is impossible and always was;
        // the old plan said it would sell the lot and then wondered why
        // the counter stopped taking things.
        let loot: Vec<ForSale> = (0..40)
            .map(|i| ForSale {
                guid: 100 + i,
                pays: 50_000,
                weighs: 100,
            })
            .collect();
        let p = plan(
            Means {
                coin: 0,
                notes: 0,
                room: 1_000_000,
                slots: 10,
            },
            &loot,
            &[],
            &[],
            FLOAT,
            MMD,
        );

        // It sells in rounds, converting in between.
        let sells = p
            .acts
            .iter()
            .filter(|a| matches!(a, Act::Sell { .. }))
            .count();
        let keeps = p
            .acts
            .iter()
            .filter(|a| matches!(a, Act::Keep { .. }))
            .count();
        assert!(sells > 1, "one sale of forty was never going to work");
        assert!(keeps >= 1, "and it has to convert to carry on");

        // Every piece of loot goes.
        let sold: usize = p
            .acts
            .iter()
            .filter_map(|a| match a {
                Act::Sell { items, .. } => Some(items.len()),
                _ => None,
            })
            .sum();
        assert_eq!(sold, 40, "the whole pile went over the counter");
        assert!(p.unmet.is_empty(), "nothing was carried home again");

        // And the takings came home as notes, not as eighty slots of
        // change.
        assert!(p.left.notes > 1_000_000, "{} in notes", p.left.notes);
        // What is left is less than one note costs, which is as far as
        // packing away can go when the Mayoi is the only note worth
        // making: it comes in lumps of two hundred and eighty-seven
        // thousand five hundred and there is no smaller lump.
        assert!(
            p.left.coin < note_cost(250_000) + FLOAT,
            "{} left as coin",
            p.left.coin
        );
    }

    #[test]
    fn a_counter_that_makes_no_notes_sells_what_it_can_and_says_so() {
        // Nowhere to put the money means the sale stops when the pack
        // does. Better to say that than to keep offering things the
        // character cannot be paid for.
        let loot: Vec<ForSale> = (0..40)
            .map(|i| ForSale {
                guid: 100 + i,
                pays: 50_000,
                weighs: 100,
            })
            .collect();
        let p = plan(
            Means {
                coin: 0,
                notes: 0,
                room: 1_000_000,
                slots: 4,
            },
            &loot,
            &[],
            &[],
            FLOAT,
            None,
        );
        let sold: usize = p
            .acts
            .iter()
            .filter_map(|a| match a {
                Act::Sell { items, .. } => Some(items.len()),
                _ => None,
            })
            .sum();
        assert!(sold > 0, "it sold what it had room for");
        assert!(sold < 40, "and not a penny more");
        assert!(
            p.unmet
                .iter()
                .any(|(what, _, why)| what == "loot to sell" && why.what.contains("no room")),
            "and said why the rest came home"
        );
    }

    #[test]
    fn notes_are_cashed_for_the_shopping_and_no_further() {
        // A fortune in notes and nothing spendable. It cashes what the
        // bill needs, smallest first, and leaves the rest packed.
        let notes = [
            Note { guid: 1, face: 250 },
            Note {
                guid: 2,
                face: 5_000,
            },
            Note {
                guid: 3,
                face: 50_000,
            },
        ];
        let p = plan(
            means(0, 55_250, 100_000),
            &[],
            &notes,
            &[want(9, "Lead Scarab", 100, 40, 10)],
            FLOAT,
            MMD,
        );
        let cashed = p.acts.iter().find_map(|a| match a {
            Act::Cash { notes, coin } => Some((notes.clone(), *coin)),
            _ => None,
        });
        let (cashed, coin) = cashed.expect("cashed something");
        assert_eq!(
            coin, 5_250,
            "the small ones covered a bill of four thousand"
        );
        assert_eq!(cashed, vec![1, 2], "the fifty thousand stayed a note");
        assert_eq!(bought(&p, "Lead Scarab"), 100);
        // Cashing comes before buying, or the counter is handed paper.
        let cash_at = p
            .acts
            .iter()
            .position(|a| matches!(a, Act::Cash { .. }))
            .expect("cashed");
        let buy_at = p
            .acts
            .iter()
            .position(|a| matches!(a, Act::Buy { .. }))
            .expect("bought");
        assert!(cash_at < buy_at);
    }

    #[test]
    fn a_shelf_that_runs_out_is_said_so_rather_than_asked_again() {
        let mut scarce = want(9, "Mana Scarab", 100, 15_000, 5);
        scarce.stock = Some(6);
        let p = plan(
            means(1_000_000, 0, 100_000),
            &[],
            &[],
            &[scarce],
            FLOAT,
            MMD,
        );
        assert_eq!(bought(&p, "Mana Scarab"), 6);
        assert_eq!(
            p.unmet.first().map(|(_, n, b)| (*n, b.what.as_str())),
            Some((94, "the shelf has no more"))
        );
    }

    #[test]
    fn a_trip_that_does_nothing_says_so_before_it_is_walked() {
        // No money, nothing to sell: whatever is on the shelf, there is
        // no reason to go.
        let p = plan(
            means(0, 0, 100_000),
            &[],
            &[],
            &[want(9, "Lead Scarab", 100, 50, 10)],
            FLOAT,
            MMD,
        );
        assert!(!p.worth_going());
        assert_eq!(p.cost(), 0);
        assert!(p.unmet.iter().any(|(_, _, b)| b.what.contains("money")));

        // Something to sell is reason enough, even with nothing to buy.
        let p = plan(
            means(0, 0, 100_000),
            &[ForSale {
                guid: 1,
                pays: 40,
                weighs: 10,
            }],
            &[],
            &[],
            FLOAT,
            MMD,
        );
        assert!(p.worth_going());
        assert_eq!(p.takings(), 40);
    }

    #[test]
    fn the_whole_errand_reads_as_one_line() {
        let p = plan(
            means(0, 0, 100_000),
            &[ForSale {
                guid: 1,
                pays: 9_000,
                weighs: 100,
            }],
            &[],
            &[want(9, "Lead Scarab", 100, 40, 10)],
            FLOAT,
            MMD,
        );
        let said = p.tell();
        assert!(said.contains("sell 1"), "{said}");
        assert!(said.contains("buy 100"), "{said}");
        assert!(said.contains("take 9000"), "{said}");
    }
}
