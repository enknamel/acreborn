//! Message opcodes and the bodies needed for login and entering the world.
//! Everything else is delivered to the caller as `(opcode, bytes)`.

use crate::wire::{Reader, Truncated, Writer};

pub mod opcode {
    pub const CHARACTER_CREATE_RESPONSE: u32 = 0xF643;
    pub const CHARACTER_CREATE: u32 = 0xF656;
    pub const CHARACTER_ENTER_WORLD: u32 = 0xF657;
    pub const CHARACTER_LIST: u32 = 0xF658;
    pub const CHARACTER_ERROR: u32 = 0xF659;
    pub const OBJECT_CREATE: u32 = 0xF745;
    pub const PLAYER_CREATE: u32 = 0xF746;
    pub const OBJECT_DELETE: u32 = 0xF747;
    pub const UPDATE_POSITION: u32 = 0xF748;
    pub const PLAYER_TELEPORT: u32 = 0xF751;
    pub const MOVEMENT_EVENT: u32 = 0xF74C;
    pub const GAME_EVENT: u32 = 0xF7B0;
    pub const GAME_ACTION: u32 = 0xF7B1;
    pub const CHARACTER_ENTER_WORLD_REQUEST: u32 = 0xF7C8;
    pub const ACCOUNT_BOOT: u32 = 0xF7DC;
    pub const UPDATE_OBJECT: u32 = 0xF7DB;
    pub const CHARACTER_ENTER_WORLD_SERVER_READY: u32 = 0xF7DF;
    pub const EMOTE_TEXT: u32 = 0x01E0;
    pub const SET_STACK_SIZE: u32 = 0x0197;
    pub const PUBLIC_UPDATE_INSTANCE_ID: u32 = 0x02DA;
    pub const PICKUP_EVENT: u32 = 0xF74A;
    pub const OBJ_DESC_EVENT: u32 = 0xF625;
    pub const PUBLIC_UPDATE_PROPERTY_INT: u32 = 0x02CE;
    pub const PRIVATE_UPDATE_PROPERTY_INT: u32 = 0x02CD;
    pub const PRIVATE_UPDATE_PROPERTY_INT64: u32 = 0x02CF;
    pub const PRIVATE_UPDATE_PROPERTY_STRING: u32 = 0x02D5;
    pub const PRIVATE_UPDATE_ATTRIBUTE: u32 = 0x02E3;
    pub const PRIVATE_UPDATE_VITAL: u32 = 0x02E7;
    pub const PRIVATE_UPDATE_ATTRIBUTE_2ND_LEVEL: u32 = 0x02E9;
    pub const HEAR_SPEECH: u32 = 0x02BB;
    pub const HEAR_RANGED_SPEECH: u32 = 0x02BC;
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

/// Appearance choices for character creation (indices into the CharGen
/// option lists, hues in 0..1).
#[derive(Debug, Clone, Default)]
pub struct Appearance {
    pub eyes: u32,
    pub nose: u32,
    pub mouth: u32,
    pub hair_color: u32,
    pub eye_color: u32,
    pub hair_style: u32,
    /// `u32::MAX` = no headgear.
    pub headgear_style: u32,
    pub headgear_color: u32,
    pub shirt_style: u32,
    pub shirt_color: u32,
    pub pants_style: u32,
    pub pants_color: u32,
    pub footwear_style: u32,
    pub footwear_color: u32,
    pub skin_hue: f64,
    pub hair_hue: f64,
    pub headgear_hue: f64,
    pub shirt_hue: f64,
    pub pants_hue: f64,
    pub footwear_hue: f64,
}

#[derive(Debug, Clone)]
pub struct CharacterCreate {
    pub account: String,
    pub name: String,
    /// 1 = Aluvian, 2 = Gharu'ndim, 3 = Sho, ...
    pub heritage: u32,
    /// 1 = male, 2 = female.
    pub gender: u32,
    pub appearance: Appearance,
    /// Index into the heritage's template list.
    pub template: i32,
    pub strength: u32,
    pub endurance: u32,
    pub coordination: u32,
    pub quickness: u32,
    pub focus: u32,
    pub self_: u32,
    pub slot: u32,
    /// Advancement class per skill id, 55 entries: 0 inactive, 1 untrained,
    /// 2 trained, 3 specialized.
    pub skills: Vec<u32>,
    /// Index into CharGen starter areas.
    pub start_area: u32,
}

impl CharacterCreate {
    /// Message body for CharacterCreate (0xF656).
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32(opcode::CHARACTER_CREATE).string16(&self.account);
        w.u32(1).u32(self.heritage).u32(self.gender);
        let a = &self.appearance;
        w.u32(a.eyes)
            .u32(a.nose)
            .u32(a.mouth)
            .u32(a.hair_color)
            .u32(a.eye_color)
            .u32(a.hair_style)
            .u32(a.headgear_style)
            .u32(a.headgear_color)
            .u32(a.shirt_style)
            .u32(a.shirt_color)
            .u32(a.pants_style)
            .u32(a.pants_color)
            .u32(a.footwear_style)
            .u32(a.footwear_color)
            .f64(a.skin_hue)
            .f64(a.hair_hue)
            .f64(a.headgear_hue)
            .f64(a.shirt_hue)
            .f64(a.pants_hue)
            .f64(a.footwear_hue);
        w.i32(self.template)
            .u32(self.strength)
            .u32(self.endurance)
            .u32(self.coordination)
            .u32(self.quickness)
            .u32(self.focus)
            .u32(self.self_)
            .u32(self.slot)
            .u32(0); // class id
        w.u32(self.skills.len() as u32);
        for &s in &self.skills {
            w.u32(s);
        }
        w.string16(&self.name).u32(self.start_area).u32(0).u32(0);
        w.finish()
    }
}

/// CharacterCreateResponse (0xF643) body.
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterCreateResponse {
    /// 1 = ok, 2 = pending, 3 = name in use, 4 = name banned, 5 = corrupt,
    /// 6 = database down, 7 = admin privilege denied.
    pub response: u32,
    pub guid: u32,
    pub name: String,
}

impl CharacterCreateResponse {
    pub fn parse(b: &[u8]) -> Result<Self, Truncated> {
        let mut r = Reader::new(b);
        let response = r.u32()?;
        if response == 1 {
            let guid = r.u32()?;
            let name = r.string16()?;
            Ok(CharacterCreateResponse {
                response,
                guid,
                name,
            })
        } else {
            Ok(CharacterCreateResponse {
                response,
                guid: 0,
                name: String::new(),
            })
        }
    }
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

/// GameEvent (0xF7B0) sub-opcodes.
pub mod event {
    pub const POPUP_STRING: u32 = 0x0004;
    pub const PLAYER_DESCRIPTION: u32 = 0x0013;
    pub const INVENTORY_PUT_OBJ_IN_CONTAINER: u32 = 0x0022;
    pub const WIELD_OBJECT: u32 = 0x0023;
    pub const INVENTORY_PUT_OBJECT_IN_3D: u32 = 0x019A;
    pub const VIEW_CONTENTS: u32 = 0x0196;
    pub const CLOSE_GROUND_CONTAINER: u32 = 0x0052;
    pub const ATTACK_DONE: u32 = 0x01A7;
    pub const VICTIM_NOTIFICATION: u32 = 0x01AC;
    pub const KILLER_NOTIFICATION: u32 = 0x01AD;
    pub const ATTACKER_NOTIFICATION: u32 = 0x01B1;
    pub const DEFENDER_NOTIFICATION: u32 = 0x01B2;
    pub const EVASION_ATTACKER_NOTIFICATION: u32 = 0x01B3;
    pub const EVASION_DEFENDER_NOTIFICATION: u32 = 0x01B4;
    pub const UPDATE_HEALTH: u32 = 0x01C0;
    pub const CHANNEL_BROADCAST: u32 = 0x0147;
    pub const IDENTIFY_OBJECT_RESPONSE: u32 = 0x00C9;
    pub const USE_DONE: u32 = 0x01C7;
    pub const EMOTE: u32 = 0x01E2;
    pub const WEENIE_ERROR: u32 = 0x028A;
    pub const WEENIE_ERROR_WITH_STRING: u32 = 0x028B;
    pub const TRANSIENT_STRING: u32 = 0x02EB;
    pub const TELL: u32 = 0x02BD;
}

/// A line of chat from any of the speech-carrying messages.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatLine {
    pub text: String,
    pub sender: String,
    pub sender_id: u32,
    /// ChatMessageType (0 broadcast, 2 speech, 3 tell, 0x1F emote...).
    pub kind: u32,
}

impl ChatLine {
    /// HearSpeech 0x02BB: text, sender, sender id, type.
    pub fn parse_hear_speech(body: &[u8]) -> Result<Self, Truncated> {
        let mut r = Reader::new(body);
        let text = r.string16()?;
        let sender = r.string16()?;
        let sender_id = r.u32()?;
        let kind = r.u32()?;
        Ok(ChatLine {
            text,
            sender,
            sender_id,
            kind,
        })
    }
    /// HearRangedSpeech 0x02BC: text, sender, sender id, range, type.
    pub fn parse_hear_ranged_speech(body: &[u8]) -> Result<Self, Truncated> {
        let mut r = Reader::new(body);
        let text = r.string16()?;
        let sender = r.string16()?;
        let sender_id = r.u32()?;
        let _range = r.f32()?;
        let kind = r.u32()?;
        Ok(ChatLine {
            text,
            sender,
            sender_id,
            kind,
        })
    }
    /// ServerMessage 0xF7E0: text, type.
    pub fn parse_server_message(body: &[u8]) -> Result<Self, Truncated> {
        let mut r = Reader::new(body);
        let text = r.string16()?;
        let kind = r.i32()? as u32;
        Ok(ChatLine {
            text,
            sender: String::new(),
            sender_id: 0,
            kind,
        })
    }
    /// EmoteText 0x01E0: sender id, sender, text.
    pub fn parse_emote_text(body: &[u8]) -> Result<Self, Truncated> {
        let mut r = Reader::new(body);
        let sender_id = r.u32()?;
        let sender = r.string16()?;
        let text = r.string16()?;
        Ok(ChatLine {
            text,
            sender,
            sender_id,
            kind: 0x1F,
        })
    }
    /// GameEvent Tell 0x02BD: text, sender, sender id, target id, type.
    pub fn parse_tell(body: &[u8]) -> Result<Self, Truncated> {
        let mut r = Reader::new(body);
        let text = r.string16()?;
        let sender = r.string16()?;
        let sender_id = r.u32()?;
        let _target = r.u32()?;
        let kind = r.u32()?;
        Ok(ChatLine {
            text,
            sender,
            sender_id,
            kind,
        })
    }
}

/// IdentifyObjectResponse (game event 0x00C9): the property tables of an
/// appraised object. Only the plain property tables are read; the profile
/// blocks that may follow are left unparsed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Appraisal {
    pub guid: u32,
    pub success: bool,
    pub ints: Vec<(u32, i32)>,
    pub int64s: Vec<(u32, i64)>,
    pub bools: Vec<(u32, bool)>,
    pub floats: Vec<(u32, f64)>,
    pub strings: Vec<(u32, String)>,
    pub dids: Vec<(u32, u32)>,
}

impl Appraisal {
    pub const FLAG_INT: u32 = 0x0001;
    pub const FLAG_BOOL: u32 = 0x0002;
    pub const FLAG_FLOAT: u32 = 0x0004;
    pub const FLAG_STRING: u32 = 0x0008;
    pub const FLAG_DID: u32 = 0x1000;
    pub const FLAG_INT64: u32 = 0x2000;
    pub const STRING_USE: u32 = 14;
    pub const STRING_SHORT_DESC: u32 = 15;
    pub const STRING_LONG_DESC: u32 = 16;

    pub fn parse(body: &[u8]) -> Result<Self, Truncated> {
        let mut r = Reader::new(body);
        let mut a = Appraisal {
            guid: r.u32()?,
            ..Default::default()
        };
        let flags = r.u32()?;
        a.success = r.u32()? != 0;
        fn header(r: &mut Reader) -> Result<u16, Truncated> {
            let n = r.u16()?;
            r.u16()?;
            Ok(n)
        }
        if flags & Self::FLAG_INT != 0 {
            for _ in 0..header(&mut r)? {
                a.ints.push((r.u32()?, r.i32()?));
            }
        }
        if flags & Self::FLAG_INT64 != 0 {
            for _ in 0..header(&mut r)? {
                a.int64s.push((r.u32()?, r.u64()? as i64));
            }
        }
        if flags & Self::FLAG_BOOL != 0 {
            for _ in 0..header(&mut r)? {
                a.bools.push((r.u32()?, r.u32()? != 0));
            }
        }
        if flags & Self::FLAG_FLOAT != 0 {
            for _ in 0..header(&mut r)? {
                a.floats.push((r.u32()?, r.f64()?));
            }
        }
        if flags & Self::FLAG_STRING != 0 {
            for _ in 0..header(&mut r)? {
                a.strings.push((r.u32()?, r.string16()?));
            }
        }
        if flags & Self::FLAG_DID != 0 {
            for _ in 0..header(&mut r)? {
                a.dids.push((r.u32()?, r.u32()?));
            }
        }
        Ok(a)
    }

    pub fn string(&self, key: u32) -> Option<&str> {
        self.strings
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// CombatMode values for ChangeCombatMode.
pub mod combat_mode {
    pub const NON_COMBAT: u32 = 1;
    pub const MELEE: u32 = 2;
    pub const MISSILE: u32 = 4;
    pub const MAGIC: u32 = 8;
}

/// AttackerNotification (0x01B1) / DefenderNotification (0x01B2): one
/// landed melee blow, from either side.
#[derive(Debug, Clone, PartialEq)]
pub struct AttackNotice {
    /// The other party's name.
    pub name: String,
    pub damage_type: u32,
    /// Fraction of the victim's health removed.
    pub percent: f64,
    pub damage: u32,
    pub critical: bool,
}

impl AttackNotice {
    pub fn parse_attacker(body: &[u8]) -> Result<Self, Truncated> {
        let mut r = Reader::new(body);
        Ok(AttackNotice {
            name: r.string16()?,
            damage_type: r.u32()?,
            percent: r.f64()?,
            damage: r.u32()?,
            critical: r.u32()? != 0,
        })
    }
    pub fn parse_defender(body: &[u8]) -> Result<Self, Truncated> {
        let mut r = Reader::new(body);
        let name = r.string16()?;
        let damage_type = r.u32()?;
        let percent = r.f64()?;
        let damage = r.u32()?;
        let _location = r.u32()?;
        let critical = r.u32()? != 0;
        Ok(AttackNotice {
            name,
            damage_type,
            percent,
            damage,
            critical,
        })
    }
}

/// UpdateHealth (0x01C0): a creature's health as a fraction.
pub fn parse_update_health(body: &[u8]) -> Result<(u32, f32), Truncated> {
    let mut r = Reader::new(body);
    Ok((r.u32()?, r.f32()?))
}

/// ViewContents (0x0196): a container's item list `(guid, container type)`.
pub fn parse_view_contents(body: &[u8]) -> Result<(u32, Vec<(u32, u32)>), Truncated> {
    let mut r = Reader::new(body);
    let container = r.u32()?;
    let n = r.u32()? as usize;
    let mut items = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        items.push((r.u32()?, r.u32()?));
    }
    Ok((container, items))
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
    fn appraisal_layout() {
        let mut w = Writer::new();
        w.u32(0x8000_0001).u32(0x0001 | 0x0008 | 0x2000).u32(1);
        w.u16(1).u16(64).u32(25).i32(3);
        w.u16(1).u16(64).u32(1).u64(99);
        w.u16(2)
            .u16(16)
            .u32(15)
            .string16("A door.")
            .u32(16)
            .string16("It is shut.");
        let a = Appraisal::parse(&w.finish()).unwrap();
        assert!(a.success);
        assert_eq!(a.ints, vec![(25, 3)]);
        assert_eq!(a.int64s, vec![(1, 99)]);
        assert_eq!(a.string(Appraisal::STRING_SHORT_DESC), Some("A door."));
        assert_eq!(a.string(Appraisal::STRING_LONG_DESC), Some("It is shut."));
        assert_eq!(a.string(Appraisal::STRING_USE), None);
    }

    #[test]
    fn combat_layouts() {
        let mut w = Writer::new();
        w.string16("Golem").u32(4).f64(0.25).u32(7).u32(1).u64(0);
        let a = AttackNotice::parse_attacker(&w.finish()).unwrap();
        assert_eq!((a.name.as_str(), a.damage, a.critical), ("Golem", 7, true));
        let mut w = Writer::new();
        w.string16("Golem")
            .u32(4)
            .f64(0.1)
            .u32(2)
            .u32(3)
            .u32(0)
            .u64(0);
        let d = AttackNotice::parse_defender(&w.finish()).unwrap();
        assert_eq!((d.damage, d.critical), (2, false));
        let mut w = Writer::new();
        w.u32(0x8000_0001).f32(0.5);
        assert_eq!(
            parse_update_health(&w.finish()).unwrap(),
            (0x8000_0001, 0.5)
        );
        let mut w = Writer::new();
        w.u32(0x9000_0001)
            .u32(2)
            .u32(0x8000_0002)
            .u32(0)
            .u32(0x8000_0003)
            .u32(1);
        let (c, items) = parse_view_contents(&w.finish()).unwrap();
        assert_eq!(c, 0x9000_0001);
        assert_eq!(items, vec![(0x8000_0002, 0), (0x8000_0003, 1)]);
    }

    #[test]
    fn chat_layouts() {
        // HearSpeech: "hi" from "Bob" (guid 0x50000001), speech (2).
        let mut w = Writer::new();
        w.string16("hi").string16("Bob").u32(0x5000_0001).u32(2);
        let l = ChatLine::parse_hear_speech(&w.finish()).unwrap();
        assert_eq!(
            l,
            ChatLine {
                text: "hi".into(),
                sender: "Bob".into(),
                sender_id: 0x5000_0001,
                kind: 2
            }
        );
        let mut w = Writer::new();
        w.string16("yo").string16("Al").u32(9).f32(30.0).u32(2);
        assert_eq!(
            ChatLine::parse_hear_ranged_speech(&w.finish())
                .unwrap()
                .kind,
            2
        );
        let mut w = Writer::new();
        w.string16("motd").i32(0);
        let l = ChatLine::parse_server_message(&w.finish()).unwrap();
        assert_eq!((l.text.as_str(), l.sender.as_str()), ("motd", ""));
        let mut w = Writer::new();
        w.u32(7).string16("Al").string16("waves.");
        let l = ChatLine::parse_emote_text(&w.finish()).unwrap();
        assert_eq!((l.sender_id, l.text.as_str()), (7, "waves."));
        assert!(ChatLine::parse_hear_speech(&[2, 0]).is_err());
    }

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

/// GameAction type ids (inside a 0xF7B1 message).
pub mod action {
    pub const TALK: u32 = 0x0015;
    pub const TARGETED_MELEE_ATTACK: u32 = 0x0008;
    pub const PUT_ITEM_IN_CONTAINER: u32 = 0x0019;
    pub const GET_AND_WIELD_ITEM: u32 = 0x001A;
    pub const DROP_ITEM: u32 = 0x001B;
    pub const USE: u32 = 0x0036;
    pub const CHANGE_COMBAT_MODE: u32 = 0x0053;
    pub const NO_LONGER_VIEWING_CONTENTS: u32 = 0x0195;
    pub const IDENTIFY_OBJECT: u32 = 0x00C8;
    /// Sent after entering the world and after each teleport; the server
    /// ignores position reports until it arrives.
    pub const LOGIN_COMPLETE: u32 = 0x00A1;
    pub const JUMP: u32 = 0xF61B;
    pub const MOVE_TO_STATE: u32 = 0xF61C;
    pub const AUTONOMOUS_POSITION: u32 = 0xF753;
}

/// Motion commands and stances used for basic movement.
pub mod motion {
    pub const INVALID: u32 = 0x0;
    pub const READY: u32 = 0x4100_0003;
    pub const WALK_FORWARD: u32 = 0x4500_0005;
    pub const WALK_BACKWARDS: u32 = 0x4500_0006;
    pub const RUN_FORWARD: u32 = 0x4400_0007;
    pub const TURN_RIGHT: u32 = 0x6500_000D;
    pub const TURN_LEFT: u32 = 0x6500_000E;
    pub const SIDE_STEP_RIGHT: u32 = 0x6500_000F;
    pub const SIDE_STEP_LEFT: u32 = 0x6500_0010;
    pub const STANCE_HAND_COMBAT: u32 = 0x8000_003C;
    pub const STANCE_NON_COMBAT: u32 = 0x8000_003D;
    pub const STANCE_SWORD_COMBAT: u32 = 0x8000_003E;
    pub const HOLD_KEY_NONE: u32 = 1;
    pub const HOLD_KEY_RUN: u32 = 2;
}

/// A position as the client reports it: cell id, local origin, orientation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WirePosition {
    pub cell: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub qw: f32,
    pub qx: f32,
    pub qy: f32,
    pub qz: f32,
}

fn write_position(w: &mut Writer, p: &WirePosition) {
    w.u32(p.cell)
        .f32(p.x)
        .f32(p.y)
        .f32(p.z)
        .f32(p.qw)
        .f32(p.qx)
        .f32(p.qy)
        .f32(p.qz);
}

/// Body of the AutonomousPosition action: where the client says it is.
/// `contact` is true when standing on the ground.
pub fn autonomous_position(p: &WirePosition, instance_seq: u16, contact: bool) -> Vec<u8> {
    let mut w = Writer::new();
    write_position(&mut w, p);
    w.u16(instance_seq).u16(0).u16(0).u16(0);
    w.u8(contact as u8);
    w.align4();
    w.finish()
}

/// Jump (0xF61B) body: extent (charge 0..=1), launch velocity in the
/// character's local frame, then the instance/control/teleport/force
/// sequences.
pub fn jump(power: f32, velocity: [f32; 3], instance_seq: u16) -> Vec<u8> {
    let mut w = Writer::new();
    w.f32(power)
        .f32(velocity[0])
        .f32(velocity[1])
        .f32(velocity[2])
        .u16(instance_seq)
        .u16(0)
        .u16(0)
        .u16(0)
        // Trailing object guid and spell id the server reads (unused).
        .u32(0)
        .u32(0);
    w.finish()
}

/// The raw input state the client reports in MoveToState.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RawMotion {
    pub running: bool,
    /// `motion::WALK_FORWARD`, `WALK_BACKWARDS`, or 0 when idle.
    pub forward: u32,
    /// `motion::SIDE_STEP_LEFT/RIGHT` or 0.
    pub sidestep: u32,
    /// `motion::TURN_LEFT/RIGHT` or 0.
    pub turn: u32,
}

/// Body of the MoveToState action: input state plus position.
pub fn move_to_state(m: &RawMotion, p: &WirePosition, instance_seq: u16, contact: bool) -> Vec<u8> {
    const CURRENT_HOLD_KEY: u32 = 0x1;
    const CURRENT_STYLE: u32 = 0x2;
    const FORWARD_COMMAND: u32 = 0x4;
    const FORWARD_SPEED: u32 = 0x10;
    const SIDESTEP_COMMAND: u32 = 0x20;
    const SIDESTEP_SPEED: u32 = 0x80;
    const TURN_COMMAND: u32 = 0x100;
    const TURN_SPEED: u32 = 0x400;
    let mut flags = CURRENT_STYLE;
    if m.running {
        flags |= CURRENT_HOLD_KEY;
    }
    if m.forward != 0 {
        flags |= FORWARD_COMMAND | FORWARD_SPEED;
    }
    if m.sidestep != 0 {
        flags |= SIDESTEP_COMMAND | SIDESTEP_SPEED;
    }
    if m.turn != 0 {
        flags |= TURN_COMMAND | TURN_SPEED;
    }
    let mut w = Writer::new();
    w.u32(flags); // command list length 0 in the high bits
    if m.running {
        w.u32(motion::HOLD_KEY_RUN);
    }
    w.u32(motion::STANCE_NON_COMBAT);
    if m.forward != 0 {
        w.u32(m.forward).f32(1.0);
    }
    if m.sidestep != 0 {
        w.u32(m.sidestep).f32(1.0);
    }
    if m.turn != 0 {
        w.u32(m.turn).f32(1.0);
    }
    write_position(&mut w, p);
    w.u16(instance_seq).u16(0).u16(0).u16(0);
    w.u8(contact as u8);
    w.align4();
    w.finish()
}
