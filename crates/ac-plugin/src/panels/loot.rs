//! Loot: the corpse or chest we are looking into, mid screen. Double-click
//! an item to take it, "Take all" for everything, "Close" to stop looking.

use super::{item_row, title, window, Item, ItemDrag, Source};
use crate::icons::IconCache;
use crate::{egui, Client, Ctx, Plugin};

/// The open container's name and contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LootView {
    pub name: String,
    pub items: Vec<Item>,
}

/// What the panel asked for.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Actions {
    pub take: Vec<u32>,
    pub close: bool,
    /// Carried items dragged onto the window: put into the container.
    pub store: Vec<u32>,
}

/// The open container, if any. Items the server has not described yet
/// are skipped.
pub fn view(c: &Client) -> Option<LootView> {
    let (guid, items) = c.world.open_container.as_ref()?;
    // Our own side packs list their contents the same way at login; they
    // belong in the inventory panel, not here.
    let me = c.world.player_guid;
    if me.is_some() && c.world.objects.get(guid).is_some_and(|o| o.container == me) {
        return None;
    }
    Some(LootView {
        name: c
            .world
            .objects
            .get(guid)
            .map(|o| o.name.clone())
            .unwrap_or_else(|| "Container".into()),
        items: items
            .iter()
            .filter_map(|g| c.world.objects.get(g))
            .map(|o| Item::of(o, false))
            .collect(),
    })
}

pub fn draw(egui: &egui::Context, icons: &mut IconCache, v: &LootView) -> Actions {
    let mut actions = Actions::default();
    let rect = egui.viewport_rect();
    let (w, h) = (rect.width(), rect.height());
    window(
        "loot",
        egui::pos2(w * 0.5 - 140.0, h * 0.5 - 120.0),
        egui::vec2(280.0, 240.0),
        190,
        8,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(264.0, 224.0));
        title(ui, &v.name);
        egui::ScrollArea::vertical()
            .max_height(170.0)
            .show(ui, |ui| {
                ui.set_min_width(250.0);
                if v.items.is_empty() {
                    ui.label(egui::RichText::new("(empty)").color(egui::Color32::from_gray(170)));
                }
                for it in &v.items {
                    if item_row(ui, icons, it, egui::Color32::WHITE).double_clicked() {
                        actions.take.push(it.guid);
                    }
                }
            });
        ui.horizontal(|ui| {
            if ui.button("Take all").clicked() {
                actions.take.extend(v.items.iter().map(|i| i.guid));
            }
            if ui.button("Close").clicked() {
                actions.close = true;
            }
            let (r, _) =
                ui.dnd_drop_zone::<ItemDrag, _>(egui::Frame::new().inner_margin(2), |ui| {
                    ui.label(
                        egui::RichText::new("drop items here to store")
                            .color(egui::Color32::from_gray(170)),
                    );
                });
            if let Some(p) = r.response.dnd_release_payload::<ItemDrag>() {
                actions.store.push(p.0);
            }
        });
    });
    actions
}

#[derive(Default)]
pub struct Loot {
    source: Source<LootView>,
}

impl Loot {
    /// Known icons, one with an overlay, to check the layering.
    pub fn demo() -> Self {
        use super::inventory::demo_item;
        let mut layered = demo_item(12, "0x06001A8A + 0x06006A21", 1, false, 0x0600_1A8A);
        layered.icon.overlay = 0x0600_6A21;
        Loot {
            source: Source::Demo(LootView {
                name: "Demo corpse".into(),
                items: vec![
                    demo_item(9, "Loot 0x06002C0D", 1, false, 0x0600_2C0D),
                    demo_item(10, "Loot 0x0600601C", 5, false, 0x0600_601C),
                    demo_item(11, "Loot 0x06006A21", 1, false, 0x0600_6A21),
                    layered,
                ],
            }),
        }
    }
}

impl Plugin for Loot {
    fn name(&self) -> &str {
        "loot"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().and_then(|c| view(c)),
        };
        let Some(v) = v else { return };
        let actions = draw(egui, cx.icons(), &v);
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            for g in actions.take {
                c.take(g);
            }
            if let Some(container) = c.world.open_container.as_ref().map(|(g, _)| *g) {
                for item in actions.store {
                    c.put_in_container(item, container);
                }
            }
            if actions.close {
                c.close_container();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_has_a_layered_icon() {
        let Loot {
            source: Source::Demo(v),
        } = Loot::demo()
        else {
            panic!("demo() is not a demo source");
        };
        assert_eq!(v.name, "Demo corpse");
        assert!(v.items.iter().any(|i| i.icon.overlay != 0));
        assert!(v.items.iter().all(|i| !i.wielded));
    }
}
