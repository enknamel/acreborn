//! Map (M): the world map of Dereth or the current landblock (a
//! dungeon's floor plan indoors), with the character, everything the
//! server has sent for the landblock (players, NPCs, monsters, items,
//! portals, doors) as dots, and the overland route being walked.
//!
//! * Tabs switch between World and Local. Drag pans, the wheel zooms,
//!   "follow" keeps the character centred. Hovering shows the map
//!   coordinates under the pointer.
//! * The list on the right is everything in the landblock: a search box
//!   and kind chips narrow it, a click selects the object (and rings it
//!   on the map), a double-click uses it.
//! * On the world map a double-click asks for a route there and the
//!   character walks it (see `ac_client::Client::travel_to`); a place
//!   name typed into "travel to" does the same by the gazetteer. The
//!   route is drawn as a line; Cancel stops it.
//!
//! The world map is rendered once from the terrain grid on a background
//! thread (the first time takes a few seconds while the grid is read
//! from the cell archive; both are cached under `~/.cache/acreborn`).
//! The local map is rendered when the landblock changes, and in a
//! dungeon whenever the character moves to another storey.

use super::{caption, has_sheet, title, window, Source};
use crate::{egui, Client, Ctx, Plugin, Settings};
use ac_scene::mapimage::MapImage;
use glam::Vec2;
use std::sync::mpsc::Receiver;

/// What an object on the map is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Player,
    Npc,
    Monster,
    Corpse,
    Portal,
    Door,
    Item,
}

impl Kind {
    pub fn of(o: &ac_world::WorldObject) -> Kind {
        use ac_world::{item_type, object_desc_flags as f};
        let d = o.object_desc_flags;
        if d & f::PLAYER != 0 {
            Kind::Player
        } else if d & f::CORPSE != 0 {
            Kind::Corpse
        } else if d & f::PORTAL != 0 || o.item_type & item_type::PORTAL != 0 {
            Kind::Portal
        } else if d & f::DOOR != 0 {
            Kind::Door
        } else if o.item_type & item_type::CREATURE != 0 || o.motion_table_id != 0 {
            if d & f::ATTACKABLE != 0 {
                Kind::Monster
            } else {
                Kind::Npc
            }
        } else {
            Kind::Item
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Player => "player",
            Kind::Npc => "npc",
            Kind::Monster => "monster",
            Kind::Corpse => "corpse",
            Kind::Portal => "portal",
            Kind::Door => "door",
            Kind::Item => "item",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            Kind::Player => egui::Color32::from_rgb(80, 200, 255),
            Kind::Npc => egui::Color32::from_rgb(120, 230, 120),
            Kind::Monster => egui::Color32::from_rgb(240, 90, 80),
            Kind::Corpse => egui::Color32::from_gray(150),
            Kind::Portal => egui::Color32::from_rgb(200, 120, 255),
            Kind::Door => egui::Color32::from_rgb(200, 170, 100),
            Kind::Item => egui::Color32::from_rgb(240, 220, 90),
        }
    }
}

/// The chips over the object list: a label and the kinds it keeps.
pub const KINDS: &[(&str, &[Kind])] = &[
    ("All", &[]),
    ("Players", &[Kind::Player]),
    ("NPCs", &[Kind::Npc]),
    ("Monsters", &[Kind::Monster]),
    ("Items", &[Kind::Item, Kind::Corpse]),
    ("Portals", &[Kind::Portal, Kind::Door]),
];

#[derive(Clone, Debug, PartialEq)]
pub struct MapObject {
    pub guid: u32,
    pub name: String,
    pub kind: Kind,
    /// World xy and z.
    pub pos: Vec2,
    pub z: f32,
    pub distance: f32,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MapView {
    /// The character's landblock (`xxyy0000`) and whether it is a dungeon.
    pub block: u32,
    pub dungeon: bool,
    pub me: Vec2,
    pub me_z: f32,
    /// Radians, 0 = north, counter-clockwise.
    pub heading: f32,
    pub coords: String,
    pub objects: Vec<MapObject>,
    /// The overland route being walked, and (next waypoint, count).
    pub route: Vec<Vec2>,
    pub travel: Option<(usize, usize)>,
    /// Town names and world positions for the world map.
    pub places: Vec<(&'static str, Vec2)>,
}

/// The character's world position, z, heading and landblock.
fn me_of(c: &Client) -> Option<(Vec2, f32, f32, u32)> {
    if let Some(p) = &c.player {
        let w = p.world_position();
        return Some((Vec2::new(w.x, w.y), w.z, p.heading, p.cell & 0xFFFF_0000));
    }
    let o = c.world.player()?;
    let pos = o.display.or(o.position)?;
    let w = ac_world::landblock_origin(pos.cell) + pos.local;
    Some((Vec2::new(w.x, w.y), w.z, 0.0, pos.cell & 0xFFFF_0000))
}

pub fn view(c: &Client) -> Option<MapView> {
    if !has_sheet(c) {
        return None;
    }
    let (me, me_z, heading, block) = me_of(c)?;
    let dungeon = c
        .player
        .as_ref()
        .map(|p| p.cell & 0xFFFF >= 0x100)
        .unwrap_or(false)
        && c.assets
            .landblock(block)
            .map(|s| s.is_dungeon)
            .unwrap_or(false);
    let mut objects: Vec<MapObject> = c
        .world
        .drawable()
        .filter(|o| !o.is_player)
        .filter_map(|o| {
            let pos = o.display.or(o.position)?;
            if pos.cell & 0xFFFF_0000 != block {
                return None;
            }
            let w = ac_world::landblock_origin(pos.cell) + pos.local;
            let p = Vec2::new(w.x, w.y);
            Some(MapObject {
                guid: o.guid,
                name: o.name.clone(),
                kind: Kind::of(o),
                pos: p,
                z: w.z,
                distance: (p - me).length(),
                selected: c.selected == Some(o.guid),
            })
        })
        .collect();
    objects.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    let coords = c
        .player
        .as_ref()
        .map(|p| {
            ac_world::map_coord_str(&ac_world::object::Position {
                cell: p.cell,
                local: p.local,
                rotation: glam::Quat::IDENTITY,
            })
        })
        .unwrap_or_default();
    Some(MapView {
        block,
        dungeon,
        me,
        me_z,
        heading,
        coords,
        objects,
        route: c.travel_route().map(|r| r.to_vec()).unwrap_or_default(),
        travel: c.travel_progress(),
        places: ac_world::towns::PLACES
            .iter()
            .map(|p| (p.name, p.world_xy()))
            .collect(),
    })
}

/// Objects of the view kept by the search line and chip, nearest first.
pub fn shown(objects: &[MapObject], search: &str, kind: usize) -> Vec<usize> {
    let needle = search.trim().to_lowercase();
    let kinds = KINDS.get(kind).map(|k| k.1).unwrap_or(&[]);
    objects
        .iter()
        .enumerate()
        .filter(|(_, o)| kinds.is_empty() || kinds.contains(&o.kind))
        .filter(|(_, o)| {
            needle.is_empty()
                || o.name.to_lowercase().contains(&needle)
                || o.kind.label().contains(&needle)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Map coordinates ("42.1N, 33.6E") of a world xy.
pub fn coords_of(world: Vec2) -> String {
    let ns = world.y / 240.0 - 102.0;
    let ew = world.x / 240.0 - 102.0;
    format!(
        "{:.1}{}, {:.1}{}",
        (ns.abs() - 0.05).max(0.0),
        if ns >= 0.0 { "N" } else { "S" },
        (ew.abs() - 0.05).max(0.0),
        if ew >= 0.0 { "E" } else { "W" }
    )
}

/// Where a world xy lands on screen for a view centred on `center`
/// showing `s` screen points per metre.
pub fn to_screen(rect: egui::Rect, center: Vec2, s: f32, world: Vec2) -> egui::Pos2 {
    let c = rect.center();
    egui::pos2(
        c.x + (world.x - center.x) * s,
        c.y - (world.y - center.y) * s,
    )
}

pub fn to_world(rect: egui::Rect, center: Vec2, s: f32, screen: egui::Pos2) -> Vec2 {
    let c = rect.center();
    Vec2::new(
        center.x + (screen.x - c.x) / s,
        center.y - (screen.y - c.y) / s,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Tab {
    World,
    Local,
}

/// What the player did this frame.
#[derive(Debug, Default, PartialEq)]
pub struct Actions {
    pub select: Option<u32>,
    pub activate: Option<u32>,
    pub travel_to: Option<Vec2>,
    pub travel_to_place: Option<String>,
    pub cancel_travel: bool,
}

/// The panel's own state. The pan is not kept across restarts: the map
/// opens on the character.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct State {
    pub tab: Tab,
    /// Not kept across restarts: a search that survived would hide most
    /// of the list on the next run and look like a broken panel.
    #[serde(skip)]
    pub search: String,
    pub kind: usize,
    pub follow: bool,
    /// Screen points per image pixel, per tab.
    pub zoom_world: f32,
    pub zoom_local: f32,
    /// World xy at the centre of the view when not following.
    #[serde(skip)]
    pub pan: Vec2,
    #[serde(skip)]
    pub place: String,
}

impl Default for State {
    fn default() -> Self {
        State {
            tab: Tab::Local,
            search: String::new(),
            kind: 0,
            follow: true,
            zoom_world: 1.0,
            zoom_local: 1.0,
            pan: Vec2::ZERO,
            place: String::new(),
        }
    }
}

/// A map texture on the GPU with the image's world transform.
pub struct MapTexture {
    pub handle: egui::TextureHandle,
    pub origin: Vec2,
    pub size: Vec2,
    pub scale: f32,
}

impl MapTexture {
    pub fn upload(egui: &egui::Context, name: &str, img: &MapImage) -> MapTexture {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [img.width as usize, img.height as usize],
            &img.rgba,
        );
        MapTexture {
            handle: egui.load_texture(name, image, egui::TextureOptions::LINEAR),
            origin: img.origin,
            size: img.size(),
            scale: img.scale,
        }
    }
}

/// Draw one tab's map into `rect`: the image, the route, the objects
/// and the character. Returns the world xy under the pointer.
#[allow(clippy::too_many_arguments)]
fn draw_map(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    tex: Option<&MapTexture>,
    v: &MapView,
    st: &mut State,
    shown: &[usize],
    world_tab: bool,
    actions: &mut Actions,
) -> Option<Vec2> {
    let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(200));
    let zoom = if world_tab {
        &mut st.zoom_world
    } else {
        &mut st.zoom_local
    };
    // Wheel zooms around the view centre.
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            *zoom = (*zoom * (1.0 + scroll * 0.005)).clamp(0.05, 40.0);
        }
    }
    let image_scale = tex.map(|t| t.scale).unwrap_or(2.0);
    let s = image_scale * *zoom;
    if resp.dragged() {
        st.follow = false;
        let d = resp.drag_delta();
        st.pan = if st.pan == Vec2::ZERO { v.me } else { st.pan };
        st.pan.x -= d.x / s;
        st.pan.y += d.y / s;
    }
    let center = if st.follow || st.pan == Vec2::ZERO {
        v.me
    } else {
        st.pan
    };
    if let Some(t) = tex {
        let nw = to_screen(rect, center, s, t.origin + Vec2::new(0.0, t.size.y));
        let se = to_screen(rect, center, s, t.origin + Vec2::new(t.size.x, 0.0));
        painter.image(
            t.handle.id(),
            egui::Rect::from_min_max(nw, se),
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "rendering map...",
            egui::FontId::proportional(14.0),
            egui::Color32::from_gray(180),
        );
    }
    // Town names on the world map.
    if world_tab {
        for (name, p) in &v.places {
            let sp = to_screen(rect, center, s, *p);
            if rect.contains(sp) {
                painter.circle_filled(sp, 2.5, egui::Color32::from_rgb(255, 235, 170));
                if s > 0.02 {
                    painter.text(
                        sp + egui::vec2(4.0, -2.0),
                        egui::Align2::LEFT_BOTTOM,
                        *name,
                        egui::FontId::proportional(11.0),
                        egui::Color32::from_rgb(255, 235, 170),
                    );
                }
            }
        }
    }
    // The route.
    if v.route.len() > 1 {
        let pts: Vec<egui::Pos2> = v
            .route
            .iter()
            .map(|p| to_screen(rect, center, s, *p))
            .collect();
        painter.add(egui::Shape::line(
            pts,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 200, 60)),
        ));
    }
    // Objects, nearest drawn last (on top).
    for &i in shown.iter().rev() {
        let o = &v.objects[i];
        let sp = to_screen(rect, center, s, o.pos);
        if !rect.contains(sp) {
            continue;
        }
        let r = if o.selected { 5.0 } else { 3.0 };
        painter.circle_filled(sp, r, o.kind.color());
        if o.selected {
            painter.circle_stroke(sp, 8.0, egui::Stroke::new(1.5, egui::Color32::WHITE));
        }
    }
    // The character: a triangle pointing along the heading.
    let me = to_screen(rect, center, s, v.me);
    let fwd = egui::vec2(-v.heading.sin(), -v.heading.cos());
    let side = egui::vec2(-fwd.y, fwd.x);
    painter.add(egui::Shape::convex_polygon(
        vec![
            me + fwd * 9.0,
            me - fwd * 5.0 + side * 5.0,
            me - fwd * 5.0 - side * 5.0,
        ],
        egui::Color32::WHITE,
        egui::Stroke::new(1.0, egui::Color32::BLACK),
    ));
    // Pointer: coordinates, and a double-click on the world map travels.
    let hover = resp.hover_pos().map(|p| to_world(rect, center, s, p));
    if world_tab && resp.double_clicked() {
        if let Some(w) = hover {
            actions.travel_to = Some(w);
        }
    }
    hover
}

/// Draw the panel.
pub fn draw(
    egui: &egui::Context,
    v: &MapView,
    st: &mut State,
    world: Option<&MapTexture>,
    local: Option<&MapTexture>,
) -> Actions {
    let mut actions = Actions::default();
    let vp = egui.viewport_rect();
    let size = egui::vec2(700.0, 460.0);
    window(
        "map",
        egui::pos2(vp.width() * 0.5 - size.x * 0.5, 60.0),
        size,
        200,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_size(size - egui::vec2(12.0, 12.0));
        ui.horizontal(|ui| {
            title(ui, "Map");
            ui.selectable_value(&mut st.tab, Tab::World, "World");
            ui.selectable_value(
                &mut st.tab,
                Tab::Local,
                if v.dungeon { "Dungeon" } else { "Local" },
            );
            ui.checkbox(&mut st.follow, "follow");
            caption(ui, format!("{}  block {:#06x}", v.coords, v.block >> 16));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some((next, n)) = v.travel {
                    if ui.small_button("Cancel").clicked() {
                        actions.cancel_travel = true;
                    }
                    caption(ui, format!("travelling: waypoint {next} of {n}"));
                } else {
                    let go = ui.small_button("Go");
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut st.place)
                            .hint_text("travel to (Arwic, 33.6N 56.6E)")
                            .desired_width(160.0),
                    );
                    let entered =
                        edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (go.clicked() || entered) && !st.place.trim().is_empty() {
                        actions.travel_to_place = Some(st.place.trim().to_string());
                    }
                }
            });
        });
        ui.horizontal_top(|ui| {
            let (_, map_rect) = ui.allocate_space(egui::vec2(440.0, 410.0));
            let shown = shown(&v.objects, &st.search, st.kind);
            let world_tab = st.tab == Tab::World;
            let tex = if world_tab { world } else { local };
            let hover = draw_map(ui, map_rect, tex, v, st, &shown, world_tab, &mut actions);
            ui.vertical(|ui| {
                ui.set_width(230.0);
                ui.add(
                    egui::TextEdit::singleline(&mut st.search)
                        .hint_text("find in landblock")
                        .desired_width(220.0),
                );
                ui.horizontal_wrapped(|ui| {
                    for (i, (label, _)) in KINDS.iter().enumerate() {
                        if ui.selectable_label(st.kind == i, *label).clicked() {
                            st.kind = i;
                        }
                    }
                });
                caption(
                    ui,
                    format!("{} of {} objects", shown.len(), v.objects.len()),
                );
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for i in &shown {
                            let o = &v.objects[*i];
                            let text = format!("{}  {:.0} m", o.name, o.distance);
                            let row = ui.add(
                                egui::Label::new(egui::RichText::new(text).color(o.kind.color()))
                                    .sense(egui::Sense::click())
                                    .selectable(false),
                            );
                            let row = row.on_hover_text(format!(
                                "{}  {}",
                                o.kind.label(),
                                coords_of(o.pos)
                            ));
                            if row.clicked() {
                                actions.select = Some(o.guid);
                            }
                            if row.double_clicked() {
                                actions.activate = Some(o.guid);
                            }
                        }
                    });
                if let Some(w) = hover {
                    caption(ui, format!("pointer: {}", coords_of(w)));
                }
                caption(
                    ui,
                    "drag pans, wheel zooms; world map: double-click to travel",
                );
            });
        });
    });
    actions
}

/// The cached world map at 8 px per landblock.
fn render_world_map(assets: &ac_scene::Assets) -> Result<MapImage, String> {
    ac_scene::worldmap::cached(assets, &ac_scene::worldgrid::WorldGrid::cache_dir(), 8)
        .map_err(|e| e.to_string())
}

/// A background render of the world map.
type WorldRender = Receiver<Result<MapImage, String>>;

pub struct Map {
    source: Source<MapView>,
    pub show: bool,
    pub state: State,
    world: Option<MapTexture>,
    world_rx: Option<WorldRender>,
    /// (block, storey band) of the local texture.
    local: Option<(u32, i32, MapTexture)>,
    local_failed: Option<u32>,
}

impl Default for Map {
    fn default() -> Self {
        Map {
            source: Source::Live,
            show: false,
            state: State::default(),
            world: None,
            world_rx: None,
            local: None,
            local_failed: None,
        }
    }
}

impl Map {
    pub fn demo() -> Self {
        let me = Vec2::new(0xA9 as f32 * 192.0 + 90.0, 0xB4 as f32 * 192.0 + 100.0);
        let obj = |guid: u32, name: &str, kind: Kind, dx: f32, dy: f32| MapObject {
            guid,
            name: name.into(),
            kind,
            pos: me + Vec2::new(dx, dy),
            z: 94.0,
            distance: (dx * dx + dy * dy).sqrt(),
            selected: guid == 2,
        };
        Map {
            source: Source::Demo(MapView {
                block: 0xA9B4_0000,
                dungeon: false,
                me,
                me_z: 94.0,
                heading: 0.6,
                coords: "42.1N, 33.6E".into(),
                objects: vec![
                    obj(1, "Reborn", Kind::Player, 12.0, 8.0),
                    obj(2, "Samuel the Blacksmith", Kind::Npc, -20.0, 30.0),
                    obj(3, "Drudge Skulker", Kind::Monster, 40.0, -35.0),
                    obj(4, "Holtburg Town Portal", Kind::Portal, -60.0, -50.0),
                    obj(5, "Dagger", Kind::Item, 5.0, -3.0),
                ],
                route: vec![me, me + Vec2::new(50.0, 80.0), me + Vec2::new(140.0, 120.0)],
                travel: Some((1, 3)),
                places: vec![("Holtburg", me + Vec2::new(-10.0, 20.0))],
            }),
            show: true,
            state: State::default(),
            world: None,
            world_rx: None,
            local: None,
            local_failed: None,
        }
    }

    /// Kick off the world render on another thread the first time.
    fn ensure_world(&mut self, data_dir: std::path::PathBuf) {
        if self.world.is_some() || self.world_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let result = ac_scene::Assets::open(&data_dir)
                .map_err(|e| e.to_string())
                .and_then(|assets| render_world_map(&assets));
            tracing::info!("world map ready in {:.1?}", started.elapsed());
            let _ = tx.send(result);
        });
        self.world_rx = Some(rx);
    }

    fn poll_world(&mut self, egui: &egui::Context) {
        let Some(rx) = &self.world_rx else { return };
        match rx.try_recv() {
            Ok(Ok(img)) => {
                self.world = Some(MapTexture::upload(egui, "world_map", &img));
                self.world_rx = None;
            }
            Ok(Err(e)) => {
                tracing::warn!("world map: {e}");
                self.world_rx = None;
            }
            Err(_) => {}
        }
    }

    /// Render the local map when the block or the storey changes.
    fn ensure_local(&mut self, egui: &egui::Context, c: &Client, v: &MapView) {
        let band = if v.dungeon {
            (v.me_z / 6.0).floor() as i32
        } else {
            0
        };
        if self
            .local
            .as_ref()
            .is_some_and(|(b, z, _)| *b == v.block && *z == band)
            || self.local_failed == Some(v.block)
        {
            return;
        }
        let z_range = v
            .dungeon
            .then_some((band as f32 * 6.0 - 3.0, band as f32 * 6.0 + 9.0));
        match ac_scene::localmap::render(&c.assets, v.block, 2.0, z_range) {
            Ok(m) => {
                self.local = Some((
                    v.block,
                    band,
                    MapTexture::upload(egui, "local_map", &m.image),
                ));
                self.local_failed = None;
            }
            Err(e) => {
                tracing::warn!("local map {:#010x}: {e}", v.block);
                self.local_failed = Some(v.block);
            }
        }
    }
}

impl Plugin for Map {
    fn name(&self) -> &str {
        "map"
    }

    fn load(&mut self, settings: &Settings) {
        if let Some(v) = settings.get("map.show") {
            self.show = v;
        }
        if let Some(v) = settings.get::<State>("map.state") {
            self.state = v;
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("map.show", self.show);
        settings.set("map.state", &self.state);
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if !self.show {
            return;
        }
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().and_then(|c| view(c)),
        };
        let Some(v) = v else { return };
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            let dir = c.assets.data_dir.clone();
            self.ensure_world(dir);
            self.poll_world(egui);
            self.ensure_local(egui, c, &v);
        }
        let actions = draw(
            egui,
            &v,
            &mut self.state,
            self.world.as_ref(),
            self.local.as_ref().map(|(_, _, t)| t),
        );
        let mut lines = Vec::new();
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            if let Some(g) = actions.select {
                c.select(Some(g));
                c.appraise(g);
            }
            if let Some(g) = actions.activate {
                c.interact(g);
            }
            if let Some(w) = actions.travel_to {
                if c.travel_to(w) {
                    lines.push(format!("travelling to {}", coords_of(w)));
                } else {
                    lines.push(format!("no route to {}", coords_of(w)));
                }
            }
            if let Some(name) = actions.travel_to_place {
                match c.travel_to_place(&name) {
                    Ok(()) => lines.push(format!("travelling to {name}")),
                    Err(e) => lines.push(e),
                }
                self.state.place.clear();
            }
            if actions.cancel_travel {
                c.cancel_travel();
            }
        }
        for l in lines {
            cx.log(l);
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if key == egui::Key::M && pressed {
            self.show = !self.show;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_objects_and_converts_coordinates() {
        let v = match Map::demo().source {
            Source::Demo(v) => v,
            Source::Live => unreachable!(),
        };
        assert_eq!(shown(&v.objects, "", 0).len(), 5);
        let npcs = shown(&v.objects, "", 2);
        assert_eq!(npcs.len(), 1);
        assert_eq!(v.objects[npcs[0]].name, "Samuel the Blacksmith");
        let drudge = shown(&v.objects, "drudge", 0);
        assert_eq!(v.objects[drudge[0]].kind, Kind::Monster);
        assert_eq!(shown(&v.objects, "portal", 0).len(), 1);
        // Holtburg's coordinates from its world position.
        let s = coords_of(Vec2::new(
            (33.6 + 102.0) * 240.0 + 12.0,
            (42.1 + 102.0) * 240.0 + 12.0,
        ));
        assert_eq!(s, "42.1N, 33.6E");
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 200.0));
        let center = Vec2::new(1000.0, 2000.0);
        let p = to_screen(rect, center, 2.0, Vec2::new(1010.0, 2020.0));
        assert_eq!(p, egui::pos2(120.0, 60.0));
        let back = to_world(rect, center, 2.0, p);
        assert!((back - Vec2::new(1010.0, 2020.0)).length() < 1e-4);
    }

    #[test]
    fn kinds_from_objects() {
        let mut o = ac_world::WorldObject::default();
        assert_eq!(Kind::of(&o), Kind::Item);
        o.object_desc_flags = ac_world::object_desc_flags::PLAYER;
        assert_eq!(Kind::of(&o), Kind::Player);
        o.object_desc_flags = ac_world::object_desc_flags::ATTACKABLE;
        o.item_type = ac_world::item_type::CREATURE;
        assert_eq!(Kind::of(&o), Kind::Monster);
        o.object_desc_flags = ac_world::object_desc_flags::VENDOR;
        assert_eq!(Kind::of(&o), Kind::Npc);
        o.object_desc_flags = ac_world::object_desc_flags::PORTAL;
        assert_eq!(Kind::of(&o), Kind::Portal);
    }
}
