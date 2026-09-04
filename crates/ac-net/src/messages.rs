//! Message opcodes and the bodies needed for login and entering the world.
//! Everything else is delivered to the caller as `(opcode, bytes)`.

use crate::wire::{Reader, Truncated, Writer};

pub mod opcode {
    pub const CHARACTER_ENTER_WORLD: u32 = 0xF657;
    pub const CHARACTER_LIST: u32 = 0xF658;
    pub const CHARACTER_ERROR: u32 = 0xF659;
    pub const OBJECT_CREATE: u32 = 0xF745;
    pub const PLAYER_CREATE: u32 = 0xF746;
    pub const OBJECT_DELETE: u32 = 0xF747;
    pub const UPDATE_POSITION: u32 = 0xF748;
    pub const MOVEMENT_EVENT: u32 = 0xF74C;
    pub const GAME_EVENT: u32 = 0xF7B0;
    pub const GAME_ACTION: u32 = 0xF7B1;
    pub const CHARACTER_ENTER_WORLD_REQUEST: u32 = 0xF7C8;
    pub const ACCOUNT_BOOT: u32 = 0xF7DC;
    pub const UPDATE_OBJECT: u32 = 0xF7DB;
    pub const CHARACTER_ENTER_WORLD_SERVER_READY: u32 = 0xF7DF;
    pub const SERVER_MESSAGE: u32 = 0xF7E0;
    pub const SERVER_NAME: u32 = 0xF7E1;
    pub const DDD_INTERROGATION: u32 = 0xF7E5;
    pub const DDD_INTERROGATION_RESPONSE: u32 = 0xF7E6;
    pub const DDD_BEGIN_DDD: u32 = 0xF7E7;
    pub const DDD_END_DDD: u32 = 0xF7EA;
}

/// Message queues (fragment `queue` field).
pub mod queue {
    pub const EVENT: u16 = 1;
    pub const CONTROL: u16 = 2;
    pub const WEENIE: u16 = 3;
    pub const LOGIN: u16 = 4;
    pub const DATABASE: u16 = 5;
    pub const SECURE_CONTROL: u16 = 6;
    pub const SECURE_WEENIE: u16 = 7;
    pub const SECURE_LOGIN: u16 = 8;
    pub const UI: u16 = 9;
    pub const SMARTBOX: u16 = 10;
}

/// Client version string the end-of-retail client reports.
pub const CLIENT_VERSION: &str = "1802";

/// Body of the LoginRequest packet (sent as the optional header of a packet
/// flagged `LOGIN_REQUEST`, not as a fragment).
pub fn login_request(account: &str, password: &str, timestamp: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.string16(CLIENT_VERSION);
    // Everything after the length dword.
    let mut rest = Writer::new();
    rest.u32(2); // NetAuthType::AccountPassword
    rest.u32(0); // AuthFlags::None
    rest.u32(timestamp);
    rest.string16(account);
    rest.string16(""); // account to log in as (admin override)
                       // "String32L": u32 byte length of what follows, which is a one-byte
                       // (two-byte above 255) length prefix and the bytes. ACE skips the
                       // prefix bytes and reads the rest as the password.
    let mut pw = Writer::new();
    if password.len() > 255 {
        pw.u16(password.len() as u16);
    } else {
        pw.u8(password.len() as u8);
    }
    pw.bytes(password.as_bytes());
    rest.u32(pw.buf.len() as u32);
    rest.bytes(&pw.buf);
    w.u32(rest.buf.len() as u32);
    w.bytes(&rest.buf);
    w.finish()
}

/// Message body for CharacterEnterWorldRequest (0xF7C8): opcode only.
pub fn enter_world_request() -> Vec<u8> {
    Writer::new()
        .u32(opcode::CHARACTER_ENTER_WORLD_REQUEST)
        .clone()
        .finish()
}

/// Message body for CharacterEnterWorld (0xF657).
pub fn enter_world(character_id: u32, account: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(opcode::CHARACTER_ENTER_WORLD)
        .u32(character_id)
        .string16(account);
    w.finish()
}

/// One archive's iteration state for the DDD interrogation response.
#[derive(Debug, Clone, Copy)]
pub struct DatIteration {
    /// 1 = portal/highres, 2 = cell, 3 = language.
    pub dat_file_id: i32,
    /// 0 for the normal archive; the "HiFi" tag for client_highres.dat.
    pub dat_file_type: i32,
    pub iterations: i32,
}

/// DDD_InterrogationResponse (0xF7E6): tells the server which DAT
/// iterations we have. With matching iterations ACE replies DDD_EndDDD.
pub fn ddd_interrogation_response(dats: &[DatIteration]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(opcode::DDD_INTERROGATION_RESPONSE);
    w.u32(1); // language: English
    w.i32(dats.len() as i32);
    for d in dats {
        w.i32(d.dat_file_type).i32(d.dat_file_id);
        // CMostlyConsecutiveIntSet: total, then a single run "-(n+1)"...
        // The client encodes a run of n consecutive iterations starting
        // at 1 as [n, -n?]; ACE only sums runs, so encode one negative run
        // covering all iterations: x < 0 contributes |x| - 1.
        w.i32(d.iterations);
        if d.iterations > 0 {
            w.i32(-(d.iterations + 1));
        }
    }
    w.i32(0); // iterations without keys: none
    w.u32(0); // flags
    w.finish()
}

#[derive(Debug, Clone, PartialEq)]
pub struct CharacterEntry {
    pub id: u32,
    pub name: String,
    pub seconds_until_deleted: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CharacterList {
    pub characters: Vec<CharacterEntry>,
    pub slot_count: u32,
    pub account: String,
    pub use_turbine_chat: bool,
    pub has_throne_of_destiny: bool,
}

impl CharacterList {
    /// Parse the body after the opcode.
    pub fn parse(b: &[u8]) -> Result<Self, Truncated> {
        let mut r = Reader::new(b);
        let _ = r.u32()?;
        let n = r.u32()?;
        let mut characters = Vec::with_capacity(n.min(64) as usize);
        for _ in 0..n {
            characters.push(CharacterEntry {
                id: r.u32()?,
                name: r.string16()?,
                seconds_until_deleted: r.u32()?,
            });
        }
        let _ = r.u32()?;
        let slot_count = r.u32()?;
        let account = r.string16()?;
        let use_turbine_chat = r.u32()? != 0;
        let has_throne_of_destiny = r.u32()? != 0;
        Ok(CharacterList {
            characters,
            slot_count,
            account,
            use_turbine_chat,
            has_throne_of_destiny,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerName {
    pub current_connections: u32,
    pub max_connections: i32,
    pub name: String,
}

impl ServerName {
    pub fn parse(b: &[u8]) -> Result<Self, Truncated> {
        let mut r = Reader::new(b);
        Ok(ServerName {
            current_connections: r.u32()?,
            max_connections: r.i32()?,
            name: r.string16()?,
        })
    }
}

/// Split a message into opcode and body.
pub fn split(msg: &[u8]) -> Option<(u32, &[u8])> {
    if msg.len() < 4 {
        return None;
    }
    Some((
        u32::from_le_bytes([msg[0], msg[1], msg[2], msg[3]]),
        &msg[4..],
    ))
}

/// GameEvent (0xF7B0) header: object guid, sequence, event type; body follows.
pub fn split_game_event(body: &[u8]) -> Option<(u32, u32, u32, &[u8])> {
    let mut r = Reader::new(body);
    let guid = r.u32().ok()?;
    let seq = r.u32().ok()?;
    let ev = r.u32().ok()?;
    Some((guid, seq, ev, r.remaining()))
}

/// Build a GameAction (0xF7B1) message: opcode, sequence, action type, body.
pub fn game_action(sequence: u32, action: u32, body: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(opcode::GAME_ACTION)
        .u32(sequence)
        .u32(action)
        .bytes(body);
    w.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_request_layout() {
        let b = login_request("acct", "pw", 7);
        let mut r = Reader::new(&b);
        assert_eq!(r.string16().unwrap(), "1802");
        let len = r.u32().unwrap() as usize;
        assert_eq!(len, r.remaining().len());
        assert_eq!(r.u32().unwrap(), 2);
        assert_eq!(r.u32().unwrap(), 0);
        assert_eq!(r.u32().unwrap(), 7);
        assert_eq!(r.string16().unwrap(), "acct");
        assert_eq!(r.string16().unwrap(), "");
        let pwlen = r.u32().unwrap() as usize;
        assert_eq!(pwlen, r.remaining().len());
        assert_eq!(r.u8().unwrap(), 2);
        assert_eq!(r.bytes(2).unwrap(), b"pw");
    }

    #[test]
    fn character_list_roundtrip() {
        let mut w = Writer::new();
        w.u32(0)
            .u32(1)
            .u32(0x5000_0001)
            .string16("Bob")
            .u32(0)
            .u32(0)
            .u32(11)
            .string16("acct")
            .u32(1)
            .u32(1);
        let cl = CharacterList::parse(&w.buf).unwrap();
        assert_eq!(cl.characters[0].name, "Bob");
        assert_eq!(cl.slot_count, 11);
        assert_eq!(cl.account, "acct");
    }
}
