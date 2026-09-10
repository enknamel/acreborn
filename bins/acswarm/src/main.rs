//! acswarm: fly around a landblock or inspect a model.
//!
//!   acswarm --landblock A9B4 [--radius 1]
//!   acswarm --model 02000001
//!   acswarm --emitter 32000273            # simulate an emitter's particles
//!   acswarm --chargen aluvian,m,3,0,0,0,0.5     # a dressed-up human head
//!
//! Controls: right mouse drag to look, WASD to move, Q/E down/up,
//! Shift to go faster, Escape for the menu (which quits).

mod camera;
mod gpu;
mod headless;
mod logging;
mod particles;
mod perf;
mod scene;
use ac_client::player;
mod chat;
mod plugins;
mod route_marks;
mod sky;
mod ui;
mod water;
mod world_fx;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use glam::Vec3;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Directory with client_portal.dat and client_cell_1.dat. When
    /// omitted, the app remembers your last choice, looks in the usual
    /// places, and otherwise opens a folder picker (default: $AC_DATA_DIR).
    #[arg(long, env = "AC_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Landblock to show, hex (e.g. A9B4 for Holtburg)
    #[arg(long)]
    landblock: Option<String>,
    /// Extra rings of landblocks around the center
    #[arg(long, default_value_t = 1)]
    radius: u32,
    /// Model (GfxObj 01xxxxxx or Setup 02xxxxxx) to show, hex
    #[arg(long)]
    model: Option<String>,
    /// Dress the model as a new character from the CharGen table:
    /// race,gender,hair,eyes,nose,mouth,skin[,hair_color,eye_color].
    /// race = heritage name or id, gender = m/f, styles/colors are option
    /// indices, skin is a 0..1 shade. Shows the race's Setup unless --model is given.
    #[arg(long)]
    chargen: Option<String>,
    /// Simulate particles at the origin for a few seconds and draw them: a
    /// ParticleEmitterInfo (32xxxxxx), a PhysicsScript (33xxxxxx) or a
    /// Setup's default script (02xxxxxx), hex. Combine with --model.
    #[arg(long)]
    emitter: Option<String>,
    /// Connect to an ACE server, log in, and view the world around the character
    #[arg(long)]
    connect: Option<String>,
    /// Extra sessions in the same process: ACCOUNT:PASSWORD[:CHARACTER], repeatable.
    /// Tab (or /switch N) picks which one the window shows and steers.
    #[arg(long = "client")]
    clients: Vec<String>,
    #[arg(short = 'a', long)]
    account: Option<String>,
    #[arg(short = 'v', long)]
    password: Option<String>,
    /// Character name to enter with (default: first)
    #[arg(long)]
    character: Option<String>,
    /// Run without a window: many sessions in one process, no GPU. What
    /// the separate `acbot` used to be. Pair with --client (repeatable),
    /// --duration, --tick-hz and --script.
    #[arg(long)]
    headless: bool,
    /// Headless: ticks per second for every session. 20 is the game's
    /// pace; a process of followers gets by on 10.
    #[arg(long = "tick-hz", alias = "hz", default_value_t = 20)]
    tick_hz: u32,
    /// Headless: run for this many seconds (0 = until Ctrl-C or every
    /// session ends).
    #[arg(long, default_value_t = 0)]
    duration: u64,
    /// Headless: a text file of lines typed after the --say lines, one
    /// per second, by every session. Blank lines and `#` are skipped.
    #[arg(long)]
    script: Option<PathBuf>,
    /// Headless: print every chat line the server sends.
    #[arg(long)]
    log_chat: bool,
    /// Play by the rules in this settings file rather than the usual
    /// one (`~/.config/acswarm/ui.json`, or `$ACSWARM_CONFIG_DIR`).
    #[arg(long)]
    settings: Option<PathBuf>,
    /// Ignore the settings file and play by the defaults.
    #[arg(long)]
    no_settings: bool,
    /// For every session without a character of this name: create one
    /// from the CharGen table (see --heritage, --gender, --template,
    /// --start-area) and enter the world with it.
    #[arg(long)]
    create: Option<String>,
    /// Heritage for --create: a name (aluvian, gharu, sho, viamontian,
    /// ...) or id 1..=13. Default Aluvian.
    #[arg(long)]
    heritage: Option<String>,
    /// Sex for --create: m or f. Default m.
    #[arg(long)]
    gender: Option<String>,
    /// Template for --create: a name (adventurer, bow, swash, life, war,
    /// wayfarer, soldier) or index. Default the first (Adventurer).
    #[arg(long)]
    template: Option<String>,
    /// Starting town for --create: holtburg, shoushi, yaraq or sanamar.
    /// Default the heritage's home town.
    #[arg(long)]
    start_area: Option<String>,
    /// Print the creation rules for --heritage (credits, skill costs,
    /// templates, towns) and exit without connecting.
    #[arg(long)]
    show_rules: bool,
    /// Render one frame to this PNG and exit (no window)
    #[arg(long)]
    screenshot: Option<PathBuf>,
    /// With --connect --screenshot: walk forward for this many seconds first
    #[arg(long, default_value_t = 0.0)]
    walk: f32,
    /// Connected headless mode: say these lines (one per second) once placed.
    #[arg(long)]
    say: Vec<String>,
    /// Connected headless mode: double-click this pixel (x,y) once placed;
    /// repeat the flag for several clicks 1.5 s apart.
    #[arg(long)]
    click: Vec<String>,
    /// Connected headless mode: use the nearest object with this name once placed.
    #[arg(long = "use")]
    use_name: Option<String>,
    /// Connected headless mode: enter melee and attack the nearest creature
    /// with this name until it dies (or 90 s pass).
    #[arg(long)]
    attack: Option<String>,
    /// Disable sound output.
    #[arg(long)]
    mute: bool,
    /// Frame rate cap for the window (0 = uncapped). Lower it when running
    /// many clients on one machine. An unfocused window draws at 10 fps
    /// at most, a hidden one not at all, and a frame nothing changed in
    /// is skipped.
    #[arg(long, default_value_t = 60)]
    fps: u32,
    /// What the window draws of the world: `active` (the session shown)
    /// or `none` (a follower: no landblocks, objects or particles reach
    /// the GPU; the overlay and panels still work).
    #[arg(long, default_value = "active", value_parser = ["active", "none"])]
    render: String,
    /// Report frame cost: the status line shows the last frame's time
    /// and draw counts, and a summary (average and percentile frame
    /// times, draw calls, culling, GPU memory) is printed at exit. A
    /// headless `--connect --screenshot` run renders offscreen at the
    /// `--fps` rate meanwhile, so it measures what a window would.
    #[arg(long)]
    perf: bool,
    /// Upload textures no larger than this on a side (0 = as they are,
    /// mostly 256): 128 quarters the texture memory, 64 sixteenths it,
    /// for a blurrier world.
    #[arg(long, default_value_t = 0)]
    max_texture: u32,
    /// Connected headless mode: open the skills panel in the screenshot.
    #[arg(long)]
    show_skills: bool,
    /// Connected headless mode: press these panel keys once placed
    /// (letters, e.g. `--press O,B,U`), so the screenshot shows them.
    #[arg(long, value_delimiter = ',')]
    press: Vec<String>,
    /// Connected headless mode: once a vendor window is open (after --use
    /// on the vendor), buy the first stock item whose name starts with this.
    #[arg(long)]
    buy: Option<String>,
    /// Connected headless mode: once a vendor window is open, sell the first
    /// pack item whose name starts with this.
    #[arg(long)]
    sell: Option<String>,
    /// Connected headless mode: cast this spell on ourselves (learnt this
    /// session from a scroll, or named by a "Scroll of NAME" in the pack).
    #[arg(long)]
    cast: Option<String>,
    /// Connected headless mode: jump once after placement.
    #[arg(long)]
    jump: bool,
    /// Connected headless mode: also write `<screenshot>.mid.png` this many
    /// seconds after placement (mid-action captures).
    #[arg(long)]
    snap_at: Option<f32>,
    /// Connected headless mode: open the corpse of the attacked creature (or
    /// the container named here) and take everything.
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    loot: Option<String>,
    /// Camera override for --screenshot: x,y,z,yaw_deg,pitch_deg
    #[arg(long)]
    camera: Option<String>,
    /// Offline --screenshot: draw every panel plugin on sample data (real
    /// icons and spells from the portal) so the overlay can be checked
    /// without a server.
    #[arg(long, hide = true)]
    demo_ui: bool,
    /// Offline: the character select screen on three sample characters
    /// (no server), for a look or a `--screenshot`.
    #[arg(long)]
    demo_select: bool,
    /// Offline: the character creation screen on the real CharGen table
    /// with the 3D preview (no server); `--press ArrowRight` steps it.
    #[arg(long)]
    demo_create: bool,
    /// Offline: the connect screen (server, account, password), for a look
    /// or a `--screenshot`.
    #[arg(long)]
    demo_connect: bool,
    /// Join the local cross-process bus so plugins here and in other
    /// acswarm/acbot processes share posts and values: HOST:PORT or PORT
    /// (default 127.0.0.1:9500, or $ACSWARM_BUS). The first process up
    /// hosts it.
    #[arg(long, num_args = 0..=1, default_missing_value = "")]
    bus: Option<String>,
    /// Connected headless mode: once session 1 is placed, start this
    /// follower through the fleet panel (what its Start button does):
    /// ACCOUNT:PASSWORD:CHARACTER[:TEMPLATE[:TOWN[:HERITAGE[:SEX]]]]. With
    /// a template the character is created when the account lacks it.
    /// Repeatable; the run ends `--fleet-stop-after` seconds later.
    #[arg(long)]
    fleet_start: Vec<String>,
    /// Connected headless mode: stop every `--fleet-start` session this
    /// many seconds after session 1 was placed, then finish.
    #[arg(long, default_value_t = 45.0)]
    fleet_stop_after: f32,
    /// How many times to log back in after a session is dropped (a
    /// network drop, a server restart), before giving up and saying so.
    /// 0 turns reconnection off; a session the player quits is never
    /// reconnected whatever this says.
    #[arg(long, default_value_t = 6)]
    reconnect_tries: u32,
}

impl Cli {
    /// The resolved data directory. [`resolve_data_dir`] fills this in
    /// before the window opens, so it is always present by the time the
    /// app runs.
    fn data_dir(&self) -> &Path {
        self.data_dir
            .as_deref()
            .expect("data_dir is resolved in main before use")
    }
}

/// Where the chosen data directory is remembered between launches.
fn saved_data_dir_file() -> PathBuf {
    ac_plugin::Settings::config_dir().join("data-dir")
}

/// A directory holds the game data when the portal DAT is inside it.
fn valid_data_dir(dir: &Path) -> bool {
    dir.join("client_portal.dat").is_file()
}

/// The remembered choice, if it is still a valid data directory.
fn read_saved_data_dir() -> Option<PathBuf> {
    let raw = std::fs::read_to_string(saved_data_dir_file()).ok()?;
    let dir = PathBuf::from(raw.trim());
    valid_data_dir(&dir).then_some(dir)
}

/// Remember a chosen data directory for next time.
fn save_data_dir(dir: &Path) {
    let file = saved_data_dir_file();
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&file, dir.to_string_lossy().as_bytes()) {
        tracing::warn!("could not remember the data folder: {e}");
    }
}

/// The usual places the game data sits, tried before asking.
fn common_data_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        out.push(home.join("Downloads").join("ac_data"));
        out.push(home.join("ac_data"));
        out.push(
            home.join("Library")
                .join("Application Support")
                .join("acswarm")
                .join("ac_data"),
        );
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join("ac_data"));
            // Inside acswarm.app: MacOS/acswarm -> Resources/ac_data.
            out.push(dir.join("..").join("Resources").join("ac_data"));
        }
    }
    out.push(ac_plugin::Settings::config_dir().join("ac_data"));
    out
}

/// Open a native folder picker (Finder on macOS, Explorer on Windows),
/// looping until the user picks a folder with the game data or cancels.
/// Returns the chosen folder, or `None` if they cancelled.
#[cfg(not(target_os = "linux"))]
fn pick_data_dir() -> Option<PathBuf> {
    loop {
        let picked = rfd::FileDialog::new()
            .set_title("Choose your Asheron's Call data folder")
            .pick_folder()?;
        if valid_data_dir(&picked) {
            return Some(picked);
        }
        rfd::MessageDialog::new()
            .set_title("acswarm")
            .set_description(
                "That folder has no client_portal.dat. Choose the folder that \
                 holds the Asheron's Call data files (client_portal.dat and \
                 client_cell_1.dat).",
            )
            .show();
    }
}

/// Linux has no bundled native picker here; the terminal is the way in.
#[cfg(target_os = "linux")]
fn pick_data_dir() -> Option<PathBuf> {
    None
}

/// Tell the user, in a dialog where there is one, that no data folder was
/// chosen and the app cannot start.
#[cfg(not(target_os = "linux"))]
fn no_data_dir_notice() {
    rfd::MessageDialog::new()
        .set_title("acswarm")
        .set_description(
            "acswarm needs the Asheron's Call data files (client_portal.dat and \
             client_cell_1.dat). Launch it again and choose the folder that holds \
             them.",
        )
        .show();
}

#[cfg(target_os = "linux")]
fn no_data_dir_notice() {}

/// Where the player's servers and remembered logins are kept.
fn servers_path() -> PathBuf {
    ac_plugin::Settings::config_dir().join("servers.json")
}

/// Read the player's servers and logins.
fn load_servers() -> ac_plugin::servers::Servers {
    ac_plugin::servers::Servers::load(&ac_plugin::Settings::load(&servers_path()))
}

/// Write the player's servers and logins back to their file.
fn save_servers(servers: &ac_plugin::servers::Servers) {
    let path = servers_path();
    let mut settings = ac_plugin::Settings::load(&path);
    servers.save(&mut settings);
    if let Err(e) = settings.save(&path) {
        tracing::warn!("could not save the server list: {e}");
        return;
    }
    keep_to_yourself(&path);
}

/// Make a file readable only by the person who owns it.
///
/// This file holds account passwords in the clear, and it was being
/// written world-readable, which on a shared machine means any other
/// account could read them. Tightening the mode is not encryption and
/// does not pretend to be: anything running as this user can still read
/// it. It closes the easy door, not every door.
fn keep_to_yourself(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The directory too: a world-readable directory is how the
        // backup file beside this one gets found.
        for target in [path.parent().unwrap_or(path), path] {
            let owner_only = if target.is_dir() { 0o700 } else { 0o600 };
            if let Err(e) =
                std::fs::set_permissions(target, std::fs::Permissions::from_mode(owner_only))
            {
                tracing::warn!("could not lock down {}: {e}", target.display());
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Settle on a data directory: an explicit choice wins; otherwise the
/// remembered one, then the usual places, then a folder picker when a
/// person is at the keyboard. Headless runs never prompt.
fn resolve_data_dir(given: Option<PathBuf>, interactive: bool) -> Result<PathBuf> {
    if let Some(dir) = given {
        if valid_data_dir(&dir) {
            save_data_dir(&dir);
        }
        return Ok(dir);
    }
    if let Some(dir) = read_saved_data_dir() {
        return Ok(dir);
    }
    for dir in common_data_dirs() {
        if valid_data_dir(&dir) {
            save_data_dir(&dir);
            return Ok(dir);
        }
    }
    if interactive {
        if let Some(dir) = pick_data_dir() {
            save_data_dir(&dir);
            return Ok(dir);
        }
        no_data_dir_notice();
        std::process::exit(0);
    }
    anyhow::bail!(
        "no data directory found: pass --data-dir or set AC_DATA_DIR (it must hold client_portal.dat)"
    )
}

/// Parse a `--fleet-start` spec into the session the fleet panel starts
/// (a follower). Fields after the character are creation choices; a
/// blank one keeps the default.
fn parse_fleet_start(spec: &str) -> Result<ac_plugin::SessionSpec> {
    let parts: Vec<&str> = spec.split(':').collect();
    anyhow::ensure!(
        parts.len() >= 3 && !parts[0].is_empty() && !parts[1].is_empty() && !parts[2].is_empty(),
        "--fleet-start wants ACCOUNT:PASSWORD:CHARACTER[:TEMPLATE[:TOWN[:HERITAGE[:SEX]]]], got {spec:?}"
    );
    let field = |i: usize| {
        parts
            .get(i)
            .filter(|p| !p.is_empty())
            .map(|p| p.to_string())
    };
    let create = (parts.len() > 3).then(|| ac_plugin::CreateSpec {
        name: parts[2].to_string(),
        template: field(3),
        town: field(4),
        heritage: field(5),
        sex: field(6),
    });
    Ok(ac_plugin::SessionSpec {
        account: parts[0].to_string(),
        password: parts[1].to_string(),
        character: create.is_none().then(|| parts[2].to_string()),
        create,
        role: ac_plugin::Role::Follower,
    })
}

/// An icon loader for the egui overlay: decodes RenderSurfaces (0x06) from
/// the portal on demand. The archives are opened on the first icon so a
/// viewer that never draws one pays nothing.
/// winit key code -> egui key by name (`KeyA` -> `A`, `Digit1` -> `1`,
/// `ArrowUp`, `F1`, `Space`...), for plugins' key hooks.
fn egui_key(code: KeyCode) -> Option<egui::Key> {
    let name = format!("{code:?}");
    let name = name
        .strip_prefix("Key")
        .or_else(|| name.strip_prefix("Digit"))
        .unwrap_or(&name);
    egui::Key::from_name(name)
}

/// Every session's client, for plugin callbacks.
fn clients_of(nets: &mut [Net]) -> Vec<&mut ac_client::Client> {
    nets.iter_mut().map(|n| &mut n.client).collect()
}

fn icon_loader(data_dir: PathBuf) -> ac_plugin::IconLoader {
    let assets: std::cell::OnceCell<Option<ac_scene::Assets>> = std::cell::OnceCell::new();
    std::rc::Rc::new(move |id| {
        let assets = assets
            .get_or_init(|| match ac_scene::Assets::open(&data_dir) {
                Ok(a) => Some(a),
                Err(e) => {
                    tracing::warn!("icons: opening DAT archives: {e}");
                    None
                }
            })
            .as_ref()?;
        match assets.texture_rgba(id, None) {
            Ok(img) => Some(img),
            Err(e) => {
                tracing::debug!("icon {id:#010x}: {e}");
                None
            }
        }
    })
}

/// Live server connection state for `--connect`.
struct Net {
    client: ac_client::Client,
    /// The server this session was started against, so a dropped
    /// session can be started again against the same one. Who it was
    /// comes from the session itself (`reconnect::Carry`), which knows
    /// the character actually in the world rather than the one asked
    /// for.
    host: String,
    /// When to come back after a drop (see `ac_client::reconnect`).
    reconnect: ac_client::reconnect::Reconnect,
    /// This client's ending has been handed to `reconnect`. Cleared when
    /// a fresh client replaces it, so the next ending is heard too.
    ending_reported: bool,
    last_generation: u64,
    pickables: Vec<scene::Pickable>,
    anims: std::collections::HashMap<u32, scene::ObjectAnim>,
    last_anim_refresh: Instant,
}

struct App {
    cli: Cli,
    window: Option<Arc<Window>>,
    gpu: Option<gpu::Gpu>,
    nets: Vec<Net>,
    /// Which session the window draws and the keys steer.
    active: usize,
    frame_dt: f32,
    camera: camera::Camera,
    keys: HashSet<KeyCode>,
    looking: bool,
    cursor: Option<(f64, f64)>,
    /// A full-power jump was asked for (`--jump`); Space itself charges
    /// through `Input::jump_held`.
    jump_requested: bool,
    last_cursor: Option<(f64, f64)>,
    last_frame: Instant,
    ui: Option<ui::Ui>,
    fps: f32,
    plugins: plugins::Host,
    /// A session switch a plugin asked for during the UI pass.
    pending_switch: Option<usize>,
    /// When the next frame may start (frame rate cap).
    next_frame: Instant,
    /// Landblocks currently uploaded to the GPU (the active session's area).
    loaded_blocks: std::collections::HashSet<u32>,
    /// The time of day the sky was last built for (see `tick_sky`).
    sky_at: Option<f32>,
    /// Block id -> is a dungeon (learnt when the block is built).
    dungeon: std::collections::HashMap<u32, bool>,
    /// Caches shared by every session: decoded meshes, GPU meshes, palettes,
    /// motion tables, block particle emitters, and the sound device.
    mesh_cache: std::collections::HashMap<u32, ac_scene::model::Mesh>,
    gpu_meshes: scene::GpuMeshCache,
    palettes: scene::Palettes,
    tables: std::collections::HashMap<u32, Option<ac_formats::motion_table::MotionTable>>,
    fx: world_fx::WorldFx,
    /// Draw the planned route as a line of marks on the ground.
    show_route: bool,
    /// The menu's Quit: the next frame saves the settings, disconnects
    /// and leaves the event loop.
    quit_requested: bool,
    /// Something was drawn as particles last frame, so an empty list
    /// this frame still has to be uploaded to clear it.
    drew_particles: bool,
    /// When the window opened, for effects that move with time.
    started: Instant,
    audio: Option<ac_audio::Audio>,
    /// Character select and creation, between login and the world.
    lobby: ac_plugin::lobby::Lobby,
    /// The creation screen's 3D preview, while it is up.
    preview: Option<Preview>,
    /// The archives every session shares, once the first connected.
    assets: Option<std::rc::Rc<ac_scene::Assets>>,
    /// Sessions plugins asked to start and stop this frame, applied
    /// once no session is being ticked (`apply_pending_sessions`).
    pending_sessions: Vec<(Vec<ac_plugin::SessionSpec>, Vec<usize>)>,
    /// Frame pacing, culling settings and cost accounting.
    render: RenderState,
    /// The server the connect screen chose (`host:port`), used for every
    /// session it and the fleet start; falls back to `--connect`.
    current_host: Option<String>,
}

/// Rendering-side state the frame loop keeps between frames.
#[derive(Default)]
struct RenderState {
    perf: perf::Perf,
    /// `--render none`: the world never reaches the GPU.
    none: bool,
    /// The window has focus; without it frames are drawn at 10 fps at most.
    unfocused: bool,
    /// The window is hidden (minimised or covered): tick, but draw nothing.
    occluded: bool,
    /// The camera and window size the last frame was drawn with; the same
    /// again with nothing else changed means the frame can be skipped.
    last_view: Option<(glam::Mat4, (u32, u32))>,
    /// When unused GPU meshes and materials were last swept.
    last_prune: Option<Instant>,
    /// The blackboard's `render.draw_distance` last applied.
    draw_distance: f32,
}

/// Blackboard key the options panel sets: how far from the eye objects are
/// drawn, metres (0 = no limit).
use plugins::panels::options::DRAW_DISTANCE_KEY;

/// Frame rate of a window without focus.
const UNFOCUSED_FPS: u32 = 10;

/// How often meshes and materials nothing references are dropped.
const PRUNE_EVERY: Duration = Duration::from_secs(30);

/// The character model drawn beside the creation screen: the look it was
/// built for, its appearance, and the turntable angle.
struct Preview {
    look: ac_scene::chargen::Look,
    setup: u32,
    app: ac_scene::model::Appearance,
    /// Mesh cache key for this look (bumped per rebuild).
    key: u64,
    yaw: f32,
    /// Instances were uploaded; cleared once when the screen closes.
    drawn: bool,
}

impl App {
    /// Where the creation screen's preview camera stands: in front of the
    /// model at the origin (it faces -Y), shifted so the model sits to the
    /// right of the creation window.
    fn preview_camera(&mut self) {
        self.camera.position = Vec3::new(-0.95, -3.1, 0.95);
        self.camera.yaw = 0.0;
        self.camera.pitch = 0.0;
        self.camera.far = 500.0;
    }

    /// Keep the creation screen's 3D preview in step with the build: a
    /// changed look is re-described (`ac_scene::chargen::describe`) and
    /// re-uploaded; otherwise the model just turns on its turntable.
    fn tick_lobby(&mut self, gpu: &mut gpu::Gpu, dt: f32) {
        let build = self.lobby.preview().cloned();
        let Some(build) = build else {
            if self.preview.take().is_some_and(|p| p.drawn) {
                gpu.set_player_instances(Vec::new());
            }
            return;
        };
        let Some(assets) = self.lobby.preview_assets() else {
            return;
        };
        let look = build.look;
        let stale = self.preview.as_ref().is_none_or(|p| p.look != look);
        if stale {
            let old = self.preview.as_ref().map(|p| p.key);
            let (setup, app) = match ac_scene::chargen::describe(&assets, &look) {
                Ok(desc) => (desc.setup_id, desc.appearance(&assets)),
                Err(e) => {
                    tracing::warn!("preview {look:?}: {e}");
                    (0, ac_scene::model::Appearance::default())
                }
            };
            if let Some(p) = &app.palette {
                self.palettes.insert(app.palette_hash, p.clone());
            }
            if let Some(old) = old {
                self.gpu_meshes.retain(|(_, k), _| *k != old);
            }
            let key = old
                .map(|k| k.wrapping_add(1))
                .unwrap_or(0xC4A7_0000_0000_0001);
            let yaw = self
                .preview
                .as_ref()
                .map(|p| p.yaw)
                .unwrap_or(std::f32::consts::PI + 0.35);
            self.preview = Some(Preview {
                look,
                setup,
                app,
                key,
                yaw,
                drawn: false,
            });
            self.preview_camera();
        }
        let Some(p) = self.preview.as_mut() else {
            return;
        };
        if !self.looking {
            p.yaw += dt * 0.35;
        }
        if p.setup == 0 {
            return;
        }
        let t = glam::Mat4::from_rotation_z(p.yaw);
        let instances = scene::instances_for(
            &assets,
            gpu,
            &mut self.gpu_meshes,
            &self.palettes,
            p.setup,
            t,
            &p.app,
            p.key,
            None,
        );
        p.drawn = true;
        gpu.set_player_instances(instances);
    }

    /// Apply what plugins asked for (chat lines to the active log, a
    /// session switch, the client's end); sessions to start or stop
    /// wait for `apply_pending_sessions`.
    fn apply_requests(&mut self, r: plugins::Requests) {
        if let Some(ui) = &mut self.ui {
            for (text, kind) in r.chat {
                ui.push_chat(text, kind);
            }
        }
        if let Some(a) = r.activate {
            self.switch_to(a);
        }
        if r.quit {
            self.quit_requested = true;
        }
        // A new log filter chosen in the options panel.
        let chosen: Option<String> = self
            .plugins
            .board
            .get(ac_plugin::panels::options::LOG_FILTER_KEY)
            .and_then(|v| v.as_str().map(str::to_string));
        if let Some(f) = chosen {
            if f != logging::current() {
                match logging::set(&f) {
                    Ok(()) => tracing::info!("log filter: {f}"),
                    Err(e) => tracing::warn!("log filter {f:?}: {e}"),
                }
            }
        }
        if r.pick_data_dir {
            self.change_data_dir();
        }
        self.defer_sessions(r.start_sessions, r.stop_sessions);
    }

    /// The Options panel's "Change data folder…": open the native picker,
    /// remember the new folder, and tell the player it takes effect on the
    /// next launch (the DAT archives are opened once at startup).
    fn change_data_dir(&mut self) {
        if let Some(dir) = pick_data_dir() {
            save_data_dir(&dir);
            let msg = format!(
                "Data folder set to {}. Restart acswarm to load it.",
                dir.display()
            );
            if let Some(ui) = &mut self.ui {
                ui.push_chat(msg, 0);
            }
        }
    }

    fn defer_sessions(&mut self, start: Vec<ac_plugin::SessionSpec>, stop: Vec<usize>) {
        if !start.is_empty() || !stop.is_empty() {
            self.pending_sessions.push((start, stop));
        }
    }

    /// Start and stop the sessions plugins asked for. Stops go first,
    /// highest index first, so each index still means the session the
    /// plugin saw; then the starts are appended in order.
    fn apply_pending_sessions(&mut self) {
        for (start, stop) in std::mem::take(&mut self.pending_sessions) {
            let mut stop = stop;
            stop.sort_unstable();
            stop.dedup();
            for i in stop.into_iter().rev() {
                self.remove_session(i);
            }
            for spec in start {
                self.start_session(spec);
            }
        }
    }

    /// Log `spec` in as another session of this process, against the
    /// server of `--connect`. What went wrong is left on the blackboard
    /// (`fleet.error.<account>`) for the fleet panel.
    fn start_session(&mut self, spec: ac_plugin::SessionSpec) {
        let account = spec.account.clone();
        let err_key = plugins::panels::fleet::error_key(&account);
        let fail = |app: &mut App, why: String| {
            tracing::warn!("cannot start a session for {account}: {why}");
            if let Some(ui) = &mut app.ui {
                ui.push_chat(format!("Cannot start {account}: {why}"), 0);
            }
            app.plugins.board.set_local(err_key.clone(), why);
        };
        let Some(host) = self
            .current_host
            .clone()
            .or_else(|| self.cli.connect.clone())
        else {
            fail(
                self,
                "no server chosen (use the connect screen or --connect)".into(),
            );
            return;
        };
        if self
            .nets
            .iter()
            .any(|n| n.client.config.account.eq_ignore_ascii_case(&account))
        {
            fail(self, "already running".into());
            return;
        }
        let assets = match &self.assets {
            Some(a) => a.clone(),
            None => match ac_scene::Assets::open(self.cli.data_dir()) {
                Ok(a) => {
                    let a = std::rc::Rc::new(a);
                    self.assets = Some(a.clone());
                    a
                }
                Err(e) => {
                    fail(self, format!("opening the DAT archives: {e}"));
                    return;
                }
            },
        };
        let cfg = ac_client::Config {
            host: host.clone(),
            account: account.clone(),
            password: spec.password.clone(),
            character: spec.character_name().map(str::to_string),
            // Enter the world straight away when a character (or a
            // character to create) is named; otherwise stop at the lobby's
            // character-select screen (the connect screen's path).
            auto_enter: spec.character_name().is_some() || spec.create.is_some(),
        };
        let mut client = match ac_client::Client::connect(cfg, assets) {
            Ok(c) => c,
            Err(e) => {
                fail(self, e.to_string());
                return;
            }
        };
        if let Some(create) = spec.create.clone() {
            client.create_when_missing(create);
        }
        let reconnect = self.reconnect_rule();
        self.nets.push(Net {
            client,
            host,
            reconnect,
            ending_reported: false,
            last_generation: 0,
            pickables: Vec::new(),
            anims: Default::default(),
            last_anim_refresh: Instant::now(),
        });
        self.plugins
            .board
            .set_local(err_key, ac_plugin::Value::Null);
        let n = self.nets.len();
        tracing::info!("session {n} started for {account} ({})", spec.role.label());
        if let Some(ui) = &mut self.ui {
            ui.push_chat(format!("Session {n} started ({account})"), 0);
        }
    }

    /// Disconnect session `i` and drop it; the sessions after it move
    /// down one and every plugin hears `session_removed`. The window
    /// keeps showing the same session when it was not the one dropped,
    /// else the nearest one left.
    fn remove_session(&mut self, i: usize) {
        if i >= self.nets.len() {
            return;
        }
        let mut net = self.nets.remove(i);
        net.client.disconnect(Instant::now());
        let account = net.client.config.account.clone();
        drop(net);
        self.plugins.session_removed(i);
        tracing::info!("session {} stopped ({account})", i + 1);
        if let Some(ui) = &mut self.ui {
            ui.push_chat(format!("Session {} stopped ({account})", i + 1), 0);
        }
        if let Some(p) = self.pending_switch.take() {
            if p != i {
                self.pending_switch = Some(if p > i { p - 1 } else { p });
            }
        }
        if self.nets.is_empty() {
            self.active = 0;
        } else if i < self.active {
            self.active -= 1;
        } else if i == self.active {
            let next = self.active.min(self.nets.len() - 1);
            self.active = usize::MAX;
            self.lobby = Default::default();
            self.switch_to(next);
        }
    }

    /// The reconnection rule a new session starts with (`--reconnect-tries`).
    fn reconnect_rule(&self) -> ac_client::reconnect::Reconnect {
        ac_client::reconnect::Reconnect::new(ac_client::reconnect::Policy {
            tries: self.cli.reconnect_tries,
            ..Default::default()
        })
    }

    /// Notice the sessions that have ended, and put the dropped ones
    /// back.
    ///
    /// A session being reconnected keeps its place in `nets`: the window
    /// goes on showing the world it was last in, the plugins keep the
    /// per-session state they index by that slot, and the fleet panel
    /// keeps its row. Only the `Client` inside is replaced, so what
    /// survives a reconnect is what `reconnect::Carry` names and nothing
    /// else.
    fn tick_reconnect(&mut self, now: Instant) {
        if self.nets.is_empty() {
            return;
        }
        use ac_client::reconnect::{Action, State};
        let mut notices: Vec<(usize, String, String)> = Vec::new();
        let mut attempts: Vec<usize> = Vec::new();
        let mut settled: Vec<String> = Vec::new();
        for (i, net) in self.nets.iter_mut().enumerate() {
            // In the world (or back at the select screen we were
            // dropped from): the attempt worked. `Client::placed` stays
            // true on a dropped client, whose scene is still up, so this
            // only counts while an attempt is actually in flight.
            let back = net.client.placed()
                || (net.client.config.character.is_none() && net.client.characters_known);
            if back && matches!(net.reconnect.state(), State::Trying { .. }) {
                net.reconnect.placed();
                settled.push(net.client.config.account.clone());
            }
            if let Some(ending) = net.client.ending() {
                if !net.ending_reported {
                    net.ending_reported = true;
                    tracing::warn!(
                        "session {} ({}) ended: {}",
                        i + 1,
                        net.client.config.account,
                        ending.describe()
                    );
                    net.reconnect.ended(ending, now);
                }
            }
            if net.reconnect.poll(now) == Action::Connect {
                attempts.push(i);
            }
            let account = net.client.config.account.clone();
            while let Some(line) = net.reconnect.take_notice() {
                notices.push((i, account.clone(), line));
            }
        }
        for (i, account, line) in notices {
            let line = if i == self.active {
                line
            } else {
                format!("[{account}] {line}")
            };
            tracing::info!("{line}");
            if let Some(ui) = &mut self.ui {
                ui.push_chat(line, 0);
            }
        }
        // A session that gave up shows in the fleet panel the way one
        // that could not be started does; one that came back clears it.
        for account in settled {
            self.plugins.board.set_local(
                plugins::panels::fleet::error_key(&account),
                ac_plugin::Value::Null,
            );
        }
        for i in attempts {
            self.reconnect_session(i, now);
        }
        for net in &self.nets {
            if let Some(stopped) = net.reconnect.stopped() {
                let why = match stopped {
                    ac_client::reconnect::Stopped::Quit => continue,
                    ac_client::reconnect::Stopped::Exhausted => {
                        "dropped, and out of retries".into()
                    }
                    ac_client::reconnect::Stopped::Fatal(why) => why.clone(),
                };
                let key = plugins::panels::fleet::error_key(&net.client.config.account);
                if self.plugins.board.get(&key).is_none() {
                    self.plugins.board.set_local(key, why);
                }
            }
        }
    }

    /// Log a dropped session back in where it stood: the same slot in
    /// `nets`, the same account against the same server, and the same
    /// character, so the player lands back in the world rather than at
    /// the character-select screen.
    fn reconnect_session(&mut self, i: usize, now: Instant) {
        let Some(net) = self.nets.get(i) else { return };
        let carry = ac_client::reconnect::Carry::of(&net.client);
        let cfg = ac_client::Config {
            host: net.host.clone(),
            account: net.client.config.account.clone(),
            password: net.client.config.password.clone(),
            character: carry.character.clone(),
            // Straight back into the world when we know who we were; a
            // session dropped at the select screen comes back to it.
            auto_enter: carry.character.is_some(),
        };
        let account = cfg.account.clone();
        let assets = match self.assets.clone() {
            Some(a) => a,
            None => {
                let net = &mut self.nets[i];
                net.reconnect.ended(
                    ac_client::reconnect::Ending::Fatal("the DAT archives are not open".into()),
                    now,
                );
                return;
            }
        };
        tracing::info!(
            "reconnecting session {} ({account}) as {}",
            i + 1,
            carry.character.as_deref().unwrap_or("<character select>")
        );
        match ac_client::Client::connect(cfg, assets) {
            Ok(mut client) => {
                carry.apply(&mut client);
                let net = &mut self.nets[i];
                // Tell the server to let the old session go, in case it
                // is only half dead (an attempt that timed out).
                net.client.disconnect(now);
                net.client = client;
                net.ending_reported = false;
                net.last_generation = 0;
                net.pickables = Vec::new();
                net.anims.clear();
                if i == self.active {
                    self.lobby = Default::default();
                }
            }
            Err(e) => {
                // The socket would not open (no route, DNS gone): this
                // attempt is spent, and the rule schedules the next.
                tracing::warn!("reconnecting {account}: {e}");
                let net = &mut self.nets[i];
                net.reconnect
                    .ended(ac_client::reconnect::Ending::Dropped(e.to_string()), now);
            }
        }
    }

    fn interact(&mut self, guid: u32) {
        if let Some(net) = self.nets.get_mut(self.active) {
            net.client.interact(guid);
        }
    }

    fn cast(&mut self, spell: u32) {
        if let Some(net) = self.nets.get_mut(self.active) {
            net.client.cast(spell);
        }
    }

    fn toggle_combat(&mut self) {
        if let Some(net) = self.nets.get_mut(self.active) {
            net.client.toggle_combat();
        }
    }

    fn use_by_name(&mut self, name: &str) -> bool {
        self.nets
            .get_mut(self.active)
            .is_some_and(|net| net.client.use_by_name(name))
    }

    /// What the chat box submitted this frame: `/commands` go to the
    /// plugins, everything else is said in the world. (The panels act on
    /// the client themselves; they are plugins.)
    fn apply_ui_commands(&mut self) {
        let Some(ui) = self.ui.as_mut() else { return };
        let outgoing = std::mem::take(&mut ui.outgoing);
        let active = self.active;
        if self.nets.is_empty() {
            return;
        }
        let mut requests: Vec<plugins::Requests> = Vec::new();
        for t in outgoing {
            if t.starts_with('/') {
                tracing::info!("command {t} (session {})", active + 1);
                let clients = clients_of(&mut self.nets);
                let r = self.plugins.command(clients, active, &t);
                for (l, _) in &r.chat {
                    tracing::info!("{t} -> {l}");
                }
                let consumed = r.consumed;
                requests.push(r);
                if !consumed {
                    // Not a plugin command: the game's own (/lifestone,
                    // /tell ...) or a server command sent as @.
                    if let Some(net) = self.nets.get_mut(active) {
                        net.client.slash_command(&t);
                    }
                }
            } else if let Some(net) = self.nets.get_mut(active) {
                net.client.say(&t);
            }
        }
        for r in requests {
            self.apply_requests(r);
        }
    }

    /// Run the egui pass for this frame: the plugins' panels (when there
    /// are sessions, or in `--demo-ui`), then the host's status line and
    /// chat. What the plugins asked for is applied afterwards.
    fn run_overlay(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, w: u32, h: u32) {
        let Some(ui) = self.ui.as_mut() else { return };
        let plugins = &mut self.plugins;
        let lobby = &mut self.lobby;
        let active = self.active;
        let draw_plugins = !self.nets.is_empty() || self.cli.demo_ui;
        let mut clients: Vec<&mut ac_client::Client> =
            self.nets.iter_mut().map(|n| &mut n.client).collect();
        let mut requests: Option<plugins::Requests> = None;
        let mut world_drop: Option<(u32, f64, f64)> = None;
        ui.hud_hidden = lobby.visible();
        // The frame's camera, for plugins that draw over the world
        // (nameplates).
        let vp = self.camera.view_proj(w as f32 / h.max(1) as f32);
        plugins.board.set(
            plugins::panels::nameplates::CAMERA_KEY,
            ac_plugin::Value::Array(
                vp.to_cols_array()
                    .iter()
                    .map(|x| (*x as f64).into())
                    .collect(),
            ),
        );
        ui.begin(
            self.window.as_deref(),
            &mut |egui| {
                if draw_plugins {
                    let borrowed: Vec<&mut ac_client::Client> =
                        clients.iter_mut().map(|c| &mut **c).collect();
                    requests = Some(plugins.ui(borrowed, active, egui));
                }
                if lobby.visible() {
                    lobby.ui(egui, clients.get_mut(active).map(|c| &mut **c));
                }
                // A carried item released over the world (no panel under
                // the pointer): give it to the creature there, else drop it.
                if let Some(p) = egui::DragAndDrop::payload::<ac_plugin::panels::ItemDrag>(egui) {
                    let released = egui.input(|i| i.pointer.any_released());
                    if released && !egui.is_pointer_over_egui() {
                        if let Some(pos) = egui.input(|i| i.pointer.latest_pos()) {
                            let ppp = egui.pixels_per_point();
                            world_drop = Some((p.0, (pos.x * ppp) as f64, (pos.y * ppp) as f64));
                        }
                    }
                }
            },
            device,
            queue,
            w,
            h,
        );
        if let Some(r) = requests {
            for (text, kind) in r.chat {
                ui.push_chat(text, kind);
            }
            self.pending_switch = r.activate;
            if r.quit {
                self.quit_requested = true;
            }
            if r.pick_data_dir {
                if let Some(dir) = pick_data_dir() {
                    save_data_dir(&dir);
                    ui.push_chat(
                        format!(
                            "Data folder set to {}. Restart acswarm to load it.",
                            dir.display()
                        ),
                        0,
                    );
                }
            }
            self.defer_sessions(r.start_sessions, r.stop_sessions);
        }
        if let Some((item, px, py)) = world_drop {
            self.world_drop(item, px, py, (w, h));
        }
        // The connect screen asked to log in, or changed the saved servers.
        if let Some(login) = self.lobby.take_connect() {
            self.begin_login(login);
        }
        if let Some(servers) = self.lobby.take_dirty_servers() {
            save_servers(&servers);
        }
    }

    /// Start a session from the connect screen: remember the server and
    /// connect, landing on the character-select screen.
    fn begin_login(&mut self, login: ac_plugin::servers::Login) {
        self.current_host = Some(login.host);
        // A character chosen on the connect screen goes straight in.
        // Passing it here is what turns on `auto_enter`, so the login
        // does not stop to ask again for something already answered.
        let character = Some(login.character).filter(|c| !c.trim().is_empty());
        self.start_session(ac_plugin::SessionSpec {
            account: login.account,
            password: login.password,
            character,
            create: None,
            role: ac_plugin::Role::default(),
        });
    }

    /// Finish a drag that ended over the 3D view: hand the item to the
    /// NPC or player under the pointer, otherwise drop it on the ground.
    fn world_drop(&mut self, item: u32, px: f64, py: f64, size: (u32, u32)) {
        use ac_world::{item_type, object_desc_flags};
        let target = self.pick(px, py, size);
        let Some(net) = self.nets.get_mut(self.active) else {
            return;
        };
        let c = &mut net.client;
        let receiver = target.filter(|g| {
            c.world.objects.get(g).is_some_and(|o| {
                o.item_type & item_type::CREATURE != 0
                    && (o.object_desc_flags & object_desc_flags::PLAYER != 0
                        || o.object_desc_flags & object_desc_flags::ATTACKABLE == 0)
            })
        });
        let usable = c
            .world
            .objects
            .get(&item)
            .is_some_and(|o| ac_world::usable::needs_target(o.usable));
        // A chest, a house hook or a storage chest in the world takes the
        // item (PutItemInContainer); creatures are handed it.
        let container = target.filter(|g| {
            c.world.objects.get(g).is_some_and(|o| {
                o.item_type & item_type::CONTAINER != 0 && o.item_type & item_type::CREATURE == 0
            })
        });
        match (receiver, target) {
            // Dragging onto someone is always a gift; a kit or a stone is
            // used on them by selecting them and using the item.
            (Some(t), _) => {
                c.give(t, item, None);
            }
            // A key on a chest, a stone on an item lying there.
            (_, Some(t)) if usable => {
                c.use_on(item, t);
            }
            (_, Some(t)) if container == Some(t) => {
                c.store_in(item, t);
            }
            _ => {
                c.drop_item(item);
            }
        }
    }

    /// Left click in the world: select the object under the cursor and ask
    /// the server to appraise it; a second click on the same object within
    /// half a second uses it (opens doors, talks to NPCs, picks up items).
    /// The object under a window pixel, unless a wall hides it.
    fn pick(&mut self, px: f64, py: f64, (w, h): (u32, u32)) -> Option<u32> {
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
        let net = self.nets.get_mut(self.active)?;
        let mut best: Option<(f32, u32)> = None;
        for p in &net.pickables {
            if let Some(t) = p.hit(near, dir) {
                tracing::trace!(
                    "hit {} at t={t:.2} (center {:?} r {:.2})",
                    net.client
                        .world
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
        // A wall in front of the hit hides it.
        if let (Some((t, _)), Some(pl)) = (best, net.client.player.as_mut()) {
            let assets = &net.client.assets;
            if pl.first_wall(assets, near, near + dir * t).is_some() {
                tracing::debug!("click blocked by static geometry");
                best = None;
            }
        }
        best.map(|(_, g)| g)
    }

    fn click(&mut self, px: f64, py: f64, (w, h): (u32, u32)) {
        use ac_net::messages::action;
        let picked = self.pick(px, py, (w, h));
        let Some(net) = self.nets.get_mut(self.active) else {
            return;
        };
        let Some(guid) = picked else {
            net.client.select(None);
            return;
        };
        let now = Instant::now();
        let again = matches!(net.client.last_click, Some((t, g)) if g == guid && now - t < Duration::from_millis(500));
        net.client.last_click = Some((now, guid));
        net.client.select(Some(guid));
        let name = net
            .client
            .world
            .objects
            .get(&guid)
            .map(|o| o.name.clone())
            .unwrap_or_default();
        if again {
            net.client.last_click = None;
            self.interact(guid);
        } else {
            tracing::info!("select {name} ({guid:#010x})");
            net.client
                .session
                .send_action(action::IDENTIFY_OBJECT, &guid.to_le_bytes());
        }
    }

    /// The status line: frame rate, where the character stands, what is
    /// selected (with its icon), combat mode. The panels are plugins.
    /// Follow the world's clock: the sky, sun and fog change as the day
    /// goes by. Only outdoors, and only when the time has moved enough to
    /// be worth rebuilding (a day in Dereth is under an hour, so this is
    /// every few seconds).
    fn tick_sky(&mut self, gpu: &mut gpu::Gpu) {
        let Some(net) = self.nets.get(self.active) else {
            return;
        };
        let indoors = net
            .client
            .player
            .as_ref()
            .map(|p| p.is_indoors())
            .unwrap_or(false);
        let block = net.client.player.as_ref().map(|p| p.cell & 0xFFFF_0000);
        if indoors || block.is_some_and(|b| self.dungeon.get(&b) == Some(&true)) {
            return;
        }
        let Some(day) = net.client.day_time() else {
            return;
        };
        if self
            .sky_at
            .is_some_and(|t: f32| (t - day.fraction).abs() < 0.004)
        {
            return;
        }
        let Some(env) = net
            .client
            .assets
            .region()
            .ok()
            .and_then(|r| sky::Environment::from_region(&r, day.fraction))
        else {
            return;
        };
        self.sky_at = Some(day.fraction);
        gpu.set_environment(env);
    }

    fn refresh_status(&mut self) {
        let Some(ui) = &mut self.ui else { return };
        let mut s = format!("{:.0} fps", self.fps);
        if self.cli.perf {
            s += &self.render.perf.status();
        }
        if let Some(net) = self.nets.get(self.active) {
            match net.client.world.player().and_then(|o| o.position) {
                Some(p) => {
                    s += &format!(
                        "  {}  cell {:#010x}  x {:.1} y {:.1} z {:.1}  objects {}",
                        ac_world::map_coord_str(&p),
                        p.cell,
                        p.local.x,
                        p.local.y,
                        p.local.z,
                        net.client.world.drawable().count()
                    );
                    ui.status_icon = ac_plugin::IconLayers::default();
                    if let Some(o) = net
                        .client
                        .selected
                        .and_then(|g| net.client.world.objects.get(&g))
                    {
                        s += &format!("  selected: {}", o.name);
                        ui.status_icon = ac_plugin::IconLayers::of(o);
                    }
                    if net.client.combat {
                        s += "  [melee]";
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
    }

    fn start_connect(&mut self) -> Result<()> {
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
        let assets = std::rc::Rc::new(
            ac_scene::Assets::open(self.cli.data_dir()).context("opening DAT archives")?,
        );
        let audio = if self.cli.mute || self.cli.screenshot.is_some() {
            None
        } else {
            match ac_audio::Audio::new() {
                Ok(a) => Some(a),
                Err(e) => {
                    tracing::warn!("audio disabled: {e}");
                    None
                }
            }
        };
        // Headless runs enter with the first character as before; a window
        // with no --character shows the select screen.
        let auto_enter = self.cli.screenshot.is_some();
        let mut configs = vec![ac_client::Config {
            host: host.clone(),
            account,
            password,
            character: self.cli.character.clone(),
            auto_enter,
        }];
        for spec in &self.cli.clients {
            let mut parts = spec.splitn(3, ':');
            let (Some(a), Some(p)) = (parts.next(), parts.next()) else {
                anyhow::bail!("--client wants ACCOUNT:PASSWORD[:CHARACTER], got {spec:?}");
            };
            configs.push(ac_client::Config {
                host: host.clone(),
                account: a.to_string(),
                password: p.to_string(),
                character: parts.next().map(str::to_string),
                auto_enter: true,
            });
        }
        self.audio = audio;
        self.assets = Some(assets.clone());
        for cfg in configs {
            let host = cfg.host.clone();
            let client = ac_client::Client::connect(cfg, assets.clone())?;
            let reconnect = self.reconnect_rule();
            self.nets.push(Net {
                client,
                host,
                reconnect,
                ending_reported: false,
                last_generation: 0,
                pickables: Vec::new(),
                anims: Default::default(),
                last_anim_refresh: Instant::now(),
            });
        }
        Ok(())
    }

    /// Pump the connection: send, receive, apply messages, rebuild scenes.
    /// Tick one session: keys steer only the active one; the others keep
    /// their connection, physics and plugins running.
    fn tick_client(
        &mut self,
        i: usize,
        now: Instant,
    ) -> Option<(ac_client::PlayerFrame, Vec<ac_client::Event>)> {
        let is_active = i == self.active;
        let jump = if is_active {
            std::mem::take(&mut self.jump_requested)
        } else {
            false
        };
        let keys = &self.keys;
        let flying = self.nets.get(i)?.client.noclip();
        let input = self.nets.get(i)?.client.player.as_ref().map(|_| {
            if is_active {
                // Flying, the jump key climbs and Control descends.
                player::Input {
                    forward: (keys.contains(&KeyCode::KeyW) as i8
                        - keys.contains(&KeyCode::KeyS) as i8) as f32,
                    strafe: (keys.contains(&KeyCode::KeyD) as i8
                        - keys.contains(&KeyCode::KeyA) as i8) as f32,
                    run: !keys.contains(&KeyCode::ShiftLeft),
                    jump: jump && !flying,
                    jump_held: keys.contains(&KeyCode::Space) && !flying,
                    climb: if flying {
                        (keys.contains(&KeyCode::Space) as i8
                            - keys.contains(&KeyCode::ControlLeft) as i8)
                            as f32
                    } else {
                        0.0
                    },
                }
            } else {
                player::Input::default()
            }
        });
        let count = self.nets.len();
        let net = self.nets.get_mut(i)?;
        let frame = net.client.tick(input, self.frame_dt, now);
        let events = net.client.drain_events();
        for ev in &events {
            match ev {
                ac_client::Event::Chat { text, kind } => {
                    tracing::info!("[{}] chat: {text}", net.client.config.account);
                    if is_active {
                        if let Some(ui) = &mut self.ui {
                            ui.push_chat(text.clone(), *kind);
                        }
                    }
                }
                ac_client::Event::Sound { wave, volume } => {
                    if is_active {
                        if let Some(audio) = &self.audio {
                            if let Err(e) = audio.play(wave, *volume) {
                                tracing::debug!("play: {e}");
                            }
                        }
                    }
                }
                ac_client::Event::Placed { .. } => {
                    if is_active {
                        self.camera.pitch = -0.15;
                        self.camera.far = 3000.0;
                    }
                }
                ac_client::Event::CharacterCreateFailed(code) => {
                    tracing::warn!(
                        "character creation failed: {}",
                        ac_client::creation::create_failure_message(*code)
                    );
                }
                ac_client::Event::CharacterCreated { name, .. } => {
                    tracing::info!("created {name}");
                }
                ac_client::Event::Characters(list) => {
                    tracing::info!("{} characters; showing the select screen", list.len());
                }
                ac_client::Event::Connected
                | ac_client::Event::Terminated(_)
                | ac_client::Event::Refused(_)
                | ac_client::Event::Effect { .. }
                | ac_client::Event::SpellLearned(_)
                | ac_client::Event::SpellForgotten(_)
                | ac_client::Event::Autoplay { .. } => {}
            }
            if is_active {
                self.lobby.on_event(ev);
            }
        }
        if is_active {
            self.lobby.tick(&net.client);
        }
        let _ = count;
        Some((frame, events))
    }

    /// Make session `i` the one the window shows; the scene follows it.
    /// The session left behind keeps no GPU state: its pickables (which
    /// hold meshes) and animation players go, and the new one is
    /// re-instanced on the next frame.
    fn switch_to(&mut self, i: usize) {
        if i < self.nets.len() && i != self.active {
            tracing::info!("switching to session {}", i + 1);
            if let Some(old) = self.nets.get_mut(self.active) {
                old.pickables = Vec::new();
                old.anims.clear();
            }
            self.nets[i].last_generation = 0;
            self.active = i;
            self.camera.pitch = -0.15;
            if let Some(ui) = &mut self.ui {
                ui.push_chat(
                    format!(
                        "Now showing session {} ({})",
                        i + 1,
                        self.nets[i].client.config.account
                    ),
                    0,
                );
            }
        }
    }

    fn tick_net(&mut self, gpu: &mut gpu::Gpu) {
        if self.nets.is_empty() {
            return;
        }
        let now = Instant::now();
        if let Some(a) = self.pending_switch.take() {
            self.switch_to(a);
        }
        self.apply_ui_commands();
        let mut frame = ac_client::PlayerFrame::default();
        let mut per_session: Vec<Vec<ac_client::Event>> = Vec::new();
        for i in 0..self.nets.len() {
            match self.tick_client(i, now) {
                Some((f, events)) => {
                    if i == self.active {
                        frame = f;
                    }
                    per_session.push(events);
                }
                None => per_session.push(Vec::new()),
            }
        }
        // Plugins see every session, one callback batch per session.
        let dt = self.frame_dt;
        for (i, events) in per_session.iter().enumerate() {
            let clients = clients_of(&mut self.nets);
            let r = self.plugins.frame(clients, i, events, dt, now);
            if i == self.active {
                self.apply_requests(r);
            } else {
                if let Some(a) = r.activate {
                    self.switch_to(a);
                }
                self.defer_sessions(r.start_sessions, r.stop_sessions);
            }
        }
        self.plugins.end_frame();
        // A session that ended without being asked to comes back here,
        // in its own slot, before the starts and stops below can move
        // the slots around.
        self.tick_reconnect(now);
        // Sessions come and go only here, between frames: no session is
        // being ticked and no plugin holds them.
        self.apply_pending_sessions();
        // A follower window draws no world at all.
        if self.render.none {
            return;
        }
        // The options panel's draw distance, and a periodic sweep of the
        // GPU meshes and materials nothing draws any more.
        let dd = self
            .plugins
            .board
            .get(DRAW_DISTANCE_KEY)
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32;
        if dd != self.render.draw_distance {
            self.render.draw_distance = dd;
            gpu.set_draw_distance(dd);
        }
        if self
            .render
            .last_prune
            .is_none_or(|t| t.elapsed() > PRUNE_EVERY)
        {
            self.render.last_prune = Some(now);
            let meshes = scene::prune_gpu_meshes(&mut self.gpu_meshes);
            let materials = gpu.prune_materials();
            if meshes + materials > 0 {
                tracing::debug!("dropped {meshes} unused gpu meshes, {materials} materials");
            }
        }
        let Some(net) = self.nets.get_mut(self.active) else {
            return;
        };
        // Stream landblocks around the character: the block we stand in
        // first, then its neighbours (outdoors only), one per frame.
        if let Some(center) = net.client.player.as_ref().map(|p| p.landblock()) {
            let mut wanted = vec![center];
            if self.dungeon.get(&center) == Some(&false) {
                let cx = ac_scene::lbid::block_x(center);
                let cy = ac_scene::lbid::block_y(center);
                for bx in cx.saturating_sub(1)..=(cx + 1).min(255) {
                    for by in cy.saturating_sub(1)..=(cy + 1).min(255) {
                        let id = ac_scene::lbid::from_xy(bx, by);
                        if id != center {
                            wanted.push(id);
                        }
                    }
                }
            }
            if let Some(&id) = wanted.iter().find(|id| !self.loaded_blocks.contains(id)) {
                let t0 = Instant::now();
                let day_fraction = net.client.day_time().map(|d| d.fraction).unwrap_or(0.5);
                match scene::build_landblock(&net.client.assets, id, &mut self.mesh_cache) {
                    Ok(built) => {
                        self.dungeon.insert(id, built.is_dungeon);
                        let assets = &net.client.assets;
                        let palettes = &self.palettes;
                        gpu.add_block(id, built.batches, |k| {
                            scene::material_image(assets, k, palettes)
                        });
                        self.fx.load_block(assets, id);
                        if id == center {
                            // Daylight sky and fog outdoors; dungeons get a
                            // black sky and no fog.
                            let env = if built.is_dungeon {
                                sky::Environment::dungeon()
                            } else {
                                assets
                                    .region()
                                    .ok()
                                    .and_then(|r| sky::Environment::from_region(&r, day_fraction))
                                    .unwrap_or_default()
                            };
                            gpu.set_environment(env);
                        }
                        tracing::info!(
                            "landblock {id:#010x} loaded in {:.0} ms{}",
                            t0.elapsed().as_secs_f32() * 1000.0,
                            if built.is_dungeon { " (dungeon)" } else { "" }
                        );
                    }
                    Err(e) => {
                        tracing::warn!("landblock {id:#010x}: {e}");
                        self.dungeon.insert(id, false);
                    }
                }
                self.loaded_blocks.insert(id);
            }
            let stale: Vec<u32> = self
                .loaded_blocks
                .iter()
                .copied()
                .filter(|id| !wanted.contains(id))
                .collect();
            let unloaded = !stale.is_empty();
            for id in stale {
                gpu.remove_block(id);
                self.fx.unload_block(id);
                self.loaded_blocks.remove(&id);
                tracing::info!("landblock {id:#010x} unloaded");
            }
            if unloaded {
                // Their textures go with them, unless a block still
                // loaded shares one.
                let n = gpu.prune_materials();
                tracing::debug!("{n} materials dropped with the unloaded blocks");
            }
        }
        {
            if !self.fx.is_empty() {
                self.fx.update(&net.client.assets, self.frame_dt);
            }
            let mut quads = self.fx.quads();
            if self.show_route {
                let now = self.started.elapsed().as_secs_f32();
                quads.extend(route_marks::quads(&mut net.client, now));
            }
            // The jump being charged, if any: where it lands.
            quads.extend(route_marks::jump_quads(&mut net.client));
            if !quads.is_empty() || self.drew_particles {
                self.drew_particles = !quads.is_empty();
                let assets = net.client.assets.clone();
                let palettes = &self.palettes;
                gpu.set_particles(particles::draws(&quads, self.camera.position), |k| {
                    scene::material_image(&assets, k, palettes)
                });
            }
        }
        let changed = net.client.world.generation != net.last_generation;
        let animate = scene::any_animated(&net.anims)
            && net.last_anim_refresh.elapsed() > Duration::from_millis(66);
        if net.client.scene_block.is_some() && (changed || animate) {
            net.last_generation = net.client.world.generation;
            let dt = net.last_anim_refresh.elapsed().as_secs_f32().min(0.2);
            net.last_anim_refresh = Instant::now();
            let (instances, picks) = scene::object_instances(
                &net.client.assets,
                gpu,
                &net.client.world,
                &mut self.gpu_meshes,
                &mut self.palettes,
                &mut net.anims,
                &mut self.tables,
                dt,
            );
            net.pickables = picks;
            gpu.set_dynamic_instances(instances);
        }
        // Third-person camera behind the character, and its model.
        if let Some(pl) = net.client.player.as_mut() {
            let pos = pl.world_position();
            let fwd = pl.forward();
            let (sp, cp) = self.camera.pitch.sin_cos();
            let back = Vec3::new(-fwd.x * cp, -fwd.y * cp, -sp) * 4.0;
            let head = pos + Vec3::new(0.0, 0.0, 1.6);
            self.camera.position = pl.clamp_camera(&net.client.assets, head, head + back);
            self.camera.yaw = pl.heading;
            if frame.dirty {
                let t = glam::Mat4::from_rotation_translation(pl.rotation(), pos);
                let (app, key) = match net.client.world.player() {
                    Some(o) => scene::appearance_of(&net.client.assets, o, &mut self.palettes),
                    None => (ac_scene::model::Appearance::default(), 0),
                };
                let light = net
                    .client
                    .world
                    .player()
                    .and_then(|o| scene::object_light(&net.client.assets, o));
                let instances = scene::instances_lit(
                    &net.client.assets,
                    gpu,
                    &mut self.gpu_meshes,
                    &self.palettes,
                    net.client.player_setup,
                    t,
                    &app,
                    key,
                    frame.pose.as_deref(),
                    light,
                );
                gpu.set_player_instances(instances);
            }
        }
    }

    fn load_scene(&mut self, gpu: &mut gpu::Gpu) -> Result<()> {
        self.render.none = self.cli.render == "none";
        gpu.set_max_texture(self.cli.max_texture);
        if self.cli.connect.is_some() {
            self.start_connect()?;
            // Daylight behind the lobby until the first landblock streams in.
            if let Some(env) = self
                .nets
                .first()
                .and_then(|n| n.client.assets.region().ok())
                .and_then(|r| sky::Environment::from_region(&r, 0.5))
            {
                gpu.set_environment(env);
            }
            return Ok(());
        }
        let assets = ac_scene::Assets::open(self.cli.data_dir()).context("opening DAT archives")?;
        if self.cli.demo_connect {
            self.lobby.open_connect(load_servers());
        }
        if self.cli.demo_select || self.cli.demo_create {
            // The lobby with no server: daylight sky, no world; the creation
            // screen's preview model is instanced by `tick_lobby`.
            let assets = std::rc::Rc::new(assets);
            self.lobby = if self.cli.demo_create {
                ac_plugin::lobby::Lobby::demo_create(assets.clone())
                    .map_err(|e| anyhow::anyhow!("{e:?}"))?
            } else {
                ac_plugin::lobby::Lobby::demo_select()
            };
            if let Some(env) = assets
                .region()
                .ok()
                .and_then(|r| sky::Environment::from_region(&r, 0.5))
            {
                gpu.set_environment(env);
            }
            self.preview_camera();
            return Ok(());
        }
        let mut palettes = scene::Palettes::default();
        let built = if self.cli.model.is_some() || self.cli.chargen.is_some() {
            let model = match &self.cli.model {
                Some(m) => Some(u32::from_str_radix(m.trim_start_matches("0x"), 16)?),
                None => None,
            };
            let (id, app) = match &self.cli.chargen {
                Some(spec) => {
                    let look = parse_look(&assets, spec)?;
                    let desc = ac_scene::chargen::describe(&assets, &look)?;
                    tracing::info!(
                        "chargen {look:?}: setup {:#010x}, {} part swaps, {} texture swaps, {} sub-palettes",
                        desc.setup_id,
                        desc.part_changes.len(),
                        desc.texture_changes.len(),
                        desc.sub_palettes.len()
                    );
                    (model.unwrap_or(desc.setup_id), desc.appearance(&assets))
                }
                None => (model.unwrap(), ac_scene::model::Appearance::default()),
            };
            if let Some(p) = &app.palette {
                palettes.insert(app.palette_hash, p.clone());
            }
            scene::build_model_with(&assets, id, &app)?
        } else if self.cli.emitter.is_some() {
            scene::Built {
                batches: Default::default(),
                center: Vec3::ZERO,
                radius: 1.0,
                is_dungeon: false,
            }
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
        gpu.set_scene(built.batches, |k| {
            scene::material_image(&assets, k, &palettes)
        });
        let (mut center, mut radius) = (built.center, built.radius);
        if let Some(e) = &self.cli.emitter {
            let id = u32::from_str_radix(e.trim_start_matches("0x"), 16)?;
            let mut demo = particles::Demo::new(&assets, id, glam::Mat4::IDENTITY)?;
            demo.simulate(&assets, 3.0);
            let (c, r) = demo.bounds();
            let quads = demo.quads();
            tracing::info!(
                "{id:#010x}: {} emitters, {} particles, centre {c:?} radius {r:.2}",
                demo.system.len(),
                quads.len()
            );
            gpu.set_particles(particles::draws(&quads, c + Vec3::Y * -r), |k| {
                scene::material_image(&assets, k, &palettes)
            });
            if self.cli.model.is_none() && self.cli.chargen.is_none() {
                (center, radius) = (c, r);
            }
        }
        let region = assets.region()?;
        // Particles alone are shown at night so glowing sprites read.
        let day = if self.cli.emitter.is_some() { 0.0 } else { 0.5 };
        if let Some(env) = sky::Environment::from_region(&region, day) {
            gpu.set_environment(env);
        }
        let d = radius.max(if self.cli.emitter.is_some() { 1.0 } else { 5.0 });
        self.camera.position = center + Vec3::new(0.0, -d * 0.8, d * 0.5);
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
            .with_title("acswarm")
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 800));
        let window = Arc::new(event_loop.create_window(attrs).expect("window"));
        let mut gpu = gpu::Gpu::new(window.clone()).expect("gpu");
        if let Err(e) = self.load_scene(&mut gpu) {
            tracing::error!("{e:#}");
            event_loop.exit();
            return;
        }
        self.plugins
            .load_settings(ac_plugin::Settings::default_path());
        let (w, h) = gpu.size();
        let mut ui = ui::Ui::new(gpu.device(), gpu.format(), Some(&window), w, h);
        let icons = icon_loader(self.cli.data_dir().to_path_buf());
        ui.set_icon_loader(icons.clone());
        self.plugins.set_icon_loader(icons);
        self.ui = Some(ui);
        self.gpu = Some(gpu);
        self.window = Some(window);
        self.last_frame = Instant::now();
        // Nothing was told to connect to and no demo is up: open the
        // server/account picker so the app starts there.
        if self.cli.connect.is_none()
            && self.cli.fleet_start.is_empty()
            && !self.cli.demo_select
            && !self.cli.demo_create
            && !self.lobby.visible()
            && self.nets.is_empty()
        {
            self.lobby.open_connect(load_servers());
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.cli.fps == 0 {
            return;
        }
        if Instant::now() >= self.next_frame {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Enter with no text field in use goes to the chat box, whatever
        // else egui has focused (a slider, a checkbox): it would swallow
        // the key otherwise.
        if let WindowEvent::KeyboardInput { event: key, .. } = &event {
            if key.physical_key == PhysicalKey::Code(KeyCode::Enter)
                && key.state == ElementState::Pressed
                && !self.nets.is_empty()
                && self.ui.as_ref().is_some_and(|u| !u.text_field_focused())
            {
                if let Some(ui) = &mut self.ui {
                    ui.chat_focus = true;
                    ui.drop_focus();
                }
                self.keys.clear();
                return;
            }
        }
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
                self.plugins.save_settings();
                if let Some(net) = self.nets.get_mut(self.active) {
                    net.client.disconnect(Instant::now());
                }
                event_loop.exit()
            }
            WindowEvent::RedrawRequested if self.quit_requested => {
                self.plugins.save_settings();
                for net in &mut self.nets {
                    net.client.disconnect(Instant::now());
                }
                event_loop.exit()
            }
            WindowEvent::Resized(size) => {
                if let Some(g) = &mut self.gpu {
                    g.resize(size.width, size.height);
                }
            }
            WindowEvent::Focused(focused) => {
                self.render.unfocused = !focused;
                if focused {
                    self.next_frame = Instant::now();
                }
            }
            WindowEvent::Occluded(occluded) => {
                self.render.occluded = occluded;
                self.render.last_view = None;
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if typing {
                        return;
                    }
                    if self.lobby.visible() {
                        if let Some(key) = egui_key(code) {
                            let active = self.active;
                            let client = self.nets.get_mut(active).map(|n| &mut n.client);
                            if self
                                .lobby
                                .key(key, event.state == ElementState::Pressed, client)
                            {
                                return;
                            }
                        }
                    }
                    if let (Some(key), false) = (egui_key(code), self.nets.is_empty()) {
                        let active = self.active;
                        let clients = clients_of(&mut self.nets);
                        let r = self.plugins.key(
                            clients,
                            active,
                            key,
                            event.state == ElementState::Pressed,
                        );
                        let used = r.consumed;
                        self.apply_requests(r);
                        if used {
                            return;
                        }
                    }
                    if code == KeyCode::Tab
                        && event.state == ElementState::Pressed
                        && !self.nets.is_empty()
                    {
                        let next = (self.active + 1) % self.nets.len();
                        self.switch_to(next);
                        return;
                    }
                    if code == KeyCode::KeyC && event.state == ElementState::Pressed {
                        self.toggle_combat();
                        return;
                    }
                    // Y flies: no walls, no floors, no gravity, Space up
                    // and Control down; Y again drops the character onto
                    // whatever is below. (F is the fellowship panel, and
                    // every other letter is taken too.)
                    if code == KeyCode::KeyY && event.state == ElementState::Pressed {
                        if let Some(net) = self.nets.get_mut(self.active) {
                            let on = !net.client.noclip();
                            net.client.set_noclip(on);
                            net.client.events.push(ac_client::Event::Chat {
                                text: if on {
                                    "Flying: walls and gravity are off. Space climbs, Control descends, Y lands.".into()
                                } else {
                                    "Landing.".into()
                                },
                                kind: 1,
                            });
                        }
                        return;
                    }
                    // T shows or hides the line of marks laid along the
                    // route the character is walking.
                    if code == KeyCode::KeyT && event.state == ElementState::Pressed {
                        self.show_route = !self.show_route;
                        if let Some(net) = self.nets.get_mut(self.active) {
                            net.client.events.push(ac_client::Event::Chat {
                                text: if self.show_route {
                                    "Route shown on the ground.".into()
                                } else {
                                    "Route hidden.".into()
                                },
                                kind: 1,
                            });
                        }
                        return;
                    }
                    // Retail's separate actions on the selected object: R
                    // uses it where it is (read a sign or a book on the
                    // ground, open a chest), G picks it up.
                    if (code == KeyCode::KeyR || code == KeyCode::KeyG)
                        && event.state == ElementState::Pressed
                    {
                        if let Some(net) = self.nets.get_mut(self.active) {
                            if let Some(g) = net.client.selected {
                                if code == KeyCode::KeyR {
                                    net.client.use_object(g);
                                } else {
                                    net.client.pick_up(g);
                                }
                            }
                        }
                        return;
                    }
                    if code == KeyCode::Enter
                        && event.state == ElementState::Pressed
                        && !self.nets.is_empty()
                    {
                        if let Some(ui) = &mut self.ui {
                            ui.chat_focus = true;
                        }
                        self.keys.clear();
                        return;
                    }
                    // Escape belongs to the menu plugin (close the newest
                    // window, else open the menu, which can quit).
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
            WindowEvent::MouseWheel { delta, .. } => {
                // With the jump key held, the wheel sets the charge, so
                // the landing spot shown on the ground can be placed by
                // hand; a notch is a twentieth of full power.
                if self.keys.contains(&KeyCode::Space) {
                    let notches = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => (p.y / 40.0) as f32,
                    };
                    if let Some(net) = self.nets.get_mut(self.active) {
                        net.client.adjust_jump_charge(notches * 0.05);
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y));
                if self.looking {
                    if let Some((lx, ly)) = self.last_cursor {
                        let dx = (position.x - lx) as f32;
                        let dy = (position.y - ly) as f32;
                        if let Some(p) = self.preview.as_mut() {
                            p.yaw += dx * 0.01;
                            self.last_cursor = Some((position.x, position.y));
                            return;
                        }
                        match self
                            .nets
                            .get_mut(self.active)
                            .and_then(|n| n.client.player.as_mut())
                        {
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
                if self.nets.is_empty() && !self.lobby.visible() {
                    self.update(dt);
                }
                if let Some(mut g) = self.gpu.take() {
                    self.tick_net(&mut g);
                    self.tick_lobby(&mut g, dt);
                    self.tick_sky(&mut g);
                    self.gpu = Some(g);
                }
                self.fps = if self.fps == 0.0 {
                    1.0 / dt.max(1e-3)
                } else {
                    self.fps * 0.95 + 0.05 / dt.max(1e-3)
                };
                self.refresh_status();
                self.plugins.autosave();
                if let Some(mut g) = self.gpu.take() {
                    let vp = self.camera.view_proj(g.aspect());
                    let (w, h) = g.size();
                    let t_overlay = Instant::now();
                    self.run_overlay(g.device(), g.queue(), w, h);
                    let overlay_ms = t_overlay.elapsed().as_secs_f32() * 1e3;
                    // Draw only when something can look different: the
                    // world or camera changed, the overlay wants a
                    // repaint, or the window is new. A hidden window
                    // draws nothing at all.
                    let overlay_changed = self.ui.as_ref().is_some_and(|u| u.repaint_wanted());
                    let view = (vp, (w, h));
                    let unchanged =
                        self.render.last_view == Some(view) && !g.is_dirty() && !overlay_changed;
                    if self.render.occluded || unchanged {
                        self.render.perf.idle_frames += 1;
                    } else {
                        let cpu_ms = now.elapsed().as_secs_f32() * 1e3;
                        let mut ui = self.ui.as_mut();
                        let mut paint =
                            |d: &wgpu::Device,
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
                        self.render.last_view = Some(view);
                        let ms = now.elapsed().as_secs_f32() * 1e3;
                        self.render.perf.frame(ms, cpu_ms, overlay_ms, g.stats());
                    }
                    self.gpu = Some(g);
                }
                // Pace frames: wake up again when the next one is due. A
                // window without focus ticks slowly; the sessions keep up
                // (the client catches up on the network each tick).
                let fps = if self.render.unfocused || self.render.occluded {
                    if self.cli.fps == 0 {
                        UNFOCUSED_FPS
                    } else {
                        self.cli.fps.min(UNFOCUSED_FPS)
                    }
                } else {
                    self.cli.fps
                };
                if fps > 0 {
                    self.next_frame = now + Duration::from_secs_f32(1.0 / fps as f32);
                    event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_frame));
                } else if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

/// Parse `--chargen race,gender,hair,eyes,nose,mouth,skin[,hair_color,eye_color]`.
fn parse_look(assets: &ac_scene::Assets, spec: &str) -> Result<ac_scene::chargen::Look> {
    let f: Vec<&str> = spec.split(',').map(str::trim).collect();
    anyhow::ensure!(
        (7..=9).contains(&f.len()),
        "--chargen wants race,gender,hair,eyes,nose,mouth,skin[,hair_color,eye_color]"
    );
    let cg = assets.chargen()?;
    let heritage = ac_scene::chargen::heritage_id(&cg, f[0])
        .with_context(|| format!("unknown race {:?}", f[0]))?;
    let gender = match f[1].to_ascii_lowercase().as_str() {
        "m" | "male" | "1" => 1,
        "f" | "female" | "2" => 2,
        g => anyhow::bail!("gender {g:?}: want m or f"),
    };
    let idx = |s: &str, what: &str| -> Result<usize> {
        s.parse()
            .with_context(|| format!("{what} {s:?}: want an index"))
    };
    let skin: f32 = f[6]
        .parse()
        .with_context(|| format!("skin {:?}: want 0..1", f[6]))?;
    Ok(ac_scene::chargen::Look {
        heritage,
        gender,
        hair_style: idx(f[2], "hair")?,
        eyes: idx(f[3], "eyes")?,
        nose: idx(f[4], "nose")?,
        mouth: idx(f[5], "mouth")?,
        skin_shade: skin,
        hair_color: f
            .get(7)
            .map(|s| idx(s, "hair_color"))
            .transpose()?
            .unwrap_or(0),
        eye_color: f
            .get(8)
            .map(|s| idx(s, "eye_color"))
            .transpose()?
            .unwrap_or(0),
        ..Default::default()
    })
}

fn main() -> Result<()> {
    // Reloadable, so the log can be turned up on one subsystem while
    // the app runs (see the Logging section of the options panel).
    // What was chosen last time is in the settings, which live beside
    // the data folder and can be read before any of it is open.
    let saved: Option<String> = ac_plugin::Settings::load(&ac_plugin::Settings::default_path())
        .get(ac_plugin::panels::options::LOG_FILTER_KEY);
    logging::init(saved.as_deref());
    // Finder (and some launchers) pass a `-psn_...` process-serial arg;
    // drop it so the parser does not choke when the app is double-clicked.
    let args = std::env::args_os().filter(|a| !a.to_string_lossy().starts_with("-psn"));
    let mut cli = Cli::parse_from(args);
    // No window yet: settle the data folder (remembered, then the usual
    // places, then a picker) so a double-click opens instead of dying on a
    // missing --data-dir. Headless `--screenshot` runs never prompt.
    let interactive = cli.screenshot.is_none();
    cli.data_dir = Some(resolve_data_dir(cli.data_dir.take(), interactive)?);
    if cli.headless {
        return headless::run(cli);
    }
    if let Some(path) = cli.screenshot.clone() {
        let mut gpu = gpu::Gpu::headless(1280, 800)?;
        let mut app = App {
            cli,
            window: None,
            gpu: None,
            nets: Vec::new(),
            active: 0,
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
            jump_requested: false,
            last_cursor: None,
            last_frame: Instant::now(),
            ui: None,
            fps: 0.0,
            plugins: plugins::builtin(),
            pending_switch: None,
            next_frame: Instant::now(),
            loaded_blocks: Default::default(),
            sky_at: None,
            dungeon: Default::default(),
            mesh_cache: Default::default(),
            gpu_meshes: Default::default(),
            palettes: Default::default(),
            tables: Default::default(),
            fx: Default::default(),
            show_route: true,
            drew_particles: false,
            quit_requested: false,
            started: Instant::now(),
            audio: None,
            lobby: Default::default(),
            preview: None,
            assets: None,
            pending_sessions: Vec::new(),
            render: Default::default(),
            current_host: None,
        };
        if let Some(bus) = app.cli.bus.clone() {
            plugins::join_bus(&mut app.plugins, &bus, app.cli.account.as_deref())?;
        }
        app.load_scene(&mut gpu)?;
        let perf_started = Instant::now();
        let (w, h) = gpu.size();
        let mut ui = ui::Ui::new(gpu.device(), gpu.format(), None, w, h);
        let icons = icon_loader(app.cli.data_dir().to_path_buf());
        ui.set_icon_loader(icons.clone());
        app.ui = Some(ui);
        if app.cli.demo_ui {
            // Only the panels, on canned data: nothing else has a session.
            app.plugins = plugins::demo(app.cli.data_dir());
        }
        app.plugins.set_icon_loader(icons);
        app.plugins
            .load_settings(ac_plugin::Settings::default_path());
        if app.cli.connect.is_some() {
            // `--fleet-start`: hand the specs to the fleet panel the way a
            // script would, on the blackboard; it puts them on its roster
            // and asks the host for the sessions once it ticks.
            let fleet: Vec<ac_plugin::SessionSpec> = app
                .cli
                .fleet_start
                .iter()
                .map(|s| parse_fleet_start(s))
                .collect::<Result<_>>()?;
            let fleet_accounts: Vec<String> = fleet.iter().map(|s| s.account.clone()).collect();
            if !fleet.is_empty() {
                app.plugins.board.set_local(
                    plugins::panels::fleet::START_KEY,
                    ac_plugin::serde_json::to_value(&fleet)?,
                );
            }
            let mut fleet_stopped_at: Option<Instant> = None;
            // Pump the connection until the player is placed and the world
            // has settled, then render from the character's viewpoint.
            let deadline = Instant::now()
                + Duration::from_secs(40)
                + Duration::from_secs_f32(app.cli.walk + 3.0 * app.cli.click.len() as f32 + 10.0)
                + if app.cli.attack.is_some() {
                    Duration::from_secs(120)
                } else {
                    Duration::ZERO
                }
                + if fleet_accounts.is_empty() {
                    Duration::ZERO
                } else {
                    Duration::from_secs_f32(app.cli.fleet_stop_after + 20.0)
                };
            let mut settled_at: Option<Instant> = None;
            let mut last_tick = Instant::now();
            let mut ticks = 0u32;
            let mut ticks_since = Instant::now();
            // Each --click entry is a double click: (entry index, clicks sent).
            let mut click_state = (0usize, 0u32);
            let use_requested = app.cli.use_name.is_some() || app.cli.attack.is_some();
            let mut said = 0usize;
            let mut bought_at: Option<Instant> = None;
            let mut retry_at = Instant::now() - Duration::from_secs(5);
            let say_delay = 3.0 * app.cli.say.len() as f32 + 1.0;
            let mut attack_started: Option<Instant> = None;
            let mut loot_state = 0u8;
            let mut loot_at = Instant::now();
            let mut listed = false;
            let mut last_flush = Instant::now();
            let mut next_perf_frame = Instant::now();
            loop {
                let frame_start = Instant::now();
                app.tick_net(&mut gpu);
                // With --perf, draw offscreen at the frame cap: the frame
                // then costs what a window's would, and is measured.
                if app.cli.perf && frame_start >= next_perf_frame {
                    let fps = app.cli.fps.max(1);
                    next_perf_frame = frame_start + Duration::from_secs_f32(1.0 / fps as f32);
                    app.refresh_status();
                    let (w, h) = gpu.size();
                    let t_overlay = Instant::now();
                    app.run_overlay(gpu.device(), gpu.queue(), w, h);
                    let overlay_ms = t_overlay.elapsed().as_secs_f32() * 1e3;
                    let vp = app.camera.view_proj(gpu.aspect());
                    let cpu_ms = frame_start.elapsed().as_secs_f32() * 1e3;
                    let mut ui = app.ui.take();
                    let mut paint = |d: &wgpu::Device,
                                     q: &wgpu::Queue,
                                     e: &mut wgpu::CommandEncoder,
                                     v: &wgpu::TextureView| {
                        if let Some(ui) = ui.as_mut() {
                            ui.paint(d, q, e, v);
                        }
                    };
                    match gpu.render_offscreen(vp, Vec3::new(0.4, 0.3, 1.0), Some(&mut paint)) {
                        Ok(_) => {
                            let ms = frame_start.elapsed().as_secs_f32() * 1e3;
                            app.render.perf.frame(ms, cpu_ms, overlay_ms, gpu.stats());
                        }
                        Err(e) => tracing::warn!("perf frame: {e:#}"),
                    }
                    app.ui = ui;
                    last_flush = Instant::now();
                }
                // No frames are presented headlessly, so flush uploads and
                // recycle dropped buffers here (a poll alone frees nothing:
                // uploads wait for a submit) or they pile up until the
                // final screenshot.
                if last_flush.elapsed() >= Duration::from_millis(100) {
                    gpu.flush();
                    last_flush = Instant::now();
                }
                let placed = app
                    .nets
                    .get(app.active)
                    .map(|n| n.client.scene_block.is_some())
                    .unwrap_or(false);
                if placed {
                    let started = *settled_at.get_or_insert_with(Instant::now);
                    let t = started.elapsed().as_secs_f32();
                    // Hold W for `walk` seconds after a short settle, then settle again.
                    let walking = t > 1.0 && t < 1.0 + app.cli.walk;
                    if t > 1.0 + 3.0 * said as f32 && said < app.cli.say.len() {
                        if let Some(ui) = app.ui.as_mut() {
                            ui.outgoing.push(app.cli.say[said].clone());
                        }
                        said += 1;
                    }
                    if let Some(at) = app.cli.snap_at {
                        if t > at {
                            app.cli.snap_at = None;
                            app.refresh_status();
                            let (w, h) = gpu.size();
                            // egui's first frame only loads fonts and asks for a repaint.
                            app.run_overlay(gpu.device(), gpu.queue(), w, h);
                            app.run_overlay(gpu.device(), gpu.queue(), w, h);
                            let vp = app.camera.view_proj(gpu.aspect());
                            let mid = path.with_extension("mid.png");
                            let mut ui = app.ui.take();
                            let mut paint =
                                |d: &wgpu::Device,
                                 q: &wgpu::Queue,
                                 e: &mut wgpu::CommandEncoder,
                                 v: &wgpu::TextureView| {
                                    if let Some(ui) = ui.as_mut() {
                                        ui.paint(d, q, e, v);
                                    }
                                };
                            if let Err(e) = gpu.render_to_png(
                                vp,
                                Vec3::new(0.4, 0.3, 1.0),
                                &mid,
                                Some(&mut paint),
                            ) {
                                tracing::warn!("mid screenshot: {e:#}");
                            } else {
                                tracing::info!("wrote {}", mid.display());
                            }
                            app.ui = ui;
                        }
                    }
                    if let Some(want) = app.cli.cast.clone() {
                        if t > 4.0 + app.cli.walk + say_delay {
                            let id = app.nets.get(app.active).and_then(|n| {
                                // Spellbook first (ids from PlayerDescription), then
                                // scrolls learnt this session or still in the pack.
                                let table = n.client.assets.spell_table().ok();
                                n.client
                                    .world
                                    .stats
                                    .spells
                                    .iter()
                                    .copied()
                                    .find(|id| {
                                        table
                                            .as_ref()
                                            .and_then(|t| t.get(*id))
                                            .is_some_and(|sp| sp.name.starts_with(&want))
                                    })
                                    .or_else(|| {
                                        n.client
                                            .known_spells
                                            .iter()
                                            .find(|(_, name)| name.starts_with(&want))
                                            .map(|(id, _)| *id)
                                    })
                                    .or_else(|| {
                                        n.client
                                            .world
                                            .inventory()
                                            .find(|o| {
                                                o.spell_id != 0
                                                    && o.name
                                                        .starts_with(&format!("Scroll of {want}"))
                                            })
                                            .map(|o| o.spell_id)
                                    })
                            });
                            match id {
                                Some(id) => app.cast(id),
                                None => tracing::warn!("no known spell named {want:?}"),
                            }
                            app.cli.cast = None;
                            bought_at = Some(Instant::now());
                        }
                    }
                    if let Some(want) = app.cli.sell.clone() {
                        if let Some(net) = app.nets.get_mut(app.active) {
                            if net.client.world.open_vendor.is_some() {
                                let found = net
                                    .client
                                    .world
                                    .inventory()
                                    .find(|o| o.name.starts_with(&want))
                                    .map(|o| (o.guid, o.name.clone()));
                                if let Some((guid, name)) = found {
                                    tracing::info!("selling {name}");
                                    net.client.sell(guid);
                                }
                                app.cli.sell = None;
                                bought_at = Some(Instant::now());
                            }
                        }
                    }
                    if let Some(want) = app.cli.buy.clone() {
                        if let Some(net) = app.nets.get_mut(app.active) {
                            if let Some(v) = &net.client.world.open_vendor {
                                tracing::info!(
                                    "vendor stock: {}",
                                    v.items
                                        .iter()
                                        .map(|i| format!(
                                            "{} ({}p)",
                                            i.desc.name,
                                            (i.desc.value as f32 * v.sell_rate).ceil()
                                        ))
                                        .collect::<Vec<_>>()
                                        .join(" | ")
                                );
                                let found = v
                                    .items
                                    .iter()
                                    .find(|i| i.desc.name.starts_with(&want))
                                    .map(|i| (i.guid, i.desc.name.clone()));
                                if let Some((guid, name)) = found {
                                    tracing::info!("buying {name}");
                                    net.client.buy(guid);
                                }
                                app.cli.buy = None;
                                bought_at = Some(Instant::now());
                            }
                        }
                    }
                    if app.cli.jump && t > 1.5 {
                        app.cli.jump = false;
                        app.jump_requested = true;
                    }
                    if !fleet_accounts.is_empty()
                        && fleet_stopped_at.is_none()
                        && t > app.cli.fleet_stop_after
                    {
                        tracing::info!("stopping the --fleet-start sessions");
                        app.plugins.board.set_local(
                            plugins::panels::fleet::STOP_KEY,
                            ac_plugin::serde_json::json!(fleet_accounts),
                        );
                        fleet_stopped_at = Some(Instant::now());
                    }
                    if t > 1.0 + app.cli.walk + say_delay
                        && retry_at.elapsed() > Duration::from_secs(1)
                    {
                        retry_at = Instant::now();
                        if let Some(name) = app.cli.use_name.clone() {
                            if app.use_by_name(&name) || t > 60.0 + say_delay {
                                app.cli.use_name = None;
                            }
                        }
                        if let Some(name) = app.cli.attack.clone() {
                            if !app.nets.get(app.active).is_some_and(|n| n.client.combat) {
                                app.toggle_combat();
                            }
                            if app.use_by_name(&name) || t > 60.0 + say_delay {
                                attack_started = Some(Instant::now());
                                app.cli.attack = None;
                            }
                        }
                    }
                    // Attack phase: wait for the target to die, then loot.
                    let fighting = attack_started.is_some_and(|s| {
                        app.nets
                            .get(app.active)
                            .is_some_and(|n| n.client.attack_target.is_some())
                            && s.elapsed() < Duration::from_secs(90)
                    });
                    let loot_only = app.cli.attack.is_none()
                        && attack_started.is_none()
                        && app.cli.loot.as_deref().is_some_and(|n| !n.is_empty());
                    if (attack_started.is_some() && !fighting || loot_only && t > 3.0 + say_delay)
                        && loot_state == 0
                    {
                        loot_state = 1;
                        loot_at = Instant::now();
                    }
                    if loot_state == 1 && loot_at.elapsed() > Duration::from_secs(2) {
                        if let Some(name) = app.cli.loot.clone() {
                            if app.nets.get(app.active).is_some_and(|n| n.client.combat) {
                                app.toggle_combat();
                            }
                            let corpse = if name.is_empty() {
                                format!(
                                    "Corpse of {}",
                                    app.nets
                                        .get(app.active)
                                        .map(|n| n.client.last_target_name.clone())
                                        .unwrap_or_default()
                                )
                            } else {
                                name
                            };
                            if app.use_by_name(&corpse) {
                                loot_state = 2;
                                loot_at = Instant::now();
                            } else if loot_at.elapsed() > Duration::from_secs(20) {
                                loot_state = 3;
                                loot_at = Instant::now();
                            }
                        } else {
                            // Nothing to loot: settle and finish.
                            loot_state = 3;
                            loot_at = Instant::now();
                        }
                    }
                    if loot_state == 2 && app.cli.loot.is_some() {
                        if let Some(net) = app.nets.get_mut(app.active) {
                            let items: Vec<u32> = net
                                .client
                                .world
                                .open_container
                                .as_ref()
                                .map(|(_, items)| items.clone())
                                .unwrap_or_default();
                            if !items.is_empty() && loot_at.elapsed() > Duration::from_secs(1) {
                                for g in items {
                                    net.client.take(g);
                                }
                                loot_state = 3;
                                loot_at = Instant::now();
                            }
                        }
                        if loot_at.elapsed() > Duration::from_secs(8) {
                            loot_state = 3;
                            loot_at = Instant::now();
                        }
                    }
                    if !listed && t > 1.5 + say_delay {
                        listed = true;
                        let mut names: Vec<String> = app
                            .nets
                            .get(app.active)
                            .map(|n| {
                                n.client
                                    .world
                                    .drawable()
                                    .map(|o| format!("{} {:#x}", o.name, o.object_desc_flags))
                                    .collect()
                            })
                            .unwrap_or_default();
                        names.sort();
                        tracing::debug!("objects in view: {}", names.join(" | "));
                        if let Some(n) = app.nets.get(app.active) {
                            let st = &n.client.world.stats;
                            tracing::info!(
                                "sheet: level {} xp {} avail {} credits {}; {} skills, {} spells, {} inventory guids, {} wielded guids",
                                st.level,
                                st.total_xp,
                                st.available_xp,
                                st.skill_credits,
                                st.skills.len(),
                                st.spells.len(),
                                st.inventory.len(),
                                st.wielded.len()
                            );
                        }
                        // The panels are plugins: press their keys.
                        let mut keys: Vec<egui::Key> = Vec::new();
                        if app.cli.show_skills {
                            keys.extend([egui::Key::K, egui::Key::P]);
                        }
                        keys.extend(
                            app.cli
                                .press
                                .iter()
                                .filter_map(|k| egui::Key::from_name(k.trim())),
                        );
                        for key in keys {
                            let active = app.active;
                            let clients = clients_of(&mut app.nets);
                            let r = app.plugins.key(clients, active, key, true);
                            app.apply_requests(r);
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
                        tracing::info!(
                            "{ticks} ticks/s; {} gpu meshes, {} materials ({} MB textures), {} instances, {} buffers ({} MB) created",
                            app.nets
                                .get(app.active)
                                .map(|_| app.gpu_meshes.len())
                                .unwrap_or(0),
                            gpu.material_count(),
                            gpu.texture_bytes() >> 20,
                            gpu.instance_count(),
                            gpu.buffer_stats().0,
                            gpu.buffer_stats().1 >> 20
                        );
                        ticks = 0;
                        ticks_since = Instant::now();
                    }
                    // Capture while still walking so the walk pose is visible,
                    // or after a short settle when not walking.
                    let pending = app.cli.attack.is_some()
                        || app.cli.use_name.is_some()
                        || said < app.cli.say.len()
                        || (!app.cli.say.is_empty() && t < say_delay + 2.0);
                    let looting = app.nets.get(app.active).is_some_and(|n| {
                        !n.client.loot_queue.is_empty() || n.client.loot_inflight.is_some()
                    });
                    let done = if !fleet_accounts.is_empty() {
                        fleet_stopped_at.is_some_and(|s| s.elapsed() > Duration::from_secs(3))
                    } else if pending
                        || looting
                        || (app.cli.buy.is_some()
                            || app.cli.sell.is_some()
                            || app.cli.cast.is_some())
                            && t < 40.0 + say_delay
                    {
                        false
                    } else if let Some(b) = bought_at {
                        b.elapsed() > Duration::from_secs(4)
                    } else if attack_started.is_some() || loot_only {
                        loot_state == 3 && loot_at.elapsed() > Duration::from_secs(3)
                            || (loot_state == 1
                                && app.cli.loot.is_none()
                                && loot_at.elapsed() > Duration::from_secs(3))
                    } else if app.cli.walk > 0.0 {
                        t > 0.9 + app.cli.walk
                    } else if !app.cli.click.is_empty() || use_requested {
                        t > 8.0 + say_delay + app.cli.click.len() as f32 * 3.0
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
        if app.lobby.visible() {
            // `--press` drives the lobby screens (ArrowRight steps the
            // creation screen); then the preview model is instanced.
            for key in app
                .cli
                .press
                .iter()
                .filter_map(|k| egui::Key::from_name(k.trim()))
            {
                app.lobby.key(key, true, None);
            }
            app.tick_lobby(&mut gpu, 0.0);
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
        if app.cli.demo_ui {
            if let Some(ui) = app.ui.as_mut() {
                ui.status_icon = ac_plugin::IconLayers::single(0x0600_2F40);
                ui.status += "  selected: Demo item";
                // A line of each kind, so the tabs and colours show.
                for (text, kind) in [
                    ("Reborn says, \"heading for the hall\"", 2u32),
                    ("+Admin tells you, \"take the left stair\"", 3),
                    ("Reborn waves.", 0xC),
                    (
                        "You hit Drudge Skulker for 12 points of slashing damage.",
                        5,
                    ),
                    (
                        "Drudge Skulker hits you for 4 points of bludgeoning damage.",
                        6,
                    ),
                    ("You cast Heal Self I.", 7),
                    ("[General] Seller: wts stormwood bow, cheap", 8),
                    ("[Fellowship] Bob Smith says, \"pulling three\"", 0x13),
                    ("You have earned 812,000 experience.", 0xD),
                    ("Welcome to Asheron's Call", 0),
                ] {
                    ui.push_chat(text.to_string(), kind);
                }
            }
        }
        // egui's first frame only loads fonts and asks for a repaint.
        app.run_overlay(gpu.device(), gpu.queue(), w, h);
        app.run_overlay(gpu.device(), gpu.queue(), w, h);
        let mut ui = app.ui.take().unwrap();
        let mut paint = |d: &wgpu::Device,
                         q: &wgpu::Queue,
                         e: &mut wgpu::CommandEncoder,
                         v: &wgpu::TextureView| ui.paint(d, q, e, v);
        if app.cli.perf && app.cli.connect.is_none() {
            // Offline there is no loop to measure in: draw the still
            // scene a hundred times from the screenshot's viewpoint.
            for _ in 0..100 {
                let ms = gpu.render_offscreen(vp, Vec3::new(0.4, 0.3, 1.0), Some(&mut paint))?;
                let stats = gpu.stats();
                app.render.perf.frame(ms, stats.encode_ms, 0.0, stats);
            }
        }
        gpu.render_to_png(vp, Vec3::new(0.4, 0.3, 1.0), &path, Some(&mut paint))?;
        tracing::info!("wrote {}", path.display());
        if app.cli.perf {
            let report = app
                .render
                .perf
                .summary(&gpu, app.gpu_meshes.len(), perf_started);
            tracing::info!("{report}");
            println!("{report}");
        }
        app.plugins.save_settings();
        if let Some(net) = app.nets.get_mut(app.active) {
            net.client.disconnect(Instant::now());
        }
        return Ok(());
    }
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        cli,
        window: None,
        gpu: None,
        nets: Vec::new(),
        active: 0,
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
        jump_requested: false,
        last_cursor: None,
        last_frame: Instant::now(),
        ui: None,
        fps: 0.0,
        plugins: plugins::builtin(),
        pending_switch: None,
        next_frame: Instant::now(),
        loaded_blocks: Default::default(),
        sky_at: None,
        dungeon: Default::default(),
        mesh_cache: Default::default(),
        gpu_meshes: Default::default(),
        palettes: Default::default(),
        tables: Default::default(),
        fx: Default::default(),
        show_route: true,
        drew_particles: false,
        quit_requested: false,
        started: Instant::now(),
        audio: None,
        lobby: Default::default(),
        preview: None,
        assets: None,
        pending_sessions: Vec::new(),
        render: Default::default(),
        current_host: None,
    };
    if let Some(bus) = app.cli.bus.clone() {
        plugins::join_bus(&mut app.plugins, &bus, app.cli.account.as_deref())?;
    }
    let perf_started = Instant::now();
    event_loop.run_app(&mut app)?;
    if app.cli.perf {
        if let Some(gpu) = &app.gpu {
            let report = app
                .render
                .perf
                .summary(gpu, app.gpu_meshes.len(), perf_started);
            tracing::info!("{report}");
            println!("{report}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fleet_start_specs_parse() {
        let s = parse_fleet_start("fleetbot1:testpass:Fleetbot One:bow:holtburg").unwrap();
        assert_eq!(
            (s.account.as_str(), s.password.as_str()),
            ("fleetbot1", "testpass")
        );
        assert_eq!(s.character, None, "created, so named once");
        let c = s.create.unwrap();
        assert_eq!(c.name, "Fleetbot One");
        assert_eq!(c.template.as_deref(), Some("bow"));
        assert_eq!(c.town.as_deref(), Some("holtburg"));
        assert_eq!(c.heritage, None);
        assert_eq!(s.role, ac_plugin::Role::Follower);
        // Without a template: enter with the character, create nothing.
        let s = parse_fleet_start("bob:pw:Bob").unwrap();
        assert_eq!(s.character.as_deref(), Some("Bob"));
        assert!(s.create.is_none());
        // Blank middle fields keep the defaults.
        let s = parse_fleet_start("bob:pw:Bob::yaraq:sho:f").unwrap();
        let c = s.create.unwrap();
        assert_eq!(c.template, None);
        assert_eq!(c.town.as_deref(), Some("yaraq"));
        assert_eq!(c.heritage.as_deref(), Some("sho"));
        assert_eq!(c.sex.as_deref(), Some("f"));
        assert!(parse_fleet_start("bob:pw").is_err());
        assert!(parse_fleet_start("bob::Bob").is_err());
    }
}
