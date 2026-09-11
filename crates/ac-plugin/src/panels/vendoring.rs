//! The shopping, laid out so it can be watched and driven by hand.
//!
//! Autovendoring is decided in `ac-vendor` from a plain description of
//! the moment: the pack, the purse and the counter. This panel shows
//! that description, the phase the trip is in, and the one thing the
//! rules want done next -- *before* it is done -- and offers to do it.
//!
//! The point is to be able to test just the shopping, exactly as
//! autoplay runs it, without turning autoplay on. **Step** carries out
//! one act and stops, which is how a trip is read a line at a time.
//! **Run** hands the decisions back to autoplay's own pace. Neither
//! changes the rules: the same `Run::step` answers in all three places,
//! here, in autoplay, and in `cargo run -p ac-vendor --example trip`.

use ac_vendor::{Act, Snapshot};

use crate::{Ctx, Plugin, Settings};

pub const OPEN_KEY: &str = "vendoring.open";

/// What the panel draws, read fresh from the character each frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VendorView {
    pub counter: Option<String>,
    /// How far the character is standing from it.
    pub away: f32,
    pub open: bool,
    pub phase: String,
    /// What the rules would do next, and how they put it.
    pub next: Option<(String, String)>,
    pub coin: u32,
    pub notes: u32,
    pub slots_free: u32,
    pub keep_slots: u32,
    pub carried: u32,
    pub ceiling: u32,
    /// Things in the pack, and how many of those are for sale here.
    pub items: usize,
    pub for_sale: usize,
    pub sold: u32,
    pub waiting: usize,
    /// The first few things it will not sell, and why.
    pub kept: Vec<(String, String)>,
}

#[derive(Default)]
pub struct Vendoring {
    show: bool,
    /// Carry out one act on the next frame.
    step_once: bool,
    /// A made-up trip, for the layout gallery and for looking at the
    /// panel without a server.
    demo: Option<VendorView>,
}

impl Vendoring {
    /// A character halfway through a trip: low on room, a fortune in
    /// coin waiting to become notes, and two things it will not part
    /// with.
    pub fn demo() -> Self {
        Vendoring {
            show: false,
            step_once: false,
            demo: Some(VendorView {
                counter: Some("Rakk the Peddler".into()),
                away: 1.2,
                open: true,
                phase: "Selling".into(),
                next: Some((
                    "buy 3".into(),
                    "packing the takings into 3 note(s)".into(),
                )),
                coin: 1_012_400,
                notes: 14,
                slots_free: 3,
                keep_slots: 3,
                carried: 21_480,
                ceiling: 24_300,
                items: 27,
                for_sale: 9,
                sold: 7,
                waiting: 0,
                kept: vec![
                    ("Tinkered Sword".into(), "it has been tinkered".into()),
                    ("Worn Breastplate".into(), "it is being worn".into()),
                ],
            }),
        }
    }
}

/// Read the character into the panel's view, without deciding
/// anything that would change it.
fn view(c: &mut ac_client::Client) -> Option<VendorView> {
    let cfg = c.autoplay.config.growth.clone();
    let snap: Snapshot = c.vendor_snapshot(&cfg);
    let counter = snap.counter.clone();
    let for_sale = snap
        .items
        .iter()
        .filter(|i| !i.keep.forbidden() && !i.wielded && i.value > 0)
        .count();
    let kept = snap
        .items
        .iter()
        .filter_map(|i| i.keep.why().map(|w| (i.name.clone(), w.to_string())))
        .take(6)
        .collect();
    // Asking what it would do next must not move the trip on, so the
    // question is put to a copy of the run rather than to the run.
    let mut asking = ac_vendor::Run::new();
    let peek = asking.step(&snap, std::time::Instant::now());
    Some(VendorView {
        counter: counter.as_ref().map(|c| c.name.clone()),
        away: counter.as_ref().map(|c| c.away).unwrap_or(0.0),
        open: counter.is_some(),
        phase: format!("{:?}", c.autoplay.growth.shop.phase),
        next: peek.act.map(|a| (act_label(&a), peek.saying.clone())),
        coin: snap.coin,
        notes: snap.notes.values().sum(),
        slots_free: snap.slots_free,
        keep_slots: snap.rules.keep_slots,
        carried: snap.carried,
        ceiling: snap.capacity.saturating_mul(3),
        items: snap.items.len(),
        for_sale,
        sold: c.autoplay.growth.shop.sold,
        waiting: c.autoplay.growth.shop.waiting_on(),
        kept,
    })
}

/// A short name for an act, for the panel's line.
fn act_label(a: &Act) -> String {
    match a {
        Act::Approach { .. } => "walk up".into(),
        Act::Open { .. } => "open".into(),
        Act::Merge { amount, .. } => format!("merge {amount}"),
        Act::Split { amount, .. } => format!("split {amount}"),
        Act::Sell { .. } => "sell".into(),
        Act::Buy { count, .. } => format!("buy {count}"),
        Act::Cash { count, .. } => format!("cash {count}"),
        Act::Close => "close".into(),
    }
}

impl Plugin for Vendoring {
    fn name(&self) -> &str {
        "vendoring"
    }

    fn load(&mut self, settings: &Settings) {
        if let Some(v) = settings.get("vendoring.show") {
            self.show = v;
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("vendoring.show", self.show);
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if let Some(ask) = super::take_open(cx.board, "vendoring") {
            self.show = ask.apply(self.show);
        }
        cx.board.set(OPEN_KEY, self.show);
        if !self.show {
            return;
        }
        let v = match &self.demo {
            Some(d) => Some(d.clone()),
            None => cx.try_client().and_then(view),
        };
        let Some(v) = v else { return };
        let mut step = false;
        egui::Window::new("Vendoring")
            .default_width(340.0)
            .show(egui, |ui| {
                match &v.counter {
                    Some(name) => {
                        ui.label(format!("{name}, {:.1} m away", v.away));
                    }
                    None => {
                        ui.label("no counter open");
                    }
                }
                ui.separator();
                ui.label(format!("phase: {}", v.phase));
                match &v.next {
                    Some((what, saying)) => {
                        ui.label(format!("next: {what} -- {saying}"));
                    }
                    None => {
                        ui.label("next: nothing to do here");
                    }
                }
                ui.separator();
                ui.label(format!(
                    "{} slot(s) free, reserve {}",
                    v.slots_free, v.keep_slots
                ));
                ui.label(format!("{} coin, {} note(s)", v.coin, v.notes));
                ui.label(format!("burden {} of {}", v.carried, v.ceiling));
                ui.label(format!(
                    "{} item(s), {} for sale here, {} sold, {} waiting",
                    v.items, v.for_sale, v.sold, v.waiting
                ));
                if !v.kept.is_empty() {
                    ui.separator();
                    ui.label("never sold:");
                    for (name, why) in &v.kept {
                        ui.label(format!("   {name} -- {why}"));
                    }
                }
                ui.separator();
                // One act, then stop: the way to read a trip a line at
                // a time without handing the character over.
                if ui.add_enabled(self.demo.is_none(), egui::Button::new("Step")).clicked() {
                    step = true;
                }
            });
        if step {
            self.step_once = true;
        }
        if self.step_once {
            self.step_once = false;
            if let Some(c) = cx.try_client() {
                let cfg = c.autoplay.config.growth.clone();
                let snap = c.vendor_snapshot(&cfg);
                let now = std::time::Instant::now();
                let next = c.autoplay.growth.shop.step(&snap, now);
                if let Some(act) = next.act {
                    c.do_vendor_act(&act, &next.saying);
                }
            }
        }
        if super::closed("vendoring") {
            self.show = false;
        }
    }
}
