//! Watch a whole trip to the shops, with no server involved.
//!
//! The rules decide; this plays the part of the counter and the pack,
//! carrying out whatever they ask for and handing back the new state.
//! A trip that loops for ever, sells something it should not, or stops
//! with the pack still full shows up here in a second rather than
//! twenty minutes into a live run.
//!
//! Usage: trip [slots] [coin]
//!   slots  how many free slots the pack starts with (default 6)
//!   coin   loose coin to start with (default 0)

use std::collections::BTreeMap;
use std::time::Instant;

use ac_vendor::counter::{Counter, Item, Keep, Rules, Want, Ware};
use ac_vendor::{Act, Run, Snapshot};

const NOTE_WCID: u32 = 20630;
const NOTE_FACE: u32 = 250_000;

fn thing(guid: u32, name: &str, value: u32, stack: u32, max: u32, wcid: u32) -> Item {
    Item {
        guid,
        wcid,
        name: name.into(),
        value,
        burden: 50 * stack,
        stack,
        max_stack: max,
        item_type: 1,
        pack: None,
        wielded: false,
        keep: Keep::default(),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let slots: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(6);
    let coin: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0);

    let mut keepsake = thing(10, "Tinkered Sword", 40_000, 1, 1, 5000);
    keepsake.keep.tinkered = true;
    let mut worn = thing(11, "Worn Breastplate", 30_000, 1, 1, 5001);
    worn.keep.equipped = true;
    worn.wielded = true;

    let mut snap = Snapshot {
        items: vec![
            thing(1, "Prismatic Taper", 814, 37, 1000, 20631),
            thing(2, "Prismatic Taper", 902, 41, 1000, 20631),
            thing(3, "Steel Long Sword", 12_500, 1, 1, 5002),
            thing(4, "Pyreal Pea", 5_000_000, 100, 100, 8330),
            thing(5, "Chainmail Hauberk", 21_000, 1, 1, 5003),
            keepsake,
            worn,
        ],
        coin,
        notes: BTreeMap::new(),
        carried: 4_000,
        capacity: 30_000,
        slots_free: slots,
        counter: Some(Counter {
            guid: 900,
            name: "Rakk the Peddler".into(),
            open: false,
            buys: 0,
            max_value: 1_000_000,
            wares: vec![
                Ware {
                    wcid: NOTE_WCID,
                    name: "Mayoi Trade Note".into(),
                    price: 287_500,
                    stock: None,
                },
                Ware {
                    wcid: 20631,
                    name: "Prismatic Taper".into(),
                    price: 26,
                    stock: None,
                },
            ],
            note_face: Some(NOTE_FACE),
            away: 14.0,
        }),
        wants: vec![Want {
            wcid: 20631,
            name: "Prismatic Taper".into(),
            short: 500,
            urgent: true,
        }],
        rules: Rules::default(),
    };

    let mut run = Run::new();
    let now = Instant::now();
    let mut next_guid = 100;
    println!(
        "starting: {} item(s), {} slot(s) free, {} coin\n",
        snap.items.len(),
        snap.slots_free,
        snap.coin
    );

    for turn in 1..=60 {
        let next = run.step(&snap, now);
        let Some(act) = next.act.clone() else {
            println!("{turn:>3}. -- {} ({:?})", next.saying, next.did);
            break;
        };
        println!("{turn:>3}. {} {:?}", next.saying, act);
        if act == Act::Close {
            break;
        }
        apply(&mut snap, &act, &mut next_guid);
    }

    println!(
        "\nfinished: {} item(s), {} slot(s) free, {} coin, {} note(s), sold {}",
        snap.items.len(),
        snap.slots_free,
        snap.coin,
        snap.notes.values().sum::<u32>(),
        run.sold
    );
    for it in &snap.items {
        println!("   left: {} x{} ({})", it.name, it.stack, it.value);
    }
}

/// The counter and the pack doing as they are told. Deliberately
/// literal: it grants every request, so that a run which asks for
/// something silly is caught by what the pack looks like afterwards
/// rather than by the simulator being clever.
fn apply(snap: &mut Snapshot, act: &Act, next_guid: &mut u32) {
    match act {
        Act::Approach { .. } => {
            if let Some(c) = snap.counter.as_mut() {
                c.away = 0.0;
            }
        }
        Act::Open { .. } => {
            if let Some(c) = snap.counter.as_mut() {
                c.open = true;
            }
        }
        Act::Merge { from, to, amount } => {
            let taken = snap.item(*from).map(|i| (i.stack, i.value, i.burden));
            if let Some((stack, value, burden)) = taken {
                let moved = (*amount).min(stack);
                let each_v = value / stack.max(1);
                let each_b = burden / stack.max(1);
                if let Some(t) = snap.items.iter_mut().find(|i| i.guid == *to) {
                    t.stack += moved;
                    t.value += each_v * moved;
                    t.burden += each_b * moved;
                }
                if moved == stack {
                    snap.items.retain(|i| i.guid != *from);
                    snap.slots_free += 1;
                } else if let Some(f) = snap.items.iter_mut().find(|i| i.guid == *from) {
                    f.stack -= moved;
                    f.value -= each_v * moved;
                    f.burden -= each_b * moved;
                }
            }
        }
        Act::Split { guid, amount } => {
            let cut = snap.item(*guid).map(|i| {
                let each_v = i.value / i.stack.max(1);
                let each_b = i.burden / i.stack.max(1);
                let mut piece = i.clone();
                piece.guid = *next_guid;
                piece.stack = *amount;
                piece.value = each_v * amount;
                piece.burden = each_b * amount;
                (piece, each_v, each_b)
            });
            if let Some((piece, each_v, each_b)) = cut {
                *next_guid += 1;
                if let Some(src) = snap.items.iter_mut().find(|i| i.guid == *guid) {
                    src.stack -= piece.stack;
                    src.value -= each_v * piece.stack;
                    src.burden -= each_b * piece.stack;
                }
                snap.items.push(piece);
                snap.slots_free = snap.slots_free.saturating_sub(1);
            }
        }
        Act::Sell { guid } => {
            if let Some(i) = snap.item(*guid).cloned() {
                snap.items.retain(|x| x.guid != *guid);
                snap.coin += i.value;
                snap.carried = snap.carried.saturating_sub(i.burden);
                snap.slots_free += 1;
            }
        }
        Act::Buy { wcid, count } => {
            let price = snap
                .counter
                .as_ref()
                .and_then(|c| c.wares.iter().find(|w| w.wcid == *wcid))
                .map(|w| w.price)
                .unwrap_or(0);
            let bill = price.saturating_mul(*count);
            snap.coin = snap.coin.saturating_sub(bill);
            if *wcid == NOTE_WCID {
                *snap.notes.entry(NOTE_FACE).or_insert(0) += count;
                snap.slots_free = snap.slots_free.saturating_sub(1);
            } else {
                snap.items.push(Item {
                    guid: *next_guid,
                    wcid: *wcid,
                    name: "bought".into(),
                    value: bill,
                    burden: 6 * count,
                    stack: *count,
                    max_stack: 1000,
                    item_type: 1,
                    pack: None,
                    wielded: false,
                    keep: Keep::default(),
                });
                *next_guid += 1;
                snap.slots_free = snap.slots_free.saturating_sub(1);
            }
            // A want that has been met is no longer short.
            for w in snap.wants.iter_mut() {
                if w.wcid == *wcid {
                    w.short = w.short.saturating_sub(*count);
                }
            }
        }
        Act::Close => {}
    }
}
