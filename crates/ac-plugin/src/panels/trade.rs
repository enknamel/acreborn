//! Secure trade with another player: open by double-clicking them (peace
//! mode, close by). Both windows show both offers; drag items from the
//! inventory into yours; Accept on both sides swaps the items. Any change
//! to an offer clears both acceptances, as the server does.

use super::{caption, item_row, title, window, Item, ItemDrag, Source};
use crate::icons::IconCache;
use crate::{egui, Client, Ctx, Plugin};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TradeView {
    pub partner: String,
    pub mine: Vec<Item>,
    pub theirs: Vec<Item>,
    pub i_accepted: bool,
    pub they_accepted: bool,
    /// "Cannot trade X: reason" from the last TradeFailure.
    pub failure: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Actions {
    pub add: Vec<u32>,
    pub accept: bool,
    pub decline: bool,
    pub reset: bool,
    pub close: bool,
}

pub fn view(c: &Client) -> Option<TradeView> {
    let t = c.world.trade.as_ref()?;
    let name = |g: &u32| {
        c.world
            .objects
            .get(g)
            .map(|o| Item::of(o, false))
            .unwrap_or_else(|| Item {
                guid: *g,
                name: format!("item {g:#010x}"),
                stack: 0,
                wielded: false,
                container: false,
                icon: Default::default(),
                wcid: 0,
                max_stack: 1,
            })
    };
    Some(TradeView {
        partner: c
            .world
            .objects
            .get(&t.partner)
            .map(|o| o.name.clone())
            .unwrap_or_else(|| "someone".into()),
        mine: t.mine.iter().map(name).collect(),
        theirs: t.theirs.iter().map(name).collect(),
        i_accepted: t.i_accepted,
        they_accepted: t.they_accepted,
        failure: t
            .failure
            .map(|(g, reason)| format!("Cannot trade {}: {}", name(&g).name, failure_text(reason))),
    })
}

/// The WeenieErrors a TradeFailure carries.
pub fn failure_text(reason: u32) -> &'static str {
    match reason {
        0x0430 => "attuned item",
        0x03F2 => "you cannot trade that",
        _ => "the server refused it",
    }
}

/// What the status line says about acceptance.
pub fn status_text(v: &TradeView) -> String {
    match (v.i_accepted, v.they_accepted) {
        (true, true) => "Both accepted: trading".into(),
        (true, false) => format!("You accepted, waiting for {}", v.partner),
        (false, true) => format!("{} accepted: press Accept to trade", v.partner),
        (false, false) => "Drag items in, then Accept".into(),
    }
}

pub fn draw(egui: &egui::Context, icons: &mut IconCache, v: &TradeView) -> Actions {
    let mut actions = Actions::default();
    let rect = egui.viewport_rect();
    let (w, h) = (rect.width(), rect.height());
    window(
        "trade",
        egui::pos2(w * 0.5 - 220.0, h * 0.5 - 150.0),
        egui::vec2(440.0, 300.0),
        190,
        8,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(424.0, 284.0));
        title(ui, format!("Trade with {}", v.partner));
        ui.columns(2, |cols| {
            let (zone, _) =
                cols[0].dnd_drop_zone::<ItemDrag, _>(egui::Frame::new().inner_margin(2), |ui| {
                    caption(ui, "Your offer (drop items here)");
                    ui.set_min_height(170.0);
                    for it in &v.mine {
                        item_row(ui, icons, it, egui::Color32::WHITE);
                    }
                    if v.mine.is_empty() {
                        ui.label(
                            egui::RichText::new("(nothing)").color(egui::Color32::from_gray(150)),
                        );
                    }
                });
            if let Some(p) = zone.response.dnd_release_payload::<ItemDrag>() {
                actions.add.push(p.0);
            }
            caption(&mut cols[1], &format!("{}'s offer", v.partner));
            cols[1].set_min_height(170.0);
            for it in &v.theirs {
                item_row(
                    &mut cols[1],
                    icons,
                    it,
                    egui::Color32::from_rgb(200, 230, 255),
                );
            }
            if v.theirs.is_empty() {
                cols[1]
                    .label(egui::RichText::new("(nothing)").color(egui::Color32::from_gray(150)));
            }
        });
        let status = status_text(v);
        ui.label(egui::RichText::new(status).color(egui::Color32::from_rgb(255, 220, 120)));
        if let Some(f) = &v.failure {
            ui.label(egui::RichText::new(f).color(egui::Color32::from_rgb(255, 120, 120)));
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!v.i_accepted, egui::Button::new("Accept"))
                .clicked()
            {
                actions.accept = true;
            }
            if ui
                .add_enabled(v.i_accepted, egui::Button::new("Decline"))
                .clicked()
            {
                actions.decline = true;
            }
            if ui.button("Reset").clicked() {
                actions.reset = true;
            }
            if ui.button("Close").clicked() {
                actions.close = true;
            }
        });
    });
    actions
}

#[derive(Default)]
pub struct Trade {
    source: Source<TradeView>,
}

impl Trade {
    pub fn demo() -> Self {
        let item = |guid, name: &str, stack, icon| Item {
            guid,
            name: name.to_string(),
            stack,
            wielded: false,
            container: false,
            icon: crate::icons::IconLayers::single(icon),
            wcid: 0,
            max_stack: 1,
        };
        Trade {
            source: Source::Demo(TradeView {
                partner: "Reborn".into(),
                mine: vec![item(1, "Iron Scarab", 10, 0x0600_1A8A)],
                theirs: vec![
                    item(2, "Pyreal", 500, 0x0600_1FB7),
                    item(3, "Hyssop", 5, 0x0600_1DE9),
                ],
                i_accepted: false,
                they_accepted: true,
                failure: None,
            }),
        }
    }
}

impl Plugin for Trade {
    fn name(&self) -> &str {
        "trade"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().and_then(|c| view(c)),
        };
        let Some(v) = v else { return };
        let a = draw(egui, cx.icons(), &v);
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            for g in a.add {
                c.add_to_trade(g);
            }
            if a.accept {
                c.accept_trade();
            }
            if a.decline {
                c.decline_trade();
            }
            if a.reset {
                c.reset_trade();
            }
            if a.close {
                c.close_trade();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_follows_acceptance() {
        let mut v = TradeView {
            partner: "Bob".into(),
            mine: vec![],
            theirs: vec![],
            i_accepted: false,
            they_accepted: false,
            failure: None,
        };
        assert_eq!(status_text(&v), "Drag items in, then Accept");
        v.they_accepted = true;
        assert_eq!(status_text(&v), "Bob accepted: press Accept to trade");
        v.i_accepted = true;
        assert_eq!(status_text(&v), "Both accepted: trading");
    }
}
