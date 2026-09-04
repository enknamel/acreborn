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
    last_cursor: Option<(f64, f64)>,
    last_frame: Instant,
}

impl App {
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
                        ac_world::Applied::Created
                        | ac_world::Applied::Moved
                        | ac_world::Applied::Deleted => continue,
                        ac_world::Applied::Failed => tracing::warn!("failed to apply a message"),
                        ac_world::Applied::Ignored => {}
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
                        _ => {}
                    }
                }
            }
        }
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
            let instances = scene::object_instances(
                &net.assets,
                gpu,
                &net.world,
                &mut net.gpu_meshes,
                &mut net.palettes,
                &mut net.anims,
                &mut net.tables,
                dt,
            );
            gpu.set_dynamic_instances(instances);
        }
        // Player movement, camera, and reporting.
        if let Some(pl) = net.player.as_mut() {
            let input = player::Input {
                forward: (self.keys.contains(&KeyCode::KeyW) as i8
                    - self.keys.contains(&KeyCode::KeyS) as i8) as f32,
                strafe: (self.keys.contains(&KeyCode::KeyD) as i8
                    - self.keys.contains(&KeyCode::KeyA) as i8) as f32,
                run: !self.keys.contains(&KeyCode::ShiftLeft),
            };
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
            self.camera.position = pos + Vec3::new(0.0, 0.0, 1.6) + back;
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
        self.gpu = Some(gpu);
        self.window = Some(window);
        self.last_frame = Instant::now();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
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
            WindowEvent::CursorMoved { position, .. } => {
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
                if let Some(g) = &mut self.gpu {
                    let vp = self.camera.view_proj(g.aspect());
                    if let Err(e) = g.render(vp, Vec3::new(0.4, 0.3, 1.0)) {
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
            last_cursor: None,
            last_frame: Instant::now(),
        };
        app.load_scene(&mut gpu)?;
        if app.cli.connect.is_some() {
            // Pump the connection until the player is placed and the world
            // has settled, then render from the character's viewpoint.
            let deadline = Instant::now() + Duration::from_secs(40);
            let mut settled_at: Option<Instant> = None;
            let mut last_tick = Instant::now();
            let mut ticks = 0u32;
            let mut ticks_since = Instant::now();
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
        gpu.render_to_png(vp, Vec3::new(0.4, 0.3, 1.0), &path)?;
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
        last_cursor: None,
        last_frame: Instant::now(),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
