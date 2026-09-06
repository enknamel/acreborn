//! The client's built-in panels, each a [`Plugin`]: vitals, radar, target
//! bar, inventory, loot, vendor, skills, spellbook, spell bar, components
//! and buffs. They double as examples of a UI plugin and can be replaced
//! by registering something else in their place.
//!
//! Every panel follows the same shape:
//!
//! * a **view**: a plain struct of what the panel draws, built from the
//!   active session each frame (`view(&Client)`), so the drawing code never
//!   touches the client;
//! * a **draw** function: egui code that paints the view and returns what
//!   the person clicked (guids to take, buy, cast...);
//! * the `Plugin` impl: `ui` builds the view, draws it, and turns the
//!   clicks into `Client` calls (`interact`, `take`, `buy`, `cast`...);
//!   `key` handles the panel's toggle (I inventory, K skills, P spellbook,
//!   B spell bar, O components, U buffs).
//!
//! A panel's data comes from a [`Source`]: `Live` reads the session, `Demo`
//! holds a canned view for the offline `--demo-ui` screenshot (clicks are
//! dropped, there is no session to send them to). [`demo`] builds the demo
//! set; [`live`] the real one.

pub mod allegiance;
pub mod appraisal;
pub mod book;
pub mod buffs;
pub mod combat;
pub mod components;
pub mod confirm;
pub mod fellowship;
pub mod housing;
pub mod inventory;
pub mod loot;
pub mod map;
pub mod nameplates;
pub mod options;
pub mod radar;
pub mod salvage;
pub mod skills;
pub mod social;
pub mod spellbar;
pub mod spellbook;
pub mod target;
pub mod trade;
pub mod vendor;
pub mod vitals;

use crate::icons::{IconCache, IconLayers};
use crate::{egui, Client, Plugin};

/// Where a panel gets what it draws.
#[derive(Default)]
pub enum Source<T> {
    /// The active session, read every frame.
    #[default]
    Live,
    /// Canned data; actions are dropped.
    Demo(T),
}

impl<T> Source<T> {
    /// The demo view, if this is one.
    pub fn demo(&self) -> Option<&T> {
        match self {
            Source::Live => None,
            Source::Demo(d) => Some(d),
        }
    }
}

/// A spell being dragged from the spellbook onto a spell bar or one of
/// its tabs (an egui drag-and-drop payload).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpellDrag(pub u32);

/// `5400` -> `1h 30m`, `754` -> `12m 34s`, `45` -> `45s`. Negative
/// values read as zero.
pub fn fmt_seconds(secs: f64) -> String {
    let total = secs.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

/// The character sheet has arrived (the server sent our stats), which is
/// when the vitals, radar and inventory appear.
pub fn has_sheet(c: &Client) -> bool {
    !c.world.stats.name.is_empty()
}

/// An item line of the inventory or loot panels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub guid: u32,
    pub name: String,
    pub stack: u32,
    pub wielded: bool,
    /// A side pack: other items can be dropped into it.
    pub container: bool,
    pub icon: IconLayers,
    pub wcid: u32,
    /// Largest stack of this item; above 1, another stack of the same
    /// kind dropped on it merges.
    pub max_stack: u32,
}

/// A carried item being dragged (egui drag-and-drop payload). Drop it on
/// a pack row or the "Pack" header to move it, on the target bar to give
/// it to the selected creature, on the loot window to put it in the open
/// chest, or on the world to drop it (over an NPC or player: give).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemDrag(pub u32);

impl Item {
    pub fn of(o: &ac_world::WorldObject, wielded: bool) -> Self {
        Item {
            guid: o.guid,
            name: o.name.clone(),
            // A salvage bag counts its units the way a stack does.
            stack: if o.name.starts_with("Salvaged ") && o.structure > 0 {
                o.structure
            } else {
                o.stack_size
            },
            wielded,
            container: o.item_type & ac_world::item_type::CONTAINER != 0,
            icon: IconLayers::of(o),
            wcid: o.weenie_class_id,
            max_stack: o.max_stack_size,
        }
    }

    /// `Name` or `Name (stack)`.
    pub fn label(&self) -> String {
        item_label(&self.name, self.stack)
    }
}

/// `Pyreal (12)` for stacks, just the name otherwise.
pub fn item_label(name: &str, stack: u32) -> String {
    if stack > 1 {
        format!("{name} ({stack})")
    } else {
        name.to_string()
    }
}

/// The translucent black frame every panel sits in.
pub fn frame(alpha: u8, margin: i8) -> egui::Frame {
    egui::Frame::new()
        .fill(egui::Color32::from_black_alpha(alpha))
        .inner_margin(margin)
}

/// A borderless window of a fixed size that opens at `pos` and can be
/// dragged anywhere inside the viewport: egui remembers where it was
/// moved for the rest of the session, and "Reset window layout" in the
/// options panel (X) puts every panel back at its default. Without a
/// title bar egui moves the window from a drag on any part of it that no
/// widget claims, so item rows, spell rows and scroll areas keep their
/// own drags (an item row starts an [`ItemDrag`], not a move).
pub fn window(
    name: &str,
    pos: egui::Pos2,
    size: egui::Vec2,
    alpha: u8,
    margin: i8,
) -> egui::Window<'static> {
    egui::Window::new(name.to_string())
        // An explicit id: a window's own id is derived from its title in
        // a way `Id::new(name)` does not reproduce, and `positions` looks
        // the panels up by name.
        .id(egui::Id::new(name.to_string()))
        .fade_in(false)
        .title_bar(false)
        .resizable(false)
        .movable(true)
        .constrain(true)
        .frame(frame(alpha, margin))
        .default_pos(remembered(name, pos))
        .fixed_size(size)
}

/// A bare movable area (the vitals, radar, target bar and combat bar,
/// which paint themselves rather than sitting in a panel frame). Its
/// position is remembered like a window's.
pub fn area(name: &str, pos: egui::Pos2) -> egui::Area {
    egui::Area::new(egui::Id::new(name.to_string()))
        .fade_in(false)
        .movable(true)
        .constrain(true)
        .default_pos(remembered(name, pos))
}

/// Where the panels were when the settings were written, and the names
/// of every panel drawn this run.
static LAYOUT: std::sync::Mutex<Option<Layout>> = std::sync::Mutex::new(None);

#[derive(Default)]
struct Layout {
    saved: std::collections::BTreeMap<String, [f32; 2]>,
    seen: std::collections::BTreeSet<String>,
}

/// Note the panel and answer with its saved position, or `fallback` when
/// it has none.
fn remembered(name: &str, fallback: egui::Pos2) -> egui::Pos2 {
    let Ok(mut guard) = LAYOUT.lock() else {
        return fallback;
    };
    let layout = guard.get_or_insert_with(Layout::default);
    if !layout.seen.contains(name) {
        layout.seen.insert(name.to_string());
    }
    match layout.saved.get(name) {
        Some([x, y]) => egui::pos2(*x, *y),
        None => fallback,
    }
}

/// Take the saved positions out of the settings (see
/// [`crate::Host::load_settings`]). Call before the first frame: a panel
/// only takes its saved position the first time it is drawn.
pub fn restore_positions(saved: std::collections::BTreeMap<String, [f32; 2]>) {
    if let Ok(mut guard) = LAYOUT.lock() {
        guard.get_or_insert_with(Layout::default).saved = saved;
    }
}

/// Where every panel drawn this run sits now, for the settings file.
pub fn positions(egui: &egui::Context) -> std::collections::BTreeMap<String, [f32; 2]> {
    let Ok(guard) = LAYOUT.lock() else {
        return Default::default();
    };
    let Some(layout) = guard.as_ref() else {
        return Default::default();
    };
    layout
        .seen
        .iter()
        .filter_map(|name| {
            let rect = egui.memory(|m| m.area_rect(egui::Id::new(name.clone())))?;
            Some((name.clone(), [rect.min.x, rect.min.y]))
        })
        .collect()
}

/// Forget the saved positions so the panels fall back to their built-in
/// places (the options panel's "Reset window layout", together with
/// egui's `reset_areas`).
pub fn forget_positions() {
    if let Ok(mut guard) = LAYOUT.lock() {
        guard.get_or_insert_with(Layout::default).saved.clear();
    }
}

/// A dim small caption ("Worn", "Level 3", column headers).
pub fn caption(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(
        egui::RichText::new(text.into())
            .color(egui::Color32::from_gray(170))
            .small(),
    );
}

/// A bold white title line.
pub fn title(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(
        egui::RichText::new(text.into())
            .color(egui::Color32::WHITE)
            .strong(),
    );
}

/// Icon plus label on one line, both clickable as one; the pointer turns
/// into a hand over it.
pub fn item_row(
    ui: &mut egui::Ui,
    icons: &mut IconCache,
    item: &Item,
    color: egui::Color32,
) -> egui::Response {
    let resp = ui
        .horizontal(|ui| {
            let icon = icons.draw(ui, item.icon, egui::Sense::click_and_drag());
            let text = ui.add(
                egui::Label::new(egui::RichText::new(item.label()).color(color))
                    .sense(egui::Sense::click_and_drag()),
            );
            icon.union(text)
        })
        .inner;
    if resp.drag_started() {
        resp.dnd_set_drag_payload(ItemDrag(item.guid));
    }
    if resp.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
    }
    resp
}

/// The panels reading the active session, in drawing order.
pub fn live() -> Vec<Box<dyn Plugin>> {
    vec![
        Box::new(nameplates::Nameplates::default()),
        Box::new(vitals::Vitals::default()),
        Box::new(radar::Radar::default()),
        Box::new(target::Target::default()),
        Box::new(vendor::Vendor::default()),
        Box::new(trade::Trade::default()),
        Box::new(fellowship::Fellowship::default()),
        Box::new(allegiance::Allegiance::default()),
        Box::new(salvage::Salvage::default()),
        Box::new(housing::Housing::default()),
        Box::new(appraisal::AppraisalPanel::default()),
        Box::new(social::Social::default()),
        Box::new(book::Book::default()),
        Box::new(confirm::Confirm::default()),
        Box::new(options::Options::default()),
        Box::new(combat::Combat::default()),
        Box::new(loot::Loot::default()),
        Box::new(inventory::Inventory::default()),
        Box::new(map::Map::default()),
        Box::new(skills::Skills::default()),
        Box::new(spellbook::Spellbook::default()),
        Box::new(spellbar::SpellBar::default()),
        Box::new(components::Components::default()),
        Box::new(buffs::Buffs::default()),
    ]
}

/// The same panels filled with sample data, for a screenshot without a
/// server: real icons from the portal and real spells from the spell
/// table when `assets` is given.
pub fn demo(assets: Option<&ac_scene::Assets>) -> Vec<Box<dyn Plugin>> {
    let tables = assets.and_then(|a| Some((a.spell_table().ok()?, a.spell_components().ok()?)));
    vec![
        Box::new(nameplates::Nameplates::demo()),
        Box::new(vitals::Vitals::demo()),
        Box::new(radar::Radar::demo()),
        Box::new(target::Target::demo()),
        Box::new(vendor::Vendor::demo()),
        Box::new(trade::Trade::demo()),
        Box::new(fellowship::Fellowship::demo()),
        Box::new(allegiance::Allegiance::demo()),
        Box::new(salvage::Salvage::demo()),
        Box::new(housing::Housing::demo()),
        Box::new(appraisal::AppraisalPanel::demo()),
        Box::new(social::Social::demo()),
        Box::new(book::Book::demo()),
        Box::new(confirm::Confirm::demo()),
        Box::new(options::Options::demo()),
        Box::new(combat::Combat::demo()),
        Box::new(loot::Loot::demo()),
        Box::new(inventory::Inventory::demo()),
        Box::new(map::Map::demo()),
        Box::new(skills::Skills::demo()),
        Box::new(spellbook::Spellbook::demo(
            tables.as_ref().map(|(t, c)| (&**t, &**c)),
        )),
        Box::new(spellbar::SpellBar::demo(tables.as_ref().map(|(t, _)| &**t))),
        Box::new(components::Components::demo(
            tables.as_ref().map(|(_, c)| &**c),
        )),
        Box::new(buffs::Buffs::demo(tables.as_ref().map(|(t, _)| &**t))),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_show_stacks() {
        assert_eq!(item_label("Pyreal", 12), "Pyreal (12)");
        assert_eq!(item_label("Dagger", 1), "Dagger");
        assert_eq!(item_label("Dagger", 0), "Dagger");
    }

    #[test]
    fn seconds_format() {
        assert_eq!(fmt_seconds(45.0), "45s");
        assert_eq!(fmt_seconds(754.0), "12m 34s");
        assert_eq!(fmt_seconds(5400.0), "1h 30m");
        assert_eq!(fmt_seconds(-3.0), "0s");
        assert_eq!(fmt_seconds(0.4), "0s");
    }

    #[test]
    fn every_panel_has_a_demo() {
        let live = live();
        let demo = demo(None);
        assert_eq!(live.len(), demo.len());
        for (a, b) in live.iter().zip(&demo) {
            assert_eq!(a.name(), b.name());
        }
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn saved_positions_win_over_the_built_in_place() {
        forget_positions();
        let default = egui::pos2(100.0, 200.0);
        assert_eq!(remembered("panel_under_test", default), default);
        restore_positions(
            [("panel_under_test".to_string(), [12.0, 34.0])]
                .into_iter()
                .collect(),
        );
        assert_eq!(
            remembered("panel_under_test", default),
            egui::pos2(12.0, 34.0)
        );
        // Reset puts it back.
        forget_positions();
        assert_eq!(remembered("panel_under_test", default), default);
    }
}
