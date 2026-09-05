//! Vendor: the shop we are trading with, top centre. The vendor's stock on
//! the left with Buy buttons, our sellable pack items on the right with
//! Sell buttons; Close ends the trade.

use super::{title, window, Source};
use crate::{egui, Client, Ctx, Plugin};

/// A vendor's stock line or one of our items offered for sale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TradeItem {
    pub guid: u32,
    pub name: String,
    pub price: u32,
    pub icon: u32,
    /// Unlimited supply (vendor stock) when true.
    pub unlimited: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VendorView {
    pub name: String,
    pub stock: Vec<TradeItem>,
    pub selling: Vec<TradeItem>,
}

/// What the panel asked for.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Actions {
    pub buy: Vec<u32>,
    pub sell: Vec<u32>,
    pub close: bool,
}

/// Vendor stock with this "-1" stack size never runs out.
pub const UNLIMITED_STACK: u32 = 0x00FF_FFFF;

/// What the vendor charges for an item of `value` at its `sell_rate`
/// (ACE's SellPrice: the rate the vendor sells at), at least 1 pyreal.
pub fn buy_price(value: u32, sell_rate: f32) -> u32 {
    ((value as f32 * sell_rate - 0.1).ceil().max(1.0)) as u32
}

/// What the vendor pays for one of our items at its `buy_rate`, at least
/// 1 pyreal.
pub fn sell_price(value: u32, buy_rate: f32) -> u32 {
    ((value as f32 * buy_rate + 0.1).floor().max(1.0)) as u32
}

/// The open vendor, if any. Only pack items with a value that are not
/// money are offered for sale.
pub fn view(c: &Client) -> Option<VendorView> {
    let v = c.world.open_vendor.as_ref()?;
    Some(VendorView {
        name: c
            .world
            .objects
            .get(&v.vendor)
            .map(|o| o.name.clone())
            .unwrap_or_else(|| "Vendor".into()),
        stock: v
            .items
            .iter()
            .map(|it| TradeItem {
                guid: it.guid,
                name: it.desc.name.clone(),
                price: buy_price(it.desc.value, v.sell_rate),
                icon: it.desc.icon_id,
                unlimited: it.stack == UNLIMITED_STACK,
            })
            .collect(),
        selling: c
            .world
            .inventory()
            .filter(|o| o.value > 0 && o.item_type & ac_world::item_type::MONEY == 0)
            .map(|o| TradeItem {
                guid: o.guid,
                name: o.name.clone(),
                price: sell_price(o.value, v.buy_rate),
                icon: o.icon_id,
                unlimited: false,
            })
            .collect(),
    })
}

fn trade_list(
    ui: &mut egui::Ui,
    salt: &str,
    header: &str,
    button: &str,
    items: &[TradeItem],
    out: &mut Vec<u32>,
) {
    ui.label(egui::RichText::new(header).color(egui::Color32::from_gray(180)));
    egui::ScrollArea::vertical()
        .id_salt(salt.to_string())
        .max_height(360.0)
        .show(ui, |ui| {
            for it in items {
                ui.horizontal(|ui| {
                    if ui.small_button(button).clicked() {
                        out.push(it.guid);
                    }
                    ui.label(
                        egui::RichText::new(format!("{}  {}p", it.name, it.price))
                            .color(egui::Color32::WHITE),
                    );
                });
            }
        });
}

pub fn draw(egui: &egui::Context, v: &VendorView) -> Actions {
    let mut actions = Actions::default();
    let w = egui.viewport_rect().width();
    window(
        "vendor",
        egui::pos2(w * 0.5 - 220.0, 60.0),
        egui::vec2(440.0, 420.0),
        200,
        8,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(424.0, 404.0));
        ui.horizontal(|ui| {
            title(ui, &v.name);
            if ui.button("Close").clicked() {
                actions.close = true;
            }
        });
        ui.columns(2, |cols| {
            trade_list(
                &mut cols[0],
                "stock",
                "For sale",
                "Buy",
                &v.stock,
                &mut actions.buy,
            );
            trade_list(
                &mut cols[1],
                "sell",
                "Your pack",
                "Sell",
                &v.selling,
                &mut actions.sell,
            );
        });
    });
    actions
}

#[derive(Default)]
pub struct Vendor {
    source: Source<VendorView>,
}

impl Vendor {
    pub fn demo() -> Self {
        let item = |guid, name: &str, price, unlimited| TradeItem {
            guid,
            name: name.into(),
            price,
            icon: 0,
            unlimited,
        };
        Vendor {
            source: Source::Demo(VendorView {
                name: "Demo Shopkeeper".into(),
                stock: vec![
                    item(0x100, "Dagger", 15, true),
                    item(0x101, "Leather Cap", 22, true),
                    item(0x102, "Apple", 2, true),
                ],
                selling: vec![
                    item(0x200, "Demo item 0x06001A8A", 3, false),
                    item(0x201, "Demo item 0x0600321E", 40, false),
                ],
            }),
        }
    }
}

impl Plugin for Vendor {
    fn name(&self) -> &str {
        "vendor"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().and_then(|c| view(c)),
        };
        let Some(v) = v else { return };
        let actions = draw(egui, &v);
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            for g in actions.buy {
                c.buy(g);
            }
            for g in actions.sell {
                c.sell(g);
            }
            if actions.close {
                c.close_vendor();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_round_the_way_ace_does() {
        // Vendors sell at a markup, rounded up, never below one pyreal.
        assert_eq!(buy_price(10, 1.0), 10);
        assert_eq!(buy_price(10, 1.25), 13);
        assert_eq!(buy_price(0, 1.5), 1);
        // And buy at a discount, rounded down, never below one pyreal.
        assert_eq!(sell_price(10, 0.5), 5);
        assert_eq!(sell_price(7, 0.5), 3);
        assert_eq!(sell_price(1, 0.1), 1);
    }
}
