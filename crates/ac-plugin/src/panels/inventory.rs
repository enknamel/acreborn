//! Inventory: what the character wears and carries, under the radar.
//! I toggles it; double-click an item to use it (wield, unwield, read,
//! drink...).

use super::{caption, has_sheet, item_row, title, window, Item, ItemDrag, Source};
use crate::icons::{IconCache, IconLayers};
use crate::{egui, Client, Ctx, Plugin};

/// Worn items first, then the pack, each by name.
pub fn sort(items: &mut [Item]) {
    items.sort_by(|a, b| b.wielded.cmp(&a.wielded).then(a.name.cmp(&b.name)));
}

/// Everything wielded and everything in the pack; `None` until the sheet
/// arrived.
pub fn view(c: &Client) -> Option<Vec<Item>> {
    if !has_sheet(c) {
        return None;
    }
    let mut items: Vec<Item> = c
        .world
        .wielded()
        .map(|o| Item::of(o, true))
        .chain(c.world.inventory().map(|o| Item::of(o, false)))
        .collect();
    sort(&mut items);
    Some(items)
}

/// What the player did in the panel this frame.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Actions {
    /// Double-clicked items (use: wield, take off, read...).
    pub activated: Vec<u32>,
    /// Items dragged onto a pack: (item, container guid; 0 = main pack).
    pub moves: Vec<(u32, u32)>,
}

/// Draw the panel: double-click uses an item, dragging one onto a side
/// pack or the "Pack" header moves it there.
pub fn draw(egui: &egui::Context, icons: &mut IconCache, items: &[Item]) -> Actions {
    let mut actions = Actions::default();
    let w = egui.viewport_rect().width();
    let r = super::radar::RADIUS;
    window(
        "inventory",
        egui::pos2(w - 268.0, 2.0 * r + 40.0),
        egui::vec2(260.0, 300.0),
        170,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(248.0, 288.0));
        title(
            ui,
            format!(
                "Inventory ({})",
                items.iter().filter(|i| !i.wielded).count()
            ),
        );
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show(ui, |ui| {
                ui.set_min_width(240.0);
                let mut shown_wielded_header = false;
                let mut shown_pack_header = false;
                for it in items {
                    if it.wielded && !shown_wielded_header {
                        caption(ui, "Worn");
                        shown_wielded_header = true;
                    }
                    if !it.wielded && !shown_pack_header {
                        // The header takes drops for the main pack.
                        let (r, _) = ui.dnd_drop_zone::<ItemDrag, _>(
                            egui::Frame::new().inner_margin(2),
                            |ui| caption(ui, "Pack"),
                        );
                        if let Some(p) = r.response.dnd_release_payload::<ItemDrag>() {
                            actions.moves.push((p.0, 0));
                        }
                        shown_pack_header = true;
                    }
                    let color = if it.wielded {
                        egui::Color32::from_rgb(180, 230, 180)
                    } else if it.container {
                        egui::Color32::from_rgb(230, 210, 150)
                    } else {
                        egui::Color32::WHITE
                    };
                    if it.container && !it.wielded {
                        let (r, dropped) = ui
                            .dnd_drop_zone::<ItemDrag, _>(egui::Frame::new(), |ui| {
                                item_row(ui, icons, it, color)
                            });
                        if r.inner.double_clicked() {
                            actions.activated.push(it.guid);
                        }
                        if let Some(p) = dropped {
                            if p.0 != it.guid {
                                actions.moves.push((p.0, it.guid));
                            }
                        }
                    } else if item_row(ui, icons, it, color).double_clicked() {
                        actions.activated.push(it.guid);
                    }
                }
            });
    });
    actions
}

pub struct Inventory {
    source: Source<Vec<Item>>,
    /// Open (I toggles it). Starts open.
    pub show: bool,
}

impl Default for Inventory {
    fn default() -> Self {
        Inventory {
            source: Source::Live,
            show: true,
        }
    }
}

/// A sample item with a bare icon, for demos.
pub fn demo_item(guid: u32, name: &str, stack: u32, wielded: bool, icon: u32) -> Item {
    Item {
        guid,
        name: name.to_string(),
        stack,
        wielded,
        container: false,
        icon: IconLayers::single(icon),
    }
}

impl Inventory {
    /// Known 32x32 icons from the portal.
    pub fn demo() -> Self {
        Inventory {
            source: Source::Demo(vec![
                demo_item(1, "Demo item 0x06000FAA", 1, true, 0x0600_0FAA),
                demo_item(2, "Demo item 0x0600189E", 1, true, 0x0600_189E),
                demo_item(3, "Demo item 0x06001A8A", 1, false, 0x0600_1A8A),
                demo_item(4, "Demo item 0x06001FB7", 12, false, 0x0600_1FB7),
                demo_item(5, "Demo item 0x0600261A", 1, false, 0x0600_261A),
                demo_item(6, "Demo item 0x06002C0D", 1, false, 0x0600_2C0D),
                demo_item(7, "Demo item 0x06002F40", 3, false, 0x0600_2F40),
                demo_item(8, "Demo item 0x0600321E", 1, false, 0x0600_321E),
            ]),
            show: true,
        }
    }
}

impl Plugin for Inventory {
    fn name(&self) -> &str {
        "inventory"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if !self.show {
            return;
        }
        let items = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().and_then(|c| view(c)),
        };
        let Some(items) = items else { return };
        let actions = draw(egui, cx.icons(), &items);
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            for g in actions.activated {
                c.interact(g);
            }
            for (item, container) in actions.moves {
                let container = if container == 0 {
                    c.world.player_guid.unwrap_or(0)
                } else {
                    container
                };
                c.put_in_container(item, container);
            }
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if key == egui::Key::I && pressed {
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
    fn worn_first_then_by_name() {
        let mut items = vec![
            demo_item(1, "Pyreal", 5, false, 0),
            demo_item(2, "Tunic", 1, true, 0),
            demo_item(3, "Dagger", 1, false, 0),
            demo_item(4, "Boots", 1, true, 0),
        ];
        sort(&mut items);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Boots", "Tunic", "Dagger", "Pyreal"]);
    }
}
