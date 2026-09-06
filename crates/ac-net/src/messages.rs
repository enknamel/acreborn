//! Message opcodes and the bodies needed for login and entering the world.
//! Everything else is delivered to the caller as `(opcode, bytes)`.

use crate::wire::{Reader, Truncated, Writer};

pub mod opcode {
    pub const CHARACTER_CREATE_RESPONSE: u32 = 0xF643;
    pub const CHARACTER_CREATE: u32 = 0xF656;
    pub const CHARACTER_ENTER_WORLD: u32 = 0xF657;
    pub const CHARACTER_LIST: u32 = 0xF658;
    pub const CHARACTER_ERROR: u32 = 0xF659;
    pub const CHARACTER_DELETE: u32 = 0xF655;
    pub const CHARACTER_RESTORE: u32 = 0xF7D9;
    pub const OBJECT_CREATE: u32 = 0xF745;
    pub const PLAYER_CREATE: u32 = 0xF746;
    pub const OBJECT_DELETE: u32 = 0xF747;
    /// Turbine chat (the General, Trade, LFG, Roleplay, society and
    /// allegiance rooms), both ways; see `turbine`.
    pub const TURBINE_CHAT: u32 = 0xF7DE;
    /// A carried item left our inventory (spent, given, dropped to the
    /// corpse): the guid; the object itself is not deleted.
    pub const INVENTORY_REMOVE_OBJECT: u32 = 0x0024;
    pub const UPDATE_POSITION: u32 = 0xF748;
    pub const SET_STATE: u32 = 0xF74B;
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
    pub const SOUND: u32 = 0xF750;
    pub const OBJ_DESC_EVENT: u32 = 0xF625;
    pub const PUBLIC_UPDATE_PROPERTY_INT: u32 = 0x02CE;
    pub const PRIVATE_UPDATE_PROPERTY_INT: u32 = 0x02CD;
    pub const PRIVATE_UPDATE_PROPERTY_INT64: u32 = 0x02CF;
    pub const PRIVATE_UPDATE_PROPERTY_STRING: u32 = 0x02D5;
    pub const PRIVATE_UPDATE_SKILL: u32 = 0x02DD;
    pub const PRIVATE_UPDATE_SKILL_LEVEL: u32 = 0x02DF;
    pub const PRIVATE_UPDATE_SKILL_AC: u32 = 0x02E1;
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

/// Message body for CharacterDelete (0xF655): the account and the
/// character's slot (its index in the last CharacterList). ACE marks the
/// character for deletion, echoes 0xF655 and sends a fresh CharacterList.
pub fn character_delete(account: &str, slot: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(opcode::CHARACTER_DELETE).string16(account).u32(slot);
    w.finish()
}

/// Message body for CharacterRestore (0xF7D9): the character id. ACE
/// answers with a CharacterCreateResponse-shaped 0xF643 (1, id, name,
/// seconds greyed out) or a failure code (name in use, corrupt).
pub fn character_restore(character_id: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(opcode::CHARACTER_RESTORE).u32(character_id);
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
    pub const FELLOWSHIP_QUIT: u32 = 0x00A3;
    pub const FELLOWSHIP_DISMISS: u32 = 0x00A4;
    pub const FELLOWSHIP_FULL_UPDATE: u32 = 0x02BE;
    pub const FELLOWSHIP_DISBAND: u32 = 0x02BF;
    pub const FELLOWSHIP_UPDATE_FELLOW: u32 = 0x02C0;
    pub const CONFIRMATION_REQUEST: u32 = 0x0274;
    pub const CONFIRMATION_DONE: u32 = 0x0276;
    pub const ALLEGIANCE_UPDATE_ABORTED: u32 = 0x0003;
    pub const ALLEGIANCE_UPDATE: u32 = 0x0020;
    pub const ALLEGIANCE_UPDATE_DONE: u32 = 0x01C8;
    pub const ALLEGIANCE_LOGIN_NOTIFICATION: u32 = 0x027A;
    pub const ALLEGIANCE_INFO_RESPONSE: u32 = 0x027C;
    /// The Turbine chat rooms we are in: allegiance room (0 without
    /// one), general, trade, lfg, roleplay, olthoi, society, three
    /// society rooms; ten u32.
    pub const SET_TURBINE_CHAT_CHANNELS: u32 = 0x0295;
    /// Salvage results: skill, skipped guids, (material, workmanship, units) list, bonus.
    pub const SALVAGE_OPERATIONS_RESULT: u32 = 0x02B4;
    /// A house sign was used: slumlord guid, then the profile.
    pub const HOUSE_PROFILE: u32 = 0x021D;
    /// Our own house (answer to HouseQuery when we own one).
    pub const HOUSE_DATA: u32 = 0x0225;
    /// Answer to HouseQuery without a house: a WeenieError (0 = none).
    pub const HOUSE_STATUS: u32 = 0x0226;
    pub const UPDATE_RENT_TIME: u32 = 0x0227;
    pub const UPDATE_RENT_PAYMENT: u32 = 0x0228;
    /// Who may enter a house we are near: sequence, house guid, record.
    pub const HOUSE_UPDATE_RESTRICTIONS: u32 = 0x0248;
    /// Our house's access records (guest list), on request.
    pub const UPDATE_HAR: u32 = 0x0257;
    pub const HOUSE_TRANSACTION: u32 = 0x0259;
    pub const AVAILABLE_HOUSES: u32 = 0x0271;
    pub const REGISTER_TRADE: u32 = 0x01FD;
    pub const OPEN_TRADE: u32 = 0x01FE;
    pub const CLOSE_TRADE: u32 = 0x01FF;
    pub const ADD_TO_TRADE: u32 = 0x0200;
    pub const REMOVE_FROM_TRADE: u32 = 0x0201;
    pub const ACCEPT_TRADE: u32 = 0x0202;
    pub const DECLINE_TRADE: u32 = 0x0203;
    pub const RESET_TRADE: u32 = 0x0205;
    pub const TRADE_FAILURE: u32 = 0x0207;
    pub const CLEAR_TRADE_ACCEPTANCE: u32 = 0x0208;
    pub const CLOSE_GROUND_CONTAINER: u32 = 0x0052;
    pub const APPROACH_VENDOR: u32 = 0x0062;
    /// `u16 spell id, u16 layer`: a spell left the spellbook.
    pub const MAGIC_REMOVE_SPELL: u32 = 0x01A8;
    /// `u16 spell id, u16 layer`: a spell entered the spellbook.
    pub const MAGIC_UPDATE_SPELL: u32 = 0x02C1;
    /// One enchantment record (see `ac_world::stats::Enchantment`).
    pub const MAGIC_UPDATE_ENCHANTMENT: u32 = 0x02C2;
    /// `u16 spell id, u16 layer`.
    pub const MAGIC_REMOVE_ENCHANTMENT: u32 = 0x02C3;
    /// `u32 count`, then enchantment records.
    pub const MAGIC_UPDATE_MULTIPLE_ENCHANTMENTS: u32 = 0x02C4;
    /// `u32 count`, then `(u16 spell id, u16 layer)` pairs.
    pub const MAGIC_REMOVE_MULTIPLE_ENCHANTMENTS: u32 = 0x02C5;
    /// No body: every enchantment is gone.
    pub const MAGIC_PURGE_ENCHANTMENTS: u32 = 0x02C6;
    /// `u16 spell id, u16 layer`: removed by a dispel.
    pub const MAGIC_DISPEL_ENCHANTMENT: u32 = 0x02C7;
    /// `u32 count`, then `(u16 spell id, u16 layer)` pairs.
    pub const MAGIC_DISPEL_MULTIPLE_ENCHANTMENTS: u32 = 0x02C8;
    /// No body: the harmful enchantments are gone (sent on death).
    pub const MAGIC_PURGE_BAD_ENCHANTMENTS: u32 = 0x0312;
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

    /// ChannelBroadcast event 0x0147: channel id, sender ("" for our
    /// own line), text. The channel id is kept in `sender_id` and the
    /// kind is `channel::KIND`.
    pub fn parse_channel_broadcast(body: &[u8]) -> Result<Self, Truncated> {
        let mut r = Reader::new(body);
        let channel = r.u32()?;
        let sender = r.string16()?;
        let text = r.string16()?;
        Ok(ChatLine {
            text,
            sender,
            sender_id: channel,
            kind: channel::KIND,
        })
    }
}

/// One material's yield in a SalvageOperationsResult.
#[derive(Debug, Clone, PartialEq)]
pub struct SalvageYield {
    pub material: u32,
    pub workmanship: f64,
    pub units: u32,
}

/// SalvageOperationsResult (game event 0x02B4): the skill used (ACE
/// `Skill`: 40 salvaging, 18/28/29/30 the tinkerings), the guids that
/// could not be salvaged, the yields, and the augmentation bonus percent.
#[derive(Debug, Clone, PartialEq)]
pub struct SalvageResult {
    pub skill: u32,
    pub skipped: Vec<u32>,
    pub yields: Vec<SalvageYield>,
    pub bonus_percent: u32,
}

impl SalvageResult {
    pub fn parse(body: &[u8]) -> Result<Self, Truncated> {
        let mut r = Reader::new(body);
        let skill = r.u32()?;
        let n = r.u32()? as usize;
        let mut skipped = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            skipped.push(r.u32()?);
        }
        let n = r.u32()? as usize;
        let mut yields = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            yields.push(SalvageYield {
                material: r.u32()?,
                workmanship: r.f64()?,
                units: r.u32()?,
            });
        }
        let bonus_percent = r.u32().unwrap_or(0);
        Ok(SalvageResult {
            skill,
            skipped,
            yields,
            bonus_percent,
        })
    }
}

/// Turbine chat (message 0xF7DE): the rooms every player can join
/// (General, Trade, LFG, Roleplay), the society rooms, and each
/// allegiance's own room (its id is the allegiance's biota id, sent in
/// SetTurbineChatChannels). A message is a "net blob": size, blob type
/// (1 event = a line from the room, 3 request = ours going out, 5
/// response = the server's ack), dispatch type, two (kind, id) pairs, a
/// cookie, then a sized payload. See ACE `GameMessageTurbineChat` and
/// `TurbineChatHandler`.
pub mod turbine {
    use super::ChatLine;
    use crate::wire::{Reader, Truncated, Writer};

    pub const ALLEGIANCE: u32 = 1;
    pub const GENERAL: u32 = 2;
    pub const TRADE: u32 = 3;
    pub const LFG: u32 = 4;
    pub const ROLEPLAY: u32 = 5;
    pub const SOCIETY: u32 = 6;
    pub const OLTHOI: u32 = 10;
    /// The `ChatLine::kind` a room line is tagged with; `sender_id`
    /// then holds the room id.
    pub const KIND: u32 = 0x2000_0000;

    const EVENT_BINARY: u32 = 1;
    const REQUEST_BINARY: u32 = 3;
    const SEND_TO_ROOM_BY_ID: u32 = 2;

    /// The room's name; ids above Olthoi are allegiance rooms.
    pub fn name(id: u32) -> &'static str {
        match id {
            ALLEGIANCE => "Allegiance",
            GENERAL => "General",
            TRADE => "Trade",
            LFG => "LFG",
            ROLEPLAY => "Roleplay",
            SOCIETY | 7..=9 => "Society",
            OLTHOI => "Olthoi",
            _ => "Allegiance",
        }
    }

    /// The room a `/g`, `/trade`, `/lfg`, `/rp` or `/a` prefix means
    /// (`ALLEGIANCE` stands for "our allegiance's room").
    pub fn from_prefix(p: &str) -> Option<u32> {
        match p {
            "g" | "general" => Some(GENERAL),
            "tr" | "trade" => Some(TRADE),
            "lfg" => Some(LFG),
            "rp" | "roleplay" => Some(ROLEPLAY),
            "a" | "allegiance" => Some(ALLEGIANCE),
            _ => None,
        }
    }

    /// The ChatType the server expects with a room id.
    pub fn chat_type(room: u32) -> u32 {
        match room {
            GENERAL | TRADE | LFG | ROLEPLAY | OLTHOI => room,
            SOCIETY..=9 => SOCIETY,
            _ => ALLEGIANCE,
        }
    }

    /// A counted UTF-16 string: a length byte (0x80 then the length
    /// when it does not fit), then the code units.
    fn read_wstring(r: &mut Reader) -> Result<String, Truncated> {
        let mut n = r.u8()? as usize;
        if n & 0x80 != 0 {
            n = r.u8()? as usize;
        }
        let bytes = r.bytes(n * 2)?;
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Ok(String::from_utf16_lossy(&units))
    }

    fn write_wstring(w: &mut Writer, s: &str) {
        let units: Vec<u16> = s.encode_utf16().take(255).collect();
        if units.len() < 128 {
            w.u8(units.len() as u8);
        } else {
            w.u8(0x80).u8(units.len() as u8);
        }
        for u in units {
            w.u16(u);
        }
    }

    /// Decode a Turbine chat message body (after the 0xF7DE opcode).
    /// `Ok(None)` for blobs that are not room lines (our own acks).
    pub fn parse(body: &[u8]) -> Result<Option<ChatLine>, Truncated> {
        let mut r = Reader::new(body);
        let _size = r.u32()?;
        let blob = r.u32()?;
        let _dispatch = r.u32()?;
        for _ in 0..5 {
            r.u32()?;
        }
        let _payload_size = r.u32()?;
        if blob != EVENT_BINARY {
            return Ok(None);
        }
        let room = r.u32()?;
        let sender = read_wstring(&mut r)?;
        let text = read_wstring(&mut r)?;
        let _extra = r.u32()?;
        let _sender_guid = r.u32()?;
        let _result = r.u32()?;
        let _chat_type = r.u32()?;
        Ok(Some(ChatLine {
            text,
            sender,
            sender_id: room,
            kind: KIND,
        }))
    }

    /// Encode a line for a room (blob type request, dispatch "send to
    /// room by id"): the whole 0xF7DE message including the opcode.
    pub fn encode(room: u32, sender: u32, text: &str, context: u32) -> Vec<u8> {
        let mut payload = Writer::new();
        payload.u32(context).u32(2).u32(2).u32(room);
        write_wstring(&mut payload, text);
        payload.u32(0x0C).u32(sender).u32(0).u32(chat_type(room));
        let payload = payload.finish();
        let mut blob = Writer::new();
        blob.u32(REQUEST_BINARY)
            .u32(SEND_TO_ROOM_BY_ID)
            .u32(1)
            .u32(0)
            .u32(0)
            .u32(0)
            .u32(0)
            .u32(payload.len() as u32)
            .bytes(&payload);
        let blob = blob.finish();
        let mut w = Writer::new();
        w.u32(super::opcode::TURBINE_CHAT)
            .u32(blob.len() as u32)
            .bytes(&blob);
        w.finish()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn room_line_round_trips() {
            // An inbound line the way ACE writes it.
            let mut payload = Writer::new();
            payload.u32(GENERAL);
            write_wstring(&mut payload, "+Admin");
            write_wstring(&mut payload, "hello général");
            payload.u32(0x0C).u32(0x5000_0002).u32(0).u32(GENERAL);
            let payload = payload.finish();
            let mut blob = Writer::new();
            blob.u32(EVENT_BINARY)
                .u32(1)
                .u32(1)
                .u32(0x000B_00B5)
                .u32(1)
                .u32(0x000B_00B5)
                .u32(0)
                .u32(payload.len() as u32)
                .bytes(&payload);
            let blob = blob.finish();
            let mut w = Writer::new();
            w.u32(blob.len() as u32).bytes(&blob);
            let line = parse(&w.finish()).unwrap().unwrap();
            assert_eq!(line.sender, "+Admin");
            assert_eq!(line.text, "hello général");
            assert_eq!((line.sender_id, line.kind), (GENERAL, KIND));
            assert_eq!(name(line.sender_id), "General");

            // Our own request has the opcode, the sizes and the room.
            let msg = encode(TRADE, 0x5000_0001, "wts bow", 7);
            let mut r = Reader::new(&msg);
            assert_eq!(r.u32().unwrap(), super::super::opcode::TURBINE_CHAT);
            let size = r.u32().unwrap() as usize;
            assert_eq!(size, msg.len() - 8);
            assert_eq!(r.u32().unwrap(), REQUEST_BINARY);
            assert_eq!(r.u32().unwrap(), SEND_TO_ROOM_BY_ID);
            assert!(parse(&msg[4..]).unwrap().is_none());
            assert_eq!(from_prefix("g"), Some(GENERAL));
            assert_eq!(chat_type(0x3300), ALLEGIANCE);
        }
    }
}

/// Group chat channel ids (ACE `Channel`), for ChatChannel 0x0147 both
/// ways.
pub mod channel {
    pub const FELLOW: u32 = 0x0000_0800;
    pub const VASSALS: u32 = 0x0000_1000;
    pub const PATRON: u32 = 0x0000_2000;
    pub const MONARCH: u32 = 0x0000_4000;
    pub const CO_VASSALS: u32 = 0x0100_0000;
    /// The `ChatLine::kind` a channel line is tagged with; not a
    /// ChatMessageType, the line carries the channel id instead.
    pub const KIND: u32 = 0x1000_0000;

    pub fn name(id: u32) -> &'static str {
        match id {
            0x1 => "Abuse",
            0x2 => "Admin",
            0x4 => "Audit",
            0x8 | 0x10 | 0x20 => "Advocate",
            0x100 => "Debug",
            0x200 => "Sentinel",
            0x400 => "Help",
            FELLOW => "Fellowship",
            VASSALS => "Vassals",
            PATRON => "Patron",
            MONARCH => "Monarch",
            CO_VASSALS => "Co-vassals",
            _ => "Channel",
        }
    }

    /// The channel a `/v`, `/p`, `/m`, `/c` or `/f` chat prefix means.
    pub fn from_prefix(p: &str) -> Option<u32> {
        match p {
            "v" | "vassals" => Some(VASSALS),
            "p" | "patron" => Some(PATRON),
            "m" | "monarch" => Some(MONARCH),
            "c" | "covassals" => Some(CO_VASSALS),
            "f" | "fellow" => Some(FELLOW),
            _ => None,
        }
    }
}

/// A weapon's appraisal block (ACE `WeaponProfile`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WeaponProfile {
    /// DamageType bits: 1 slash, 2 pierce, 4 bludgeon, 8 cold, 0x10 fire,
    /// 0x20 acid, 0x40 electric, 0x400 nether.
    pub damage_type: u32,
    /// Attack speed, lower is faster (0..=100).
    pub speed: u32,
    pub skill: u32,
    pub damage: u32,
    /// 0..=1: the low end of the damage roll is `damage * (1 - variance)`.
    pub variance: f64,
    pub damage_mod: f64,
    pub length: f64,
    pub max_velocity: f64,
    /// Attack skill multiplier (1.05 = +5%).
    pub offense: f64,
    pub max_velocity_estimated: u32,
}

/// An armor piece's protections (ACE `ArmorProfile`), as multipliers of
/// the armor level per damage type.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArmorProfile {
    pub slash: f32,
    pub pierce: f32,
    pub bludgeon: f32,
    pub cold: f32,
    pub fire: f32,
    pub acid: f32,
    pub nether: f32,
    pub electric: f32,
}

/// A creature's appraisal block (ACE `CreatureProfile`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreatureProfile {
    pub flags: u32,
    pub health: u32,
    pub health_max: u32,
    /// Strength, Endurance, Quickness, Coordination, Focus, Self; only
    /// with flag 8.
    pub attributes: Option<[u32; 6]>,
    pub stamina: u32,
    pub mana: u32,
    pub stamina_max: u32,
    pub mana_max: u32,
    /// (highlight, colour) bitmasks of buffed/debuffed attributes.
    pub attribute_marks: Option<(u16, u16)>,
}

/// IdentifyObjectResponse (game event 0x00C9): the property tables of an
/// appraised object and, by flags, its spell list, armor, creature and
/// weapon profiles, hook profile, enchantment marks and per-location
/// armor levels (ACE `AppraiseInfo`).
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
    /// Spell ids the item casts or carries.
    pub spells: Vec<u32>,
    pub armor: Option<ArmorProfile>,
    pub creature: Option<CreatureProfile>,
    pub weapon: Option<WeaponProfile>,
    /// (flags, valid locations, ammo type) of a hook.
    pub hook: Option<(u32, u32, u32)>,
    /// (highlight, colour) marks for armor, weapon and resist values
    /// changed by enchantments.
    pub armor_marks: Option<(u16, u16)>,
    pub weapon_marks: Option<(u16, u16)>,
    pub resist_marks: Option<(u16, u16)>,
    /// A creature's armor by location: head, chest, abdomen, upper arm,
    /// lower arm, hand, upper leg, lower leg, foot.
    pub armor_levels: Option<[u32; 9]>,
}

impl Appraisal {
    pub const FLAG_INT: u32 = 0x0001;
    pub const FLAG_BOOL: u32 = 0x0002;
    pub const FLAG_FLOAT: u32 = 0x0004;
    pub const FLAG_STRING: u32 = 0x0008;
    pub const FLAG_DID: u32 = 0x1000;
    pub const FLAG_INT64: u32 = 0x2000;
    pub const FLAG_SPELL_BOOK: u32 = 0x0010;
    pub const FLAG_WEAPON_PROFILE: u32 = 0x0020;
    pub const FLAG_HOOK_PROFILE: u32 = 0x0040;
    pub const FLAG_ARMOR_PROFILE: u32 = 0x0080;
    pub const FLAG_CREATURE_PROFILE: u32 = 0x0100;
    pub const FLAG_ARMOR_MARKS: u32 = 0x0200;
    pub const FLAG_RESIST_MARKS: u32 = 0x0400;
    pub const FLAG_WEAPON_MARKS: u32 = 0x0800;
    pub const FLAG_ARMOR_LEVELS: u32 = 0x4000;
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
        if flags & Self::FLAG_SPELL_BOOK != 0 {
            let n = r.u32()? as usize;
            for _ in 0..n.min(256) {
                a.spells.push(r.u32()?);
            }
        }
        if flags & Self::FLAG_ARMOR_PROFILE != 0 {
            a.armor = Some(ArmorProfile {
                slash: r.f32()?,
                pierce: r.f32()?,
                bludgeon: r.f32()?,
                cold: r.f32()?,
                fire: r.f32()?,
                acid: r.f32()?,
                nether: r.f32()?,
                electric: r.f32()?,
            });
        }
        if flags & Self::FLAG_CREATURE_PROFILE != 0 {
            let cflags = r.u32()?;
            let mut c = CreatureProfile {
                flags: cflags,
                health: r.u32()?,
                health_max: r.u32()?,
                ..Default::default()
            };
            if cflags & 0x8 != 0 {
                let mut attrs = [0u32; 6];
                for a in attrs.iter_mut() {
                    *a = r.u32()?;
                }
                c.attributes = Some(attrs);
                c.stamina = r.u32()?;
                c.mana = r.u32()?;
                c.stamina_max = r.u32()?;
                c.mana_max = r.u32()?;
            }
            if cflags & 0x1 != 0 {
                c.attribute_marks = Some((r.u16()?, r.u16()?));
            }
            a.creature = Some(c);
        }
        if flags & Self::FLAG_WEAPON_PROFILE != 0 {
            a.weapon = Some(WeaponProfile {
                damage_type: r.u32()?,
                speed: r.u32()?,
                skill: r.u32()?,
                damage: r.u32()?,
                variance: r.f64()?,
                damage_mod: r.f64()?,
                length: r.f64()?,
                max_velocity: r.f64()?,
                offense: r.f64()?,
                max_velocity_estimated: r.u32()?,
            });
        }
        if flags & Self::FLAG_HOOK_PROFILE != 0 {
            a.hook = Some((r.u32()?, r.u32()?, r.u32()?));
        }
        if flags & Self::FLAG_ARMOR_MARKS != 0 {
            a.armor_marks = Some((r.u16()?, r.u16()?));
        }
        if flags & Self::FLAG_WEAPON_MARKS != 0 {
            a.weapon_marks = Some((r.u16()?, r.u16()?));
        }
        if flags & Self::FLAG_RESIST_MARKS != 0 {
            a.resist_marks = Some((r.u16()?, r.u16()?));
        }
        if flags & Self::FLAG_ARMOR_LEVELS != 0 {
            let mut levels = [0u32; 9];
            for l in levels.iter_mut() {
                *l = r.u32()?;
            }
            a.armor_levels = Some(levels);
        }
        Ok(a)
    }

    pub fn int(&self, key: u32) -> Option<i32> {
        self.ints.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }

    pub fn float(&self, key: u32) -> Option<f64> {
        self.floats.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }

    pub fn bool(&self, key: u32) -> Option<bool> {
        self.bools.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }

    pub fn string(&self, key: u32) -> Option<&str> {
        self.strings
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }
}

#[cfg(test)]
mod appraisal_tests {
    use super::*;

    #[test]
    fn parses_profiles() {
        let mut w = Writer::new();
        w.u32(0x8000_0001);
        w.u32(
            Appraisal::FLAG_INT
                | Appraisal::FLAG_STRING
                | Appraisal::FLAG_SPELL_BOOK
                | Appraisal::FLAG_WEAPON_PROFILE
                | Appraisal::FLAG_ARMOR_PROFILE
                | Appraisal::FLAG_CREATURE_PROFILE
                | Appraisal::FLAG_ARMOR_LEVELS,
        );
        w.u32(1);
        w.u16(2).u16(16).u32(19).i32(150).u32(44).i32(12);
        w.u16(1).u16(8).u32(15).string16("A dagger.");
        w.u32(2).u32(2091).u32(1);
        // Armor profile (8 f32), then creature, then weapon.
        for v in [1.0f32, 1.2, 0.8, 0.5, 0.5, 0.5, 0.0, 0.5] {
            w.f32(v);
        }
        w.u32(0x9).u32(40).u32(50);
        for v in [10u32, 20, 30, 40, 50, 60, 70, 80, 90, 100] {
            w.u32(v);
        }
        w.u16(1).u16(2);
        w.u32(2)
            .u32(20)
            .u32(1)
            .u32(12)
            .f64(0.5)
            .f64(1.0)
            .f64(0.3)
            .f64(0.0)
            .f64(1.05)
            .u32(0);
        for v in 1..=9u32 {
            w.u32(v * 10);
        }
        let a = Appraisal::parse(&w.finish()).unwrap();
        assert!(a.success);
        assert_eq!(a.int(19), Some(150));
        assert_eq!(a.string(15), Some("A dagger."));
        assert_eq!(a.spells, vec![2091, 1]);
        assert_eq!(a.armor.as_ref().map(|p| p.pierce), Some(1.2));
        let c = a.creature.as_ref().unwrap();
        assert_eq!((c.health, c.health_max), (40, 50));
        assert_eq!(c.attributes, Some([10, 20, 30, 40, 50, 60]));
        assert_eq!((c.stamina, c.mana_max), (70, 100));
        assert_eq!(c.attribute_marks, Some((1, 2)));
        let wp = a.weapon.as_ref().unwrap();
        assert_eq!(
            (wp.damage_type, wp.speed, wp.skill, wp.damage),
            (2, 20, 1, 12)
        );
        assert!((wp.offense - 1.05).abs() < 1e-9);
        assert_eq!(a.armor_levels.map(|l| l[8]), Some(90));
    }
}

/// PropertyInt ids carried by the Public/PrivateUpdatePropertyInt
/// messages (`u8 sequence, [u32 guid], u32 property, i32 value`).
pub mod property_int {
    pub const MAX_STACK_SIZE: u32 = 11;
    pub const STACK_SIZE: u32 = 12;
    pub const VALUE: u32 = 19;
}

/// SetStackSize (0x0197): `u8 sequence, u32 guid, u32 stack size, u32
/// value`, sent for every change of a stack in view (including our own
/// packs, after a spell burns components or a vendor buy merges).
pub fn parse_set_stack_size(body: &[u8]) -> Result<(u32, u32, u32), Truncated> {
    let mut r = Reader::new(body);
    let _seq = r.u8()?;
    Ok((r.u32()?, r.u32()?, r.u32()?))
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

/// CastTargetedSpell body: target guid then spell id.
pub fn cast_targeted(target: u32, spell: u32) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(target).u32(spell);
    w.finish()
}

/// Buy/Sell body: vendor, count, then `(amount, guid)` per item, and the
/// alternate currency id (0 for pyreals).
pub fn trade(vendor: u32, items: &[(u32, i32)]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(vendor).u32(items.len() as u32);
    for (guid, amount) in items {
        w.i32(*amount).u32(*guid);
    }
    w.u32(0);
    w.finish()
}

/// Sound (0xF750): an object plays a sound-table entry: `(guid, sound type, volume)`.
pub fn parse_sound(body: &[u8]) -> Result<(u32, u32, f32), Truncated> {
    let mut r = Reader::new(body);
    Ok((r.u32()?, r.u32()?, r.f32()?))
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
    fn delete_and_restore_bodies() {
        let d = character_delete("acct", 2);
        let mut r = Reader::new(&d);
        assert_eq!(r.u32().unwrap(), opcode::CHARACTER_DELETE);
        assert_eq!(r.string16().unwrap(), "acct");
        assert_eq!(r.u32().unwrap(), 2);
        assert!(r.remaining().is_empty());
        let rs = character_restore(0x5000_0001);
        assert_eq!(rs, [0xD9, 0xF7, 0, 0, 1, 0, 0, 0x50]);
    }

    #[test]
    fn create_response_ok_and_failure() {
        let mut w = Writer::new();
        w.u32(1).u32(0x5000_0002).string16("Bob").u32(0);
        let r = CharacterCreateResponse::parse(&w.buf).unwrap();
        assert_eq!(
            (r.response, r.guid, r.name.as_str()),
            (1, 0x5000_0002, "Bob")
        );
        let r = CharacterCreateResponse::parse(&3u32.to_le_bytes()).unwrap();
        assert_eq!((r.response, r.guid, r.name.as_str()), (3, 0, ""));
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
    pub const USE_WITH_TARGET: u32 = 0x0035;
    pub const SET_SINGLE_CHARACTER_OPTION: u32 = 0x0005;
    pub const FELLOWSHIP_CREATE: u32 = 0x00A2;
    pub const FELLOWSHIP_QUIT: u32 = 0x00A3;
    pub const FELLOWSHIP_DISMISS: u32 = 0x00A4;
    pub const FELLOWSHIP_RECRUIT: u32 = 0x00A5;
    pub const FELLOWSHIP_UPDATE_REQUEST: u32 = 0x00A6;
    pub const CONFIRMATION_RESPONSE: u32 = 0x0275;
    pub const SWEAR_ALLEGIANCE: u32 = 0x001D;
    pub const BREAK_ALLEGIANCE: u32 = 0x001E;
    pub const ALLEGIANCE_UPDATE_REQUEST: u32 = 0x001F;
    pub const QUERY_ALLEGIANCE_NAME: u32 = 0x0030;
    pub const CLEAR_ALLEGIANCE_NAME: u32 = 0x0031;
    pub const SET_ALLEGIANCE_NAME: u32 = 0x0033;
    pub const ALLEGIANCE_INFO_REQUEST: u32 = 0x027B;
    /// Group chat: channel id (see `channel`), text.
    pub const CHAT_CHANNEL: u32 = 0x0147;
    pub const SET_MOTD: u32 = 0x0254;
    pub const QUERY_MOTD: u32 = 0x0255;
    pub const CLEAR_MOTD: u32 = 0x0256;
    /// Salvage with an Ust: tool guid, count, item guids (ACE names it
    /// after the retail client's "create tinkering tool" verb).
    pub const CREATE_TINKERING_TOOL: u32 = 0x027D;
    /// Stacks: merge (from, to, amount), split into a container (stack,
    /// container, placement, amount), onto the ground (stack, amount)
    /// or into a wield slot (stack, EquipMask, amount).
    pub const STACKABLE_MERGE: u32 = 0x0054;
    pub const STACKABLE_SPLIT_TO_CONTAINER: u32 = 0x0055;
    pub const STACKABLE_SPLIT_TO_3D: u32 = 0x0056;
    pub const STACKABLE_SPLIT_TO_WIELD: u32 = 0x019B;
    /// Housing (see docs/game/mechanics.md, section 9).
    pub const BUY_HOUSE: u32 = 0x021C;
    pub const HOUSE_QUERY: u32 = 0x021E;
    pub const ABANDON_HOUSE: u32 = 0x021F;
    pub const RENT_HOUSE: u32 = 0x0221;
    pub const ADD_PERMANENT_GUEST: u32 = 0x0245;
    pub const REMOVE_PERMANENT_GUEST: u32 = 0x0246;
    pub const SET_OPEN_HOUSE_STATUS: u32 = 0x0247;
    pub const CHANGE_STORAGE_PERMISSION: u32 = 0x0249;
    pub const BOOT_SPECIFIC_HOUSE_GUEST: u32 = 0x024A;
    pub const REMOVE_ALL_STORAGE_PERMISSION: u32 = 0x024C;
    pub const REQUEST_FULL_GUEST_LIST: u32 = 0x024D;
    pub const QUERY_LORD: u32 = 0x0258;
    pub const ADD_ALL_STORAGE_PERMISSION: u32 = 0x025C;
    pub const REMOVE_ALL_PERMANENT_GUESTS: u32 = 0x025E;
    pub const BOOT_EVERYONE: u32 = 0x025F;
    pub const SET_HOOKS_VISIBILITY: u32 = 0x0266;
    pub const MODIFY_ALLEGIANCE_GUEST_PERMISSION: u32 = 0x0267;
    pub const MODIFY_ALLEGIANCE_STORAGE_PERMISSION: u32 = 0x0268;
    pub const LIST_AVAILABLE_HOUSES: u32 = 0x0270;
    pub const OPEN_TRADE_NEGOTIATIONS: u32 = 0x01F6;
    pub const CLOSE_TRADE_NEGOTIATIONS: u32 = 0x01F7;
    pub const ADD_TO_TRADE: u32 = 0x01F8;
    pub const ACCEPT_TRADE: u32 = 0x01FA;
    pub const DECLINE_TRADE: u32 = 0x01FB;
    pub const RESET_TRADE: u32 = 0x0204;
    pub const RAISE_VITAL: u32 = 0x0044;
    pub const RAISE_ATTRIBUTE: u32 = 0x0045;
    pub const RAISE_SKILL: u32 = 0x0046;
    pub const TRAIN_SKILL: u32 = 0x0047;
    pub const SET_AFK_MODE: u32 = 0x000F;
    pub const SET_AFK_MESSAGE: u32 = 0x0010;
    pub const TELL: u32 = 0x005D;
    pub const TELE_TO_LIFESTONE: u32 = 0x0063;
    pub const EMOTE: u32 = 0x01DF;
    pub const SOUL_EMOTE: u32 = 0x01E1;
    pub const TELE_TO_HOUSE: u32 = 0x0262;
    pub const TELE_TO_MANSION: u32 = 0x0278;
    pub const TELE_TO_MARKETPLACE: u32 = 0x028D;
    pub const ENTER_PK_LITE: u32 = 0x028F;
    pub const RECALL_ALLEGIANCE_HOMETOWN: u32 = 0x02AB;
    pub const DIE: u32 = 0x0279;
    pub const TARGETED_MELEE_ATTACK: u32 = 0x0008;
    pub const TARGETED_MISSILE_ATTACK: u32 = 0x000A;
    pub const PUT_ITEM_IN_CONTAINER: u32 = 0x0019;
    pub const GET_AND_WIELD_ITEM: u32 = 0x001A;
    pub const DROP_ITEM: u32 = 0x001B;
    pub const USE: u32 = 0x0036;
    pub const CAST_UNTARGETED_SPELL: u32 = 0x0048;
    pub const CAST_TARGETED_SPELL: u32 = 0x004A;
    pub const CHANGE_COMBAT_MODE: u32 = 0x0053;
    pub const BUY: u32 = 0x005F;
    pub const SELL: u32 = 0x0060;
    pub const NO_LONGER_VIEWING_CONTENTS: u32 = 0x0195;
    pub const IDENTIFY_OBJECT: u32 = 0x00C8;
    pub const GIVE_OBJECT_REQUEST: u32 = 0x00CD;
    pub const REMOVE_SPELL: u32 = 0x01A8;
    pub const ADD_SPELL_FAVORITE: u32 = 0x01E3;
    pub const REMOVE_SPELL_FAVORITE: u32 = 0x01E4;
    pub const SET_DESIRED_COMPONENT_LEVEL: u32 = 0x0224;
    pub const SPELLBOOK_FILTER: u32 = 0x0286;
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
