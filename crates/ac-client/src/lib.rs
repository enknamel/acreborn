//! A headless game session: the connection, the world the server describes,
//! our character, and the gameplay commands a UI or a script can issue.
//! Nothing here renders; several `Client`s can live in one process.

pub mod player;

use std::time::{Duration, Instant};

/// Things the session reports to whoever drives it (a UI, a script).
#[derive(Debug, Clone)]
pub enum Event {
    /// A line for the chat log; `kind` is the server's ChatMessageType.
    Chat { text: String, kind: u32 },
    /// A sound to play at a volume (0..=1).
    Sound {
        wave: std::rc::Rc<ac_formats::wave::Wave>,
        volume: f32,
    },
}

pub struct Client {
    pub socket: std::net::UdpSocket,
    pub primary: std::net::SocketAddr,
    pub secondary: std::net::SocketAddr,
    pub session: ac_net::session::Session,
    pub world: ac_world::World,
    pub assets: std::rc::Rc<ac_scene::Assets>,
    pub characters: Vec<ac_net::messages::CharacterEntry>,
    pub characters_known: bool,
    pub ddd_done: bool,
    pub enter_requested: bool,
    /// Landblock the static scene is built around, once the player is placed.
    pub scene_block: Option<u32>,
    /// Server-requested MoveTo for our own character, until the server
    /// reports us idle again.
    pub move_to: Option<ac_world::object::MoveTarget>,
    pub move_to_since: Instant,
    /// Melee combat mode is on.
    pub combat: bool,
    /// Magic combat mode is on.
    pub magic: bool,
    /// Spells we know by name (learnt from scrolls this session).
    pub known_spells: std::collections::HashMap<u32, String>,
    /// Creature we keep swinging at until it dies or we stop.
    pub attack_target: Option<u32>,
    /// An attack was sent and AttackDone has not come back yet.
    pub attack_pending: bool,
    pub last_attack: Instant,
    pub attack_backoff: Duration,
    /// Name of the last creature we attacked (its corpse is what we loot).
    pub last_target_name: String,
    pub sound_tables:
        std::collections::HashMap<u32, Option<std::rc::Rc<ac_formats::sound_table::SoundTable>>>,
    pub waves: std::collections::HashMap<u32, Option<std::rc::Rc<ac_formats::wave::Wave>>>,
    /// Items still to take from the open container, one at a time (the
    /// server refuses a second pickup while one is in progress).
    pub loot_queue: std::collections::VecDeque<u32>,
    pub loot_inflight: Option<(u32, Instant)>,
    pub selected: Option<u32>,
    pub last_click: Option<(Instant, u32)>,
    pub player: Option<player::Player>,
    pub player_setup: u32,
    /// Pending events for the driver.
    pub events: Vec<Event>,
}

impl Client {
    pub fn play_sound(&mut self, body: &[u8]) {
        use ac_net::messages::parse_sound;
        let Ok((guid, kind, volume)) = parse_sound(body) else {
            return;
        };
        let (name, table_id) = match self.world.objects.get(&guid) {
            Some(o) => (
                o.name.clone(),
                if o.sound_table_id != 0 {
                    o.sound_table_id
                } else if o.is_player
                    || o.object_desc_flags & ac_world::object_desc_flags::PLAYER != 0
                {
                    0x2000_0001
                } else {
                    0
                },
            ),
            None => return,
        };
        if table_id == 0 {
            return;
        }
        let assets = &self.assets;
        let table = self
            .sound_tables
            .entry(table_id)
            .or_insert_with(|| {
                assets
                    .portal
                    .read(table_id)
                    .ok()
                    .and_then(|b| ac_formats::sound_table::SoundTable::parse(table_id, &b).ok())
                    .map(std::rc::Rc::new)
            })
            .clone();
        let Some(table) = table else { return };
        let Some(wave_id) = ac_formats::sound_table::sound_for(&table, kind) else {
            return;
        };
        let wave = self
            .waves
            .entry(wave_id)
            .or_insert_with(|| {
                assets
                    .portal
                    .read(wave_id)
                    .ok()
                    .and_then(|b| ac_formats::wave::Wave::parse(wave_id, &b).ok())
                    .map(std::rc::Rc::new)
            })
            .clone();
        let Some(wave) = wave else { return };
        tracing::debug!("sound {kind:#x} from {name}: wave {wave_id:#010x} vol {volume:.2}");
        self.events.push(Event::Sound {
            wave,
            volume: volume.clamp(0.0, 1.0),
        });
    }

    pub fn chat_message(&mut self, op: u32, body: &[u8]) {
        use ac_net::messages::{event, opcode, ChatLine};
        if op == opcode::SOUND {
            self.play_sound(body);
            return;
        }
        let line = match op {
            opcode::HEAR_SPEECH => ChatLine::parse_hear_speech(body),
            opcode::HEAR_RANGED_SPEECH => ChatLine::parse_hear_ranged_speech(body),
            opcode::SERVER_MESSAGE => ChatLine::parse_server_message(body),
            opcode::EMOTE_TEXT => ChatLine::parse_emote_text(body),
            opcode::GAME_EVENT => match ac_net::messages::split_game_event(body) {
                Some((_, _, event::TELL, rest)) => ChatLine::parse_tell(rest),
                Some((_, _, event::IDENTIFY_OBJECT_RESPONSE, rest)) => {
                    self.appraisal(rest);
                    return;
                }
                Some((_, _, event::ATTACK_DONE, rest)) => {
                    let err = rest
                        .get(..4)
                        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                        .unwrap_or(0);
                    {
                        self.attack_pending = false;
                        // ACE always reports ActionCancelled (0x36) here: it
                        // just means the swing sequence ended.
                        tracing::debug!("attack done ({err:#x})");
                        self.attack_backoff = Duration::from_millis(300);
                    }
                    return;
                }
                Some((_, _, event::ATTACKER_NOTIFICATION, rest)) => {
                    match ac_net::messages::AttackNotice::parse_attacker(rest) {
                        Ok(n) => Ok(ChatLine {
                            text: format!(
                                "You {} {} for {} points{}.",
                                if n.critical { "critically hit" } else { "hit" },
                                n.name,
                                n.damage,
                                if n.percent >= 0.999 {
                                    ", killing it"
                                } else {
                                    ""
                                }
                            ),
                            sender: String::new(),
                            sender_id: 0,
                            kind: 5,
                        }),
                        Err(e) => Err(e),
                    }
                }
                Some((_, _, event::DEFENDER_NOTIFICATION, rest)) => {
                    match ac_net::messages::AttackNotice::parse_defender(rest) {
                        Ok(n) => Ok(ChatLine {
                            text: format!(
                                "{} {} you for {} points.",
                                n.name,
                                if n.critical {
                                    "critically hits"
                                } else {
                                    "hits"
                                },
                                n.damage
                            ),
                            sender: String::new(),
                            sender_id: 0,
                            kind: 6,
                        }),
                        Err(e) => Err(e),
                    }
                }
                Some((_, _, event::EVASION_ATTACKER_NOTIFICATION, rest)) => {
                    match ac_net::wire::Reader::new(rest).string16() {
                        Ok(n) => Ok(ChatLine {
                            text: format!("{n} evades your attack."),
                            sender: String::new(),
                            sender_id: 0,
                            kind: 5,
                        }),
                        Err(e) => Err(e),
                    }
                }
                Some((_, _, event::EVASION_DEFENDER_NOTIFICATION, rest)) => {
                    match ac_net::wire::Reader::new(rest).string16() {
                        Ok(n) => Ok(ChatLine {
                            text: format!("You evade {n}'s attack."),
                            sender: String::new(),
                            sender_id: 0,
                            kind: 6,
                        }),
                        Err(e) => Err(e),
                    }
                }
                Some((_, _, event::VICTIM_NOTIFICATION | event::KILLER_NOTIFICATION, rest)) => {
                    match ac_net::wire::Reader::new(rest).string16() {
                        Ok(t) => Ok(ChatLine {
                            text: t,
                            sender: String::new(),
                            sender_id: 0,
                            kind: 0,
                        }),
                        Err(e) => Err(e),
                    }
                }
                Some((_, _, event::TRANSIENT_STRING, rest)) => {
                    match ac_net::wire::Reader::new(rest).string16() {
                        Ok(t) => Ok(ChatLine {
                            text: t,
                            sender: String::new(),
                            sender_id: 0,
                            kind: 7,
                        }),
                        Err(e) => Err(e),
                    }
                }
                Some((_, _, event::WEENIE_ERROR, rest)) => {
                    let code = rest
                        .get(..4)
                        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                        .unwrap_or(0);
                    tracing::info!("weenie error {code:#x}");
                    return;
                }
                Some((_, _, event::POPUP_STRING, rest)) => {
                    match ac_net::wire::Reader::new(rest).string16() {
                        Ok(t) => Ok(ChatLine {
                            text: t,
                            sender: String::new(),
                            sender_id: 0,
                            kind: 0,
                        }),
                        Err(e) => Err(e),
                    }
                }
                _ => return,
            },
            _ => return,
        };
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("chat message {op:#06x}: {e}");
                return;
            }
        };
        let text = match (op, line.sender.is_empty()) {
            (_, true) => line.text.clone(),
            (opcode::EMOTE_TEXT, _) => format!("{} {}", line.sender, line.text),
            (opcode::GAME_EVENT, _) => format!("{} tells you, \"{}\"", line.sender, line.text),
            _ => format!("{} says, \"{}\"", line.sender, line.text),
        };
        tracing::info!("chat: {text}");
        self.events.push(Event::Chat {
            text,
            kind: line.kind,
        });
    }

    pub fn interact(&mut self, guid: u32) {
        use ac_net::messages::action;
        use ac_world::{item_type, object_desc_flags};
        let me = self.world.player_guid;
        let Some(o) = self.world.objects.get(&guid) else {
            return;
        };
        let stuck = o.object_desc_flags
            & (object_desc_flags::STUCK
                | object_desc_flags::PLAYER
                | object_desc_flags::DOOR
                | object_desc_flags::VENDOR
                | object_desc_flags::PORTAL
                | object_desc_flags::CORPSE)
            != 0
            || o.item_type & item_type::CREATURE != 0;
        let carried = me.is_some() && (o.container == me || o.wielder == me);
        let name = o.name.clone();
        let attackable = o.object_desc_flags & object_desc_flags::ATTACKABLE != 0
            && o.object_desc_flags & object_desc_flags::PLAYER == 0
            && o.item_type & item_type::CREATURE != 0;
        let mut w = ac_net::wire::Writer::new();
        if self.combat && attackable {
            self.attack(guid);
            return;
        }
        if carried && o.spell_id != 0 {
            if let Some(spell) = name.strip_prefix("Scroll of ") {
                self.known_spells.insert(o.spell_id, spell.to_string());
            }
        }
        if carried && o.wielder != me && o.item_type & item_type::WIELDABLE != 0 {
            tracing::info!("wield {name} ({guid:#010x}) at {:#x}", o.valid_locations);
            w.u32(guid).u32(o.valid_locations);
            self.session
                .send_action(action::GET_AND_WIELD_ITEM, &w.finish());
        } else if carried && o.wielder == me {
            tracing::info!("take off {name} ({guid:#010x})");
            w.u32(guid).u32(me.unwrap_or(0)).u32(0);
            self.session
                .send_action(action::PUT_ITEM_IN_CONTAINER, &w.finish());
        } else if !carried && !stuck && o.position.is_some() {
            tracing::info!("pick up {name} ({guid:#010x})");
            w.u32(guid).u32(me.unwrap_or(0)).u32(0);
            self.session
                .send_action(action::PUT_ITEM_IN_CONTAINER, &w.finish());
        } else {
            tracing::info!("use {name} ({guid:#010x})");
            self.session.send_action(action::USE, &guid.to_le_bytes());
        }
    }

    pub fn cast(&mut self, spell: u32) {
        use ac_net::messages::{action, combat_mode};
        if !self.magic {
            self.session.send_action(
                action::CHANGE_COMBAT_MODE,
                &combat_mode::MAGIC.to_le_bytes(),
            );
            self.magic = true;
            self.combat = false;
        }
        let table = self.assets.spell_table().ok();
        let entry = table.as_ref().and_then(|t| t.get(spell));
        let name = entry
            .map(|sp| sp.name.clone())
            .or_else(|| self.known_spells.get(&spell).cloned())
            .unwrap_or_default();
        // Spells that need a target go to the selected creature (or
        // ourselves when nothing is selected); the rest are self casts.
        let needs_target = entry
            .map(|sp| sp.needs_target())
            .unwrap_or_else(|| name.contains("Other"));
        let target = if needs_target {
            self.selected.or(self.world.player_guid)
        } else {
            None
        };
        match target {
            Some(t) => {
                tracing::info!("cast {name} ({spell}) on {t:#010x}");
                self.session.send_action(
                    action::CAST_TARGETED_SPELL,
                    &ac_net::messages::cast_targeted(t, spell),
                );
            }
            None => {
                tracing::info!("cast {name} ({spell})");
                self.session
                    .send_action(action::CAST_UNTARGETED_SPELL, &spell.to_le_bytes());
            }
        }
    }

    pub fn attack(&mut self, guid: u32) {
        use ac_net::messages::action;
        if !self.combat {
            return;
        }
        let name = self
            .world
            .objects
            .get(&guid)
            .map(|o| o.name.clone())
            .unwrap_or_default();
        tracing::info!("attack {name} ({guid:#010x})");
        self.last_target_name = name.clone();
        let mut w = ac_net::wire::Writer::new();
        w.u32(guid).u32(2).f32(0.5);
        self.session
            .send_action(action::TARGETED_MELEE_ATTACK, &w.finish());
        self.attack_target = Some(guid);
        self.attack_pending = true;
        self.last_attack = Instant::now();
        self.selected = Some(guid);
    }

    pub fn tick_combat(&mut self) {
        let Some(target) = self.attack_target else {
            return;
        };
        let alive = self
            .world
            .objects
            .get(&target)
            .map(|o| o.health.is_none_or(|h| h > 0.0))
            .unwrap_or(false);
        if !alive || !self.combat {
            tracing::info!("attack target gone");
            self.attack_target = None;
            self.attack_pending = false;
            return;
        }
        // Re-sending while the server walks us to the target would cancel
        // that walk, so wait for it; back off after a refused attack.
        if !self.attack_pending
            && self.move_to.is_none()
            && self.last_attack.elapsed() > self.attack_backoff
        {
            self.attack(target);
        }
    }

    pub fn use_by_name(&mut self, name: &str) -> bool {
        let me = self.player.as_ref().map(|p| p.world_position());
        let my_guid = self.world.player_guid;
        let mut best: Option<(f32, u32)> = None;
        for o in self.world.objects.values() {
            if !o.name.starts_with(name) {
                continue;
            }
            // Carried items count as distance zero, so they win over the floor;
            // exact names beat prefix matches.
            let carried = my_guid.is_some() && (o.container == my_guid || o.wielder == my_guid);
            let exact = if o.name == name { 0.0 } else { 1000.0 };
            let d = exact
                + if carried {
                    0.0
                } else {
                    let Some(p) = o.display.or(o.position) else {
                        continue;
                    };
                    me.map(|m| (ac_world::landblock_origin(p.cell) + p.local).distance(m))
                        .unwrap_or(0.0)
                };
            if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                best = Some((d, o.guid));
            }
        }
        match best {
            Some((_, guid)) => {
                self.selected = Some(guid);
                self.interact(guid);
                true
            }
            None => {
                tracing::debug!("no object named {name:?} in view yet");
                false
            }
        }
    }

    pub fn toggle_combat(&mut self) {
        use ac_net::messages::{action, combat_mode};
        self.combat = !self.combat;
        self.magic = false;
        let mode = if self.combat {
            combat_mode::MELEE
        } else {
            combat_mode::NON_COMBAT
        };
        tracing::info!(
            "combat mode {}",
            if self.combat { "melee" } else { "peace" }
        );
        self.session
            .send_action(action::CHANGE_COMBAT_MODE, &mode.to_le_bytes());
        if !self.combat {
            self.attack_target = None;
            self.attack_pending = false;
        }
    }

    fn appraisal(&mut self, body: &[u8]) {
        use ac_net::messages::Appraisal;
        let a = match Appraisal::parse(body) {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!("appraisal: {e}");
                return;
            }
        };
        let name = self
            .world
            .objects
            .get(&a.guid)
            .map(|o| o.name.clone())
            .unwrap_or_else(|| format!("{:#010x}", a.guid));
        let mut lines = vec![name.clone()];
        for key in [
            Appraisal::STRING_SHORT_DESC,
            Appraisal::STRING_LONG_DESC,
            Appraisal::STRING_USE,
        ] {
            if let Some(t) = a.string(key) {
                if !t.is_empty() {
                    lines.push(t.to_string());
                }
            }
        }
        if !a.success {
            lines.push("(appraisal failed)".into());
        }
        tracing::info!(
            "appraise {name}: {} properties",
            a.ints.len() + a.strings.len()
        );
        for l in lines {
            self.events.push(Event::Chat { text: l, kind: 1 });
        }
    }

    pub fn tick_loot(&mut self) {
        use ac_net::messages::action;
        let me = self.world.player_guid.unwrap_or(0);
        // The in-flight pickup is done once the item is ours, gone, or stale.
        if let Some((guid, since)) = self.loot_inflight {
            let landed = self
                .world
                .objects
                .get(&guid)
                .is_none_or(|o| o.container == Some(me) || o.wielder == Some(me));
            if landed || since.elapsed() > Duration::from_secs(4) {
                self.loot_inflight = None;
            }
        }
        if self.loot_inflight.is_none() {
            if let Some(guid) = self.loot_queue.pop_front() {
                let name = self
                    .world
                    .objects
                    .get(&guid)
                    .map(|o| o.name.clone())
                    .unwrap_or_default();
                tracing::info!("take {name} ({guid:#010x})");
                let mut w = ac_net::wire::Writer::new();
                w.u32(guid).u32(me).u32(0);
                self.session
                    .send_action(action::PUT_ITEM_IN_CONTAINER, &w.finish());
                self.loot_inflight = Some((guid, Instant::now()));
            }
        }
    }

    /// Queue an item of the open container to be picked up.
    pub fn take(&mut self, guid: u32) {
        if !self.loot_queue.contains(&guid) && self.loot_inflight.map(|(g, _)| g) != Some(guid) {
            self.loot_queue.push_back(guid);
        }
    }

    /// Stop looking into the open ground container.
    pub fn close_container(&mut self) {
        use ac_net::messages::action;
        if let Some((c, _)) = self.world.open_container.take() {
            self.session
                .send_action(action::NO_LONGER_VIEWING_CONTENTS, &c.to_le_bytes());
        }
    }

    /// Buy one of a vendor's stock items.
    pub fn buy(&mut self, guid: u32) {
        use ac_net::messages::{action, trade};
        let Some(vendor) = self.world.open_vendor.as_ref().map(|v| v.vendor) else {
            return;
        };
        tracing::info!("buy {guid:#010x} from {vendor:#010x}");
        self.session
            .send_action(action::BUY, &trade(vendor, &[(guid, 1)]));
    }

    /// Sell a pack item (its whole stack) to the open vendor.
    pub fn sell(&mut self, guid: u32) {
        use ac_net::messages::{action, trade};
        let Some(vendor) = self.world.open_vendor.as_ref().map(|v| v.vendor) else {
            return;
        };
        let amount = self
            .world
            .objects
            .get(&guid)
            .map(|o| o.stack_size.max(1) as i32)
            .unwrap_or(1);
        tracing::info!("sell {guid:#010x} to {vendor:#010x}");
        self.session
            .send_action(action::SELL, &trade(vendor, &[(guid, amount)]));
    }

    pub fn close_vendor(&mut self) {
        self.world.open_vendor = None;
    }

    /// Say something (or an @command) in local chat.
    pub fn say(&mut self, text: &str) {
        let mut w = ac_net::wire::Writer::new();
        w.string16(text);
        self.session
            .send_action(ac_net::messages::action::TALK, &w.finish());
    }

    /// Select an object (what the target bar and appraisal refer to).
    pub fn select(&mut self, guid: Option<u32>) {
        self.selected = guid;
    }

    /// Events produced since the last drain.
    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
}
