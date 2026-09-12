//! Emptying a corpse.
//!
//! The rules are handed a description of the body and the character
//! standing over it, and answer with one thing to do: walk to it, open
//! it, ask what something is, take something, shut it. They open no
//! socket and keep no clock of their own.
//!
//! Three things cost a live run to learn and are written down here
//! rather than left to be rediscovered:
//!
//! * A corpse is emptied at the speed the server answers. A take is
//!   answered by the item leaving, so the next goes out then -- not
//!   four hundred milliseconds later whatever happened.
//! * Only the things that cannot be judged otherwise are asked about,
//!   and they are asked about together. One identify per item, waited
//!   on in turn, was most of the time a character spent standing over
//!   a body.
//! * It is shut behind us however it ends -- emptied, full, too laden,
//!   or given up on. A corpse left open is one the server still has
//!   the character standing over.

pub mod corpse;
pub mod run;

pub use corpse::{Lying, Open, Verdict, REACH};
pub use run::{Act, Next, Run};
