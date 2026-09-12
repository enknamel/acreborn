//! Map (bindable from the menu): the world map of Dereth or the current landblock (a
//! dungeon's floor plan indoors), with the character, everything the
//! server has sent for the landblock (players, NPCs, monsters, items,
//! portals, doors) as dots, and the overland route being walked.
//!
//! * Tabs switch between World and Local. Drag pans, the wheel zooms,
//!   "follow" keeps the character centred. Hovering shows the map
//!   coordinates under the pointer.
//! * The list on the right is what is worth walking to: people, ways
//!   out, anything that can be picked up and anything fixed that can be
//!   used (a lifestone, a forge, a chest, a door). The scenery the
//!   server also places, fixed and lifeless, is left out. A search box
//!   and kind chips narrow it, a click selects the object (and rings it
//!   on the map), a double-click uses it. The same search also looks
//!   through the whole world: its towns, the lifestones, shops and
//!   named folk of the gazetteer (`ac_world::landmarks`), and the mouth
//!   of every portal standing out in the open (`ac_world::portals`).
//!   Portals matter most of the three -- almost every dungeon in Dereth
//!   is a name on a portal and nothing else on the map -- and one
//!   answer is given per name, because fifty-three lines reading
//!   "Portal to Town Network" are no answer at all. They appear under
//!   "elsewhere in the world" with how far away they are, and a click
//!   travels there.
//! * On the world map a double-click asks for a route there and the
//!   character walks it (see `ac_client::Client::travel_to`); a place
//!   name typed into "travel to" does the same by the gazetteer. The
//!   route is drawn as a line; Cancel stops it.
//!
//! The world map is rendered once from the terrain grid on a background
//! thread (the first time takes a few seconds while the grid is read
//! from the cell archive; both are cached under `~/.cache/acswarm`).
//! The local map covers the landblocks around the character, seven
//! across by default and up to eleven, and is drawn on a background
//! thread when the character moves to another landblock (or, in a
//! dungeon, another storey). Beyond the block they stand in only things
//! a few metres across are drawn, so towns and walls show through
//! instead of a thicket of trees. Forty-nine landblocks take about a
//! tenth of a second and thirty megabytes of pixels.

use super::{caption, has_sheet, title_bar, window, Source};
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

/// Whether an object is worth a line in the list: someone to talk to or
/// fight, a way out, something to pick up, or something to use. What is
/// left out is the scenery the server also sends: statues, furniture,
/// fences and the like, which are fixed, lifeless and do nothing.
pub fn worth_listing(o: &ac_world::WorldObject) -> bool {
    use ac_world::{item_type, object_desc_flags as f};
    let d = o.object_desc_flags;
    if d & (f::PLAYER | f::CORPSE | f::PORTAL | f::VENDOR) != 0 {
        return true;
    }
    if o.item_type & (item_type::CREATURE | item_type::PORTAL) != 0 || o.motion_table_id != 0 {
        return true;
    }
    // Anything that can be carried off.
    if d & f::STUCK == 0 {
        return true;
    }
    // Fixed, but there is something to do with it: a lifestone, a forge,
    // a chest, a door, a sign worth reading. `Usable::No` is 1.
    o.usable != 0 && o.usable != 1
}

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
    /// What the search found elsewhere in the world: (label, position,
    /// metres away). Towns, lifestones, shops and standing NPCs from
    /// `ac_world::landmarks` and the gazetteer, nearest first.
    pub elsewhere: Vec<(String, Vec2, f32)>,
}

/// The towns, landmarks and portals matching `search`, nearest to `me`
/// first.
///
/// Three sources, because a player looking for somewhere means any of
/// them: a town by name, a standing thing (lifestones, shopkeepers and
/// the named folk the gazetteer knows), and the mouth of a portal.
///
/// Portals are what a search is most often really after -- almost
/// every dungeon in Dereth is a name on a portal and nothing else on
/// the map -- and they were the one source missing. Only the ones
/// standing out in the world are offered: a portal at the bottom of a
/// dungeon is not somewhere to walk to from here, and the data is
/// mostly such portals. The ruined ones say so themselves and are left
/// out.
pub fn world_search(search: &str, me: Vec2) -> Vec<(String, Vec2, f32)> {
    let needle = search.trim().to_lowercase();
    if needle.len() < 3 {
        return Vec::new();
    }
    let mut out: Vec<(String, Vec2, f32)> = ac_world::towns::PLACES
        .iter()
        .filter(|p| p.name.to_lowercase().contains(&needle))
        .map(|p| (format!("{} (town)", p.name), p.world_xy(), 0.0))
        .collect();
    // One per name here too. The gazetteer holds five hundred and
    // twenty-two Wailing Statues and a hundred and sixty-two Statues;
    // a search that answers with all of them has answered with none,
    // and the nearest of each is the one worth walking to.
    let mut seen: Vec<String> = Vec::new();
    for l in ac_world::landmarks::search(&needle, Some(me)) {
        if seen.iter().any(|n| n == &l.name) {
            continue;
        }
        seen.push(l.name.clone());
        out.push((format!("{} ({})", l.name, l.kind.label()), l.xy(), 0.0));
        if seen.len() >= 40 {
            break;
        }
    }
    out.extend(portal_search(&needle, me));
    for e in &mut out {
        e.2 = e.1.distance(me);
    }
    out.sort_by(|a, b| a.2.total_cmp(&b.2));
    out.truncate(20);
    out
}

/// The mouths of portals matching `needle`, nearest first and one per
/// name.
///
/// One per name matters: fifty-three portals are called "Portal to Town
/// Network" and a list of fifty-three identical lines is a list of
/// none. The nearest of each is the one worth walking to anyway.
fn portal_search(needle: &str, me: Vec2) -> Vec<(String, Vec2, f32)> {
    let mut found: Vec<&ac_world::portals::Portal> = ac_world::portals::named(needle)
        .into_iter()
        .filter(|p| p.works() && p.mouth_outdoors())
        .collect();
    found.sort_by(|a, b| {
        a.from_xy()
            .distance(me)
            .total_cmp(&b.from_xy().distance(me))
    });
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for p in found {
        if seen.contains(&p.name.as_str()) {
            continue;
        }
        seen.push(&p.name);
        out.push((format!("{} (portal)", p.name), p.from_xy(), 0.0));
        if out.len() >= 20 {
            break;
        }
    }
    out
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
        .filter(|o| worth_listing(o))
        .filter_map(|o| {
            // Everything worth listing that the server has told us
            // about, not only the landblock the character stands in: the
            // list is sorted by distance and the map covers the blocks
            // around them.
            let pos = o.display.or(o.position)?;
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
        elsewhere: Vec::new(),
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
    /// How many landblocks either side of the character the local map
    /// covers: 0 is the one they stand in, 3 the forty-nine around them.
    pub local_radius: u32,
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
            local_radius: 3,
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
        title_bar(ui, "map", "Map");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut st.tab, Tab::World, "World");
            ui.selectable_value(
                &mut st.tab,
                Tab::Local,
                if v.dungeon { "Dungeon" } else { "Local" },
            );
            ui.checkbox(&mut st.follow, "follow");
            if st.tab == Tab::Local && !v.dungeon {
                caption(ui, "area");
                egui::ComboBox::from_id_salt("local_radius")
                    .selected_text(format!("{} blocks", st.local_radius * 2 + 1))
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for r in [0u32, 1, 2, 3, 5] {
                            let label = if r == 0 {
                                "1 block".to_string()
                            } else {
                                format!("{} blocks", r * 2 + 1)
                            };
                            ui.selectable_value(&mut st.local_radius, r, label);
                        }
                    });
            }
            caption(ui, format!("{}  block {:#06x}", v.coords, v.block >> 16));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some((next, n)) = v.travel {
                    if ui.small_button("Cancel").clicked() {
                        actions.cancel_travel = true;
                    }
                    caption(ui, format!("travelling: step {next} of {n}"));
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
                        .hint_text("find here or anywhere")
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
                if !v.elsewhere.is_empty() {
                    caption(ui, "elsewhere in the world");
                    egui::ScrollArea::vertical()
                        .id_salt("map_elsewhere")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for (label, at, away) in &v.elsewhere {
                                let text = if *away >= 1000.0 {
                                    format!("{label}  {:.1} km", away / 1000.0)
                                } else {
                                    format!("{label}  {away:.0} m")
                                };
                                let row = ui
                                    .add(
                                        egui::Label::new(
                                            egui::RichText::new(text)
                                                .color(egui::Color32::from_rgb(255, 235, 170)),
                                        )
                                        .sense(egui::Sense::click())
                                        .selectable(false),
                                    )
                                    .on_hover_text(format!(
                                        "{}  (click to travel)",
                                        coords_of(*at)
                                    ));
                                if row.clicked() {
                                    actions.travel_to = Some(*at);
                                }
                            }
                        });
                }
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
    /// (block, storey band, radius) of the local texture, and the one
    /// being drawn now.
    local: Option<((u32, i32, u32), MapTexture)>,
    local_want: Option<(u32, i32, u32)>,
    local_rx: Option<Receiver<Result<MapImage, String>>>,
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
            local_want: None,
            local_rx: None,
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
                elsewhere: vec![
                    (
                        "Arwic (town)".into(),
                        me + Vec2::new(5600.0, -2000.0),
                        5950.0,
                    ),
                    (
                        "Aun Ralirea (npc)".into(),
                        me + Vec2::new(300.0, 40.0),
                        302.0,
                    ),
                ],
            }),
            show: true,
            state: State::default(),
            world: None,
            world_rx: None,
            local: None,
            local_want: None,
            local_rx: None,
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
    /// Draw the landblocks around the character, on another thread: a
    /// wide area is tens of megabytes of pixels and a tenth of a second
    /// of work, which is not something to do on the frame the map is
    /// opened. The old picture stays up until the new one is ready.
    fn ensure_local(&mut self, c: &Client, v: &MapView, radius: u32) {
        let band = if v.dungeon {
            (v.me_z / 6.0).floor() as i32
        } else {
            0
        };
        // A dungeon is a landblock to itself, so it is never widened.
        let radius = if v.dungeon { 0 } else { radius };
        let want = (v.block, band, radius);
        if self.local.as_ref().is_some_and(|(w, _)| *w == want)
            || self.local_want == Some(want)
            || self.local_failed == Some(v.block)
        {
            return;
        }
        let z_range = v
            .dungeon
            .then_some((band as f32 * 6.0 - 3.0, band as f32 * 6.0 + 9.0));
        let dir = c.assets.data_dir.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let result = ac_scene::Assets::open(&dir)
                .map_err(|e| e.to_string())
                .and_then(|assets| {
                    ac_scene::localmap::render_area(&assets, want.0, radius, 2.0, z_range)
                        .map_err(|e| e.to_string())
                });
            tracing::info!(
                "local map {:#010x} (radius {radius}) in {:.0?}",
                want.0,
                started.elapsed()
            );
            let _ = tx.send(result.map(|m| m.image));
        });
        self.local_want = Some(want);
        self.local_rx = Some(rx);
    }

    fn poll_local(&mut self, egui: &egui::Context) {
        let Some(rx) = &self.local_rx else { return };
        match rx.try_recv() {
            Ok(Ok(img)) => {
                if let Some(want) = self.local_want.take() {
                    self.local = Some((want, MapTexture::upload(egui, "local_map", &img)));
                }
                self.local_rx = None;
                self.local_failed = None;
            }
            Ok(Err(e)) => {
                let block = self.local_want.take().map(|w| w.0);
                tracing::warn!("local map {block:?}: {e}");
                self.local_failed = block;
                self.local_rx = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.local_want = None;
                self.local_rx = None;
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
        if let Some(ask) = super::take_open(cx.board, "map") {
            self.show = ask.apply(self.show);
        }
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
            self.poll_local(egui);
            let radius = self.state.local_radius;
            self.ensure_local(c, &v, radius);
        }
        let mut v = v;
        v.elsewhere = world_search(&self.state.search, v.me);
        let actions = draw(
            egui,
            &v,
            &mut self.state,
            self.world.as_ref(),
            self.local.as_ref().map(|(_, t)| t),
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
        if super::closed("map") {
            self.show = false;
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if pressed && crate::keys::bound("map", key) {
            self.show = !self.show;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;

    /// Holtburg, in world coordinates, as somewhere to search from.
    fn holtburg() -> Vec2 {
        ac_world::towns::PLACES
            .iter()
            .find(|p| p.name == "Holtburg")
            .map(|p| p.world_xy())
            .expect("Holtburg is a town")
    }

    #[test]
    fn a_dungeon_is_found_by_the_portal_that_leads_to_it() {
        // Almost every dungeon in Dereth is a name on a portal and
        // nothing else on the map, which is why the search was missing
        // most of the world before portals were in it.
        let found = world_search("halls of metos", holtburg());
        assert!(
            found.iter().any(|(label, _, _)| label.contains("(portal)")),
            "{found:?}"
        );
    }

    #[test]
    fn the_same_portal_name_is_offered_once_not_fifty_three_times() {
        let found = world_search("town network", holtburg());
        let n = found
            .iter()
            .filter(|(l, _, _)| l.starts_with("Portal to Town Network"))
            .count();
        assert_eq!(n, 1, "{found:?}");
    }

    #[test]
    fn a_portal_at_the_bottom_of_a_dungeon_is_not_somewhere_to_walk_to() {
        // "Surface" is the commonest portal name in the data and every
        // one of them stands underground.
        let found = world_search("surface", holtburg());
        assert!(
            !found.iter().any(|(l, _, _)| l.starts_with("Surface")),
            "{found:?}"
        );
    }

    #[test]
    fn towns_and_the_people_in_them_are_still_found() {
        let town = world_search("holtburg", holtburg());
        assert!(
            town.iter().any(|(l, _, _)| l.contains("(town)")),
            "{town:?}"
        );
        // The gazetteer's named folk and shopkeepers were already
        // searchable and must stay so.
        let folk = world_search("ulgrim", holtburg());
        assert!(!folk.is_empty(), "nobody found");
    }

    #[test]
    fn one_of_each_name_not_five_hundred_of_one() {
        // The gazetteer is full of scenery that shares a name.
        let found = world_search("wailing statue", holtburg());
        let n = found
            .iter()
            .filter(|(l, _, _)| l.starts_with("Wailing Statue"))
            .count();
        assert_eq!(n, 1, "{found:?}");
    }

    #[test]
    fn the_nearest_answer_comes_first() {
        let from = holtburg();
        let found = world_search("portal", from);
        for pair in found.windows(2) {
            assert!(pair[0].2 <= pair[1].2, "{found:?}");
        }
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
    fn only_things_worth_a_line_are_listed() {
        use ac_world::{item_type, object_desc_flags as f};
        let scenery = |usable: u32| ac_world::WorldObject {
            object_desc_flags: f::STUCK,
            usable,
            ..Default::default()
        };
        // A statue: fixed, lifeless, nothing to do with it.
        assert!(!worth_listing(&scenery(0)));
        // `Usable::No` says as much outright.
        assert!(!worth_listing(&scenery(1)));
        // A lifestone or a forge is fixed but there is something to do.
        assert!(worth_listing(&scenery(8)));
        // Anything that can be picked up.
        assert!(worth_listing(&ac_world::WorldObject::default()));
        // People, corpses and ways out.
        for flag in [f::PLAYER, f::CORPSE, f::PORTAL, f::VENDOR] {
            assert!(worth_listing(&ac_world::WorldObject {
                object_desc_flags: f::STUCK | flag,
                ..Default::default()
            }));
        }
        assert!(worth_listing(&ac_world::WorldObject {
            object_desc_flags: f::STUCK,
            item_type: item_type::CREATURE,
            ..Default::default()
        }));
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
