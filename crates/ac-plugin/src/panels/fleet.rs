//! The fleet (from the menu): every character being played, in this
//! process and in every other process on the bus, one row each, with
//! the controls a leader needs while playing one client by hand and
//! having the rest follow (see `docs/multi-session.md`, "Fleet view").
//!
//! The rows come from two places. This process's sessions are read
//! directly (`team::describe`, the same word the team plugin puts on
//! the bus), whether or not their team rules are on. Other processes'
//! sessions are heard on `autoplay.mate` (kept in a [`Roster`] of this
//! panel's own, so it does not depend on the team plugin's) and what
//! each is doing on `autoplay.event`. Experience an hour is worked out
//! here, from the totals seen over the last quarter of an hour.
//!
//! Controls on a session of this process act on it at once; on another
//! process's session they go out as a `fleet.request`, which the team
//! plugin over there applies (`team::Request`). A row of this process
//! is clickable to switch the window to it.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use super::{caption, title_bar, window, Source};
use crate::team::{self, Request, Roster, MATE_TOPIC, REQUEST_TOPIC};
use crate::{egui, Ctx, Plugin, Settings};
use ac_client::autoplay::Mate;

/// Where a character is played: the process (this one, or a name on
/// the bus) and the session within it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub process: String,
    pub session: usize,
}

impl Key {
    pub fn new(process: &str, session: usize) -> Self {
        Key {
            process: process.to_string(),
            session,
        }
    }
}

/// One character, as the panel draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: Key,
    /// A session of this process: clickable to switch to.
    pub local: bool,
    /// The session the window shows right now.
    pub active: bool,
    pub name: String,
    pub leader: bool,
    pub level: i32,
    pub health: f32,
    pub stamina: f32,
    pub mana: f32,
    /// Experience an hour, once enough has been seen to say.
    pub xp_per_hour: Option<f64>,
    pub available_xp: i64,
    /// The nearest town (and a landmark, standing at one).
    pub place: String,
    /// Metres to the leader; `None` for the leader itself.
    pub to_leader: Option<f32>,
    /// The last autoplay line: "fighting Drudge Skulker".
    pub doing: String,
    pub autoplay: bool,
    pub following: bool,
    pub flying: bool,
    pub in_fellowship: bool,
}

impl Row {
    pub fn alive(&self) -> bool {
        self.health > 0.0
    }
}

/// What the panel draws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FleetView {
    pub rows: Vec<Row>,
    /// A bus is attached: other processes can be seen.
    pub on_bus: bool,
}

/// The header's sums.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Totals {
    pub sessions: usize,
    pub alive: usize,
    /// Mean health fraction over everyone.
    pub avg_health: f32,
    /// Experience an hour, summed over those with a rate.
    pub xp_per_hour: f64,
}

impl FleetView {
    pub fn totals(&self) -> Totals {
        let n = self.rows.len();
        Totals {
            sessions: n,
            alive: self.rows.iter().filter(|r| r.alive()).count(),
            avg_health: if n == 0 {
                0.0
            } else {
                self.rows.iter().map(|r| r.health).sum::<f32>() / n as f32
            },
            xp_per_hour: self.rows.iter().filter_map(|r| r.xp_per_hour).sum(),
        }
    }
}

/// Samples are taken at most this often...
const SAMPLE_EVERY: Duration = Duration::from_secs(5);
/// ...kept this long...
const WINDOW: Duration = Duration::from_secs(15 * 60);
/// ...and a rate is not quoted before this much has passed.
const MIN_SPAN: Duration = Duration::from_secs(30);

/// Experience an hour per character, from the totals seen over time:
/// the gain since the oldest sample in the window, over the time since.
#[derive(Debug, Default)]
pub struct XpMeter {
    samples: BTreeMap<Key, VecDeque<(Instant, i64)>>,
}

impl XpMeter {
    /// Note `total_xp` for `key` at `now`. Samples closer together than
    /// [`SAMPLE_EVERY`] are skipped; a total that went down (another
    /// character logged in on the same session) starts over.
    pub fn sample(&mut self, key: &Key, total_xp: i64, now: Instant) {
        let s = self.samples.entry(key.clone()).or_default();
        if let Some((t, last)) = s.back() {
            if now.duration_since(*t) < SAMPLE_EVERY {
                return;
            }
            if total_xp < *last {
                s.clear();
            }
        }
        s.push_back((now, total_xp));
        while s
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) > WINDOW)
        {
            s.pop_front();
        }
    }

    /// XP an hour for `key` as of `now`, or `None` until [`MIN_SPAN`]
    /// has been watched.
    pub fn rate(&self, key: &Key, now: Instant) -> Option<f64> {
        let s = self.samples.get(key)?;
        let (t0, x0) = *s.front()?;
        let (_, x1) = *s.back()?;
        let span = now.duration_since(t0);
        if span < MIN_SPAN {
            return None;
        }
        Some((x1 - x0) as f64 / span.as_secs_f64() * 3600.0)
    }

    /// Drop the characters `keep` does not.
    pub fn retain(&mut self, keep: impl Fn(&Key) -> bool) {
        self.samples.retain(|k, _| keep(k));
    }
}

/// A character as seen this frame, before it becomes a [`Row`].
#[derive(Clone, Debug)]
pub struct Seen {
    pub key: Key,
    pub local: bool,
    pub active: bool,
    pub mate: Mate,
    pub doing: String,
}

/// The rows, leader first, then by name.
pub fn rows(seen: &[Seen], xp: &XpMeter, now: Instant) -> Vec<Row> {
    let leader = team::leader_name(seen.iter().map(|s| &s.mate));
    let leader_at = seen
        .iter()
        .find(|s| leader.as_deref() == Some(s.mate.name.as_str()))
        .map(|s| s.mate.world);
    let mut rows: Vec<Row> = seen
        .iter()
        .map(|s| {
            let m = &s.mate;
            let is_leader = leader.as_deref() == Some(m.name.as_str());
            Row {
                key: s.key.clone(),
                local: s.local,
                active: s.active,
                name: m.name.clone(),
                leader: is_leader,
                level: m.level,
                health: m.health,
                stamina: m.stamina,
                mana: m.mana,
                xp_per_hour: xp.rate(&s.key, now),
                available_xp: m.available_xp,
                place: place_of(m.world),
                to_leader: if is_leader {
                    None
                } else {
                    leader_at.map(|l| l.truncate().distance(m.world.truncate()))
                },
                doing: s.doing.clone(),
                autoplay: m.autoplay,
                following: m.following,
                flying: m.flying,
                in_fellowship: m.in_fellowship,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.leader
            .cmp(&a.leader)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.key.cmp(&b.key))
    });
    rows
}

/// Within this many metres a character is "in" a town.
const IN_TOWN: f32 = 150.0;
/// Within this many metres a character stands "at" a landmark.
const AT_LANDMARK: f32 = 25.0;

/// The nearest town, with the distance to it when out of town, and
/// the landmark stood at, if any: "Holtburg", "Holtburg 1.2 km",
/// "Holtburg (Life Stone)".
pub fn place_of(world: glam::Vec3) -> String {
    let xy = world.truncate();
    let town = ac_world::towns::PLACES
        .iter()
        .map(|p| (p, p.world_xy().distance(xy)))
        .min_by(|a, b| a.1.total_cmp(&b.1));
    let mut s = match town {
        Some((p, d)) if d <= IN_TOWN => p.name.to_string(),
        Some((p, d)) => format!("{} {}", p.name, fmt_distance(d)),
        None => String::new(),
    };
    if let Some(l) = ac_world::landmarks::near(xy, AT_LANDMARK).first() {
        s = format!("{s} ({})", l.name);
    }
    s
}

/// `15 m`, `1.2 km`.
pub fn fmt_distance(m: f32) -> String {
    if m < 1000.0 {
        format!("{} m", m.round() as i64)
    } else {
        format!("{:.1} km", m / 1000.0)
    }
}

/// `850`, `12.3k`, `1.2M`, for an XP an hour figure.
pub fn fmt_xp(x: f64) -> String {
    let a = x.abs();
    if a >= 1e6 {
        format!("{:.1}M", x / 1e6)
    } else if a >= 1e3 {
        format!("{:.1}k", x / 1e3)
    } else {
        format!("{}", x.round() as i64)
    }
}

/// What the panel's draw returned.
#[derive(Debug, Default, PartialEq)]
pub struct Actions {
    /// A row of this process was clicked: show that session.
    pub activate: Option<usize>,
    /// Requests made of rows.
    pub ask: Vec<(Key, Request)>,
    /// The compact toggle was clicked.
    pub compact: Option<bool>,
}

const HEALTH: egui::Color32 = egui::Color32::from_rgb(200, 40, 40);
const STAMINA: egui::Color32 = egui::Color32::from_rgb(220, 180, 40);
const MANA: egui::Color32 = egui::Color32::from_rgb(50, 90, 220);
const GOLD: egui::Color32 = egui::Color32::from_rgb(255, 210, 90);

/// A small filled bar with its percentage on it.
fn bar(ui: &mut egui::Ui, width: f32, frac: f32, color: egui::Color32) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 14.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.0, egui::Color32::from_gray(40));
    let mut fill = rect;
    fill.set_width(rect.width() * frac.clamp(0.0, 1.0));
    p.rect_filled(fill, 2.0, color);
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("{}%", (frac.clamp(0.0, 1.0) * 100.0).round() as i32),
        egui::FontId::proportional(11.0),
        egui::Color32::WHITE,
    );
    resp.on_hover_text(format!("{:.0}%", frac.clamp(0.0, 1.0) * 100.0));
}

/// The name cell: a star for the leader, "me" for the session shown,
/// clickable for a session of this process.
fn name_cell(ui: &mut egui::Ui, r: &Row, a: &mut Actions) {
    ui.horizontal(|ui| {
        if r.leader {
            ui.label(egui::RichText::new("★").color(GOLD))
                .on_hover_text("The leader: the others follow it");
        }
        let colour = if !r.alive() {
            egui::Color32::from_gray(140)
        } else if r.active {
            GOLD
        } else {
            egui::Color32::WHITE
        };
        let text = egui::RichText::new(&r.name).color(colour).strong();
        if r.local {
            let resp = ui
                .add(egui::Button::new(text).frame(false))
                .on_hover_text("A session of this process: click to switch to it");
            if resp.clicked() {
                a.activate = Some(r.key.session);
            }
        } else {
            ui.label(text)
                .on_hover_text(format!("Played by process \"{}\"", r.key.process));
        }
        if r.active {
            caption(ui, "me");
        }
    });
}

/// The flags cell: following, flying, in a fellowship.
fn flags(r: &Row) -> String {
    let mut f = Vec::new();
    if r.following {
        f.push("following");
    }
    if r.flying {
        f.push("flying");
    }
    if r.in_fellowship {
        f.push("fellow");
    }
    if !r.alive() {
        f.push("dead");
    }
    f.join(", ")
}

/// The per-row controls: autoplay and follow toggles, and stop.
fn controls(ui: &mut egui::Ui, r: &Row, a: &mut Actions) {
    let mut auto = r.autoplay;
    if ui
        .checkbox(&mut auto, "")
        .on_hover_text("Play on its own")
        .changed()
    {
        a.ask.push((
            r.key.clone(),
            if auto {
                Request::AutoplayOn
            } else {
                Request::AutoplayOff
            },
        ));
    }
    let mut follow = r.following;
    if ui
        .checkbox(&mut follow, "")
        .on_hover_text("Follow the leader")
        .changed()
    {
        a.ask.push((
            r.key.clone(),
            if follow {
                Request::FollowOn
            } else {
                Request::FollowOff
            },
        ));
    }
    if ui
        .add(egui::Button::new("stop").small())
        .on_hover_text("Autoplay off, journey and fight cancelled")
        .clicked()
    {
        a.ask.push((r.key.clone(), Request::Stop));
    }
}

/// The header under the title: the sums, the all-hands buttons and the
/// compact toggle.
fn header(ui: &mut egui::Ui, v: &FleetView, compact: bool, a: &mut Actions) {
    let t = v.totals();
    ui.horizontal(|ui| {
        caption(
            ui,
            format!(
                "{} session{}, {} alive, health {}%, {} XP/h",
                t.sessions,
                if t.sessions == 1 { "" } else { "s" },
                t.alive,
                (t.avg_health * 100.0).round() as i32,
                fmt_xp(t.xp_per_hour)
            ),
        );
        if !v.on_bus {
            caption(ui, "· this process only (no --bus)");
        }
    });
    ui.horizontal(|ui| {
        let all = |a: &mut Actions, r: Request| {
            for row in &v.rows {
                a.ask.push((row.key.clone(), r));
            }
        };
        ui.label(egui::RichText::new("All:").small());
        if ui
            .add(egui::Button::new("autoplay on").small())
            .on_hover_text("Everyone plays on their own")
            .clicked()
        {
            all(a, Request::AutoplayOn);
        }
        if ui.add(egui::Button::new("autoplay off").small()).clicked() {
            all(a, Request::AutoplayOff);
        }
        if ui
            .add(egui::Button::new("regroup").small())
            .on_hover_text("Everyone drops their fight and comes to the leader")
            .clicked()
        {
            all(a, Request::Regroup);
        }
        if ui
            .add(egui::Button::new("stop").small())
            .on_hover_text("Everyone: autoplay off, journeys and fights cancelled")
            .clicked()
        {
            all(a, Request::Stop);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut c = compact;
            if ui
                .checkbox(&mut c, egui::RichText::new("compact").small())
                .on_hover_text("One line per character")
                .changed()
            {
                a.compact = Some(c);
            }
        });
    });
}

/// The full table.
fn table(ui: &mut egui::Ui, v: &FleetView, a: &mut Actions) {
    egui::Grid::new("fleet.rows")
        .striped(true)
        .spacing([10.0, 4.0])
        .min_col_width(20.0)
        .show(ui, |ui| {
            for h in [
                "Name", "From", "Lvl", "Health", "Stam", "Mana", "XP/h", "Where", "Leader",
                "Doing", "Flags", "Auto", "Follow", "",
            ] {
                caption(ui, h);
            }
            ui.end_row();
            for r in &v.rows {
                name_cell(ui, r, a);
                ui.label(
                    egui::RichText::new(if r.local {
                        "here".to_string()
                    } else {
                        r.key.process.clone()
                    })
                    .small(),
                );
                ui.label(format!("{}", r.level));
                bar(ui, 56.0, r.health, HEALTH);
                bar(ui, 44.0, r.stamina, STAMINA);
                bar(ui, 44.0, r.mana, MANA);
                match r.xp_per_hour {
                    Some(x) => {
                        ui.label(fmt_xp(x))
                            .on_hover_text(format!("{} XP unspent", r.available_xp));
                    }
                    None => {
                        ui.label(egui::RichText::new("…").weak())
                            .on_hover_text("Not watched long enough yet");
                    }
                }
                ui.label(&r.place);
                ui.label(match r.to_leader {
                    Some(d) => fmt_distance(d),
                    None if r.leader => "—".to_string(),
                    None => "?".to_string(),
                });
                ui.add(egui::Label::new(&r.doing).truncate());
                ui.label(egui::RichText::new(flags(r)).small());
                controls(ui, r, a);
                ui.end_row();
            }
        });
}

/// One line per character, for a leader's screen.
fn lines(ui: &mut egui::Ui, v: &FleetView, a: &mut Actions) {
    for r in &v.rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            name_cell(ui, r, a);
            ui.label(egui::RichText::new(format!("L{}", r.level)).small());
            bar(ui, 40.0, r.health, HEALTH);
            bar(ui, 30.0, r.stamina, STAMINA);
            bar(ui, 30.0, r.mana, MANA);
            ui.label(
                egui::RichText::new(match r.xp_per_hour {
                    Some(x) => format!("{}/h", fmt_xp(x)),
                    None => "…/h".to_string(),
                })
                .small(),
            );
            let mut where_ = r.place.clone();
            if let Some(d) = r.to_leader {
                where_ = format!("{where_}, {} off", fmt_distance(d));
            }
            ui.label(egui::RichText::new(where_).small());
            ui.add(egui::Label::new(egui::RichText::new(&r.doing).small()).truncate());
            let f = flags(r);
            if !f.is_empty() {
                caption(ui, f);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                controls(ui, r, a);
            });
        });
    }
}

/// Draw the panel. `compact` picks one line per character over the
/// table.
pub fn draw(egui: &egui::Context, v: &FleetView, compact: bool) -> Actions {
    let mut a = Actions::default();
    let w = egui.viewport_rect().width();
    let width = if compact { 640.0 } else { 920.0 };
    let rows = v.rows.len().max(1) as f32;
    let height = (if compact {
        78.0 + rows * 22.0
    } else {
        100.0 + rows * 24.0
    })
    .min(420.0);
    window(
        "fleet",
        egui::pos2((w - width) * 0.5, 40.0),
        egui::vec2(width, height),
        220,
        8,
    )
    .show(egui, |ui| {
        ui.set_min_width(width - 16.0);
        title_bar(ui, "fleet", "Fleet");
        header(ui, v, compact, &mut a);
        ui.separator();
        if v.rows.is_empty() {
            ui.weak("Nobody in the world yet.");
            return;
        }
        egui::ScrollArea::vertical()
            .max_height(height - 90.0)
            .show(ui, |ui| {
                if compact {
                    lines(ui, v, &mut a);
                } else {
                    table(ui, v, &mut a);
                }
            });
    });
    a
}

/// The panel.
#[derive(Default)]
pub struct Fleet {
    source: Source<FleetView>,
    pub show: bool,
    pub compact: bool,
    /// Other processes' sessions, as last heard.
    roster: Roster,
    /// The last `autoplay.event` line of each other process's session.
    events: BTreeMap<Key, String>,
    xp: XpMeter,
}

impl Fleet {
    pub fn demo() -> Self {
        let now = Instant::now();
        let holt = ac_world::towns::find("Holtburg")
            .map(|p| p.world_xy())
            .unwrap_or_default();
        let at = |dx: f32, dy: f32| glam::Vec3::new(holt.x + dx, holt.y + dy, 100.0);
        let mate = |name: &str, guid, level, h, s, m, world| Mate {
            name: name.into(),
            guid,
            level,
            health: h,
            stamina: s,
            mana: m,
            world,
            total_xp: 1_000_000,
            available_xp: 12_345,
            ..Default::default()
        };
        let mut xp = XpMeter::default();
        let key = |p: &str, s| Key::new(p, s);
        for (k, gain) in [
            (key("local", 0), 60_000),
            (key("local", 1), 45_000),
            (key("bob", 0), 30_000),
        ] {
            xp.sample(&k, 1_000_000, now - Duration::from_secs(600));
            xp.sample(&k, 1_000_000 + gain, now - Duration::from_secs(10));
        }
        let mut leader = mate("Reborn", 1, 42, 0.82, 0.6, 0.4, at(20.0, 5.0));
        leader.leads = true;
        leader.autoplay = false;
        let mut alys = mate("Alys", 2, 38, 0.55, 0.9, 0.7, at(24.0, -3.0));
        alys.autoplay = true;
        alys.following = true;
        alys.in_fellowship = true;
        let mut bran = mate("Brannoc", 3, 40, 0.97, 0.3, 0.1, at(600.0, 900.0));
        bran.autoplay = true;
        bran.following = true;
        bran.flying = true;
        let ceol = mate("Ceol", 4, 35, 0.0, 0.0, 0.2, at(-30.0, 40.0));
        let seen = vec![
            Seen {
                key: key("local", 0),
                local: true,
                active: true,
                mate: leader,
                doing: "waiting".into(),
            },
            Seen {
                key: key("local", 1),
                local: true,
                active: false,
                mate: alys,
                doing: "fighting Drudge Skulker".into(),
            },
            Seen {
                key: key("bob", 0),
                local: false,
                active: false,
                mate: bran,
                doing: "following: 1.1 km to go".into(),
            },
            Seen {
                key: key("bob", 1),
                local: false,
                active: false,
                mate: ceol,
                doing: "recovering: at the lifestone".into(),
            },
        ];
        Fleet {
            source: Source::Demo(FleetView {
                rows: rows(&seen, &xp, now),
                on_bus: true,
            }),
            show: true,
            ..Default::default()
        }
    }

    /// Hear the bus: other processes' words about themselves, and what
    /// each is doing.
    fn hear(&mut self, cx: &mut Ctx) {
        let now = cx.now;
        for m in cx.board.messages_on(MATE_TOPIC).filter(|m| m.is_remote()) {
            let Some(origin) = m.origin.as_deref() else {
                continue;
            };
            let Ok(mate) = serde_json::from_value::<Mate>(m.value.clone()) else {
                continue;
            };
            let key = Key::new(origin, mate.session);
            self.xp.sample(&key, mate.total_xp, now);
            self.roster.hear(origin, mate.session, mate, now);
        }
        for m in cx
            .board
            .messages_on(crate::host::AUTOPLAY_TOPIC)
            .filter(|m| m.is_remote())
        {
            let (Some(origin), Some(session)) = (
                m.origin.as_deref(),
                m.value.get("session").and_then(|s| s.as_u64()),
            ) else {
                continue;
            };
            let text = m
                .value
                .get("text")
                .and_then(|t| t.as_str())
                .unwrap_or_default();
            self.events
                .insert(Key::new(origin, session as usize), text.to_string());
        }
        // A process that went away stops speaking: its rows go too.
        self.roster.forget_quiet(now);
        let mut gone: Vec<Key> = self.events.keys().cloned().collect();
        gone.retain(|k| {
            !self
                .roster
                .iter()
                .any(|(p, s, _)| p == k.process && s == k.session)
        });
        for k in gone {
            self.events.remove(&k);
        }
    }

    /// The name this process goes by on the bus.
    fn me(cx: &Ctx) -> String {
        cx.board.bus_name().unwrap_or("local").to_string()
    }

    /// Everyone seen right now: this process's sessions, then the
    /// roster's.
    fn seen(&self, cx: &Ctx) -> Vec<Seen> {
        let me = Self::me(cx);
        let mut seen: Vec<Seen> = cx
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let mate = team::describe(c, i)?;
                let doing =
                    super::autoplay::status_line(c.autoplay.doing.label(), &c.autoplay.status);
                Some(Seen {
                    key: Key::new(&me, i),
                    local: true,
                    active: i == cx.index,
                    mate,
                    doing,
                })
            })
            .collect();
        for (process, session, mate) in self.roster.iter() {
            if process == me {
                continue;
            }
            let key = Key::new(process, session);
            let doing = self.events.get(&key).cloned().unwrap_or_default();
            seen.push(Seen {
                key,
                local: false,
                active: false,
                mate: mate.clone(),
                doing,
            });
        }
        seen
    }
}

impl Plugin for Fleet {
    fn name(&self) -> &str {
        "fleet"
    }

    fn load(&mut self, settings: &Settings) {
        if let Some(v) = settings.get("fleet.show") {
            self.show = v;
        }
        if let Some(v) = settings.get("fleet.compact") {
            self.compact = v;
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("fleet.show", self.show);
        settings.set("fleet.compact", self.compact);
    }

    /// Keep hearing and sampling whether or not the panel is open, so
    /// the rates are there when it opens.
    fn tick(&mut self, cx: &mut Ctx) {
        if !matches!(self.source, Source::Live) {
            return;
        }
        if cx.index == 0 {
            self.hear(cx);
        }
        let me = Self::me(cx);
        let i = cx.index;
        let now = cx.now;
        if let Some(total) = cx
            .try_client()
            .and_then(|c| (!c.world.stats.name.is_empty()).then_some(c.world.stats.total_xp))
        {
            self.xp.sample(&Key::new(&me, i), total, now);
        }
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        if let Some(ask) = super::take_open(cx.board, "fleet") {
            self.show = ask.apply(self.show);
        }
        if !self.show {
            return;
        }
        let v = match &self.source {
            Source::Demo(d) => d.clone(),
            Source::Live => FleetView {
                rows: rows(&self.seen(cx), &self.xp, cx.now),
                on_bus: cx.board.bus_name().is_some(),
            },
        };
        let a = draw(egui, &v, self.compact);
        if super::closed("fleet") {
            self.show = false;
        }
        if let Some(c) = a.compact {
            self.compact = c;
            cx.settings.set("fleet.compact", c);
        }
        if matches!(self.source, Source::Demo(_)) {
            return;
        }
        if let Some(i) = a.activate {
            if i < cx.clients.len() {
                cx.activate = Some(i);
            }
        }
        let me = Self::me(cx);
        for (key, r) in a.ask {
            if key.process == me {
                if let Some(c) = cx.clients.get_mut(key.session) {
                    r.apply(c);
                }
            } else {
                let name = v
                    .rows
                    .iter()
                    .find(|row| row.key == key)
                    .map(|row| row.name.clone())
                    .unwrap_or_default();
                cx.post(REQUEST_TOPIC, r.message(&key.process, key.session, &name));
            }
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if pressed && crate::keys::bound("fleet", key) {
            self.show = !self.show;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(name: &str, process: &str, session: usize, leads: bool) -> Seen {
        Seen {
            key: Key::new(process, session),
            local: process == "local",
            active: false,
            mate: Mate {
                name: name.into(),
                guid: session as u32 + 1,
                leads,
                health: 1.0,
                ..Default::default()
            },
            doing: String::new(),
        }
    }

    #[test]
    fn xp_an_hour_needs_a_while_and_slides() {
        let t0 = Instant::now();
        let k = Key::new("local", 0);
        let mut m = XpMeter::default();
        m.sample(&k, 1000, t0);
        assert_eq!(m.rate(&k, t0), None, "nothing to say yet");
        // Too soon after the last: not kept.
        m.sample(&k, 1500, t0 + Duration::from_secs(2));
        m.sample(&k, 2000, t0 + Duration::from_secs(60));
        let r = m.rate(&k, t0 + Duration::from_secs(60)).unwrap();
        assert!((r - 60_000.0).abs() < 1.0, "1000 XP in a minute: {r}");
        // Standing still, the rate falls as time passes.
        let r = m.rate(&k, t0 + Duration::from_secs(120)).unwrap();
        assert!((r - 30_000.0).abs() < 1.0, "{r}");
        // Out of the window the old samples go, and the rate is over
        // what remains.
        m.sample(&k, 2000, t0 + WINDOW + Duration::from_secs(30));
        let r = m.rate(&k, t0 + WINDOW + Duration::from_secs(30)).unwrap();
        assert!(r.abs() < 1.0, "no gain in the window: {r}");
        // A total that went down starts over.
        m.sample(&k, 10, t0 + WINDOW + Duration::from_secs(60));
        assert_eq!(m.rate(&k, t0 + WINDOW + Duration::from_secs(60)), None);
        // Another character is unaffected and unknown.
        assert_eq!(m.rate(&Key::new("bob", 0), t0), None);
        m.retain(|key| key.process == "bob");
        assert_eq!(m.rate(&k, t0 + WINDOW + Duration::from_secs(120)), None);
    }

    #[test]
    fn the_leader_comes_first_then_the_names() {
        let now = Instant::now();
        let xp = XpMeter::default();
        let list = vec![
            seen("Zed", "bob", 1, false),
            seen("alys", "local", 1, false),
            seen("Reborn", "local", 0, true),
            seen("Brannoc", "bob", 0, false),
        ];
        let sorted = rows(&list, &xp, now);
        let names: Vec<&str> = sorted.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Reborn", "alys", "Brannoc", "Zed"]);
        assert!(sorted[0].leader && sorted[0].to_leader.is_none());
        assert!(sorted
            .iter()
            .skip(1)
            .all(|r| !r.leader && r.to_leader.is_some()));
        assert!(sorted[1].local && !sorted[2].local);
        // Nobody asked to lead: the first name (by byte order, as the
        // team plugin picks it) does, and the rest sort without case.
        let mut list = list;
        list[2].mate.leads = false;
        let sorted = rows(&list, &xp, now);
        let names: Vec<&str> = sorted.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Brannoc", "alys", "Reborn", "Zed"]);
        assert!(sorted[0].leader);
        let t = FleetView {
            rows: sorted,
            on_bus: true,
        }
        .totals();
        assert_eq!((t.sessions, t.alive), (4, 4));
        assert!((t.avg_health - 1.0).abs() < 1e-6);
        assert_eq!(t.xp_per_hour, 0.0);
    }

    #[test]
    fn places_and_figures_read_well() {
        let holt = ac_world::towns::find("Holtburg").unwrap().world_xy();
        assert_eq!(
            place_of(glam::Vec3::new(holt.x + 30.0, holt.y, 0.0)),
            "Holtburg"
        );
        assert_eq!(
            place_of(glam::Vec3::new(holt.x + 1200.0, holt.y, 0.0)),
            "Holtburg 1.2 km"
        );
        assert_eq!(fmt_distance(15.4), "15 m");
        assert_eq!(fmt_distance(1234.0), "1.2 km");
        assert_eq!(fmt_xp(850.0), "850");
        assert_eq!(fmt_xp(12_340.0), "12.3k");
        assert_eq!(fmt_xp(1_234_000.0), "1.2M");
    }

    #[test]
    fn the_demo_has_a_leader_with_rates() {
        let Source::Demo(v) = Fleet::demo().source else {
            unreachable!()
        };
        assert_eq!(v.rows.len(), 4);
        assert!(v.rows[0].leader && v.rows[0].name == "Reborn");
        assert!(v.rows.iter().take(3).all(|r| r.xp_per_hour.is_some()));
        assert_eq!(v.totals().alive, 3);
    }
}
