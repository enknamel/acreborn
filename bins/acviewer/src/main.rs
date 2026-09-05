//! acviewer: fly around a landblock or inspect a model.
//!
//!   acviewer --landblock A9B4 [--radius 1]
//!   acviewer --model 02000001
//!   acviewer --chargen aluvian,m,3,0,0,0,0.5     # a dressed-up human head
//!
//! Controls: right mouse drag to look, WASD to move, Q/E down/up,
//! Shift to go faster, Escape to quit.

mod camera;
mod gpu;
mod player;
mod scene;
mod sky;
mod ui;
mod water;

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
    /// Dress the model as a new character from the CharGen table:
    /// race,gender,hair,eyes,nose,mouth,skin[,hair_color,eye_color].
    /// race = heritage name or id, gender = m/f, styles/colors are option
    /// indices, skin is a 0..1 shade. Shows the race's Setup unless --model is given.
    #[arg(long)]
    chargen: Option<String>,
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
    /// Connected headless mode: open the skills panel in the screenshot.
    #[arg(long)]
    show_skills: bool,
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
    /// Offline --screenshot: fill the inventory and loot panels with sample
    /// items so the icon overlay can be checked without a server.
    #[arg(long, hide = true)]
    demo_ui: bool,
}

/// An icon loader for the egui overlay: decodes RenderSurfaces (0x06) from
/// the portal on demand. The archives are opened on the first icon so a
/// viewer that never draws one pays nothing.
fn icon_loader(data_dir: PathBuf) -> ui::IconLoader {
    let assets: std::cell::OnceCell<Option<ac_scene::Assets>> = std::cell::OnceCell::new();
    Box::new(move |id| {
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

/// Spellbook rows for the given spell ids, sorted by level then name.
/// Ids missing from the table are skipped.
fn spell_rows(
    table: &ac_formats::spell_table::SpellTable,
    comps: &ac_formats::spell_components::SpellComponentTable,
    ids: impl IntoIterator<Item = u32>,
) -> Vec<ui::SpellRow> {
    let mut rows: Vec<ui::SpellRow> = ids
        .into_iter()
        .filter_map(|id| {
            let s = table.get(id)?;
            Some(ui::SpellRow {
                id,
                name: s.name.clone(),
                level: s.level(),
                school: ac_formats::spell_table::school::short_name(s.school),
                mana: s.base_mana,
                self_targeted: s.is_self_targeted(),
                icon: s.icon_id,
                description: s.description.clone(),
                words: comps.spell_words(s.formula()),
            })
        })
        .collect();
    rows.sort_by(|a, b| a.level.cmp(&b.level).then_with(|| a.name.cmp(&b.name)));
    rows
}

/// Sample panels for `--demo-ui`: known 32x32 icons from the portal, and
/// a spellbook of real spells from the spell table.
fn demo_ui(ui: &mut ui::Ui, data_dir: &std::path::Path) {
    let item = |guid: u32, name: &str, stack: u32, wielded: bool, icon: u32| ui::Item {
        guid,
        name: name.to_string(),
        stack,
        wielded,
        icon,
        icon_overlay: 0,
        icon_underlay: 0,
    };
    ui.sheet = Some(ui::Sheet {
        name: "Demo".into(),
        level: 1,
        vitals: Vec::new(),
        total_xp: 0,
        available_xp: 0,
        skill_credits: 0,
        skills: Vec::new(),
    });
    ui.items = vec![
        item(1, "Demo item 0x06000FAA", 1, true, 0x0600_0FAA),
        item(2, "Demo item 0x0600189E", 1, true, 0x0600_189E),
        item(3, "Demo item 0x06001A8A", 1, false, 0x0600_1A8A),
        item(4, "Demo item 0x06001FB7", 12, false, 0x0600_1FB7),
        item(5, "Demo item 0x0600261A", 1, false, 0x0600_261A),
        item(6, "Demo item 0x06002C0D", 1, false, 0x0600_2C0D),
        item(7, "Demo item 0x06002F40", 3, false, 0x0600_2F40),
        item(8, "Demo item 0x0600321E", 1, false, 0x0600_321E),
    ];
    ui.loot = Some((
        "Demo corpse".into(),
        vec![
            item(9, "Loot 0x06002C0D", 1, false, 0x0600_2C0D),
            item(10, "Loot 0x0600601C", 5, false, 0x0600_601C),
            item(11, "Loot 0x06006A21", 1, false, 0x0600_6A21),
            ui::Item {
                icon_overlay: 0x0600_6A21,
                ..item(12, "0x06001A8A + 0x06006A21", 1, false, 0x0600_1A8A)
            },
        ],
    ));
    ui.status_icon = ui::IconLayers {
        underlay: 0,
        icon: 0x0600_2F40,
        overlay: 0,
    };
    ui.status += "  selected: Demo item";
    match ac_scene::Assets::open(data_dir)
        .and_then(|a| Ok((a.spell_table()?, a.spell_components()?)))
    {
        Ok((table, comps)) => {
            // Strength Other/Self I, Heal Other/Self I, Infuse Mana Other I,
            // Invulnerability Other I, Fire Protection Self I, Armor Self I,
            // Acid Stream III, Shock Wave II, Mind Blossom.
            ui.spells = spell_rows(&table, &comps, [1, 2, 5, 6, 9, 17, 20, 24, 60, 65, 2091]);
        }
        Err(e) => tracing::warn!("demo spellbook: {e}"),
    }
    ui.show_spells = true;
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
    /// Landblocks currently uploaded to the GPU.
    loaded_blocks: std::collections::HashSet<u32>,
    /// Block id -> is a dungeon (learnt when the block is built).
    dungeon: std::collections::HashMap<u32, bool>,
    mesh_cache: std::collections::HashMap<u32, ac_scene::model::Mesh>,
    last_generation: u64,
    pickables: Vec<scene::Pickable>,
    /// Server-requested MoveTo for our own character, until the server
    /// reports us idle again.
    move_to: Option<ac_world::object::MoveTarget>,
    move_to_since: Instant,
    /// Melee combat mode is on.
    combat: bool,
    /// Magic combat mode is on.
    magic: bool,
    /// Spells we know by name (learnt from scrolls this session).
    known_spells: std::collections::HashMap<u32, String>,
    /// Creature we keep swinging at until it dies or we stop.
    attack_target: Option<u32>,
    /// An attack was sent and AttackDone has not come back yet.
    attack_pending: bool,
    last_attack: Instant,
    attack_backoff: Duration,
    /// Name of the last creature we attacked (its corpse is what we loot).
    last_target_name: String,
    /// Sound output, when a device could be opened and --mute is off.
    audio: Option<ac_audio::Audio>,
    sound_tables:
        std::collections::HashMap<u32, Option<std::rc::Rc<ac_formats::sound_table::SoundTable>>>,
    waves: std::collections::HashMap<u32, Option<std::rc::Rc<ac_formats::wave::Wave>>>,
    /// Items still to take from the open container, one at a time (the
    /// server refuses a second pickup while one is in progress).
    loot_queue: std::collections::VecDeque<u32>,
    loot_inflight: Option<(u32, Instant)>,
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
    /// Space was pressed since the last frame.
    jump_requested: bool,
    last_cursor: Option<(f64, f64)>,
    last_frame: Instant,
    ui: Option<ui::Ui>,
    fps: f32,
}

impl App {
    /// Play a server Sound message through the object's sound table.
    fn play_sound(&mut self, body: &[u8]) {
        use ac_net::messages::parse_sound;
        let Ok((guid, kind, volume)) = parse_sound(body) else {
            return;
        };
        let Some(net) = self.net.as_mut() else { return };
        let (name, table_id) = match net.world.objects.get(&guid) {
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
        let assets = &net.assets;
        let table = net
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
        let Some(wave_id) = ac_audio::sound_for(&table, kind) else {
            return;
        };
        let wave = net
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
        if let Some(audio) = &net.audio {
            if let Err(e) = audio.play(&wave, volume.clamp(0.0, 1.0)) {
                tracing::debug!("play: {e}");
            }
        }
    }

    fn chat_message(&mut self, op: u32, body: &[u8]) {
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
                    if let Some(net) = self.net.as_mut() {
                        net.attack_pending = false;
                        // ACE always reports ActionCancelled (0x36) here: it
                        // just means the swing sequence ended.
                        tracing::debug!("attack done ({err:#x})");
                        net.attack_backoff = Duration::from_millis(300);
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
        // A wall in front of the hit hides it.
        if let (Some((t, _)), Some(pl)) = (best, net.player.as_mut()) {
            let assets = &net.assets;
            if pl.first_wall(assets, near, near + dir * t).is_some() {
                tracing::debug!("click blocked by static geometry");
                best = None;
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
            net.last_click = None;
            self.interact(guid);
        } else {
            tracing::info!("select {name} ({guid:#010x})");
            net.session
                .send_action(action::IDENTIFY_OBJECT, &guid.to_le_bytes());
        }
    }

    /// Double-click semantics: ground items are picked up, carried
    /// wieldables are put on, worn items are taken off, everything else
    /// is used.
    fn interact(&mut self, guid: u32) {
        use ac_net::messages::action;
        use ac_world::{item_type, object_desc_flags};
        let Some(net) = self.net.as_mut() else { return };
        let me = net.world.player_guid;
        let Some(o) = net.world.objects.get(&guid) else {
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
        if net.combat && attackable {
            self.attack(guid);
            return;
        }
        if carried && o.spell_id != 0 {
            if let Some(spell) = name.strip_prefix("Scroll of ") {
                net.known_spells.insert(o.spell_id, spell.to_string());
            }
        }
        if carried && o.wielder != me && o.item_type & item_type::WIELDABLE != 0 {
            tracing::info!("wield {name} ({guid:#010x}) at {:#x}", o.valid_locations);
            w.u32(guid).u32(o.valid_locations);
            net.session
                .send_action(action::GET_AND_WIELD_ITEM, &w.finish());
        } else if carried && o.wielder == me {
            tracing::info!("take off {name} ({guid:#010x})");
            w.u32(guid).u32(me.unwrap_or(0)).u32(0);
            net.session
                .send_action(action::PUT_ITEM_IN_CONTAINER, &w.finish());
        } else if !carried && !stuck && o.position.is_some() {
            tracing::info!("pick up {name} ({guid:#010x})");
            w.u32(guid).u32(me.unwrap_or(0)).u32(0);
            net.session
                .send_action(action::PUT_ITEM_IN_CONTAINER, &w.finish());
        } else {
            tracing::info!("use {name} ({guid:#010x})");
            net.session.send_action(action::USE, &guid.to_le_bytes());
        }
    }

    /// Cast a spell on ourselves (untargeted), entering magic mode first.
    fn cast(&mut self, spell: u32) {
        use ac_net::messages::{action, combat_mode};
        let Some(net) = self.net.as_mut() else { return };
        if !net.magic {
            net.session.send_action(
                action::CHANGE_COMBAT_MODE,
                &combat_mode::MAGIC.to_le_bytes(),
            );
            net.magic = true;
            net.combat = false;
        }
        tracing::info!(
            "cast {} ({spell})",
            net.known_spells.get(&spell).cloned().unwrap_or_default()
        );
        net.session
            .send_action(action::CAST_UNTARGETED_SPELL, &spell.to_le_bytes());
    }

    /// Enter or leave melee combat mode.
    fn toggle_combat(&mut self) {
        use ac_net::messages::{action, combat_mode};
        let Some(net) = self.net.as_mut() else { return };
        net.combat = !net.combat;
        net.magic = false;
        let mode = if net.combat {
            combat_mode::MELEE
        } else {
            combat_mode::NON_COMBAT
        };
        tracing::info!("combat mode {}", if net.combat { "melee" } else { "peace" });
        net.session
            .send_action(action::CHANGE_COMBAT_MODE, &mode.to_le_bytes());
        if !net.combat {
            net.attack_target = None;
            net.attack_pending = false;
        }
        if let Some(ui) = &mut self.ui {
            ui.combat = net.combat;
        }
    }

    /// Swing at a creature (medium height, half power) and keep swinging
    /// after each AttackDone until it dies or combat mode ends.
    fn attack(&mut self, guid: u32) {
        use ac_net::messages::action;
        let Some(net) = self.net.as_mut() else { return };
        if !net.combat {
            return;
        }
        let name = net
            .world
            .objects
            .get(&guid)
            .map(|o| o.name.clone())
            .unwrap_or_default();
        tracing::info!("attack {name} ({guid:#010x})");
        net.last_target_name = name.clone();
        let mut w = ac_net::wire::Writer::new();
        w.u32(guid).u32(2).f32(0.5);
        net.session
            .send_action(action::TARGETED_MELEE_ATTACK, &w.finish());
        net.attack_target = Some(guid);
        net.attack_pending = true;
        net.last_attack = Instant::now();
        net.selected = Some(guid);
    }

    /// Combat bookkeeping each tick: repeat attacks, drop dead targets.
    fn tick_combat(&mut self) {
        let Some(net) = self.net.as_mut() else { return };
        let Some(target) = net.attack_target else {
            return;
        };
        let alive = net
            .world
            .objects
            .get(&target)
            .map(|o| o.health.is_none_or(|h| h > 0.0))
            .unwrap_or(false);
        if !alive || !net.combat {
            tracing::info!("attack target gone");
            net.attack_target = None;
            net.attack_pending = false;
            return;
        }
        // Re-sending while the server walks us to the target would cancel
        // that walk, so wait for it; back off after a refused attack.
        if !net.attack_pending
            && net.move_to.is_none()
            && net.last_attack.elapsed() > net.attack_backoff
        {
            self.attack(target);
        }
    }

    /// Buy from / sell to the open vendor, as the UI asked.
    fn tick_vendor(&mut self) {
        use ac_net::messages::{action, trade};
        let (buy, sell, close) = match self.ui.as_mut() {
            Some(ui) => (
                std::mem::take(&mut ui.vendor_buy),
                std::mem::take(&mut ui.vendor_sell),
                std::mem::take(&mut ui.vendor_close),
            ),
            None => return,
        };
        let Some(net) = self.net.as_mut() else { return };
        let Some(vendor) = net.world.open_vendor.as_ref().map(|v| v.vendor) else {
            return;
        };
        for guid in buy {
            tracing::info!("buy {guid:#010x} from {vendor:#010x}");
            net.session
                .send_action(action::BUY, &trade(vendor, &[(guid, 1)]));
        }
        for guid in sell {
            let amount = net
                .world
                .objects
                .get(&guid)
                .map(|o| o.stack_size.max(1) as i32)
                .unwrap_or(1);
            tracing::info!("sell {guid:#010x} to {vendor:#010x}");
            net.session
                .send_action(action::SELL, &trade(vendor, &[(guid, amount)]));
        }
        if close {
            net.world.open_vendor = None;
        }
    }

    /// Take items out of the open container / close it, as the UI asked.
    fn tick_loot(&mut self) {
        use ac_net::messages::action;
        let (take, close) = match self.ui.as_mut() {
            Some(ui) => (
                std::mem::take(&mut ui.loot_take),
                std::mem::take(&mut ui.loot_close),
            ),
            None => return,
        };
        let Some(net) = self.net.as_mut() else { return };
        let me = net.world.player_guid.unwrap_or(0);
        for guid in take {
            if !net.loot_queue.contains(&guid) && net.loot_inflight.map(|(g, _)| g) != Some(guid) {
                net.loot_queue.push_back(guid);
            }
        }
        // The in-flight pickup is done once the item is ours, gone, or stale.
        if let Some((guid, since)) = net.loot_inflight {
            let landed = net
                .world
                .objects
                .get(&guid)
                .is_none_or(|o| o.container == Some(me) || o.wielder == Some(me));
            if landed || since.elapsed() > Duration::from_secs(4) {
                net.loot_inflight = None;
            }
        }
        if net.loot_inflight.is_none() {
            if let Some(guid) = net.loot_queue.pop_front() {
                let name = net
                    .world
                    .objects
                    .get(&guid)
                    .map(|o| o.name.clone())
                    .unwrap_or_default();
                tracing::info!("take {name} ({guid:#010x})");
                let mut w = ac_net::wire::Writer::new();
                w.u32(guid).u32(me).u32(0);
                net.session
                    .send_action(action::PUT_ITEM_IN_CONTAINER, &w.finish());
                net.loot_inflight = Some((guid, Instant::now()));
            }
        }
        if close {
            if let Some((c, _)) = net.world.open_container.take() {
                net.session
                    .send_action(action::NO_LONGER_VIEWING_CONTENTS, &c.to_le_bytes());
            }
        }
    }

    /// Send Use for the nearest drawable object called `name` (test hook).
    fn use_by_name(&mut self, name: &str) -> bool {
        let Some(net) = self.net.as_mut() else {
            return false;
        };
        let me = net.player.as_ref().map(|p| p.world_position());
        let my_guid = net.world.player_guid;
        let mut best: Option<(f32, u32)> = None;
        for o in net.world.objects.values() {
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
                net.selected = Some(guid);
                self.interact(guid);
                true
            }
            None => {
                tracing::debug!("no object named {name:?} in view yet");
                false
            }
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
                    ui.status_icon = ui::IconLayers::default();
                    if let Some(o) = net.selected.and_then(|g| net.world.objects.get(&g)) {
                        s += &format!("  selected: {}", o.name);
                        ui.status_icon = ui::IconLayers {
                            underlay: o.icon_underlay,
                            icon: o.icon_id,
                            overlay: o.icon_overlay,
                        };
                    }
                    if net.combat {
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
        let Some(net) = &self.net else {
            ui.sheet = None;
            ui.blips.clear();
            return;
        };
        let st = &net.world.stats;
        let skill_table = net.assets.skill_table().ok();
        ui.sheet = (!st.name.is_empty()).then(|| {
            let mut skills: Vec<ui::SkillRow> = st
                .skills
                .iter()
                .map(|sk| ui::SkillRow {
                    name: ac_world::stats::skill_name(sk.id),
                    value: st.skill_value(sk, skill_table.as_ref().and_then(|t| t.get(sk.id))),
                    ranks: sk.ranks,
                    advancement: sk.advancement,
                    training: ac_world::stats::sac_name(sk.advancement),
                })
                .collect();
            skills.sort_by(|a, b| b.advancement.cmp(&a.advancement).then(a.name.cmp(b.name)));
            ui::Sheet {
                name: st.name.clone(),
                level: st.level,
                vitals: (0..3)
                    .map(|i| ui::VitalBar {
                        name: ac_world::stats::VITAL_NAMES[i],
                        current: st.vitals[i].current,
                        max: st.vital_max(i),
                    })
                    .collect(),
                total_xp: st.total_xp,
                available_xp: st.available_xp,
                skill_credits: st.skill_credits,
                skills,
            }
        });
        if ui.spells.len() != st.spells.len() {
            ui.spells = match (net.assets.spell_table(), net.assets.spell_components()) {
                (Ok(table), Ok(comps)) => spell_rows(&table, &comps, st.spells.iter().copied()),
                _ => Vec::new(),
            };
        }
        ui.target = net
            .attack_target
            .or(net.selected)
            .and_then(|g| net.world.objects.get(&g))
            .filter(|o| o.item_type & ac_world::item_type::CREATURE != 0)
            .map(|o| (o.name.clone(), o.health.unwrap_or(1.0)));
        ui.loot = net.world.open_container.as_ref().map(|(c, items)| {
            let name = net
                .world
                .objects
                .get(c)
                .map(|o| o.name.clone())
                .unwrap_or_else(|| "Container".into());
            let list = items
                .iter()
                .filter_map(|g| net.world.objects.get(g))
                .map(|o| ui::Item {
                    guid: o.guid,
                    name: o.name.clone(),
                    stack: o.stack_size,
                    wielded: false,
                    icon: o.icon_id,
                    icon_overlay: o.icon_overlay,
                    icon_underlay: o.icon_underlay,
                })
                .collect();
            (name, list)
        });
        ui.vendor = net.world.open_vendor.as_ref().map(|v| {
            let name = net
                .world
                .objects
                .get(&v.vendor)
                .map(|o| o.name.clone())
                .unwrap_or_else(|| "Vendor".into());
            let stock = v
                .items
                .iter()
                .map(|it| ui::TradeItem {
                    guid: it.guid,
                    name: it.desc.name.clone(),
                    // ACE's "SellPrice" is the rate the vendor sells at.
                    price: ((it.desc.value as f32 * v.sell_rate - 0.1).ceil().max(1.0)) as u32,
                    icon: it.desc.icon_id,
                    unlimited: it.stack == 0x00FF_FFFF,
                })
                .collect();
            let selling = net
                .world
                .inventory()
                .filter(|o| o.value > 0 && o.item_type & ac_world::item_type::MONEY == 0)
                .map(|o| ui::TradeItem {
                    guid: o.guid,
                    name: o.name.clone(),
                    price: ((o.value as f32 * v.buy_rate + 0.1).floor().max(1.0)) as u32,
                    icon: o.icon_id,
                    unlimited: false,
                })
                .collect();
            ui::Vendor {
                name,
                stock,
                selling,
            }
        });
        ui.items.clear();
        for o in net.world.wielded() {
            ui.items.push(ui::Item {
                guid: o.guid,
                name: o.name.clone(),
                stack: o.stack_size,
                wielded: true,
                icon: o.icon_id,
                icon_overlay: o.icon_overlay,
                icon_underlay: o.icon_underlay,
            });
        }
        for o in net.world.inventory() {
            ui.items.push(ui::Item {
                guid: o.guid,
                name: o.name.clone(),
                stack: o.stack_size,
                wielded: false,
                icon: o.icon_id,
                icon_overlay: o.icon_overlay,
                icon_underlay: o.icon_underlay,
            });
        }
        ui.items
            .sort_by(|a, b| b.wielded.cmp(&a.wielded).then(a.name.cmp(&b.name)));
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
            loaded_blocks: Default::default(),
            dungeon: Default::default(),
            mesh_cache: Default::default(),
            last_generation: 0,
            pickables: Vec::new(),
            move_to: None,
            move_to_since: Instant::now(),
            combat: false,
            magic: false,
            known_spells: Default::default(),
            attack_target: None,
            attack_pending: false,
            last_attack: Instant::now(),
            attack_backoff: Duration::from_millis(300),
            last_target_name: String::new(),
            audio,
            sound_tables: Default::default(),
            waves: Default::default(),
            loot_queue: Default::default(),
            loot_inflight: None,
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
                        ac_world::Applied::Moved => continue,
                        ac_world::Applied::PlayerMoveTo | ac_world::Applied::PlayerMotion => {
                            // A MoveTo aimed at us is ours to carry out; a plain
                            // motion state for us means the server is done
                            // walking us (or echoed our own state).
                            let stance = net.world.player().map(|o| o.motion.style);
                            let target = net.world.player_mut().and_then(|o| o.target.take());
                            match target {
                                Some(t) => {
                                    if net.move_to.is_none() {
                                        tracing::debug!("server move-to {t:?}");
                                        net.move_to_since = Instant::now();
                                    }
                                    net.move_to = Some(t);
                                }
                                None => {
                                    if net.move_to.take().is_some() {
                                        tracing::debug!("server move-to finished");
                                        // Take the server's idea of where we ended up.
                                        if let (Some(pl), Some(p)) = (
                                            net.player.as_mut(),
                                            net.world.player().and_then(|o| o.position),
                                        ) {
                                            pl.cell = p.cell;
                                            pl.local = p.local;
                                            pl.dirty = true;
                                        }
                                    }
                                }
                            }
                            // Our stance follows the server (combat mode changes).
                            if let (Some(st), Some(pl)) = (stance, net.player.as_mut()) {
                                if st != 0 {
                                    pl.set_stance(&net.assets, 0x8000_0000 | st as u32);
                                }
                            }
                            continue;
                        }
                        ac_world::Applied::Appearance => {
                            // Our own look changed: redraw the character.
                            if let Some(pl) = net.player.as_mut() {
                                pl.dirty = true;
                            }
                            continue;
                        }
                        ac_world::Applied::Created
                        | ac_world::Applied::Deleted
                        | ac_world::Applied::Stats
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
                                if ev == 0x00A0 && rest.len() >= 8 {
                                    let item =
                                        u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
                                    let err =
                                        u32::from_le_bytes([rest[4], rest[5], rest[6], rest[7]]);
                                    tracing::warn!(
                                        "inventory action failed for {item:#010x}, error {err:#x}"
                                    );
                                    if net.loot_inflight.map(|(g, _)| g) == Some(item) {
                                        net.loot_inflight = None;
                                    }
                                } else if ev == ac_net::messages::event::USE_DONE && rest.len() >= 4
                                {
                                    let err =
                                        u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
                                    tracing::debug!("use done, error {err:#x}");
                                    // The walk the server asked for is over either way.
                                    net.move_to = None;
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
        self.tick_combat();
        self.tick_loot();
        self.tick_vendor();
        let activated: Vec<u32> = self
            .ui
            .as_mut()
            .map(|u| std::mem::take(&mut u.activated))
            .unwrap_or_default();
        for guid in activated {
            self.interact(guid);
        }
        let casts: Vec<u32> = self
            .ui
            .as_mut()
            .map(|u| std::mem::take(&mut u.cast_requests))
            .unwrap_or_default();
        for spell in casts {
            self.cast(spell);
        }
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
        // Stream landblocks around the character: the block we stand in
        // first, then its neighbours (outdoors only), one per frame.
        if let Some(center) = net.player.as_ref().map(|p| p.landblock()) {
            let mut wanted = vec![center];
            if net.dungeon.get(&center) == Some(&false) {
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
            if let Some(&id) = wanted.iter().find(|id| !net.loaded_blocks.contains(id)) {
                let t0 = Instant::now();
                match scene::build_landblock(&net.assets, id, &mut net.mesh_cache) {
                    Ok(built) => {
                        net.dungeon.insert(id, built.is_dungeon);
                        let assets = &net.assets;
                        let palettes = &net.palettes;
                        gpu.add_block(id, built.batches, |k| {
                            scene::material_image(assets, k, palettes)
                        });
                        if id == center {
                            // Daylight sky and fog outdoors; dungeons get a
                            // black sky and no fog.
                            let env = if built.is_dungeon {
                                sky::Environment::dungeon()
                            } else {
                                assets
                                    .region()
                                    .ok()
                                    .and_then(|r| sky::Environment::from_region(&r, 0.5))
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
                        net.dungeon.insert(id, false);
                    }
                }
                net.loaded_blocks.insert(id);
            }
            let stale: Vec<u32> = net
                .loaded_blocks
                .iter()
                .copied()
                .filter(|id| !wanted.contains(id))
                .collect();
            for id in stale {
                gpu.remove_block(id);
                net.loaded_blocks.remove(&id);
                tracing::info!("landblock {id:#010x} unloaded");
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
                jump: std::mem::take(&mut self.jump_requested),
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
                let _ = arrived;
            }
            let now = Instant::now();
            let dt = self.frame_dt;
            pl.update(&net.assets, &input, dt);
            if let Some(j) = pl.last_jump.take() {
                tracing::info!("jump power {:.2} velocity {:?}", j.power, j.velocity);
                net.session.send_action(
                    ac_net::messages::action::JUMP,
                    &ac_net::messages::jump(j.power, j.velocity.to_array(), 1),
                );
            }
            // One-shot motions the server broadcast for us (attacks, emotes).
            let mut cmds = Vec::new();
            if let Some(o) = net.world.player_mut() {
                while let Some(c) = o.commands.pop() {
                    cmds.push(c);
                }
            }
            for c in cmds {
                pl.play_command(&net.assets, c.command as u32, c.speed);
            }
            let pose = pl.animate(&net.assets, &input, dt);
            if pose.is_some() {
                pl.dirty = true;
            }
            let quiet =
                net.move_to.is_some() && net.move_to_since.elapsed() < Duration::from_secs(12);
            if !quiet && net.move_to.is_some() {
                tracing::debug!("server move-to timed out");
                net.move_to = None;
            }
            pl.report(&mut net.session, &input, now, quiet);
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
        let region = assets.region()?;
        if let Some(env) = sky::Environment::from_region(&region, 0.5) {
            gpu.set_environment(env);
        }
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
        let mut ui = ui::Ui::new(gpu.device(), gpu.format(), Some(&window), w, h);
        ui.set_icon_loader(icon_loader(self.cli.data_dir.clone()));
        self.ui = Some(ui);
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
                    if code == KeyCode::Space
                        && event.state == ElementState::Pressed
                        && self.net.is_some()
                    {
                        self.jump_requested = true;
                    }
                    if code == KeyCode::KeyC && event.state == ElementState::Pressed {
                        self.toggle_combat();
                        return;
                    }
                    if code == KeyCode::KeyI && event.state == ElementState::Pressed {
                        if let Some(ui) = &mut self.ui {
                            ui.show_inventory = !ui.show_inventory;
                        }
                        return;
                    }
                    if code == KeyCode::KeyK && event.state == ElementState::Pressed {
                        if let Some(ui) = &mut self.ui {
                            ui.show_skills = !ui.show_skills;
                        }
                        return;
                    }
                    if code == KeyCode::KeyP && event.state == ElementState::Pressed {
                        if let Some(ui) = &mut self.ui {
                            ui.show_spells = !ui.show_spells;
                        }
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
            jump_requested: false,
            last_cursor: None,
            last_frame: Instant::now(),
            ui: None,
            fps: 0.0,
        };
        app.load_scene(&mut gpu)?;
        let (w, h) = gpu.size();
        let mut ui = ui::Ui::new(gpu.device(), gpu.format(), None, w, h);
        ui.set_icon_loader(icon_loader(app.cli.data_dir.clone()));
        app.ui = Some(ui);
        if app.cli.connect.is_some() {
            // Pump the connection until the player is placed and the world
            // has settled, then render from the character's viewpoint.
            let deadline = Instant::now()
                + Duration::from_secs(40)
                + Duration::from_secs_f32(app.cli.walk + 3.0 * app.cli.click.len() as f32 + 10.0)
                + if app.cli.attack.is_some() {
                    Duration::from_secs(120)
                } else {
                    Duration::ZERO
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
            loop {
                app.tick_net(&mut gpu);
                // No frames are presented headlessly, so recycle GPU
                // staging buffers here or they pile up for the final poll.
                let _ = gpu.device().poll(wgpu::PollType::Poll);
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
                            if let Some(ui) = app.ui.as_mut() {
                                ui.begin(None, gpu.device(), gpu.queue(), w, h);
                                ui.begin(None, gpu.device(), gpu.queue(), w, h);
                            }
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
                            let id = app.net.as_ref().and_then(|n| {
                                n.known_spells
                                    .iter()
                                    .find(|(_, name)| name.starts_with(&want))
                                    .map(|(id, _)| *id)
                                    .or_else(|| {
                                        n.world
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
                        if let (Some(net), Some(ui)) = (app.net.as_ref(), app.ui.as_mut()) {
                            if net.world.open_vendor.is_some() {
                                if let Some(o) =
                                    net.world.inventory().find(|o| o.name.starts_with(&want))
                                {
                                    tracing::info!("selling {}", o.name);
                                    ui.vendor_sell.push(o.guid);
                                }
                                app.cli.sell = None;
                                bought_at = Some(Instant::now());
                            }
                        }
                    }
                    if let Some(want) = app.cli.buy.clone() {
                        if let (Some(net), Some(ui)) = (app.net.as_ref(), app.ui.as_mut()) {
                            if let Some(v) = &net.world.open_vendor {
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
                                if let Some(it) =
                                    v.items.iter().find(|i| i.desc.name.starts_with(&want))
                                {
                                    tracing::info!("buying {}", it.desc.name);
                                    ui.vendor_buy.push(it.guid);
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
                            if !app.net.as_ref().is_some_and(|n| n.combat) {
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
                        app.net.as_ref().is_some_and(|n| n.attack_target.is_some())
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
                            if app.net.as_ref().is_some_and(|n| n.combat) {
                                app.toggle_combat();
                            }
                            let corpse = if name.is_empty() {
                                format!(
                                    "Corpse of {}",
                                    app.net
                                        .as_ref()
                                        .map(|n| n.last_target_name.clone())
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
                        if let (Some(ui), Some(net)) = (app.ui.as_mut(), app.net.as_ref()) {
                            if let Some((_, items)) = &net.world.open_container {
                                if !items.is_empty()
                                    && ui.loot_take.is_empty()
                                    && loot_at.elapsed() > Duration::from_secs(1)
                                {
                                    ui.loot_take.extend(items.iter().copied());
                                    loot_state = 3;
                                    loot_at = Instant::now();
                                }
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
                            .net
                            .as_ref()
                            .map(|n| {
                                n.world
                                    .drawable()
                                    .map(|o| format!("{} {:#x}", o.name, o.object_desc_flags))
                                    .collect()
                            })
                            .unwrap_or_default();
                        names.sort();
                        tracing::debug!("objects in view: {}", names.join(" | "));
                        if let Some(n) = app.net.as_ref() {
                            let st = &n.world.stats;
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
                        if app.cli.show_skills {
                            if let Some(ui) = app.ui.as_mut() {
                                ui.show_skills = true;
                            }
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
                            "{ticks} ticks/s; {} gpu meshes, {} materials, {} instances",
                            app.net.as_ref().map(|n| n.gpu_meshes.len()).unwrap_or(0),
                            gpu.material_count(),
                            gpu.instance_count()
                        );
                        ticks = 0;
                        ticks_since = Instant::now();
                    }
                    // Capture while still walking so the walk pose is visible,
                    // or after a short settle when not walking.
                    let pending = app.cli.attack.is_some() || app.cli.use_name.is_some();
                    let looting = app
                        .net
                        .as_ref()
                        .is_some_and(|n| !n.loot_queue.is_empty() || n.loot_inflight.is_some());
                    let done = if pending
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
        if app.cli.demo_ui {
            demo_ui(&mut ui, &app.cli.data_dir);
        }
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
        jump_requested: false,
        last_cursor: None,
        last_frame: Instant::now(),
        ui: None,
        fps: 0.0,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
