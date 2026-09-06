//! Buffs: the enchantment registry (`Client::enchantments`) as two compact
//! lists, beneficial and harmful, each entry with its icon, name and time
//! left, soonest to expire first; vitae on its own line. U toggles it (on
//! by default).
//!
//! Time left is `start_time + duration - elapsed`: ACE sends `start_time`
//! as the (non-positive) number of seconds the spell has already run,
//! counting down from 0, so the client needs no server clock, only how
//! long ago it saw that value ([`Anchors`]). A positive `start_time` would
//! be an absolute server time the client has no base for; those entries
//! show the full duration marked `~`. Item spells and vitae have no timer
//! (`duration < 0`).

use std::collections::HashMap;
use std::time::Instant;

use ac_formats::spell_table::SpellTable;
use ac_world::stats::Enchantment;

use super::{caption, fmt_seconds, has_sheet, window, Source};
use crate::icons::{IconCache, IconLayers};
use crate::{egui, Client, Ctx, Plugin};

/// The vitae penalty's spell id (ACE `SpellId.Vitae`).
pub const VITAE_SPELL: u16 = 666;

/// One active enchantment.
#[derive(Clone, Debug, PartialEq)]
pub struct Buff {
    pub spell: u16,
    pub layer: u16,
    pub name: String,
    /// RenderSurface (0x06) id of the spell icon.
    pub icon: u32,
    /// Seconds left; `None` for spells without a timer.
    pub remaining: Option<f64>,
    /// The client could not anchor `start_time`: `remaining` is the full
    /// duration.
    pub approx: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuffsView {
    pub beneficial: Vec<Buff>,
    pub harmful: Vec<Buff>,
    /// Vitae penalty in percent (5 for 95% vitae), if any.
    pub vitae: Option<u32>,
}

/// Seconds left on an enchantment seen `elapsed` seconds ago with the
/// given `start_time` and `duration`; `None` without a timer. The flag is
/// set when `start_time` is an absolute time the client cannot anchor.
pub fn remaining(start_time: f64, duration: f64, elapsed: f64) -> Option<(f64, bool)> {
    if duration < 0.0 {
        return None;
    }
    if start_time > 0.0 {
        return Some((duration, true));
    }
    Some(((duration + start_time - elapsed).max(0.0), false))
}

/// `0.95` (95% vitae) -> `5`.
pub fn vitae_percent(stat_mod_value: f32) -> u32 {
    ((1.0 - stat_mod_value as f64) * 100.0).round().max(0.0) as u32
}

/// Soonest to expire first; timerless entries last; by name within.
pub fn sort(buffs: &mut [Buff]) {
    buffs.sort_by(|a, b| {
        match (a.remaining, b.remaining) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then_with(|| a.name.cmp(&b.name))
    });
}

/// When each enchantment's `start_time` was last seen, so its countdown
/// runs on the local clock between server updates.
#[derive(Debug, Default)]
pub struct Anchors {
    seen: HashMap<(u16, u16), (f64, Instant)>,
}

impl Anchors {
    /// Seconds since `start_time` was first seen for this enchantment
    /// (0 when it is new or changed, which a refresh does).
    pub fn elapsed(&mut self, spell: u16, layer: u16, start_time: f64, now: Instant) -> f64 {
        let e = self.seen.entry((spell, layer)).or_insert((start_time, now));
        if e.0 != start_time {
            *e = (start_time, now);
        }
        now.duration_since(e.1).as_secs_f64()
    }

    /// Forget enchantments that are gone.
    pub fn retain(&mut self, live: &[Enchantment]) {
        self.seen
            .retain(|k, _| live.iter().any(|e| (e.spell_id, e.layer) == *k));
    }
}

/// Build the view from the registry, naming spells through `table`.
pub fn build(
    table: Option<&SpellTable>,
    enchantments: &[Enchantment],
    anchors: &mut Anchors,
    now: Instant,
) -> BuffsView {
    let mut v = BuffsView::default();
    anchors.retain(enchantments);
    for e in enchantments {
        if e.spell_id == VITAE_SPELL {
            v.vitae = Some(vitae_percent(e.stat_mod_value));
            continue;
        }
        let sp = table.and_then(|t| t.get(e.spell_id as u32));
        let elapsed = anchors.elapsed(e.spell_id, e.layer, e.start_time, now);
        let (remaining, approx) = match remaining(e.start_time, e.duration, elapsed) {
            Some((r, a)) => (Some(r), a),
            None => (None, false),
        };
        let buff = Buff {
            spell: e.spell_id,
            layer: e.layer,
            name: sp
                .map(|s| s.name.clone())
                .unwrap_or_else(|| format!("spell {}", e.spell_id)),
            icon: sp.map(|s| s.icon_id).unwrap_or(0),
            remaining,
            approx,
        };
        if sp.is_some_and(|s| s.is_beneficial()) {
            v.beneficial.push(buff);
        } else {
            v.harmful.push(buff);
        }
    }
    sort(&mut v.beneficial);
    sort(&mut v.harmful);
    v
}

/// This session's enchantments; `None` until the sheet arrived.
pub fn view(c: &Client, anchors: &mut Anchors, now: Instant) -> Option<BuffsView> {
    if !has_sheet(c) {
        return None;
    }
    let table = c.assets.spell_table().ok();
    Some(build(table.as_deref(), c.enchantments(), anchors, now))
}

fn list(
    ui: &mut egui::Ui,
    icons: &mut IconCache,
    header: &str,
    buffs: &[Buff],
    color: egui::Color32,
) {
    if buffs.is_empty() {
        return;
    }
    caption(ui, format!("{header} ({})", buffs.len()));
    for b in buffs {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            icons.draw(ui, IconLayers::single(b.icon), egui::Sense::hover());
            ui.label(egui::RichText::new(&b.name).color(color));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let time = match b.remaining {
                    Some(r) if b.approx => format!("~{}", fmt_seconds(r)),
                    Some(r) => fmt_seconds(r),
                    None => "--".to_string(),
                };
                ui.label(
                    egui::RichText::new(time)
                        .color(egui::Color32::from_gray(200))
                        .small(),
                );
            });
        });
    }
}

/// Draw the panel under the inventory, on the right.
pub fn draw(egui: &egui::Context, icons: &mut IconCache, v: &BuffsView) {
    let w = egui.viewport_rect().width();
    let r = super::radar::RADIUS;
    window(
        "buffs",
        egui::pos2(w - 348.0 - 268.0, 2.0 * r + 40.0),
        egui::vec2(260.0, 200.0),
        170,
        6,
    )
    .show(egui, |ui| {
        ui.set_min_width(248.0);
        ui.horizontal(|ui| {
            super::title(ui, "Buffs");
            if let Some(p) = v.vitae {
                ui.label(
                    egui::RichText::new(format!("Vitae -{p}%"))
                        .color(egui::Color32::from_rgb(255, 150, 120))
                        .strong(),
                );
            }
        });
        egui::ScrollArea::vertical()
            .max_height(170.0)
            .show(ui, |ui| {
                ui.set_min_width(240.0);
                if v.beneficial.is_empty() && v.harmful.is_empty() {
                    caption(ui, "(no active spells)");
                }
                list(
                    ui,
                    icons,
                    "Beneficial",
                    &v.beneficial,
                    egui::Color32::from_rgb(180, 230, 180),
                );
                list(
                    ui,
                    icons,
                    "Harmful",
                    &v.harmful,
                    egui::Color32::from_rgb(255, 170, 150),
                );
            });
    });
}

pub struct Buffs {
    source: Source<BuffsView>,
    /// Open (U toggles it). Starts open.
    pub show: bool,
    anchors: Anchors,
}

impl Default for Buffs {
    fn default() -> Self {
        Buffs {
            source: Source::Live,
            show: true,
            anchors: Anchors::default(),
        }
    }
}

impl Buffs {
    /// Three real enchantments (two buffs, one debuff) and a vitae line
    /// when the table is given, open.
    pub fn demo(table: Option<&SpellTable>) -> Self {
        let find = |name: &str, fallback: u16| -> u16 {
            table
                .and_then(|t| {
                    t.spells
                        .iter()
                        .find(|(_, s)| s.name == name)
                        .map(|(id, _)| *id as u16)
                })
                .unwrap_or(fallback)
        };
        let ench = |spell: u16, start_time: f64, duration: f64, layer: u16| Enchantment {
            spell_id: spell,
            layer,
            start_time,
            duration,
            ..Default::default()
        };
        let list = [
            ench(find("Strength Self I", 1), -120.0, 1800.0, 1),
            ench(find("Focus Self I", 2), -1500.0, 1800.0, 1),
            ench(find("Weakness Other I", 3), -20.0, 300.0, 1),
            Enchantment {
                spell_id: VITAE_SPELL,
                duration: -1.0,
                stat_mod_value: 0.9,
                ..Default::default()
            },
        ];
        let mut anchors = Anchors::default();
        let v = build(table, &list, &mut anchors, Instant::now());
        Buffs {
            source: Source::Demo(v),
            show: true,
            anchors,
        }
    }
}

impl Plugin for Buffs {
    fn name(&self) -> &str {
        "buffs"
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if !self.show {
            return;
        }
        let now = cx.now;
        let v = match &self.source {
            Source::Demo(d) => Some(d.clone()),
            Source::Live => cx
                .try_client()
                .and_then(|c| view(c, &mut self.anchors, now)),
        };
        if let Some(v) = v {
            draw(egui, cx.icons(), &v);
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if key == egui::Key::U && pressed {
            self.show = !self.show;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn remaining_counts_down_from_the_server_offset() {
        // 30 minutes, cast 2 minutes before the message, seen 10 s ago.
        assert_eq!(remaining(-120.0, 1800.0, 10.0), Some((1670.0, false)));
        // Never negative.
        assert_eq!(remaining(-1800.0, 1800.0, 5.0), Some((0.0, false)));
        // No timer.
        assert_eq!(remaining(0.0, -1.0, 5.0), None);
        // Absolute start time: unanchored, full duration.
        assert_eq!(remaining(12345.0, 600.0, 5.0), Some((600.0, true)));
    }

    #[test]
    fn vitae_reads_as_a_penalty() {
        assert_eq!(vitae_percent(0.95), 5);
        assert_eq!(vitae_percent(0.6), 40);
        assert_eq!(vitae_percent(1.0), 0);
    }

    #[test]
    fn anchors_reset_when_the_server_refreshes() {
        let mut a = Anchors::default();
        let t0 = Instant::now();
        assert_eq!(a.elapsed(1, 1, -10.0, t0), 0.0);
        let t1 = t0 + Duration::from_secs(5);
        assert!((a.elapsed(1, 1, -10.0, t1) - 5.0).abs() < 1e-6);
        // A recast comes with a new start time: the countdown restarts.
        assert_eq!(a.elapsed(1, 1, 0.0, t1), 0.0);
        a.retain(&[]);
        assert_eq!(a.elapsed(1, 1, -3.0, t1), 0.0);
    }

    #[test]
    fn sorted_soonest_first_timerless_last() {
        let b = |name: &str, remaining| Buff {
            spell: 0,
            layer: 0,
            name: name.into(),
            icon: 0,
            remaining,
            approx: false,
        };
        let mut list = vec![
            b("item", None),
            b("long", Some(900.0)),
            b("short", Some(30.0)),
        ];
        sort(&mut list);
        let names: Vec<&str> = list.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["short", "long", "item"]);
    }

    #[test]
    fn demo_splits_buffs_and_vitae() {
        let Buffs {
            source: Source::Demo(v),
            ..
        } = Buffs::demo(None)
        else {
            panic!("demo() is not a demo source");
        };
        // Without a table nothing is known to be beneficial.
        assert_eq!(v.beneficial.len() + v.harmful.len(), 3);
        assert_eq!(v.vitae, Some(10));
        assert!(v
            .harmful
            .windows(2)
            .all(|w| w[0].remaining <= w[1].remaining));
    }
}
