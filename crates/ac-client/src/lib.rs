//! A headless game session: the connection, the world the server describes,
//! our character, and the gameplay commands a UI or a script can issue.
//! Nothing here renders; several `Client`s can live in one process.

pub mod creation;
pub mod magic;
pub mod player;
pub mod route;

use std::time::{Duration, Instant};

/// Things the session reports to whoever drives it (a UI, a script).
#[derive(Debug, Clone)]
pub enum Event {
    /// A line for the chat log; `kind` is the server's ChatMessageType.
    Chat {
        text: String,
        kind: u32,
    },
    /// A sound to play at a volume (0..=1).
    Sound {
        wave: std::rc::Rc<ac_formats::wave::Wave>,
        volume: f32,
    },
    Connected,
    Terminated(String),
    /// CharacterError / AccountBoot opcode.
    Refused(u32),
    /// The character stands in the world; a scene can be built around it.
    Placed {
        cell: u32,
    },
    /// A spell entered the spellbook (MagicUpdateSpell).
    SpellLearned(u32),
    /// A spell left the spellbook (MagicRemoveSpell).
    SpellForgotten(u32),
    /// The account's characters, when the client is not entering the
    /// world by itself (`Config::auto_enter` off and no `character`
    /// named, or the named one is missing). Re-emitted whenever the
    /// server refreshes the list (after a delete, entries pending
    /// deletion carry `seconds_until_deleted > 0`) and after a restore.
    Characters(Vec<ac_net::messages::CharacterEntry>),
    /// The server accepted a `create_character`; the client enters the
    /// world with it.
    CharacterCreated {
        id: u32,
        name: String,
    },
    /// The server refused a `create_character` (or a
    /// `restore_character`, which is answered by the same message) with
    /// an ACE `CharacterGenerationVerificationResponse` code; see
    /// `creation::create_failure_message`.
    CharacterCreateFailed(u32),
}

/// How to reach the server and who to be.
#[derive(Debug, Clone)]
pub struct Config {
    /// `host` or `host:port` of the login (primary) port.
    pub host: String,
    pub account: String,
    pub password: String,
    /// Character to enter with; the first on the account when None.
    pub character: Option<String>,
    /// Enter the world as soon as the character list arrives (with
    /// `character`, or the first one). Off, and with no `character`
    /// named, the client emits `Event::Characters` instead and waits for
    /// `enter_world` / `create_character`. A named `character` always
    /// auto-enters.
    pub auto_enter: bool,
}

/// What the character did this frame, for whoever draws it.
#[derive(Debug, Default)]
pub struct PlayerFrame {
    /// Position, look or pose changed: the model needs re-placing.
    pub dirty: bool,
    /// Part transforms from the current animation frame.
    pub pose: Option<Vec<glam::Mat4>>,
}

pub struct Client {
    pub config: Config,
    pub socket: std::net::UdpSocket,
    pub primary: std::net::SocketAddr,
    pub secondary: std::net::SocketAddr,
    pub session: ac_net::session::Session,
    pub world: ac_world::World,
    pub assets: std::rc::Rc<ac_scene::Assets>,
    pub characters: Vec<ac_net::messages::CharacterEntry>,
    pub characters_known: bool,
    pub ddd_done: bool,
    /// CharacterEnterWorldRequest was sent (see `entering`).
    pub enter_requested: bool,
    /// Character we are entering (or will, once the handshake is done).
    pub entering: Option<u32>,
    /// A create or restore whose CharacterCreateResponse is still to come.
    pub pending_create: Option<creation::Pending>,
    /// Landblock the static scene is built around, once the player is placed.
    pub scene_block: Option<u32>,
    /// Server-requested MoveTo for our own character, until the server
    /// reports us idle again.
    pub move_to: Option<ac_world::object::MoveTarget>,
    pub move_to_since: Instant,
    /// Route steering toward `move_to` when the straight line to it is
    /// blocked (see `route`).
    pub steering: route::Steering,
    /// Melee combat mode is on.
    pub combat: bool,
    /// Magic combat mode is on.
    pub magic: bool,
    /// Names of spells learnt from scrolls this session, for spells the
    /// SpellTable lacks; `world.stats.spells` is the spellbook itself.
    pub known_spells: std::collections::HashMap<u32, String>,
    /// Refuse to cast when components of the current formula are missing
    /// (`CastCheck::MissingComponents`). Off by default: the server
    /// decides whether components are required (`require_spell_comps`),
    /// and a refused cast is only a chat line.
    pub require_components: bool,
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
    /// Open the sockets, start the login handshake, and return the session.
    pub fn connect(config: Config, assets: std::rc::Rc<ac_scene::Assets>) -> std::io::Result<Self> {
        use ac_net::messages::DatIteration;
        use ac_net::session::{Config as NetConfig, Session};
        let host = config.host.clone();
        let primary: std::net::SocketAddr = if host.contains(':') {
            host.parse().map_err(std::io::Error::other)?
        } else {
            format!("{host}:9000")
                .parse()
                .map_err(std::io::Error::other)?
        };
        let secondary = std::net::SocketAddr::new(primary.ip(), primary.port() + 1);
        let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;
        let now = Instant::now();
        let mut session = Session::new(
            NetConfig {
                account: config.account.clone(),
                password: config.password.clone(),
                dats: vec![
                    DatIteration {
                        dat_file_id: 1,
                        dat_file_type: 0,
                        iterations: 2072,
                    },
                    DatIteration {
                        dat_file_id: 2,
                        dat_file_type: 0,
                        iterations: 982,
                    },
                ],
                echo_interval: Duration::from_secs(5),
                ack_interval: Duration::from_secs(2),
            },
            now,
        );
        session.login(now);
        tracing::info!("connecting to {primary} as {}", config.account);
        Ok(Client {
            config,
            socket,
            primary,
            secondary,
            session,
            world: ac_world::World::default(),
            assets,
            characters: Vec::new(),
            characters_known: false,
            ddd_done: false,
            enter_requested: false,
            entering: None,
            pending_create: None,
            scene_block: None,
            move_to: None,
            move_to_since: Instant::now(),
            steering: route::Steering::new(Instant::now()),
            combat: false,
            magic: false,
            known_spells: Default::default(),
            require_components: false,
            attack_target: None,
            attack_pending: false,
            last_attack: Instant::now(),
            attack_backoff: Duration::from_millis(300),
            last_target_name: String::new(),
            sound_tables: Default::default(),
            waves: Default::default(),
            loot_queue: Default::default(),
            loot_inflight: None,
            selected: None,
            last_click: None,
            player: None,
            player_setup: 0,
            events: Vec::new(),
        })
    }

    /// Send a clean disconnect (flushing it immediately).
    pub fn disconnect(&mut self, now: Instant) {
        self.session.disconnect(now);
        self.flush_outgoing();
    }

    fn flush_outgoing(&mut self) {
        use ac_net::session::Port;
        for (port, dg) in self.session.outgoing() {
            let to = if port == Port::Primary {
                self.primary
            } else {
                self.secondary
            };
            let _ = self.socket.send_to(&dg, to);
        }
    }

    /// The character is in the world and our physics body exists.
    pub fn placed(&self) -> bool {
        self.scene_block.is_some()
    }

    /// Pump the network, apply the server's messages, run the gameplay
    /// timers, and once the character is placed run its physics with
    /// `input` (None for a session nobody is steering). Returns what the
    /// renderer needs to know about the character this frame.
    pub fn tick(&mut self, input: Option<player::Input>, dt: f32, now: Instant) -> PlayerFrame {
        use ac_net::messages::{self, opcode, queue};
        use ac_net::session::{Event, Port};
        let mut chat_pending: Vec<(u32, Vec<u8>)> = Vec::new();
        for (port, dg) in self.session.outgoing() {
            let to = if port == Port::Primary {
                self.primary
            } else {
                self.secondary
            };
            let _ = self.socket.send_to(&dg, to);
        }
        let mut buf = [0u8; 2048];
        while let Ok((n, _)) = self.socket.recv_from(&mut buf) {
            self.session.receive(&buf[..n], now);
        }
        self.session.poll(now);
        for ev in self.session.events() {
            match ev {
                Event::Connected { client_id } => {
                    tracing::info!("connected, client id {client_id}");
                    self.events.push(self::Event::Connected);
                }
                Event::Terminated(why) => {
                    tracing::warn!("terminated: {why}");
                    self.events.push(self::Event::Terminated(why));
                }
                Event::Message(msg) => {
                    match self.world.apply(&msg) {
                        ac_world::Applied::PlayerSet => {
                            // The server ignores our positions until we say we landed.
                            self.session
                                .send_action(ac_net::messages::action::LOGIN_COMPLETE, &[]);
                            continue;
                        }
                        ac_world::Applied::Moved => continue,
                        ac_world::Applied::PlayerMoved => {
                            // A teleport or a server correction: stand where
                            // the server put us and forget the current route.
                            if let (Some(pl), Some(p)) = (
                                self.player.as_mut(),
                                self.world.player().and_then(|o| o.position),
                            ) {
                                pl.cell = p.cell;
                                pl.local = p.local;
                                pl.dirty = true;
                            }
                            self.steering.reset();
                        }
                        ac_world::Applied::PlayerMoveTo | ac_world::Applied::PlayerMotion => {
                            // A MoveTo aimed at us is ours to carry out; a plain
                            // motion state for us means the server is done
                            // walking us (or echoed our own state).
                            let stance = self.world.player().map(|o| o.motion.style);
                            let target = self.world.player_mut().and_then(|o| o.target.take());
                            match target {
                                Some(t) => {
                                    if self.move_to.is_none() {
                                        tracing::debug!("server move-to {t:?}");
                                        self.move_to_since = Instant::now();
                                    }
                                    self.move_to = Some(t);
                                }
                                None => {
                                    if self.move_to.take().is_some() {
                                        tracing::debug!("server move-to finished");
                                        // Take the server's idea of where we ended up.
                                        if let (Some(pl), Some(p)) = (
                                            self.player.as_mut(),
                                            self.world.player().and_then(|o| o.position),
                                        ) {
                                            pl.cell = p.cell;
                                            pl.local = p.local;
                                            pl.dirty = true;
                                        }
                                    }
                                }
                            }
                            // Our stance follows the server (combat mode changes).
                            if let (Some(st), Some(pl)) = (stance, self.player.as_mut()) {
                                if st != 0 {
                                    pl.set_stance(&self.assets, 0x8000_0000 | st as u32);
                                }
                            }
                            continue;
                        }
                        ac_world::Applied::Appearance => {
                            // Our own look changed: redraw the character.
                            if let Some(pl) = self.player.as_mut() {
                                pl.dirty = true;
                            }
                            continue;
                        }
                        ac_world::Applied::Spellbook { spell, known } => {
                            tracing::info!(
                                "spell {spell} {}",
                                if known { "learned" } else { "forgotten" }
                            );
                            self.events.push(if known {
                                self::Event::SpellLearned(spell)
                            } else {
                                self::Event::SpellForgotten(spell)
                            });
                            continue;
                        }
                        ac_world::Applied::Created
                        | ac_world::Applied::Deleted
                        | ac_world::Applied::Stats
                        | ac_world::Applied::Enchantments
                        | ac_world::Applied::Health
                        | ac_world::Applied::Vendor
                        | ac_world::Applied::Inventory => continue,
                        ac_world::Applied::Failed => tracing::warn!("failed to apply a message"),
                        ac_world::Applied::Ignored => {}
                    }
                    if let Some((op, body)) = messages::split(&msg) {
                        chat_pending.push((op, body.to_vec()));
                    }
                    let Some((op, body)) = messages::split(&msg) else {
                        continue;
                    };
                    match op {
                        opcode::CHARACTER_LIST => {
                            if let Ok(cl) = messages::CharacterList::parse(body) {
                                tracing::info!(
                                    "characters: {:?}",
                                    cl.characters.iter().map(|c| &c.name).collect::<Vec<_>>()
                                );
                                self.characters = cl.characters;
                                self.characters_known = true;
                                self.lobby_ready();
                            }
                        }
                        opcode::DDD_END_DDD => {
                            self.ddd_done = true;
                            self.lobby_ready();
                        }
                        opcode::CHARACTER_CREATE_RESPONSE => {
                            match messages::CharacterCreateResponse::parse(body) {
                                Ok(r) => self.create_response(r),
                                Err(e) => tracing::warn!("CharacterCreateResponse: {e}"),
                            }
                        }
                        opcode::CHARACTER_DELETE => {
                            // Acknowledged; the refreshed list follows.
                            tracing::info!("character deletion acknowledged");
                        }
                        _ => {}
                    }
                    let Some((op, _)) = messages::split(&msg) else {
                        continue;
                    };
                    match op {
                        opcode::CHARACTER_ENTER_WORLD_SERVER_READY => {
                            let id = self
                                .entering
                                .or_else(|| self.pick_character().map(|c| c.id));
                            if let Some(id) = id {
                                let account = self.config.account.clone();
                                self.session
                                    .send_message(queue::UI, messages::enter_world(id, &account));
                            } else {
                                tracing::warn!("server ready but no character to enter with");
                            }
                        }
                        opcode::PLAYER_TELEPORT => {
                            // After a server teleport, take the new position and
                            // tell the server we landed.
                            if let (Some(pl), Some(p)) = (
                                self.player.as_mut(),
                                self.world.player().and_then(|o| o.position),
                            ) {
                                pl.cell = p.cell;
                                pl.local = p.local;
                                pl.dirty = true;
                            }
                            self.session
                                .send_action(ac_net::messages::action::LOGIN_COMPLETE, &[]);
                        }
                        opcode::CHARACTER_ERROR | opcode::ACCOUNT_BOOT => {
                            let code = body
                                .get(..4)
                                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                                .unwrap_or(0);
                            tracing::error!("server refused: {op:#06x} (code {code:#x})");
                            // A refused enter (not owned, still in world,
                            // pending deletion) may be retried with another
                            // character.
                            if op == opcode::CHARACTER_ERROR && self.scene_block.is_none() {
                                self.enter_requested = false;
                                self.entering = None;
                            }
                            self.events.push(self::Event::Refused(op));
                        }
                        opcode::GAME_EVENT => {
                            if let Some((_, _, ev, rest)) = messages::split_game_event(body) {
                                if ev == 0x00A0 && rest.len() >= 8 {
                                    let item =
                                        u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
                                    let err =
                                        u32::from_le_bytes([rest[4], rest[5], rest[6], rest[7]]);
                                    tracing::warn!(
                                        "inventory action failed for {item:#010x}, error {err:#x}"
                                    );
                                    if self.loot_inflight.map(|(g, _)| g) == Some(item) {
                                        self.loot_inflight = None;
                                    }
                                } else if ev == ac_net::messages::event::USE_DONE && rest.len() >= 4
                                {
                                    let err =
                                        u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
                                    tracing::debug!("use done, error {err:#x}");
                                    // The walk the server asked for is over either way.
                                    self.move_to = None;
                                } else {
                                    tracing::debug!("game event {ev:#06x} ({} bytes)", rest.len());
                                }
                            }
                        }
                        _ => tracing::debug!("message {op:#06x} ({} bytes)", body.len()),
                    }
                }
            }
        }
        for (op, body) in chat_pending {
            self.chat_message(op, &body);
        }
        self.tick_combat();
        self.tick_loot();
        // Build the static scene once the player is placed.
        if self.scene_block.is_none() {
            if let Some(p) = self.world.player().and_then(|o| o.position) {
                let block = p.landblock();
                tracing::info!(
                    "player at cell {:#010x} local {:?}; loading landblocks",
                    p.cell,
                    p.local
                );
                self.player_setup = self
                    .world
                    .player()
                    .map(|o| o.setup_id)
                    .unwrap_or(0x0200_0001);
                let mut pl = player::Player::new(&self.assets, p.cell, p.local, p.rotation);
                let table_id = self
                    .world
                    .player()
                    .map(|o| o.motion_table_id)
                    .filter(|&t| t != 0)
                    .unwrap_or(0x0900_0001);
                pl.set_motion_table(&self.assets, self.player_setup, table_id);
                self.player = Some(pl);
                self.events.push(self::Event::Placed { cell: p.cell });
                self.scene_block = Some(block);
            }
        }
        self.world.tick(dt);
        self.tick_player(input.unwrap_or_default(), dt, now)
    }

    /// The character `Config::character` names (case-insensitive), or the
    /// first on the account.
    fn pick_character(&self) -> Option<&ac_net::messages::CharacterEntry> {
        match &self.config.character {
            Some(name) => self
                .characters
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(name)),
            None => self.characters.first(),
        }
    }

    /// Once the DDD exchange is done and the character list is known:
    /// send a held `enter_world`, enter with the configured (or first)
    /// character when auto-entering, or hand the list to the driver and
    /// wait. Runs again on every refreshed list.
    fn lobby_ready(&mut self) {
        if !(self.ddd_done && self.characters_known) {
            return;
        }
        if self.enter_requested {
            return;
        }
        if self.entering.is_some() {
            self.send_enter_request();
            return;
        }
        let auto = self.config.auto_enter || self.config.character.is_some();
        let pick = if auto {
            self.pick_character().map(|c| c.id)
        } else {
            None
        };
        match pick {
            Some(id) => self.enter_world(id),
            None => {
                if auto {
                    match &self.config.character {
                        Some(name) => tracing::error!(
                            "no character named {name:?} on this account (have {:?})",
                            self.characters.iter().map(|c| &c.name).collect::<Vec<_>>()
                        ),
                        None => tracing::error!(
                            "no character on this account; create one (acclient/acbot --create)"
                        ),
                    }
                }
                self.events
                    .push(self::Event::Characters(self.characters.clone()));
            }
        }
    }

    fn tick_player(&mut self, input: player::Input, dt: f32, now: Instant) -> PlayerFrame {
        // Player movement, camera, and reporting.
        if let Some(pl) = self.player.as_mut() {
            let mut input = input;
            // Server-driven MoveTo (using something out of reach): run toward
            // the target until close enough, unless the user takes over.
            let manual = input.forward != 0.0 || input.strafe != 0.0;
            if self.move_to.is_none() || manual {
                self.steering.reset();
            }
            if let Some(t) = self.move_to {
                let goal = match t {
                    ac_world::object::MoveTarget::Object(g) => self
                        .world
                        .objects
                        .get(&g)
                        .and_then(|o| o.display.or(o.position))
                        .map(|p| (ac_world::landblock_origin(p.cell) + p.local, 1.0, p.cell)),
                    ac_world::object::MoveTarget::Position { cell, local } => {
                        Some((ac_world::landblock_origin(cell) + local, 0.3, cell))
                    }
                };
                if let Some((g, stop, goal_cell)) = goal {
                    let d = g - pl.world_position();
                    let flat = glam::Vec2::new(d.x, d.y);
                    if !manual && flat.length() > stop {
                        // Straight at the goal while nothing is in the
                        // way; through the waypoints of a route otherwise.
                        let aim = self.steering.steer(pl, &self.assets, g, goal_cell, now);
                        let d = aim - pl.world_position();
                        let flat = glam::Vec2::new(d.x, d.y);
                        if flat.length() > 1e-3 {
                            pl.heading = (-flat.x).atan2(flat.y);
                        }
                        input.forward = 1.0;
                        input.run = true;
                    }
                }
            }
            pl.update(&self.assets, &input, dt);
            if let Some(j) = pl.last_jump.take() {
                tracing::info!("jump power {:.2} velocity {:?}", j.power, j.velocity);
                self.session.send_action(
                    ac_net::messages::action::JUMP,
                    &ac_net::messages::jump(j.power, j.velocity.to_array(), 1),
                );
            }
            // One-shot motions the server broadcast for us (attacks, emotes).
            let mut cmds = Vec::new();
            if let Some(o) = self.world.player_mut() {
                while let Some(c) = o.commands.pop() {
                    cmds.push(c);
                }
            }
            for c in cmds {
                pl.play_command(&self.assets, c.command as u32, c.speed);
            }
            let pose = pl.animate(&self.assets, &input, dt);
            if pose.is_some() {
                pl.dirty = true;
            }
            let quiet =
                self.move_to.is_some() && self.move_to_since.elapsed() < Duration::from_secs(12);
            if !quiet && self.move_to.is_some() {
                tracing::debug!("server move-to timed out");
                self.move_to = None;
            }
            pl.report(&mut self.session, &input, now, quiet);
            let dirty = pl.dirty;
            if pl.dirty {
                pl.dirty = false;
                if let Some(o) = self.world.player_mut() {
                    o.position = Some(ac_world::Position {
                        cell: pl.cell,
                        local: pl.local,
                        rotation: pl.rotation(),
                    });
                }
            }
            return PlayerFrame { dirty, pose };
        }
        PlayerFrame::default()
    }

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

    /// Cast a spell (see [`Client::try_cast`]); the outcome is logged.
    pub fn cast(&mut self, spell: u32) {
        let _ = self.try_cast(spell);
    }

    /// Cast a spell on the selected target (or ourselves), switching to
    /// magic mode first when needed. `CastCheck::Ok` once the cast was
    /// sent. Without a wielded caster nothing is sent (`NoCaster`: the
    /// server would only drop us back to peace mode); missing components
    /// refuse the cast only with `require_components` set. An unknown
    /// spell or low mana is logged and sent anyway, the server being the
    /// judge (Mana Conversion can make a cast the estimate rejects).
    pub fn try_cast(&mut self, spell: u32) -> magic::CastCheck {
        use ac_net::messages::{action, combat_mode};
        use magic::CastCheck;
        let check = self.can_cast(spell);
        match &check {
            CastCheck::Ok => {}
            CastCheck::NoCaster => {
                tracing::warn!("cannot cast {spell}: no magic caster wielded");
                return check;
            }
            CastCheck::MissingComponents(missing) if self.require_components => {
                tracing::warn!("cannot cast {spell}: missing components {missing:?}");
                return check;
            }
            other => tracing::info!("casting {spell} although {other:?}"),
        }
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
        CastCheck::Ok
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

    /// Wield the carried item whose name starts with `name` (the first
    /// exact match wins); nothing happens when it is wielded already.
    /// Returns false when no such item is carried.
    pub fn wield_by_name(&mut self, name: &str) -> bool {
        use ac_net::messages::action;
        let me = self.world.player_guid;
        let Some((guid, locations, wielded)) = self
            .world
            .objects
            .values()
            .filter(|o| me.is_some() && (o.container == me || o.wielder == me))
            .filter(|o| o.name.starts_with(name))
            .min_by_key(|o| if o.name == name { 0 } else { 1 })
            .map(|o| (o.guid, o.valid_locations, o.wielder == me))
        else {
            return false;
        };
        if !wielded {
            let mut w = ac_net::wire::Writer::new();
            w.u32(guid).u32(locations);
            self.session
                .send_action(action::GET_AND_WIELD_ITEM, &w.finish());
        }
        true
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

    /// Drop a carried item on the ground in front of the character
    /// (DropItem 0x001B). The server answers with InventoryPutObjectIn3D
    /// and creates the object in the world.
    pub fn drop_item(&mut self, guid: u32) -> bool {
        use ac_net::messages::action;
        let me = self.world.player_guid;
        let Some(o) = self.world.objects.get(&guid) else {
            return false;
        };
        if me.is_none() || (o.container != me && o.wielder != me) {
            return false;
        }
        tracing::info!("drop {} ({guid:#010x})", o.name);
        self.session
            .send_action(action::DROP_ITEM, &guid.to_le_bytes());
        true
    }

    /// Move a carried item into a container (PutItemInContainer 0x0019):
    /// our own guid for the main pack, a carried side pack, or the ground
    /// container we are looking into (a chest; corpses refuse).
    pub fn put_in_container(&mut self, item: u32, container: u32) -> bool {
        use ac_net::messages::action;
        let me = self.world.player_guid;
        let Some(o) = self.world.objects.get(&item) else {
            return false;
        };
        if me.is_none() || (o.container != me && o.wielder != me) || item == container {
            return false;
        }
        let name = o.name.clone();
        let target = self
            .world
            .objects
            .get(&container)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "pack".into());
        tracing::info!("put {name} ({item:#010x}) in {target} ({container:#010x})");
        let mut w = ac_net::wire::Writer::new();
        w.u32(item).u32(container).u32(0);
        self.session
            .send_action(action::PUT_ITEM_IN_CONTAINER, &w.finish());
        true
    }

    /// Hand a carried item (or `amount` of a stack; the whole stack when
    /// None) to an NPC or another player (GiveObjectRequest 0x00CD). The
    /// target must be in reach; NPCs answer with an emote, players must
    /// allow gifts in their options.
    pub fn give(&mut self, target: u32, item: u32, amount: Option<u32>) -> bool {
        use ac_net::messages::action;
        let me = self.world.player_guid;
        let Some(o) = self.world.objects.get(&item) else {
            return false;
        };
        if me.is_none() || (o.container != me && o.wielder != me) || Some(target) == me {
            return false;
        }
        let amount = amount.unwrap_or(o.stack_size.max(1)).max(1);
        let name = o.name.clone();
        let who = self
            .world
            .objects
            .get(&target)
            .map(|t| t.name.clone())
            .unwrap_or_default();
        tracing::info!("give {amount} x {name} ({item:#010x}) to {who} ({target:#010x})");
        let mut w = ac_net::wire::Writer::new();
        w.u32(target).u32(item).u32(amount);
        self.session
            .send_action(action::GIVE_OBJECT_REQUEST, &w.finish());
        true
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
        self.buy_amount(guid, 1);
    }

    /// Buy `amount` of a vendor's stock item (a stack for stackables).
    pub fn buy_amount(&mut self, guid: u32, amount: u32) {
        use ac_net::messages::{action, trade};
        let Some(vendor) = self.world.open_vendor.as_ref().map(|v| v.vendor) else {
            return;
        };
        if amount == 0 {
            return;
        }
        tracing::info!("buy {amount} x {guid:#010x} from {vendor:#010x}");
        self.session
            .send_action(action::BUY, &trade(vendor, &[(guid, amount as i32)]));
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
    /// A chat line starting with `/`: the retail client's own commands
    /// become their game actions; anything else goes to the server as an
    /// `@command` (ACE runs its command manager on those and answers
    /// "Unknown command" for the rest). Returns false for an empty line.
    pub fn slash_command(&mut self, line: &str) -> bool {
        use ac_net::messages::action;
        let body = line.trim_start_matches(['/', '@']).trim();
        if body.is_empty() {
            return false;
        }
        let (name, args) = body
            .split_once(char::is_whitespace)
            .map(|(n, a)| (n, a.trim()))
            .unwrap_or((body, ""));
        let name = name.to_ascii_lowercase();
        let mut w = ac_net::wire::Writer::new();
        match name.as_str() {
            "lifestone" | "ls" => self.session.send_action(action::TELE_TO_LIFESTONE, &[]),
            "die" => self.session.send_action(action::DIE, &[]),
            "house" | "home" => self.session.send_action(action::TELE_TO_HOUSE, &[]),
            "mansion" | "mp" => self.session.send_action(action::TELE_TO_MANSION, &[]),
            "hometown" => self
                .session
                .send_action(action::RECALL_ALLEGIANCE_HOMETOWN, &[]),
            "marketplace" | "mkt" => self.session.send_action(action::TELE_TO_MARKETPLACE, &[]),
            "pklite" => self.session.send_action(action::ENTER_PK_LITE, &[]),
            "afk" => {
                if !args.is_empty() {
                    w.string16(args);
                    self.session
                        .send_action(action::SET_AFK_MESSAGE, &w.finish());
                    w = ac_net::wire::Writer::new();
                }
                w.u32(1);
                self.session.send_action(action::SET_AFK_MODE, &w.finish());
            }
            "back" => {
                w.u32(0);
                self.session.send_action(action::SET_AFK_MODE, &w.finish());
            }
            "tell" | "t" => {
                // `/tell Name, message` or `/tell Name message`.
                let (target, msg) = match args.split_once(',') {
                    Some((t, m)) => (t.trim(), m.trim()),
                    None => args
                        .split_once(char::is_whitespace)
                        .map(|(t, m)| (t.trim(), m.trim()))
                        .unwrap_or((args, "")),
                };
                if target.is_empty() || msg.is_empty() {
                    return false;
                }
                w.string16(msg).string16(target);
                self.session.send_action(action::TELL, &w.finish());
            }
            "emote" | "e" | "me" => {
                w.string16(args);
                self.session.send_action(action::EMOTE, &w.finish());
            }
            _ => {
                // The server's own commands (@acehelp, @myquests, admin
                // commands...): ACE reads them from Talk with an @ prefix.
                w.string16(&format!("@{body}"));
                self.session.send_action(action::TALK, &w.finish());
            }
        }
        true
    }

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
