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

use crate::did::Because;

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
    /// Turn what is left over, above the float, into trade notes: a
    /// fortune in coin fills a pack and a fortune in notes does not.
    Keep { spend: u32 },
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
                Act::Keep { spend } => *spend,
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

/// Work out what the trip does.
///
/// The order is the argument. Selling comes first because it is what
/// pays for the buying and what makes room to carry it; cashing comes
/// next because a counter takes coin and not notes; buying is held to
/// whichever of purse, weight and slots runs out first; and only what
/// is left over after all of that becomes notes to carry home.
pub fn plan(means: Means, sale: &[ForSale], notes: &[Note], wanted: &[Wanted], float: u32) -> Plan {
    let mut left = means;
    let mut plan = Plan::default();

    // 1. Sell. Takings are coin, the weight comes off, the slots come
    //    back. Everything after this is richer and lighter for it.
    if !sale.is_empty() {
        let takings: u32 = sale.iter().map(|s| s.pays).sum();
        let lighter: u32 = sale.iter().map(|s| s.weighs).sum();
        left.coin = left.coin.saturating_add(takings);
        left.room = left.room.saturating_add(lighter);
        left.slots = left.slots.saturating_add(sale.len() as u32);
        plan.acts.push(Act::Sell {
            items: sale.iter().map(|s| s.guid).collect(),
            takings,
        });
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
    let spare = left.coin.saturating_sub(float);
    if spare > 0 {
        left.coin -= spare;
        left.notes = left.notes.saturating_add(spare);
        plan.acts.push(Act::Keep { spend: spare });
    }

    plan.left = left;
    plan
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
        );
        assert_eq!(bought(&p, "Prismatic Taper"), 500);
        // Seven and a half thousand spent, two and a half left, of
        // which the float keeps two: only the rest travels as notes.
        assert_eq!(p.cost(), 500 * 15 + 500);
        assert!(matches!(p.acts.last(), Some(Act::Keep { spend: 500 })));
        assert_eq!(p.left.coin, FLOAT);
        // Buying comes before converting, always.
        let buy_at = p
            .acts
            .iter()
            .position(|a| matches!(a, Act::Buy { .. }))
            .expect("bought something");
        let keep_at = p
            .acts
            .iter()
            .position(|a| matches!(a, Act::Keep { .. }))
            .expect("kept something");
        assert!(buy_at < keep_at);
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
        let p = plan(means(1_000_000, 0, 100_000), &[], &[], &[scarce], FLOAT);
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
        );
        let said = p.tell();
        assert!(said.contains("sell 1"), "{said}");
        assert!(said.contains("buy 100"), "{said}");
        assert!(said.contains("take 9000"), "{said}");
    }
}
