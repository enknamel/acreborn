//! Autovendoring: the trip to the shops, decided without a server.
//!
//! A trip to town is the one thing this client does with interacting
//! preconditions over a shared pool -- the purse, the pack's slots,
//! what the character can lift, what this counter stocks, what it will
//! take, and what the player's profile says is worth selling. Every
//! one of those has caused a bug, and nearly every one of those bugs
//! was invisible until a character was standing at a real counter.
//!
//! So the deciding is kept apart from the doing. [`Run::step`] is
//! handed a [`Snapshot`] -- a plain description of the pack, the purse
//! and the counter -- and answers with at most one [`Act`]. It opens no
//! socket, moves nothing, and keeps no clock of its own. That makes the
//! whole of the shopping replayable at a desk, which is where it ought
//! to be argued about, and leaves the client with the easy half: build
//! the snapshot, carry out the act.
//!
//! The order of the trip is fixed and was settled the hard way:
//!
//! 1. **Compress the pack.** Slots are what a sale runs out of, and
//!    loose change wastes them. It is done before anything is sold, and
//!    again at the top of every round, because each sale makes the
//!    character lighter and the server weighs a merge against what is
//!    carried.
//! 2. **Sell until low on room** -- not out of it. Waiting for the last
//!    slot means the next handful of coin has nowhere to go.
//! 3. **Turn the takings into trade notes**, and only into those: a
//!    slot of Mayoi notes carries sixty two million and a slot of coin
//!    carries twenty five thousand.
//! 4. **Round again** until there is nothing left worth selling, then
//!    buy what the character came for.

pub mod counter;
pub mod errand;
pub mod run;

pub use counter::{Counter, Item, Keep, Rules, Snapshot, Want, Ware};
pub use run::{Act, Next, Phase, Run};
