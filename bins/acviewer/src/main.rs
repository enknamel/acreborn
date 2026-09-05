//! acviewer: fly around a landblock or inspect a model.
//!
//!   acviewer --landblock A9B4 [--radius 1]
//!   acviewer --model 02000001
//!   acviewer --emitter 32000273            # simulate an emitter's particles
//!   acviewer --chargen aluvian,m,3,0,0,0,0.5     # a dressed-up human head
//!
//! Controls: right mouse drag to look, WASD to move, Q/E down/up,
//! Shift to go faster, Escape to quit.

mod camera;
mod gpu;
mod particles;
mod scene;
use ac_client::player;
mod plugins;
mod sky;
mod ui;
mod water;
mod world_fx;

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
    /// many clients on one machine.
    #[arg(long, default_value_t = 60)]
    fps: u32,
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
    client: ac_client::Client,
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
    /// Space was pressed since the last frame.
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
    /// Block id -> is a dungeon (learnt when the block is built).
    dungeon: std::collections::HashMap<u32, bool>,
    /// Caches shared by every session: decoded meshes, GPU meshes, palettes,
    /// motion tables, block particle emitters, and the sound device.
    mesh_cache: std::collections::HashMap<u32, ac_scene::model::Mesh>,
    gpu_meshes: scene::GpuMeshCache,
    palettes: scene::Palettes,
    tables: std::collections::HashMap<u32, Option<ac_formats::motion_table::MotionTable>>,
    fx: world_fx::WorldFx,
    audio: Option<ac_audio::Audio>,
}

impl App {
    /// Apply what plugins asked for (chat lines to the active log, a
    /// session switch).
    fn apply_requests(&mut self, r: plugins::Requests) {
        if let Some(ui) = &mut self.ui {
            for (text, kind) in r.chat {
                ui.push_chat(text, kind);
            }
        }
        if let Some(a) = r.activate {
            self.switch_to(a);
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
            if let Some(ui) = &mut self.ui {
                ui.combat = net.client.combat;
            }
        }
    }

    fn use_by_name(&mut self, name: &str) -> bool {
        self.nets
            .get_mut(self.active)
            .is_some_and(|net| net.client.use_by_name(name))
    }

    /// Everything the UI asked for this frame goes to the session as commands.
    fn apply_ui_commands(&mut self) {
        let Some(ui) = self.ui.as_mut() else { return };
        let outgoing = std::mem::take(&mut ui.outgoing);
        let buy = std::mem::take(&mut ui.vendor_buy);
        let sell = std::mem::take(&mut ui.vendor_sell);
        let close_vendor = std::mem::take(&mut ui.vendor_close);
        let take = std::mem::take(&mut ui.loot_take);
        let close_loot = std::mem::take(&mut ui.loot_close);
        let activated = std::mem::take(&mut ui.activated);
        let casts = std::mem::take(&mut ui.cast_requests);
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
                requests.push(r);
            } else if let Some(net) = self.nets.get_mut(active) {
                net.client.say(&t);
            }
        }
        let Some(net) = self.nets.get_mut(active) else {
            return;
        };
        let c = &mut net.client;
        for g in buy {
            c.buy(g);
        }
        for g in sell {
            c.sell(g);
        }
        if close_vendor {
            c.close_vendor();
        }
        for g in take {
            c.take(g);
        }
        if close_loot {
            c.close_container();
        }
        for g in activated {
            c.interact(g);
        }
        for sp in casts {
            c.cast(sp);
        }
        for r in requests {
            self.apply_requests(r);
        }
    }
    /// Play a server Sound message through the object's sound table.

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
        let Some(net) = self.nets.get_mut(self.active) else {
            return;
        };
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
        let Some((_, guid)) = best else {
            net.client.selected = None;
            return;
        };
        let now = Instant::now();
        let again = matches!(net.client.last_click, Some((t, g)) if g == guid && now - t < Duration::from_millis(500));
        net.client.last_click = Some((now, guid));
        net.client.selected = Some(guid);
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

    /// Double-click semantics: ground items are picked up, carried
    /// wieldables are put on, worn items are taken off, everything else
    /// is used.

    /// Cast a spell on ourselves (untargeted), entering magic mode first.

    /// Enter or leave melee combat mode.

    /// Swing at a creature (medium height, half power) and keep swinging
    /// after each AttackDone until it dies or combat mode ends.

    /// Combat bookkeeping each tick: repeat attacks, drop dead targets.

    /// Buy from / sell to the open vendor, as the UI asked.

    /// Take items out of the open container / close it, as the UI asked.

    /// Send Use for the nearest drawable object called `name` (test hook).

    fn refresh_status(&mut self) {
        let Some(ui) = &mut self.ui else { return };
        let mut s = format!("{:.0} fps", self.fps);
        if let Some(net) = self.nets.get(self.active) {
            match net.client.world.player().and_then(|o| o.position) {
                Some(p) => {
                    s += &format!(
                        "  cell {:#010x}  x {:.1} y {:.1} z {:.1}  objects {}",
                        p.cell,
                        p.local.x,
                        p.local.y,
                        p.local.z,
                        net.client.world.drawable().count()
                    );
                    ui.status_icon = ui::IconLayers::default();
                    if let Some(o) = net
                        .client
                        .selected
                        .and_then(|g| net.client.world.objects.get(&g))
                    {
                        s += &format!("  selected: {}", o.name);
                        ui.status_icon = ui::IconLayers {
                            underlay: o.icon_underlay,
                            icon: o.icon_id,
                            overlay: o.icon_overlay,
                        };
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
        let Some(net) = self.nets.get(self.active) else {
            ui.sheet = None;
            ui.blips.clear();
            return;
        };
        let st = &net.client.world.stats;
        let skill_table = net.client.assets.skill_table().ok();
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
            ui.spells = match (
                net.client.assets.spell_table(),
                net.client.assets.spell_components(),
            ) {
                (Ok(table), Ok(comps)) => spell_rows(&table, &comps, st.spells.iter().copied()),
                _ => Vec::new(),
            };
        }
        ui.target = net
            .client
            .attack_target
            .or(net.client.selected)
            .and_then(|g| net.client.world.objects.get(&g))
            .filter(|o| o.item_type & ac_world::item_type::CREATURE != 0)
            .map(|o| (o.name.clone(), o.health.unwrap_or(1.0)));
        ui.loot = net.client.world.open_container.as_ref().map(|(c, items)| {
            let name = net
                .client
                .world
                .objects
                .get(c)
                .map(|o| o.name.clone())
                .unwrap_or_else(|| "Container".into());
            let list = items
                .iter()
                .filter_map(|g| net.client.world.objects.get(g))
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
        ui.vendor = net.client.world.open_vendor.as_ref().map(|v| {
            let name = net
                .client
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
                .client
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
        for o in net.client.world.wielded() {
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
        for o in net.client.world.inventory() {
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
        let (me, heading) = match (&net.client.player, net.client.world.player()) {
            (Some(p), _) => (ac_world::landblock_origin(p.cell) + p.local, p.heading),
            (None, Some(o)) => match o.display.or(o.position) {
                Some(pos) => (ac_world::landblock_origin(pos.cell) + pos.local, 0.0),
                None => return,
            },
            _ => return,
        };
        let fwd = glam::Vec2::new(-heading.sin(), heading.cos());
        let right = glam::Vec2::new(heading.cos(), heading.sin());
        for o in net.client.world.drawable() {
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
            ac_scene::Assets::open(&self.cli.data_dir).context("opening DAT archives")?,
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
        let mut configs = vec![ac_client::Config {
            host: host.clone(),
            account,
            password,
            character: self.cli.character.clone(),
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
            });
        }
        self.audio = audio;
        for cfg in configs {
            let client = ac_client::Client::connect(cfg, assets.clone())?;
            self.nets.push(Net {
                client,
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
        let input = self.nets.get(i)?.client.player.as_ref().map(|_| {
            if is_active {
                player::Input {
                    forward: (keys.contains(&KeyCode::KeyW) as i8
                        - keys.contains(&KeyCode::KeyS) as i8) as f32,
                    strafe: (keys.contains(&KeyCode::KeyD) as i8
                        - keys.contains(&KeyCode::KeyA) as i8) as f32,
                    run: !keys.contains(&KeyCode::ShiftLeft),
                    jump,
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
                ac_client::Event::Connected
                | ac_client::Event::Terminated(_)
                | ac_client::Event::Refused(_) => {}
            }
        }
        let _ = count;
        Some((frame, events))
    }

    /// Make session `i` the one the window shows; the scene follows it.
    fn switch_to(&mut self, i: usize) {
        if i < self.nets.len() && i != self.active {
            tracing::info!("switching to session {}", i + 1);
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
            } else if let Some(a) = r.activate {
                self.switch_to(a);
            }
        }
        self.plugins.end_frame();
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
            for id in stale {
                gpu.remove_block(id);
                self.fx.unload_block(id);
                self.loaded_blocks.remove(&id);
                tracing::info!("landblock {id:#010x} unloaded");
            }
        }
        if !self.fx.is_empty() {
            self.fx.update(&net.client.assets, self.frame_dt);
            let quads = self.fx.quads();
            let assets = &net.client.assets;
            let palettes = &self.palettes;
            gpu.set_particles(particles::draws(&quads, self.camera.position), |k| {
                scene::material_image(assets, k, palettes)
            });
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
                if let Some(net) = self.nets.get_mut(self.active) {
                    net.client.disconnect(Instant::now());
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
                    if code == KeyCode::Space
                        && event.state == ElementState::Pressed
                        && !self.nets.is_empty()
                    {
                        self.jump_requested = true;
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
                        && !self.nets.is_empty()
                    {
                        if let Some(ui) = &mut self.ui {
                            ui.chat_focus = true;
                        }
                        self.keys.clear();
                        return;
                    }
                    if code == KeyCode::Escape {
                        if let Some(net) = self.nets.get_mut(self.active) {
                            net.client.disconnect(Instant::now());
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
                if self.nets.is_empty() {
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
                        let plugins = &mut self.plugins;
                        let active = self.active;
                        let mut clients: Vec<&mut ac_client::Client> =
                            self.nets.iter_mut().map(|n| &mut n.client).collect();
                        let mut requests: Option<plugins::Requests> = None;
                        ui.begin(
                            self.window.as_deref(),
                            &mut |egui| {
                                if !clients.is_empty() {
                                    let borrowed: Vec<&mut ac_client::Client> =
                                        clients.iter_mut().map(|c| &mut **c).collect();
                                    requests = Some(plugins.ui(borrowed, active, egui));
                                }
                            },
                            g.device(),
                            g.queue(),
                            w,
                            h,
                        );
                        if let Some(r) = requests {
                            for (text, kind) in r.chat {
                                ui.push_chat(text, kind);
                            }
                            self.pending_switch = r.activate;
                        }
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
                // Pace frames: wake up again when the next one is due.
                if self.cli.fps > 0 {
                    self.next_frame = now + Duration::from_secs_f32(1.0 / self.cli.fps as f32);
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
            dungeon: Default::default(),
            mesh_cache: Default::default(),
            gpu_meshes: Default::default(),
            palettes: Default::default(),
            tables: Default::default(),
            fx: Default::default(),
            audio: None,
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
                            if let Some(ui) = app.ui.as_mut() {
                                ui.begin(None, &mut |_| {}, gpu.device(), gpu.queue(), w, h);
                                ui.begin(None, &mut |_| {}, gpu.device(), gpu.queue(), w, h);
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
                        if let (Some(net), Some(ui)) = (app.nets.get(app.active), app.ui.as_mut()) {
                            if net.client.world.open_vendor.is_some() {
                                if let Some(o) = net
                                    .client
                                    .world
                                    .inventory()
                                    .find(|o| o.name.starts_with(&want))
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
                        if let (Some(net), Some(ui)) = (app.nets.get(app.active), app.ui.as_mut()) {
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
                        if let (Some(ui), Some(net)) = (app.ui.as_mut(), app.nets.get(app.active)) {
                            if let Some((_, items)) = &net.client.world.open_container {
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
                        if app.cli.show_skills {
                            if let Some(ui) = app.ui.as_mut() {
                                ui.show_skills = true;
                                ui.show_spells = true;
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
                            app.nets
                                .get(app.active)
                                .map(|_| app.gpu_meshes.len())
                                .unwrap_or(0),
                            gpu.material_count(),
                            gpu.instance_count()
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
        ui.begin(None, &mut |_| {}, gpu.device(), gpu.queue(), w, h);
        ui.begin(None, &mut |_| {}, gpu.device(), gpu.queue(), w, h);
        let mut paint = |d: &wgpu::Device,
                         q: &wgpu::Queue,
                         e: &mut wgpu::CommandEncoder,
                         v: &wgpu::TextureView| ui.paint(d, q, e, v);
        gpu.render_to_png(vp, Vec3::new(0.4, 0.3, 1.0), &path, Some(&mut paint))?;
        tracing::info!("wrote {}", path.display());
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
        dungeon: Default::default(),
        mesh_cache: Default::default(),
        gpu_meshes: Default::default(),
        palettes: Default::default(),
        tables: Default::default(),
        fx: Default::default(),
        audio: None,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
