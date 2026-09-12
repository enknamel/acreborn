//! What the character and the counter look like to the vendoring
//! rules.
//!
//! Everything here is a plain value. Nothing reaches back into a world,
//! a socket or a character, which is the whole point: a trip to the
//! shops can then be decided, replayed and argued about without a
//! server being awake, and the same decisions run live.

use std::collections::BTreeMap;

/// One thing in the pack, as the shopping needs to see it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Item {
    pub guid: u32,
    /// Weenie class: what kind of thing it is. Two stacks join only if
    /// these match.
    pub wcid: u32,
    pub name: String,
    /// What the whole stack is worth, which is how the server counts it
    /// and what a counter's ceiling is measured against. A hundred
    /// Pyreal Peas worth five million are refused by a counter that
    /// will not look at anything over a million.
    pub value: u32,
    /// What the whole stack weighs. A side pack's weight already
    /// includes everything inside it.
    pub burden: u32,
    pub stack: u32,
    /// The largest this stack may grow to. Whether two things combine
    /// is this, never how many are in them now: two single tapers are
    /// one stack of two.
    pub max_stack: u32,
    /// `ac_world::item_type` bits.
    pub item_type: u32,
    /// The container it sits in, or `None` for the main pack.
    pub pack: Option<u32>,
    pub wielded: bool,
    /// Why it must not be sold, whatever any rule says.
    pub keep: Keep,
}

/// The reasons an item is not for sale, whatever a profile says about
/// it. These are settled and no policy may override them: a tinkered or
/// inscribed item is somebody's work, an equipped one is being worn,
/// and the last two are the server's own word.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keep {
    pub tinkered: bool,
    pub inscribed: bool,
    pub equipped: bool,
    pub retained: bool,
    pub unsellable: bool,
}

impl Keep {
    /// Whether anything at all forbids selling it, and what.
    pub fn why(&self) -> Option<&'static str> {
        if self.tinkered {
            Some("it has been tinkered")
        } else if self.inscribed {
            Some("it has been inscribed")
        } else if self.equipped {
            Some("it is being worn")
        } else if self.retained {
            Some("it is retained")
        } else if self.unsellable {
            Some("the server marks it unsellable")
        } else {
            None
        }
    }

    pub fn forbidden(&self) -> bool {
        self.why().is_some()
    }
}

impl Item {
    /// What one of them is worth.
    pub fn each(&self) -> u32 {
        self.value / self.stack.max(1)
    }

    /// Whether it stacks at all.
    pub fn stackable(&self) -> bool {
        self.max_stack > 1
    }

    /// Room left in it.
    pub fn room(&self) -> u32 {
        self.max_stack.saturating_sub(self.stack)
    }
}

/// Something a counter has on its shelf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ware {
    pub wcid: u32,
    pub name: String,
    /// What one costs here, the counter's markup included.
    pub price: u32,
    /// How many it has, or `None` for a shelf that never empties.
    pub stock: Option<u32>,
    /// What one of them weighs, or 0 when the counter did not say.
    ///
    /// A purse says how many can be paid for and this says how many can
    /// be carried home. Buying without it is how a character came to be
    /// too laden to buy anything and went on asking anyway.
    pub burden: u32,
}

/// The counter being stood at.
#[derive(Clone, Debug, PartialEq)]
pub struct Counter {
    pub guid: u32,
    pub name: String,
    /// The window is open and it will trade.
    pub open: bool,
    /// `ac_world::item_type` bits it will take.
    pub buys: u32,
    /// The most it will pay for one thing; 0 for no limit.
    pub max_value: u32,
    pub wares: Vec<Ware>,
    /// The face value of the trade note it deals in, when it deals in
    /// them at all. Only these are worth making: coin takes slots and a
    /// note does not.
    pub note_face: Option<u32>,
    /// How far the character is standing from it.
    pub away: f32,
}

/// Something the character wants to leave with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Want {
    pub wcid: u32,
    pub name: String,
    /// How many more are wanted than are carried.
    pub short: u32,
    /// It cannot hunt without this, so the trip is for nothing without
    /// it.
    pub urgent: bool,
}

/// The rules of the trip, as the player set them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    /// Stop selling and make notes with this many slots still free.
    /// Low on room, not out of it: waiting for the last slot means the
    /// next handful of coin has nowhere to go.
    pub keep_slots: u32,
    /// Coin to keep loose rather than turn into notes, so that small
    /// change is payable without breaking one.
    pub float: u32,
    /// Do not bother selling anything worth less than this.
    pub floor_value: u32,
    /// How near the counter the character must stand before it will
    /// trade.
    pub reach: f32,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            keep_slots: 3,
            float: 2_000,
            floor_value: 0,
            reach: 3.0,
        }
    }
}

/// Everything the shopping is decided from, at one moment.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Snapshot {
    pub items: Vec<Item>,
    /// Loose coin.
    pub coin: u32,
    /// Trade notes held, by face value.
    pub notes: BTreeMap<u32, u32>,
    /// Burden carried and the most that may be carried (150 x Strength).
    pub carried: u32,
    pub capacity: u32,
    /// Slots free across the main pack and the side packs.
    pub slots_free: u32,
    pub counter: Option<Counter>,
    pub wants: Vec<Want>,
    pub rules: Rules,
}

impl Snapshot {
    /// What the character can spend: coin and notes both, since a
    /// counter takes either.
    pub fn purse(&self) -> u32 {
        self.notes
            .iter()
            .fold(self.coin, |t, (face, n)| t.saturating_add(face * n))
    }

    /// How much more may be carried before the server starts refusing.
    /// Three times capacity is its hard ceiling.
    pub fn burden_room(&self) -> u32 {
        self.capacity.saturating_mul(3).saturating_sub(self.carried)
    }

    pub fn item(&self, guid: u32) -> Option<&Item> {
        self.items.iter().find(|i| i.guid == guid)
    }
}
