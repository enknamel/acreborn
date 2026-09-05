//! Housing (H): our house (type, where, whether this period's
//! maintenance is paid) with Recall, guest management (add by name,
//! remove, storage permission, open house, allegiance access, boot) and
//! Abandon; and, after using a house sign, that house's requirements,
//! owner and price with Buy and Pay maintenance, which take the items
//! from the pack.

use super::{caption, title, window, Source};
use crate::{egui, Client, Ctx, Plugin};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaymentRow {
    pub name: String,
    pub needed: u32,
    pub paid: u32,
    /// How many of the item the pack holds.
    pub have: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnHouse {
    pub kind: String,
    pub cell: u32,
    pub rent_paid: bool,
    pub rent: Vec<PaymentRow>,
    /// The guest list, once requested.
    pub open: Option<bool>,
    pub allegiance_guests: bool,
    pub allegiance_storage: bool,
    pub guests: Vec<(String, bool)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignView {
    pub kind: String,
    pub owner_name: String,
    pub for_sale: bool,
    pub min_level: i32,
    pub min_rank: i32,
    pub requires_monarch: bool,
    pub buy: Vec<PaymentRow>,
    pub rent: Vec<PaymentRow>,
    pub can_buy: bool,
    pub can_rent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HousingView {
    /// None until the server answered the house query.
    pub loaded: bool,
    pub house: Option<OwnHouse>,
    pub sign: Option<SignView>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Actions {
    pub recall: bool,
    pub buy: bool,
    pub rent: bool,
    pub abandon: bool,
    pub refresh: bool,
    pub add_guest: Option<String>,
    pub remove_guest: Option<String>,
    pub storage: Option<(String, bool)>,
    pub open: Option<bool>,
    pub allegiance_guests: Option<bool>,
    pub allegiance_storage: Option<bool>,
    pub boot_all: bool,
}

fn rows(c: &Client, list: &[ac_world::housing::Payment]) -> Vec<PaymentRow> {
    list.iter()
        .map(|p| PaymentRow {
            name: p.name.clone(),
            needed: p.needed,
            paid: p.paid,
            have: c
                .world
                .inventory()
                .filter(|o| o.weenie_class_id == p.wcid)
                .map(|o| o.stack_size.max(1))
                .sum(),
        })
        .collect()
}

pub fn view(c: &Client) -> HousingView {
    let house = match c.world.house.as_ref() {
        Some(Some(h)) => {
            let access = c.world.house_access.as_ref();
            Some(OwnHouse {
                kind: ac_world::housing::kind_name(h.kind).to_string(),
                cell: h.position.map(|p| p.cell).unwrap_or(0),
                rent_paid: h.rent_paid(),
                rent: rows(c, &h.rent),
                open: access.map(|a| a.open),
                allegiance_guests: access.map(|a| a.allegiance_guests).unwrap_or(false),
                allegiance_storage: access.map(|a| a.allegiance_storage).unwrap_or(false),
                guests: access
                    .map(|a| {
                        a.guests
                            .iter()
                            .map(|g| (g.name.clone(), g.storage))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        }
        _ => None,
    };
    let sign = c.world.house_profile.as_ref().map(|p| SignView {
        kind: ac_world::housing::kind_name(p.kind).to_string(),
        owner_name: p.owner_name.clone(),
        for_sale: p.owner == 0,
        min_level: p.min_level,
        min_rank: p.min_rank,
        requires_monarch: p.requires_monarch,
        buy: rows(c, &p.buy),
        rent: rows(c, &p.rent),
        can_buy: p.owner == 0 && c.payment_items(&p.buy).is_some(),
        can_rent: p.owner != 0
            && p.rent.iter().any(|r| r.outstanding() > 0)
            && c.payment_items(&p.rent).is_some(),
    });
    HousingView {
        loaded: c.world.house.is_some(),
        house,
        sign,
    }
}

/// "Pyreal 10,000 / 10,000 (have 25,000)".
pub fn payment_text(p: &PaymentRow) -> String {
    format!(
        "{} {} / {} (have {})",
        p.name,
        group(p.paid),
        group(p.needed),
        group(p.have)
    )
}

/// Thousands separators.
pub fn group(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn payment_list(ui: &mut egui::Ui, header: &str, list: &[PaymentRow]) {
    caption(ui, header);
    for p in list {
        let color = if p.paid >= p.needed {
            egui::Color32::from_rgb(180, 230, 180)
        } else if p.have >= p.needed - p.paid {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_rgb(255, 170, 170)
        };
        ui.label(egui::RichText::new(payment_text(p)).color(color).small());
    }
}

pub fn draw(egui: &egui::Context, v: &HousingView, guest_name: &mut String) -> Actions {
    let mut actions = Actions::default();
    let w = egui.viewport_rect().width();
    window(
        "housing",
        egui::pos2(w * 0.5 - 250.0, 60.0),
        egui::vec2(500.0, 330.0),
        170,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_size(egui::vec2(484.0, 314.0));
        match &v.house {
            None => {
                title(ui, "Housing");
                ui.label(
                    egui::RichText::new(if v.loaded {
                        "You own no house. Use a house sign to see what it costs."
                    } else {
                        "Waiting for the server..."
                    })
                    .color(egui::Color32::from_gray(180))
                    .small(),
                );
            }
            Some(h) => {
                title(ui, format!("Your {} ({:#010x})", h.kind, h.cell));
                ui.label(
                    egui::RichText::new(if h.rent_paid {
                        "Maintenance paid for this period."
                    } else {
                        "Maintenance due: pay it at the house sign."
                    })
                    .color(if h.rent_paid {
                        egui::Color32::from_rgb(180, 230, 180)
                    } else {
                        egui::Color32::from_rgb(255, 200, 120)
                    }),
                );
                payment_list(ui, "Maintenance", &h.rent);
                ui.horizontal(|ui| {
                    if ui.button("Recall").clicked() {
                        actions.recall = true;
                    }
                    if ui.button("Refresh").clicked() {
                        actions.refresh = true;
                    }
                    if ui.button("Boot everyone").clicked() {
                        actions.boot_all = true;
                    }
                    if ui.button("Abandon").clicked() {
                        actions.abandon = true;
                    }
                });
                match h.open {
                    None => {
                        caption(ui, "Guests: press Refresh for the list");
                    }
                    Some(open) => {
                        ui.horizontal(|ui| {
                            let mut o = open;
                            if ui.checkbox(&mut o, "Open house").changed() {
                                actions.open = Some(o);
                            }
                            let mut ag = h.allegiance_guests;
                            if ui.checkbox(&mut ag, "Allegiance may enter").changed() {
                                actions.allegiance_guests = Some(ag);
                            }
                            let mut ast = h.allegiance_storage;
                            if ui.checkbox(&mut ast, "and use storage").changed() {
                                actions.allegiance_storage = Some(ast);
                            }
                        });
                        egui::ScrollArea::vertical()
                            .max_height(80.0)
                            .show(ui, |ui| {
                                for (name, storage) in &h.guests {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(name).color(egui::Color32::WHITE),
                                        );
                                        let mut st = *storage;
                                        if ui.checkbox(&mut st, "storage").changed() {
                                            actions.storage = Some((name.clone(), st));
                                        }
                                        if ui.small_button("Remove").clicked() {
                                            actions.remove_guest = Some(name.clone());
                                        }
                                    });
                                }
                            });
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Guest");
                    ui.add(egui::TextEdit::singleline(guest_name).desired_width(140.0));
                    if ui
                        .add_enabled(!guest_name.trim().is_empty(), egui::Button::new("Add"))
                        .clicked()
                    {
                        actions.add_guest = Some(guest_name.trim().to_string());
                    }
                });
            }
        }
        if let Some(s) = &v.sign {
            ui.separator();
            title(
                ui,
                if s.for_sale {
                    format!("{} for sale", s.kind)
                } else {
                    format!("{} owned by {}", s.kind, s.owner_name)
                },
            );
            let mut reqs = Vec::new();
            if s.min_level > 0 {
                reqs.push(format!("level {}", s.min_level));
            }
            if s.min_rank > 0 {
                reqs.push(format!("allegiance rank {}", s.min_rank));
            }
            if s.requires_monarch {
                reqs.push("monarch".into());
            }
            if !reqs.is_empty() {
                caption(ui, format!("Requires {}", reqs.join(", ")));
            }
            ui.columns(2, |cols| {
                payment_list(&mut cols[0], "Price", &s.buy);
                payment_list(&mut cols[1], "Maintenance", &s.rent);
            });
            ui.horizontal(|ui| {
                if s.for_sale
                    && ui
                        .add_enabled(s.can_buy, egui::Button::new("Buy"))
                        .clicked()
                {
                    actions.buy = true;
                }
                if !s.for_sale
                    && ui
                        .add_enabled(s.can_rent, egui::Button::new("Pay maintenance"))
                        .clicked()
                {
                    actions.rent = true;
                }
            });
        }
    });
    actions
}

pub struct Housing {
    source: Source<HousingView>,
    pub show: bool,
    guest_name: String,
    /// The last `house_profile_seq` seen; a new sign opens the window.
    seen_profile: u64,
}

impl Default for Housing {
    fn default() -> Self {
        Housing {
            source: Source::Live,
            show: false,
            guest_name: String::new(),
            seen_profile: 0,
        }
    }
}

impl Housing {
    pub fn demo() -> Self {
        let pay = |name: &str, needed, paid, have| PaymentRow {
            name: name.into(),
            needed,
            paid,
            have,
        };
        Housing {
            source: Source::Demo(HousingView {
                loaded: true,
                house: Some(OwnHouse {
                    kind: "Apartment".into(),
                    cell: 0x7200_018C,
                    rent_paid: false,
                    rent: vec![pay("Pyreal", 10_000, 2_500, 25_000)],
                    open: Some(false),
                    allegiance_guests: true,
                    allegiance_storage: false,
                    guests: vec![("Reborn".into(), true), ("Test Mage".into(), false)],
                }),
                sign: Some(SignView {
                    kind: "Cottage".into(),
                    owner_name: String::new(),
                    for_sale: true,
                    min_level: 20,
                    min_rank: -1,
                    requires_monarch: false,
                    buy: vec![
                        pay("Pyreal", 300_000, 0, 25_000),
                        pay("Writ of Refuge", 1, 0, 1),
                        pay("Iron Heart", 1, 0, 0),
                    ],
                    rent: vec![pay("Pyreal", 30_000, 0, 25_000)],
                    can_buy: false,
                    can_rent: false,
                }),
            }),
            show: true,
            guest_name: String::new(),
            seen_profile: 0,
        }
    }
}

impl Plugin for Housing {
    fn name(&self) -> &str {
        "housing"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        // A used sign opens the window on its own.
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            let seq = c.world.house_profile_seq;
            if seq != self.seen_profile {
                self.seen_profile = seq;
                self.show = true;
            }
        }
        if !self.show {
            return;
        }
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx.try_client().map(|c| view(c)),
        };
        let Some(v) = v else { return };
        let a = draw(egui, &v, &mut self.guest_name);
        if let (Source::Live, Some(c)) = (&self.source, cx.try_client()) {
            if a.recall {
                c.slash_command("/house");
            }
            if a.buy {
                c.buy_house();
            }
            if a.rent {
                c.rent_house();
            }
            if a.abandon {
                c.abandon_house();
            }
            if a.refresh {
                c.house_query();
                c.house_guest_list();
            }
            if let Some(n) = a.add_guest {
                c.house_guest(&n, true);
                self.guest_name.clear();
            }
            if let Some(n) = a.remove_guest {
                c.house_guest(&n, false);
            }
            if let Some((n, on)) = a.storage {
                c.house_storage(&n, on);
            }
            if let Some(o) = a.open {
                c.house_open(o);
            }
            if let Some(on) = a.allegiance_guests {
                c.house_allegiance(false, on);
            }
            if let Some(on) = a.allegiance_storage {
                c.house_allegiance(true, on);
            }
            if a.boot_all {
                c.house_boot("");
            }
        }
    }

    fn key(&mut self, cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if key == egui::Key::H && pressed {
            self.show = !self.show;
            if self.show {
                if let Some(c) = cx.try_client() {
                    c.house_query();
                    c.house_guest_list();
                }
            }
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payment_text_groups_thousands() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1_000), "1,000");
        assert_eq!(group(300_000), "300,000");
        let p = PaymentRow {
            name: "Pyreal".into(),
            needed: 10_000,
            paid: 2_500,
            have: 25_000,
        };
        assert_eq!(payment_text(&p), "Pyreal 2,500 / 10,000 (have 25,000)");
    }
}
