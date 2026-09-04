//! Wire protocol for talking to an Asheron's Call server (ACE). Sans-IO:
//! [`session::Session`] consumes datagrams and time, and produces datagrams
//! and decoded messages; the caller owns the sockets.

pub mod hash32;
pub mod isaac;
pub mod messages;
pub mod packet;
pub mod session;
pub mod wire;
