//! Getting a character from where it is to where it wants to be.
//!
//! Four questions, and they are not the same question:
//!
//! * Can I stand there, and can I walk from here to there? That is
//!   geometry, and it lives in `ac-scene` with the triangles.
//! * Which way round what is in between? That is [`steering`], a route
//!   through one landblock, re-planned as the character moves.
//! * Is this a walk at all, or a journey of portals and recalls? That
//!   is [`means`], and it is arithmetic over two positions.
//! * What do I do when there is no way? That is the one that kept
//!   being got wrong.
//!
//! The last is worth saying plainly, because it cost a day. Walking at
//! a goal you have just decided is unreachable is not a fallback, it is
//! a character pressed against a wall with its legs going. The rules
//! here answer [`Aim::NoWay`] instead, and the caller chooses something
//! else -- another road, another shop, another target.
//!
//! Nothing here opens a socket or touches a character. What it needs of
//! the world it asks for through [`Ground`], which a test can answer
//! with a few rectangles: every navigation fault found the hard way in
//! a live dungeon is a unit test in this crate now.

pub mod means;
pub mod steering;

pub use means::{how_to_get_there, Means, WALKABLE};
pub use steering::{Aim, Ground, Steering};
