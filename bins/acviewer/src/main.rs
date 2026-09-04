//! acviewer: fly around a landblock or inspect a model.
//!
//!   acviewer --landblock A9B4 [--radius 1]
//!   acviewer --model 02000001
//!
//! Controls: right mouse drag to look, WASD to move, Q/E down/up,
//! Shift to go faster, Escape to quit.

mod camera;
mod gpu;
mod player;
mod scene;
mod ui;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use glam::Vec3;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Directory with client_portal.dat and client_cell_1.dat (default: $AC_DATA_DIR)
    #[arg(long, env = "AC_DATA_DIR")]
    data_dir: PathBuf,
    /// Landblock to show, hex (e.g. A9B4 for Holtburg)
    #[arg(long)]
    landblock: Option<String>,
    /// Extra rings of landblocks around the center
    #[arg(long, default_value_t = 1)]
    radius: u32,
    /// Model (GfxObj 01xxxxxx or Setup 02xxxxxx) to show, hex
    #[arg(long)]
    model: Option<String>,
    /// Connect to an ACE server, log in, and view the world around the character
    #[arg(long)]
    connect: Option<String>,
    #[arg(short = 'a', long)]
    account: Option<String>,
    #[arg(short = 'v', long)]
    password: Option<String>,
    /// Character name to enter with (default: first)
    #[arg(long)]
    character: Option<String>,
    /// Render one frame to this PNG and exit (no window)
    #[arg(long)]
    screenshot: Option<PathBuf>,
    /// With --connect --screenshot: walk forward for this many seconds first
    #[arg(long, default_value_t = 0.0)]
    walk: f32,
    /// Connected headless mode: say this line once the player is placed.
    #[arg(long)]
    say: Option<String>,
    /// Connected headless mode: double-click this pixel (x,y) once placed;
    /// repeat the flag for several clicks 1.5 s apart.
    #[arg(long)]
    click: Vec<String>,
    /// Connected headless mode: use the nearest object with this name once placed.
    #[arg(long = "use")]
    use_name: Option<String>,
    /// Camera override for --screenshot: x,y,z,yaw_deg,pitch_deg
    #[arg(long)]
    camera: Option<String>,
}

/// Live server connection state for `--connect`.
struct Net {
    socket: std::net::UdpSocket,
    primary: std::net::SocketAddr,
    secondary: std::net::SocketAddr,
    session: ac_net::session::Session,
    world: ac_world::World,
    assets: ac_scene::Assets,
    characters: Vec<ac_net::messages::CharacterEntry>,
    characters_known: bool,
    ddd_done: bool,
    enter_requested: bool,
    /// Landblock the static scene is built around, once the player is placed.
    scene_block: Option<u32>,
    last_generation: u64,
    pickables: Vec<scene::Pickable>,
    /// Server-requested MoveTo for our own character, until reached.
    move_to: Option<ac_world::object::MoveTarget>,
    selected: Option<u32>,
    last_click: Option<(Instant, u32)>,
    gpu_meshes: scene::GpuMeshCache,
    palettes: scene::Palettes,
    player: Option<player::Player>,
    player_setup: u32,
    anims: std::collections::HashMap<u32, scene::ObjectAnim>,
    tables: std::collections::HashMap<u32, Option<ac_formats::motion_table::MotionTable>>,
    last_anim_refresh: Instant,
}

struct App {
    cli: Cli,
    window: Option<Arc<Window>>,
    gpu: Option<gpu::Gpu>,
    net: Option<Net>,
    frame_dt: f32,
    camera: camera::Camera,
    keys: HashSet<KeyCode>,
    looking: bool,
    cursor: Option<(f64, f64)>,
    last_cursor: Option<(f64, f64)>,
    last_frame: Instant,
    ui: Option<ui::Ui>,
    fps: f32,
}

impl App {
    fn chat_message(&mut self, op: u32, body: &[u8]) {
        use ac_net::messages::{event, opcode, ChatLine};
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
        if let Some(ui) = &mut self.ui {
            ui.push_chat(text, line.kind);
        }
    }

    /// Left click in the world: select the object under the cursor and ask
    /// the server to appraise it; a second click on the same object within
    /// half a second uses it (opens doors, talks to NPCs, picks up items).
    fn click(&mut self, px: f64, py: f64, (w, h): (u32, u32)) {
        use ac_net::messages::action;
        let ndc = glam::Vec3::new(
            (2.0 * px as f32 / w.max(1) as f32) - 1.0,
            1.0 - (2.0 * py as f32 / h.max(1) as f32),
            0.0,
        );
        let aspect = w as f32 / h.max(1) as f32;
        let inv = self.camera.view_proj(aspect).inverse();
        let near = inv.project_point3(ndc);
        let far = inv.project_point3(ndc.with_z(1.0));
        let dir = (far - near).normalize_or_zero();
        let Some(net) = self.net.as_mut() else { return };
        let mut best: Option<(f32, u32)> = None;
        for p in &net.pickables {
            if let Some(t) = p.hit(near, dir) {
                tracing::trace!(
                    "hit {} at t={t:.2} (center {:?} r {:.2})",
                    net.world
                        .objects
                        .get(&p.guid)
                        .map(|o| o.name.as_str())
                        .unwrap_or("?"),
                    p.center,
                    p.radius
                );
                if best.map(|(bt, _)| t < bt).unwrap_or(true) {
                    best = Some((t, p.guid));
                }
            }
        }
        let Some((_, guid)) = best else {
            net.selected = None;
            return;
        };
        let now = Instant::now();
        let again = matches!(net.last_click, Some((t, g)) if g == guid && now - t < Duration::from_millis(500));
        net.last_click = Some((now, guid));
        net.selected = Some(guid);
        let name = net
            .world
            .objects
            .get(&guid)
            .map(|o| o.name.clone())
            .unwrap_or_default();
        if again {
            tracing::info!("use {name} ({guid:#010x})");
            net.session.send_action(action::USE, &guid.to_le_bytes());
            net.last_click = None;
        } else {
            tracing::info!("select {name} ({guid:#010x})");
            net.session
                .send_action(action::IDENTIFY_OBJECT, &guid.to_le_bytes());
        }
    }

    /// Send Use for the nearest drawable object called `name` (test hook).
    fn use_by_name(&mut self, name: &str) {
        let Some(net) = self.net.as_mut() else { return };
        let me = net.player.as_ref().map(|p| p.world_position());
        let mut best: Option<(f32, u32)> = None;
        for o in net.world.drawable() {
            if o.name != name {
                continue;
            }
            let Some(p) = o.display.or(o.position) else {
                continue;
            };
            let d = me
                .map(|m| (ac_world::landblock_origin(p.cell) + p.local).distance(m))
                .unwrap_or(0.0);
            if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                best = Some((d, o.guid));
            }
        }
        match best {
            Some((_, guid)) => {
                tracing::info!("use {name} ({guid:#010x})");
                net.selected = Some(guid);
                net.session
                    .send_action(ac_net::messages::action::USE, &guid.to_le_bytes());
            }
            None => tracing::warn!("no object named {name:?} in view"),
        }
    }

    fn refresh_status(&mut self) {
        let Some(ui) = &mut self.ui else { return };
        let mut s = format!("{:.0} fps", self.fps);
        if let Some(net) = &self.net {
            match net.world.player().and_then(|o| o.position) {
                Some(p) => {
                    s += &format!(
                        "  cell {:#010x}  x {:.1} y {:.1} z {:.1}  objects {}",
                        p.cell,
                        p.local.x,
                        p.local.y,
                        p.local.z,
                        net.world.drawable().count()
                    );
                    if let Some(o) = net.selected.and_then(|g| net.world.objects.get(&g)) {
                        s += &format!("  selected: {}", o.name);
                    }
                }
                None => s += "  connecting...",
            }
        } else {
            let c = &self.camera;
            s += &format!(
                "  x {:.1} y {:.1} z {:.1}",
                c.position.x, c.position.y, c.position.z
            );
        }
        ui.status = s;
        let Some(net) = &self.net else {
            ui.sheet = None;
            ui.blips.clear();
            return;
        };
        let st = &net.world.stats;
        ui.sheet = (!st.name.is_empty()).then(|| ui::Sheet {
            name: st.name.clone(),
            level: st.level,
            vitals: (0..3)
                .map(|i| ui::VitalBar {
                    name: ac_world::stats::VITAL_NAMES[i],
                    current: st.vitals[i].current,
                    max: st.vital_max(i),
                })
                .collect(),
        });
        ui.blips.clear();
        let (me, heading) = match (&net.player, net.world.player()) {
            (Some(p), _) => (ac_world::landblock_origin(p.cell) + p.local, p.heading),
            (None, Some(o)) => match o.display.or(o.position) {
                Some(pos) => (ac_world::landblock_origin(pos.cell) + pos.local, 0.0),
                None => return,
            },
            _ => return,
        };
        let fwd = glam::Vec2::new(-heading.sin(), heading.cos());
        let right = glam::Vec2::new(heading.cos(), heading.sin());
        for o in net.world.drawable() {
            if o.is_player {
                continue;
            }
            let Some(pos) = o.display.or(o.position) else {
                continue;
            };
            let d = ac_world::landblock_origin(pos.cell) + pos.local - me;
            let d = glam::Vec2::new(d.x, d.y);
            let kind = if o.object_desc_flags & ac_world::object_desc_flags::PLAYER != 0 {
                ui::BlipKind::Player
            } else if o.motion_table_id != 0 {
                ui::BlipKind::Creature
            } else {
                ui::BlipKind::Other
            };
            ui.blips.push(ui::Blip {
                x: d.dot(right),
                y: d.dot(fwd),
                kind,
            });
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
            .net
            .as_ref()
            .and_then(|n| n.world.objects.get(&a.guid))
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
        if let Some(ui) = &mut self.ui {
            for l in lines {
                ui.push_chat(l, 1);
            }
        }
    }

    fn send_chat(&mut self) {
        let Some(ui) = &mut self.ui else { return };
        let lines = std::mem::take(&mut ui.outgoing);
        let Some(net) = self.net.as_mut() else { return };
        for t in lines {
            let mut w = ac_net::wire::Writer::new();
            w.string16(&t);
            net.session
                .send_action(ac_net::messages::action::TALK, &w.finish());
        }
    }

    fn start_connect(&mut self) -> Result<()> {
        use ac_net::messages::DatIteration;
        use ac_net::session::{Config, Session};
        let host = self.cli.connect.clone().unwrap();
        let account = self
            .cli
            .account
            .clone()
            .context("--connect needs --account")?;
        let password = self
            .cli
            .password
            .clone()
            .context("--connect needs --password")?;
        let primary: std::net::SocketAddr = if host.contains(':') {
            host.parse()?
        } else {
            format!("{host}:9000").parse()?
        };
        let secondary = std::net::SocketAddr::new(primary.ip(), primary.port() + 1);
        let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;
        let assets = ac_scene::Assets::open(&self.cli.data_dir).context("opening DAT archives")?;
        let now = Instant::now();
        let mut session = Session::new(
            Config {
                account,
                password,
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
                echo_interval: std::time::Duration::from_secs(5),
                ack_interval: std::time::Duration::from_secs(2),
            },
            now,
        );
        session.login(now);
        tracing::info!("connecting to {primary}");
        self.net = Some(Net {
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
            scene_block: None,
            last_generation: 0,
            pickables: Vec::new(),
            move_to: None,
            selected: None,
            last_click: None,
            gpu_meshes: Default::default(),
            palettes: Default::default(),
            player: None,
            player_setup: 0,
            anims: Default::default(),
            tables: Default::default(),
            last_anim_refresh: Instant::now(),
        });
        Ok(())
    }

    /// Pump the connection: send, receive, apply messages, rebuild scenes.
    fn tick_net(&mut self, gpu: &mut gpu::Gpu) {
        use ac_net::messages::{self, opcode, queue};
        use ac_net::session::{Event, Port};
        let Some(net) = self.net.as_mut() else { return };
        let now = Instant::now();
        let mut chat_pending: Vec<(u32, Vec<u8>)> = Vec::new();
        for (port, dg) in net.session.outgoing() {
            let to = if port == Port::Primary {
                net.primary
            } else {
                net.secondary
            };
            let _ = net.socket.send_to(&dg, to);
        }
        let mut buf = [0u8; 2048];
        loop {
            match net.socket.recv_from(&mut buf) {
                Ok((n, _)) => net.session.receive(&buf[..n], now),
                Err(_) => break,
            }
        }
        net.session.poll(now);
        for ev in net.session.events() {
            match ev {
                Event::Connected { client_id } => {
                    tracing::info!("connected, client id {client_id}")
                }
                Event::Terminated(why) => tracing::warn!("terminated: {why}"),
                Event::Message(msg) => {
                    match net.world.apply(&msg) {
                        ac_world::Applied::PlayerSet => {
                            // The server ignores our positions until we say we landed.
                            net.session
                                .send_action(ac_net::messages::action::LOGIN_COMPLETE, &[]);
                            continue;
                        }
                        ac_world::Applied::Moved => {
                            // A MoveTo aimed at us is ours to carry out; the
                            // object table only sees our echoed motion states.
                            if let Some(o) = net.world.player_mut() {
                                if let Some(t) = o.target.take() {
                                    tracing::debug!("server move-to {t:?}");
                                    net.move_to = Some(t);
                                }
                            }
                            continue;
                        }
                        ac_world::Applied::Created
                        | ac_world::Applied::Deleted
                        | ac_world::Applied::Stats => continue,
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
                                net.characters = cl.characters;
                                net.characters_known = true;
                            }
                        }
                        opcode::DDD_END_DDD => net.ddd_done = true,
                        _ => {}
                    }
                    let Some((op, _)) = messages::split(&msg) else {
                        continue;
                    };
                    match op {
                        _ if net.ddd_done && net.characters_known && !net.enter_requested => {
                            let pick = match &self.cli.character {
                                Some(name) => net
                                    .characters
                                    .iter()
                                    .find(|c| c.name.eq_ignore_ascii_case(name)),
                                None => net.characters.first(),
                            };
                            match pick {
                                Some(c) => {
                                    tracing::info!("entering world as {}", c.name);
                                    net.session.send_message(queue::UI, messages::enter_world_request());
                                    net.enter_requested = true;
                                }
                                None => tracing::error!("no character on this account; create one with acclient --create first"),
                            }
                        }
                        opcode::CHARACTER_ENTER_WORLD_SERVER_READY => {
                            let pick = match &self.cli.character {
                                Some(name) => net
                                    .characters
                                    .iter()
                                    .find(|c| c.name.eq_ignore_ascii_case(name)),
                                None => net.characters.first(),
                            };
                            if let Some(c) = pick {
                                let account = self.cli.account.clone().unwrap_or_default();
                                net.session
                                    .send_message(queue::UI, messages::enter_world(c.id, &account));
                            }
                        }
                        opcode::PLAYER_TELEPORT => {
                            // After a server teleport, take the new position and
                            // tell the server we landed.
                            if let (Some(pl), Some(p)) = (
                                net.player.as_mut(),
                                net.world.player().and_then(|o| o.position),
                            ) {
                                pl.cell = p.cell;
                                pl.local = p.local;
                                pl.dirty = true;
                            }
                            net.session
                                .send_action(ac_net::messages::action::LOGIN_COMPLETE, &[]);
                        }
                        opcode::CHARACTER_ERROR | opcode::ACCOUNT_BOOT => {
                            tracing::error!("server refused: {}", op)
                        }
                        opcode::GAME_EVENT => {
                            if let Some((_, _, ev, rest)) = messages::split_game_event(body) {
                                if ev == ac_net::messages::event::USE_DONE && rest.len() >= 4 {
                                    let err =
                                        u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
                                    tracing::debug!("use done, error {err:#x}");
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
        self.send_chat();
        let Some(net) = self.net.as_mut() else { return };
        // Build the static scene once the player is placed.
        if net.scene_block.is_none() {
            if let Some(p) = net.world.player().and_then(|o| o.position) {
                let block = p.landblock();
                tracing::info!(
                    "player at cell {:#010x} local {:?}; loading landblocks",
                    p.cell,
                    p.local
                );
                match scene::build_landblocks(&net.assets, block, 1) {
                    Ok(built) => {
                        let assets = &net.assets;
                        let palettes = &net.palettes;
                        gpu.set_scene(built.batches, |k| {
                            scene::material_image(assets, k, palettes)
                        });
                    }
                    Err(e) => tracing::error!("scene: {e:#}"),
                }
                net.player_setup = net
                    .world
                    .player()
                    .map(|o| o.setup_id)
                    .unwrap_or(0x0200_0001);
                let mut pl = player::Player::new(&net.assets, p.cell, p.local, p.rotation);
                let table_id = net
                    .world
                    .player()
                    .map(|o| o.motion_table_id)
                    .filter(|&t| t != 0)
                    .unwrap_or(0x0900_0001);
                pl.set_motion_table(&net.assets, net.player_setup, table_id);
                net.player = Some(pl);
                self.camera.pitch = -0.15;
                self.camera.far = 3000.0;
                net.scene_block = Some(block);
            }
        }
        net.world.tick(self.frame_dt);
        let changed = net.world.generation != net.last_generation;
        let animate = scene::any_animated(&net.anims)
            && net.last_anim_refresh.elapsed() > Duration::from_millis(66);
        if net.scene_block.is_some() && (changed || animate) {
            net.last_generation = net.world.generation;
            let dt = net.last_anim_refresh.elapsed().as_secs_f32().min(0.2);
            net.last_anim_refresh = Instant::now();
            let (instances, picks) = scene::object_instances(
                &net.assets,
                gpu,
                &net.world,
                &mut net.gpu_meshes,
                &mut net.palettes,
                &mut net.anims,
                &mut net.tables,
                dt,
            );
            net.pickables = picks;
            gpu.set_dynamic_instances(instances);
        }
        // Player movement, camera, and reporting.
        if let Some(pl) = net.player.as_mut() {
            let mut input = player::Input {
                forward: (self.keys.contains(&KeyCode::KeyW) as i8
                    - self.keys.contains(&KeyCode::KeyS) as i8) as f32,
                strafe: (self.keys.contains(&KeyCode::KeyD) as i8
                    - self.keys.contains(&KeyCode::KeyA) as i8) as f32,
                run: !self.keys.contains(&KeyCode::ShiftLeft),
            };
            // Server-driven MoveTo (using something out of reach): run toward
            // the target until close enough, unless the user takes over.
            let manual = input.forward != 0.0 || input.strafe != 0.0;
            if let Some(t) = net.move_to {
                let goal = match t {
                    ac_world::object::MoveTarget::Object(g) => net
                        .world
                        .objects
                        .get(&g)
                        .and_then(|o| o.display.or(o.position))
                        .map(|p| (ac_world::landblock_origin(p.cell) + p.local, 1.0)),
                    ac_world::object::MoveTarget::Position { cell, local } => {
                        Some((ac_world::landblock_origin(cell) + local, 0.3))
                    }
                };
                let arrived = match goal {
                    Some((g, stop)) if !manual => {
                        let d = g - pl.world_position();
                        let flat = glam::Vec2::new(d.x, d.y);
                        if flat.length() > stop {
                            pl.heading = (-flat.x).atan2(flat.y);
                            input.forward = 1.0;
                            input.run = true;
                            false
                        } else {
                            true
                        }
                    }
                    _ => true,
                };
                if arrived {
                    tracing::debug!("move-to done ({t:?})");
                    net.move_to = None;
                }
            }
            let now = Instant::now();
            let dt = self.frame_dt;
            pl.update(&net.assets, &input, dt);
            let pose = pl.animate(&net.assets, &input, dt);
            if pose.is_some() {
                pl.dirty = true;
            }
            pl.report(&mut net.session, &input, now);
            // Third-person camera behind the character.
            let pos = pl.world_position();
            let fwd = pl.forward();
            let (sp, cp) = self.camera.pitch.sin_cos();
            let back = Vec3::new(-fwd.x * cp, -fwd.y * cp, -sp) * 4.0;
            let head = pos + Vec3::new(0.0, 0.0, 1.6);
            self.camera.position = pl.clamp_camera(&net.assets, head, head + back);
            self.camera.yaw = pl.heading;
            if pl.dirty {
                pl.dirty = false;
                if let Some(o) = net.world.player_mut() {
                    o.position = Some(ac_world::Position {
                        cell: pl.cell,
                        local: pl.local,
                        rotation: pl.rotation(),
                    });
                }
                let t = glam::Mat4::from_rotation_translation(pl.rotation(), pos);
                let (app, key) = match net.world.player() {
                    Some(o) => scene::appearance_of(&net.assets, o, &mut net.palettes),
                    None => (ac_scene::model::Appearance::default(), 0),
                };
                let instances = scene::instances_for(
                    &net.assets,
                    gpu,
                    &mut net.gpu_meshes,
                    &net.palettes,
                    net.player_setup,
                    t,
                    &app,
                    key,
                    pose.as_deref(),
                );
                gpu.set_player_instances(instances);
            }
        }
    }

    fn load_scene(&mut self, gpu: &mut gpu::Gpu) -> Result<()> {
        if self.cli.connect.is_some() {
            return self.start_connect();
        }
        let assets = ac_scene::Assets::open(&self.cli.data_dir).context("opening DAT archives")?;
        let built = if let Some(m) = &self.cli.model {
            let id = u32::from_str_radix(m.trim_start_matches("0x"), 16)?;
            scene::build_model(&assets, id)?
        } else {
            let lb = self.cli.landblock.as_deref().unwrap_or("A9B4");
            let id = u32::from_str_radix(lb.trim_start_matches("0x"), 16)? << 16;
            scene::build_landblocks(&assets, id, self.cli.radius)?
        };
        let tris: usize = built.batches.values().map(|b| b.indices.len() / 3).sum();
        tracing::info!(
            "{} materials, {tris} triangles, center {:?} radius {:.1}",
            built.batches.len(),
            built.center,
            built.radius
        );
        let palettes = scene::Palettes::default();
        gpu.set_scene(built.batches, |k| {
            scene::material_image(&assets, k, &palettes)
        });
        let d = built.radius.max(5.0);
        self.camera.position = built.center + Vec3::new(0.0, -d * 0.8, d * 0.5);
        self.camera.yaw = 0.0;
        self.camera.pitch = -0.45;
        self.camera.speed = (d * 0.25).max(2.0);
        self.camera.far = (d * 10.0).max(500.0);
        Ok(())
    }

    fn update(&mut self, dt: f32) {
        let mut v = Vec3::ZERO;
        let f = self.camera.forward();
        let r = self.camera.right();
        if self.keys.contains(&KeyCode::KeyW) {
            v += f;
        }
        if self.keys.contains(&KeyCode::KeyS) {
            v -= f;
        }
        if self.keys.contains(&KeyCode::KeyD) {
            v += r;
        }
        if self.keys.contains(&KeyCode::KeyA) {
            v -= r;
        }
        if self.keys.contains(&KeyCode::KeyE) || self.keys.contains(&KeyCode::Space) {
            v += Vec3::Z;
        }
        if self.keys.contains(&KeyCode::KeyQ) {
            v -= Vec3::Z;
        }
        let boost = if self.keys.contains(&KeyCode::ShiftLeft) {
            4.0
        } else {
            1.0
        };
        self.camera.position += v.normalize_or_zero() * self.camera.speed * boost * dt;
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("acviewer")
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 800));
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        let mut gpu = gpu::Gpu::new(window.clone()).expect("gpu");
        if let Err(e) = self.load_scene(&mut gpu) {
            tracing::error!("{e:#}");
            event_loop.exit();
            return;
        }
        let (w, h) = gpu.size();
        self.ui = Some(ui::Ui::new(gpu.device(), gpu.format(), Some(&window), w, h));
        self.gpu = Some(gpu);
        self.window = Some(window);
        self.last_frame = Instant::now();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let (Some(ui), Some(w)) = (&mut self.ui, &self.window) {
            if !matches!(event, WindowEvent::RedrawRequested) && ui.on_event(w, &event) {
                // egui took it (typing in the chat box, clicking the overlay).
                if matches!(event, WindowEvent::KeyboardInput { .. }) {
                    self.keys.clear();
                }
                return;
            }
        }
        let typing = self
            .ui
            .as_ref()
            .map(|u| u.wants_keyboard())
            .unwrap_or(false);
        match event {
            WindowEvent::CloseRequested => {
                if let Some(net) = self.net.as_mut() {
                    net.session.disconnect(Instant::now());
                    for (port, dg) in net.session.outgoing() {
                        let to = if port == ac_net::session::Port::Primary {
                            net.primary
                        } else {
                            net.secondary
                        };
                        let _ = net.socket.send_to(&dg, to);
                    }
                }
                event_loop.exit()
            }
            WindowEvent::Resized(size) => {
                if let Some(g) = &mut self.gpu {
                    g.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if typing {
                        return;
                    }
                    if code == KeyCode::Enter
                        && event.state == ElementState::Pressed
                        && self.net.is_some()
                    {
                        if let Some(ui) = &mut self.ui {
                            ui.chat_focus = true;
                        }
                        self.keys.clear();
                        return;
                    }
                    if code == KeyCode::Escape {
                        if let Some(net) = self.net.as_mut() {
                            net.session.disconnect(Instant::now());
                            for (port, dg) in net.session.outgoing() {
                                let to = if port == ac_net::session::Port::Primary {
                                    net.primary
                                } else {
                                    net.secondary
                                };
                                let _ = net.socket.send_to(&dg, to);
                            }
                        }
                        event_loop.exit();
                    }
                    match event.state {
                        ElementState::Pressed => {
                            self.keys.insert(code);
                        }
                        ElementState::Released => {
                            self.keys.remove(&code);
                        }
                    }
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Right,
                ..
            } => {
                self.looking = state == ElementState::Pressed;
                self.last_cursor = None;
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if let (Some((x, y)), Some(size)) =
                    (self.cursor, self.gpu.as_ref().map(|g| g.size()))
                {
                    self.click(x, y, size);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y));
                if self.looking {
                    if let Some((lx, ly)) = self.last_cursor {
                        let dx = (position.x - lx) as f32;
                        let dy = (position.y - ly) as f32;
                        match self.net.as_mut().and_then(|n| n.player.as_mut()) {
                            Some(pl) => {
                                pl.turn(-dx * 0.003);
                                self.camera.pitch =
                                    (self.camera.pitch - dy * 0.003).clamp(-1.2, 0.6);
                            }
                            None => self.camera.look(dx, dy),
                        }
                    }
                    self.last_cursor = Some((position.x, position.y));
                }
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.last_frame).as_secs_f32().min(0.1);
                self.last_frame = now;
                self.frame_dt = dt;
                if self.net.is_none() {
                    self.update(dt);
                }
                if let Some(mut g) = self.gpu.take() {
                    self.tick_net(&mut g);
                    self.gpu = Some(g);
                }
                self.fps = if self.fps == 0.0 {
                    1.0 / dt.max(1e-3)
                } else {
                    self.fps * 0.95 + 0.05 / dt.max(1e-3)
                };
                self.refresh_status();
                if let Some(g) = &mut self.gpu {
                    let vp = self.camera.view_proj(g.aspect());
                    let (w, h) = g.size();
                    let mut ui = self.ui.as_mut();
                    if let Some(ui) = ui.as_deref_mut() {
                        ui.begin(self.window.as_deref(), g.device(), g.queue(), w, h);
                    }
                    let mut paint = |d: &wgpu::Device,
                                     q: &wgpu::Queue,
                                     e: &mut wgpu::CommandEncoder,
                                     v: &wgpu::TextureView| {
                        if let Some(ui) = ui.as_deref_mut() {
                            ui.paint(d, q, e, v);
                        }
                    };
                    if let Err(e) = g.render(vp, Vec3::new(0.4, 0.3, 1.0), Some(&mut paint)) {
                        tracing::error!("render: {e:#}");
                    }
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("acviewer=info")),
        )
        .init();
    let cli = Cli::parse();
    if let Some(path) = cli.screenshot.clone() {
        let mut gpu = gpu::Gpu::headless(1280, 800)?;
        let mut app = App {
            cli,
            window: None,
            gpu: None,
            net: None,
            frame_dt: 0.0,
            camera: camera::Camera {
                position: Vec3::ZERO,
                yaw: 0.0,
                pitch: 0.0,
                fov_y: 60f32.to_radians(),
                near: 0.5,
                far: 2000.0,
                speed: 20.0,
            },
            keys: HashSet::new(),
            looking: false,
            cursor: None,
            last_cursor: None,
            last_frame: Instant::now(),
            ui: None,
            fps: 0.0,
        };
        app.load_scene(&mut gpu)?;
        let (w, h) = gpu.size();
        app.ui = Some(ui::Ui::new(gpu.device(), gpu.format(), None, w, h));
        if app.cli.connect.is_some() {
            // Pump the connection until the player is placed and the world
            // has settled, then render from the character's viewpoint.
            let deadline = Instant::now() + Duration::from_secs(40);
            let mut settled_at: Option<Instant> = None;
            let mut last_tick = Instant::now();
            let mut ticks = 0u32;
            let mut ticks_since = Instant::now();
            // Each --click entry is a double click: (entry index, clicks sent).
            let mut click_state = (0usize, 0u32);
            let use_requested = app.cli.use_name.is_some();
            loop {
                app.tick_net(&mut gpu);
                let placed = app
                    .net
                    .as_ref()
                    .map(|n| n.scene_block.is_some())
                    .unwrap_or(false);
                if placed {
                    let started = *settled_at.get_or_insert_with(Instant::now);
                    let t = started.elapsed().as_secs_f32();
                    // Hold W for `walk` seconds after a short settle, then settle again.
                    let walking = t > 1.0 && t < 1.0 + app.cli.walk;
                    if t > 1.0 {
                        if let (Some(line), Some(ui)) = (app.cli.say.take(), app.ui.as_mut()) {
                            ui.outgoing.push(line);
                        }
                    }
                    if t > 1.0 + app.cli.walk {
                        if let Some(name) = app.cli.use_name.take() {
                            app.use_by_name(&name);
                        }
                    }
                    let (ci, sent) = click_state;
                    if ci < app.cli.click.len() && t > 1.0 + app.cli.walk + ci as f32 * 3.0 {
                        let c = app.cli.click[ci].clone();
                        let v: Vec<f64> =
                            c.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                        if v.len() == 2 {
                            app.click(v[0], v[1], gpu.size());
                        }
                        click_state = if sent + 1 >= 2 {
                            (ci + 1, 0)
                        } else {
                            (ci, sent + 1)
                        };
                    }
                    if walking {
                        app.keys.insert(KeyCode::KeyW);
                    } else {
                        app.keys.remove(&KeyCode::KeyW);
                    }
                    app.frame_dt = last_tick.elapsed().as_secs_f32().min(0.1);
                    last_tick = Instant::now();
                    ticks += 1;
                    if ticks_since.elapsed() >= Duration::from_secs(1) {
                        tracing::info!("{ticks} ticks/s (frame work incl. player rebuild)");
                        ticks = 0;
                        ticks_since = Instant::now();
                    }
                    // Capture while still walking so the walk pose is visible,
                    // or after a short settle when not walking.
                    let done = if app.cli.walk > 0.0 {
                        t > 0.9 + app.cli.walk
                    } else if !app.cli.click.is_empty() || use_requested {
                        t > 8.0 + app.cli.click.len() as f32 * 3.0
                    } else {
                        t > 3.0
                    };
                    if done {
                        break;
                    }
                }
                if Instant::now() > deadline {
                    anyhow::bail!("timed out waiting for the player to be placed");
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        if let Some(c) = &app.cli.camera {
            let v: Vec<f32> = c
                .split(',')
                .map(|x| x.trim().parse())
                .collect::<std::result::Result<_, _>>()?;
            anyhow::ensure!(v.len() == 5, "--camera wants x,y,z,yaw,pitch");
            app.camera.position = Vec3::new(v[0], v[1], v[2]);
            app.camera.yaw = v[3].to_radians();
            app.camera.pitch = v[4].to_radians();
        }
        let vp = app.camera.view_proj(gpu.aspect());
        app.refresh_status();
        let mut ui = app.ui.take().unwrap();
        // egui's first frame only loads fonts and asks for a repaint.
        ui.begin(None, gpu.device(), gpu.queue(), w, h);
        ui.begin(None, gpu.device(), gpu.queue(), w, h);
        let mut paint = |d: &wgpu::Device,
                         q: &wgpu::Queue,
                         e: &mut wgpu::CommandEncoder,
                         v: &wgpu::TextureView| ui.paint(d, q, e, v);
        gpu.render_to_png(vp, Vec3::new(0.4, 0.3, 1.0), &path, Some(&mut paint))?;
        tracing::info!("wrote {}", path.display());
        if let Some(net) = app.net.as_mut() {
            net.session.disconnect(Instant::now());
            for (port, dg) in net.session.outgoing() {
                let to = if port == ac_net::session::Port::Primary {
                    net.primary
                } else {
                    net.secondary
                };
                let _ = net.socket.send_to(&dg, to);
            }
        }
        return Ok(());
    }
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        cli,
        window: None,
        gpu: None,
        net: None,
        frame_dt: 0.0,
        camera: camera::Camera {
            position: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            fov_y: 60f32.to_radians(),
            near: 0.5,
            far: 2000.0,
            speed: 20.0,
        },
        keys: HashSet::new(),
        looking: false,
        cursor: None,
        last_cursor: None,
        last_frame: Instant::now(),
        ui: None,
        fps: 0.0,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
