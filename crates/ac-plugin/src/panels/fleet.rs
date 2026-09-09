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
//!
//! The panel also starts the fleet. Its **Sessions** section is a
//! roster of accounts (kept in the settings under [`ROSTER_KEY`], the
//! passwords in plain text: a private-server convenience) with, for
//! each, the character to enter with, what to create when the account
//! lacks it ([`CreateSpec`]: template, heritage, sex, town) and a
//! [`Role`]. Start asks the host for a session (`Ctx::start_session`);
//! once its character stands in the world the role is applied
//! ([`apply_role`]): a follower gets team, follow and autoplay on, the
//! leader team and lead. "I lead" does the same for the session being
//! played and remembers its account ([`LEAD_KEY`]) for the next launch.
//! A script or the command line drives the same path through the
//! blackboard keys [`START_KEY`] and [`STOP_KEY`].

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use super::{caption, title_bar, window, Source};
use crate::team::{self, Request, Roster, MATE_TOPIC, REQUEST_TOPIC};
use crate::{egui, CreateSpec, Ctx, Event, Plugin, Role, SessionSpec, Settings, Value};
use ac_client::autoplay::Mate;

/// The settings key the roster (a list of [`SessionSpec`]) is kept under.
pub const ROSTER_KEY: &str = "fleet.roster";
/// The settings key naming the account whose session leads ("I lead").
pub const LEAD_KEY: &str = "fleet.lead_account";
/// A blackboard key a script or the command line sets to start
/// sessions through the panel: a [`SessionSpec`] or a list of them
/// (each is added to the roster and started). An optional `"process"`
/// field on the value names the process meant (its bus name, or
/// `"local"`); the others ignore it. Taken (set to null) once read.
pub const START_KEY: &str = "fleet.start";
/// A blackboard key naming accounts (a string or a list) whose sessions
/// to stop; `process` as for [`START_KEY`].
pub const STOP_KEY: &str = "fleet.stop";
/// The blackboard key under which the host reports why a session it
/// was asked to start for `account` could not be: `fleet.error.<account>`.
pub fn error_key(account: &str) -> String {
    format!("fleet.error.{}", account.to_ascii_lowercase())
}
/// An account asked to start that has not appeared after this long is
/// no longer "starting".
const STARTING_FOR: Duration = Duration::from_secs(15);

/// The choices the Add form offers, as the names the creation rules
/// accept (`CharacterBuild::from_options`, case-insensitive prefixes).
pub const HERITAGES: [&str; 11] = [
    "Aluvian",
    "Gharu'ndim",
    "Sho",
    "Viamontian",
    "Umbraen",
    "Penumbraen",
    "Lugian",
    "Tumerok",
    "Empyrean",
    "Undead",
    "Gearknight",
];
pub const SEXES: [(&str, &str); 2] = [("m", "male"), ("f", "female")];
pub const TEMPLATES: [&str; 7] = [
    "Adventurer",
    "Bow Hunter",
    "Swashbuckler",
    "Life Caster",
    "War Mage",
    "Wayfarer",
    "Soldier",
];
pub const TOWNS: [&str; 4] = ["Holtburg", "Shoushi", "Yaraq", "Sanamar"];

/// Switch the team rules on for a role: a follower follows, fights and
/// plays on its own; the leader leads (and does not follow); manual
/// leaves everything as it is.
pub fn apply_role(cfg: &mut ac_client::autoplay::Config, role: Role) {
    match role {
        Role::Leader => {
            cfg.team.enabled = true;
            cfg.team.lead = true;
            cfg.team.follow = false;
        }
        Role::Follower => {
            cfg.enabled = true;
            cfg.team.enabled = true;
            cfg.team.follow = true;
            cfg.team.lead = false;
        }
        Role::Manual => {}
    }
}

/// Where a roster entry stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// No session for the account.
    Stopped,
    /// Asked for, not seen yet.
    Starting,
    /// Logging in.
    Connecting,
    /// The account lacked the character; it is being created.
    Creating(String),
    /// In the world as this character.
    InWorld(String),
    Failed(String),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Stopped => "not running".into(),
            Status::Starting => "starting".into(),
            Status::Connecting => "connecting".into(),
            Status::Creating(n) => format!("character missing: creating {n}"),
            Status::InWorld(n) => format!("in world as {n}"),
            Status::Failed(why) => format!("failed: {why}"),
        }
    }

    pub fn running(&self) -> bool {
        matches!(
            self,
            Status::Starting | Status::Connecting | Status::Creating(_) | Status::InWorld(_)
        )
    }
}

/// What the panel reads off a running session to place it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub placed: bool,
    pub name: String,
    pub creating: Option<String>,
    pub create_error: Option<String>,
    /// The connection ended: why.
    pub ended: Option<String>,
}

impl Snapshot {
    pub fn of(c: &ac_client::Client, ended: Option<&str>) -> Self {
        Snapshot {
            placed: c.placed(),
            name: c.world.stats.name.clone(),
            creating: c.creating().map(str::to_string),
            create_error: c.create_error().map(str::to_string),
            ended: ended.map(str::to_string),
        }
    }
}

/// The status of a roster entry from what is known: its session (if
/// any), whether a start was asked recently, and what the host said
/// when it could not start it.
pub fn status_of(session: Option<&Snapshot>, starting: bool, error: Option<&str>) -> Status {
    match session {
        Some(s) => {
            if let Some(why) = &s.ended {
                Status::Failed(why.clone())
            } else if let Some(why) = &s.create_error {
                Status::Failed(why.clone())
            } else if s.placed {
                Status::InWorld(s.name.clone())
            } else if let Some(n) = &s.creating {
                Status::Creating(n.clone())
            } else {
                Status::Connecting
            }
        }
        None if starting => Status::Starting,
        None => match error {
            Some(why) => Status::Failed(why.to_string()),
            None => Status::Stopped,
        },
    }
}

/// One roster entry as the panel draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionRow {
    /// Index in the roster.
    pub index: usize,
    pub spec: SessionSpec,
    /// The session index in this process, when running.
    pub running: Option<usize>,
    pub status: Status,
}

/// The Add form's fields, kept between frames.
#[derive(Clone, Debug, PartialEq)]
pub struct AddForm {
    pub account: String,
    pub password: String,
    pub character: String,
    pub role: Role,
    /// Create the character when the account lacks it.
    pub create: bool,
    pub heritage: usize,
    pub sex: usize,
    pub template: usize,
    pub town: usize,
    /// Why the last Add was refused.
    pub error: Option<String>,
}

impl Default for AddForm {
    fn default() -> Self {
        AddForm {
            account: String::new(),
            password: String::new(),
            character: String::new(),
            role: Role::Follower,
            create: true,
            heritage: 0,
            sex: 0,
            template: 0,
            town: 0,
            error: None,
        }
    }
}

impl AddForm {
    /// The entry the form describes, or what is wrong with it.
    pub fn spec(&self) -> Result<SessionSpec, String> {
        let account = self.account.trim();
        if account.is_empty() {
            return Err("an account name is needed".into());
        }
        if account.contains(':') {
            return Err("an account name cannot contain ':'".into());
        }
        if self.password.is_empty() {
            return Err("a password is needed".into());
        }
        let character = self.character.trim();
        let create = if self.create {
            if !ac_client::creation::valid_name(character) {
                return Err(format!(
                    "to create it, a character name of {}..={} letters is needed",
                    ac_client::creation::NAME_MIN,
                    ac_client::creation::NAME_MAX
                ));
            }
            Some(CreateSpec {
                name: character.to_string(),
                heritage: HERITAGES.get(self.heritage).map(|s| s.to_string()),
                sex: SEXES.get(self.sex).map(|s| s.0.to_string()),
                template: TEMPLATES.get(self.template).map(|s| s.to_string()),
                town: TOWNS.get(self.town).map(|s| s.to_string()),
            })
        } else {
            None
        };
        Ok(SessionSpec {
            account: account.to_string(),
            password: self.password.clone(),
            character: (!character.is_empty() && !self.create).then(|| character.to_string()),
            create,
            role: self.role,
        })
    }

    /// Clear the fields (the dropdown choices stay) after an Add.
    pub fn clear(&mut self) {
        self.account.clear();
        self.password.clear();
        self.character.clear();
        self.error = None;
    }
}

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
    /// The roster's role for the account, for a session of this process
    /// that the roster knows.
    pub role: Option<Role>,
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
    /// The roster, with where each entry stands.
    pub sessions: Vec<SessionRow>,
    /// The session being played leads (its team rules say so).
    pub lead: bool,
    /// The account of the session being played, if any.
    pub active_account: Option<String>,
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

    /// Session `index` of `process` went: its samples go, and the
    /// sessions above it keep theirs under their new index.
    pub fn session_removed(&mut self, process: &str, index: usize) {
        let mut shifted = BTreeMap::new();
        for (k, v) in std::mem::take(&mut self.samples) {
            if k.process != process || k.session < index {
                shifted.insert(k, v);
            } else if k.session > index {
                shifted.insert(Key::new(process, k.session - 1), v);
            }
        }
        self.samples = shifted;
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
    pub role: Option<Role>,
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
                role: s.role,
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
    } else if a >= 1e5 {
        format!("{:.0}k", x / 1e3)
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
    /// Roster entries (by roster index) to start, stop, remove.
    pub start: Vec<usize>,
    pub stop: Vec<usize>,
    pub remove: Vec<usize>,
    /// A roster entry's role was changed.
    pub set_role: Vec<(usize, Role)>,
    /// The Add form was submitted.
    pub add: bool,
    /// Start every follower that is not running.
    pub start_followers: bool,
    /// "I lead" was ticked or unticked for the session being played.
    pub lead: Option<bool>,
    /// The Sessions section was opened or closed.
    pub sessions_open: Option<bool>,
    /// The "items" button: open the Items window (every character's
    /// inventory, see `super::holdings`).
    pub items: bool,
}

const HEALTH: egui::Color32 = egui::Color32::from_rgb(200, 40, 40);
const STAMINA: egui::Color32 = egui::Color32::from_rgb(220, 180, 40);
const MANA: egui::Color32 = egui::Color32::from_rgb(50, 90, 220);
const GOLD: egui::Color32 = egui::Color32::from_rgb(255, 210, 90);

/// A cell of fixed width whose text is cut with an ellipsis when it
/// does not fit (the whole of it on hover), left-aligned.
fn cell(ui: &mut egui::Ui, width: f32, text: impl Into<egui::RichText>, hover: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 16.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.add(egui::Label::new(text.into()).truncate())
                .on_hover_text(hover);
        },
    );
}

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
        if let Some(role) = r.role {
            caption(ui, role.label());
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
            if ui
                .add(egui::Button::new("items").small())
                .on_hover_text("Search every character's inventory, online or not")
                .clicked()
            {
                a.items = true;
            }
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
                cell(ui, 140.0, r.place.as_str(), &r.place);
                ui.label(match r.to_leader {
                    Some(d) => fmt_distance(d),
                    None if r.leader => "—".to_string(),
                    None => "?".to_string(),
                });
                cell(ui, 150.0, r.doing.as_str(), &r.doing);
                let f = flags(r);
                cell(ui, 104.0, egui::RichText::new(&f).small(), &f);
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
            cell(ui, 150.0, egui::RichText::new(&where_).small(), &where_);
            cell(ui, 150.0, egui::RichText::new(&r.doing).small(), &r.doing);
            let f = flags(r);
            if !f.is_empty() {
                cell(
                    ui,
                    90.0,
                    egui::RichText::new(&f)
                        .small()
                        .color(egui::Color32::from_gray(170)),
                    &f,
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                controls(ui, r, a);
            });
        });
    }
}

/// A dropdown over fixed choices.
fn combo(ui: &mut egui::Ui, id: &str, choice: &mut usize, labels: &[&str]) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(labels.get(*choice).copied().unwrap_or("?"))
        .width(96.0)
        .show_ui(ui, |ui| {
            for (i, l) in labels.iter().enumerate() {
                if ui.selectable_value(choice, i, *l).changed() {
                    changed = true;
                }
            }
        });
    changed
}

/// A role dropdown.
fn role_combo(ui: &mut egui::Ui, id: &str, role: &mut Role) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(role.label())
        .width(80.0)
        .show_ui(ui, |ui| {
            for r in Role::ALL {
                if ui.selectable_value(role, r, r.label()).changed() {
                    changed = true;
                }
            }
        });
    changed
}

/// The roster rows.
fn roster_table(ui: &mut egui::Ui, v: &FleetView, a: &mut Actions) {
    egui::Grid::new("fleet.roster")
        .striped(true)
        .spacing([10.0, 4.0])
        .min_col_width(20.0)
        .show(ui, |ui| {
            for h in [
                "Account",
                "Password",
                "Character",
                "Role",
                "If missing",
                "Status",
                "",
            ] {
                caption(ui, h);
            }
            ui.end_row();
            for r in &v.sessions {
                let spec = &r.spec;
                match r.running {
                    Some(i) => {
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new(&spec.account).strong())
                                    .frame(false),
                            )
                            .on_hover_text("Running here: click to switch to it")
                            .clicked()
                        {
                            a.activate = Some(i);
                        }
                    }
                    None => {
                        ui.label(&spec.account);
                    }
                }
                ui.label(egui::RichText::new("••••••").weak())
                    .on_hover_text("Kept in the settings file in plain text");
                cell(
                    ui,
                    110.0,
                    spec.character_name().unwrap_or("(first)"),
                    spec.character_name()
                        .unwrap_or("The first character on the account"),
                );
                let mut role = spec.role;
                if role_combo(ui, &format!("fleet.role.{}", r.index), &mut role) {
                    a.set_role.push((r.index, role));
                }
                let create = spec
                    .create
                    .as_ref()
                    .map(|c| {
                        let s = c.summary();
                        if s.is_empty() {
                            "create".to_string()
                        } else {
                            format!("create: {s}")
                        }
                    })
                    .unwrap_or_else(|| "—".to_string());
                cell(ui, 150.0, egui::RichText::new(&create).small(), &create);
                let status = r.status.label();
                let colour = match r.status {
                    Status::Failed(_) => egui::Color32::from_rgb(230, 90, 90),
                    Status::InWorld(_) => egui::Color32::from_rgb(120, 220, 120),
                    _ => egui::Color32::from_gray(200),
                };
                cell(
                    ui,
                    170.0,
                    egui::RichText::new(&status).color(colour).small(),
                    &status,
                );
                ui.horizontal(|ui| {
                    if r.status.running() {
                        if ui
                            .add(egui::Button::new("Stop").small())
                            .on_hover_text("Disconnect and drop the session")
                            .clicked()
                        {
                            a.stop.push(r.index);
                        }
                    } else if ui
                        .add(egui::Button::new("Start").small())
                        .on_hover_text("Log in as another session of this process")
                        .clicked()
                    {
                        a.start.push(r.index);
                    }
                    if ui
                        .add(egui::Button::new("Remove").small())
                        .on_hover_text("Forget the account (stops it first)")
                        .clicked()
                    {
                        a.remove.push(r.index);
                    }
                });
                ui.end_row();
            }
        });
}

/// The Add form.
fn add_form(ui: &mut egui::Ui, form: &mut AddForm, a: &mut Actions) {
    caption(ui, "Add an account");
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut form.account)
                .id_salt("fleet.add.account")
                .hint_text("account")
                .desired_width(110.0),
        );
        ui.add(
            egui::TextEdit::singleline(&mut form.password)
                .id_salt("fleet.add.password")
                .hint_text("password")
                .password(true)
                .desired_width(110.0),
        );
        ui.add(
            egui::TextEdit::singleline(&mut form.character)
                .id_salt("fleet.add.character")
                .hint_text("character")
                .desired_width(130.0),
        )
        .on_hover_text("The character to enter with; empty means the account's first");
        role_combo(ui, "fleet.add.role", &mut form.role);
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut form.create, "create if missing")
            .on_hover_text("When the account has no character of that name, make one:");
        ui.add_enabled_ui(form.create, |ui| {
            combo(ui, "fleet.add.template", &mut form.template, &TEMPLATES);
            combo(ui, "fleet.add.heritage", &mut form.heritage, &HERITAGES);
            let sexes: Vec<&str> = SEXES.iter().map(|s| s.1).collect();
            combo(ui, "fleet.add.sex", &mut form.sex, &sexes);
            combo(ui, "fleet.add.town", &mut form.town, &TOWNS);
        });
        if ui.button("Add").clicked() {
            a.add = true;
        }
    });
    if let Some(e) = &form.error {
        ui.colored_label(egui::Color32::from_rgb(230, 90, 90), e);
    }
}

/// The Sessions section: the roster, the all-hands buttons, the form.
fn sessions(ui: &mut egui::Ui, v: &FleetView, form: &mut AddForm, a: &mut Actions) {
    ui.horizontal(|ui| {
        let followers = v
            .sessions
            .iter()
            .filter(|r| r.spec.role == Role::Follower && !r.status.running())
            .count();
        if ui
            .add_enabled(followers > 0, egui::Button::new("Start all followers").small())
            .on_hover_text("Log every follower in that is not running")
            .clicked()
        {
            a.start_followers = true;
        }
        let mut lead = v.lead;
        let hover = match &v.active_account {
            Some(acct) => format!(
                "The session being played ({acct}) leads: the followers come to it. Remembered for the next launch."
            ),
            None => "No session is being played".to_string(),
        };
        if ui
            .add_enabled(
                v.active_account.is_some(),
                egui::Checkbox::new(&mut lead, egui::RichText::new("I lead").small()),
            )
            .on_hover_text(hover)
            .changed()
        {
            a.lead = Some(lead);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            caption(ui, "passwords are kept in plain text in the settings file");
        });
    });
    if v.sessions.is_empty() {
        ui.weak("No accounts yet. Add a follower below, then Start it.");
    } else {
        roster_table(ui, v, a);
    }
    ui.add_space(4.0);
    add_form(ui, form, a);
}

/// Draw the panel. `compact` picks one line per character over the
/// table; `sessions_open` whether the Sessions section is unfolded.
pub fn draw(
    egui: &egui::Context,
    v: &FleetView,
    compact: bool,
    sessions_open: bool,
    form: &mut AddForm,
) -> Actions {
    let mut a = Actions::default();
    let w = egui.viewport_rect().width();
    let width = (if compact { 760.0_f32 } else { 1080.0 }).min(w - 16.0);
    let rows = v.rows.len().max(1) as f32;
    let table_h = if compact {
        96.0 + rows * 24.0
    } else {
        122.0 + rows * 26.0
    };
    // The section's own header, the roster's header row and rows, and
    // the three lines of the Add form.
    let sessions_h = if sessions_open {
        176.0 + v.sessions.len().max(1) as f32 * 26.0
    } else {
        24.0
    };
    let height = (table_h + sessions_h).min(640.0);
    window(
        "fleet",
        egui::pos2((w - width) * 0.5, 40.0),
        egui::vec2(width, height),
        // Nearly opaque: it is read over whatever else is open.
        245,
        8,
    )
    .show(egui, |ui| {
        ui.set_min_width(width - 16.0);
        title_bar(ui, "fleet", "Fleet");
        header(ui, v, compact, &mut a);
        ui.separator();
        egui::ScrollArea::vertical()
            .max_height(height - 100.0)
            .show(ui, |ui| {
                if v.rows.is_empty() {
                    ui.weak("Nobody in the world yet.");
                } else if compact {
                    lines(ui, v, &mut a);
                } else {
                    table(ui, v, &mut a);
                }
                ui.separator();
                let r = egui::CollapsingHeader::new(egui::RichText::new("Sessions").strong())
                    .id_salt("fleet.sessions")
                    .default_open(sessions_open)
                    .open(Some(sessions_open))
                    .show(ui, |ui| sessions(ui, v, form, &mut a));
                if r.header_response.clicked() {
                    a.sessions_open = Some(!sessions_open);
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
    /// The Sessions section is unfolded.
    pub sessions_open: bool,
    /// Other processes' sessions, as last heard.
    roster: Roster,
    /// The last `autoplay.event` line of each other process's session.
    events: BTreeMap<Key, String>,
    xp: XpMeter,
    /// The accounts to run as extra sessions (settings: [`ROSTER_KEY`]).
    pub accounts: Vec<SessionSpec>,
    /// The account whose session leads (settings: [`LEAD_KEY`]).
    pub lead_account: Option<String>,
    pub form: AddForm,
    /// Accounts asked to start, and when, until their session appears.
    starting: BTreeMap<String, Instant>,
    /// Sessions whose connection ended: index -> why.
    ended: BTreeMap<usize, String>,
    /// Sessions whose role was applied on placement.
    role_applied: std::collections::BTreeSet<usize>,
    /// This process's name on the bus, as last seen.
    local: String,
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
                role: Some(Role::Leader),
            },
            Seen {
                key: key("local", 1),
                local: true,
                active: false,
                mate: alys,
                doing: "fighting Drudge Skulker".into(),
                role: Some(Role::Follower),
            },
            Seen {
                key: key("bob", 0),
                local: false,
                active: false,
                mate: bran,
                doing: "following: 1.1 km to go".into(),
                role: None,
            },
            Seen {
                key: key("bob", 1),
                local: false,
                active: false,
                mate: ceol,
                doing: "recovering: at the lifestone".into(),
                role: None,
            },
        ];
        let entry = |account: &str, name: &str, role, create: bool| SessionSpec {
            account: account.into(),
            password: "secret".into(),
            character: (!create).then(|| name.to_string()),
            create: create.then(|| CreateSpec {
                name: name.into(),
                template: Some("Bow Hunter".into()),
                town: Some("Holtburg".into()),
                ..Default::default()
            }),
            role,
        };
        let sessions = vec![
            SessionRow {
                index: 0,
                spec: entry("alys", "Alys", Role::Follower, false),
                running: Some(1),
                status: Status::InWorld("Alys".into()),
            },
            SessionRow {
                index: 1,
                spec: entry("dain", "Dain", Role::Follower, true),
                running: Some(2),
                status: Status::Creating("Dain".into()),
            },
            SessionRow {
                index: 2,
                spec: entry("edda", "Edda", Role::Manual, true),
                running: None,
                status: Status::Failed("that name is already in use".into()),
            },
        ];
        Fleet {
            source: Source::Demo(FleetView {
                rows: rows(&seen, &xp, now),
                on_bus: true,
                sessions,
                lead: true,
                active_account: Some("alice".into()),
            }),
            show: true,
            sessions_open: true,
            ..Default::default()
        }
    }

    /// The roster's role for `account`, if it is on it; else the
    /// leader's, when "I lead" named it.
    fn role_for(&self, account: &str) -> Option<Role> {
        if let Some(e) = self
            .accounts
            .iter()
            .find(|e| e.account.eq_ignore_ascii_case(account))
        {
            return Some(e.role);
        }
        self.lead_account
            .as_deref()
            .filter(|l| l.eq_ignore_ascii_case(account))
            .map(|_| Role::Leader)
    }

    /// The session index running `account`, if any.
    fn session_of(cx: &Ctx, account: &str) -> Option<usize> {
        cx.clients
            .iter()
            .position(|c| c.config.account.eq_ignore_ascii_case(account))
    }

    /// The roster with where each entry stands.
    fn session_rows(&self, cx: &Ctx) -> Vec<SessionRow> {
        self.accounts
            .iter()
            .enumerate()
            .map(|(index, spec)| {
                let running = Self::session_of(cx, &spec.account);
                let snap = running
                    .map(|i| Snapshot::of(cx.clients[i], self.ended.get(&i).map(String::as_str)));
                let starting = self
                    .starting
                    .contains_key(&spec.account.to_ascii_lowercase());
                let error = cx
                    .board
                    .get(&error_key(&spec.account))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                SessionRow {
                    index,
                    spec: spec.clone(),
                    running,
                    status: status_of(snap.as_ref(), starting, error.as_deref()),
                }
            })
            .collect()
    }

    /// Ask the host for roster entry `i`'s session, unless it runs.
    fn start(&mut self, cx: &mut Ctx, i: usize) {
        let Some(spec) = self.accounts.get(i).cloned() else {
            return;
        };
        if let Some(s) = Self::session_of(cx, &spec.account) {
            cx.log(format!(
                "{} is already running (session {})",
                spec.account,
                s + 1
            ));
            return;
        }
        let key = spec.account.to_ascii_lowercase();
        if self.starting.contains_key(&key) {
            return;
        }
        cx.board.set_local(error_key(&spec.account), Value::Null);
        self.starting.insert(key, cx.now);
        cx.log(format!(
            "Starting {} as {}{}",
            spec.account,
            spec.character_name().unwrap_or("its first character"),
            match &spec.create {
                Some(c) => format!(" (creating it if missing: {})", c.summary()),
                None => String::new(),
            }
        ));
        tracing::info!(
            account = spec.account,
            role = spec.role.label(),
            "fleet: starting a session"
        );
        cx.start_session(spec);
    }

    /// Ask the host to drop the session running `account`.
    fn stop(&mut self, cx: &mut Ctx, account: &str) {
        self.starting.remove(&account.to_ascii_lowercase());
        if let Some(s) = Self::session_of(cx, account) {
            cx.log(format!("Stopping {account} (session {})", s + 1));
            tracing::info!(account, session = s, "fleet: stopping a session");
            cx.stop_session(s);
        }
    }

    /// Put `spec` on the roster, replacing an entry for the same account.
    fn upsert(&mut self, spec: SessionSpec) -> usize {
        match self
            .accounts
            .iter()
            .position(|e| e.account.eq_ignore_ascii_case(&spec.account))
        {
            Some(i) => {
                self.accounts[i] = spec;
                i
            }
            None => {
                self.accounts.push(spec);
                self.accounts.len() - 1
            }
        }
    }

    fn save_roster(&self, cx: &mut Ctx) {
        cx.settings.set(ROSTER_KEY, &self.accounts);
        cx.settings.set(LEAD_KEY, &self.lead_account);
    }

    /// Apply session `i`'s role now, whether or not it was before.
    fn apply_role_now(cx: &mut Ctx, i: usize, role: Role) {
        if let Some(c) = cx.clients.get_mut(i) {
            apply_role(&mut c.autoplay.config, role);
            tracing::info!(
                session = i,
                account = c.config.account,
                role = role.label(),
                "fleet: role applied"
            );
        }
    }

    /// A [`START_KEY`] or [`STOP_KEY`] value meant for this process:
    /// its `process` field is absent or names us. Taken once read.
    fn take_request(cx: &mut Ctx, key: &str) -> Option<Value> {
        let v = cx.board.get(key)?.clone();
        if v.is_null() {
            return None;
        }
        cx.board.set_local(key, Value::Null);
        let me = Self::me(cx);
        let for_us = v
            .get("process")
            .and_then(Value::as_str)
            .is_none_or(|p| p == me);
        for_us.then_some(v)
    }

    /// Starts and stops asked for on the blackboard (a script, the
    /// command line's `--fleet-start`).
    fn take_board_requests(&mut self, cx: &mut Ctx) {
        if let Some(v) = Self::take_request(cx, START_KEY) {
            let list = match v.get("sessions") {
                Some(l) => l.clone(),
                None => v,
            };
            let specs: Vec<SessionSpec> = match list {
                Value::Array(items) => items
                    .into_iter()
                    .filter_map(|i| serde_json::from_value(i).ok())
                    .collect(),
                other => serde_json::from_value(other).ok().into_iter().collect(),
            };
            let mut changed = false;
            for spec in specs {
                if spec.account.is_empty() {
                    continue;
                }
                let i = self.upsert(spec);
                changed = true;
                self.start(cx, i);
            }
            if changed {
                self.save_roster(cx);
            }
        }
        if let Some(v) = Self::take_request(cx, STOP_KEY) {
            let list = match v.get("accounts") {
                Some(l) => l.clone(),
                None => v,
            };
            let accounts: Vec<String> = match list {
                Value::Array(items) => items
                    .into_iter()
                    .filter_map(|i| i.as_str().map(str::to_string))
                    .collect(),
                Value::String(s) => vec![s],
                _ => Vec::new(),
            };
            for a in accounts {
                self.stop(cx, &a);
            }
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
                    role: self.role_for(&c.config.account),
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
                role: None,
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
        if let Some(v) = settings.get("fleet.sessions_open") {
            self.sessions_open = v;
        }
        if let Some(v) = settings.get::<Vec<SessionSpec>>(ROSTER_KEY) {
            self.accounts = v;
        }
        if let Some(v) = settings.get::<Option<String>>(LEAD_KEY) {
            self.lead_account = v;
        }
    }

    fn save(&self, settings: &mut Settings) {
        settings.set("fleet.show", self.show);
        settings.set("fleet.compact", self.compact);
        settings.set("fleet.sessions_open", self.sessions_open);
        settings.set(ROSTER_KEY, &self.accounts);
        settings.set(LEAD_KEY, &self.lead_account);
    }

    fn session_removed(&mut self, index: usize) {
        crate::shift_removed(&mut self.ended, index);
        self.role_applied = self
            .role_applied
            .iter()
            .filter(|&&i| i != index)
            .map(|&i| if i > index { i - 1 } else { i })
            .collect();
        let local = self.local.clone();
        self.xp.session_removed(&local, index);
    }

    /// Placement applies the session's role; a connection that ends is
    /// remembered for the status column.
    fn on_event(&mut self, cx: &mut Ctx, ev: &Event) {
        if !matches!(self.source, Source::Live) {
            return;
        }
        let i = cx.index;
        match ev {
            Event::Placed { .. } => {
                if self.role_applied.insert(i) {
                    let account = cx.try_client().map(|c| c.config.account.clone());
                    if let Some(role) = account.as_deref().and_then(|a| self.role_for(a)) {
                        Self::apply_role_now(cx, i, role);
                        if role != Role::Manual {
                            cx.log(format!(
                                "{} is in the world as a {}",
                                account.unwrap_or_default(),
                                role.label()
                            ));
                        }
                    }
                }
                if let Some(c) = cx.try_client() {
                    let key = c.config.account.to_ascii_lowercase();
                    self.starting.remove(&key);
                }
            }
            Event::Connected => {
                self.ended.remove(&i);
                if let Some(c) = cx.try_client() {
                    let key = c.config.account.to_ascii_lowercase();
                    self.starting.remove(&key);
                }
            }
            Event::Terminated(why) => {
                self.ended.insert(i, format!("disconnected: {why}"));
            }
            Event::Refused(op) => {
                self.ended
                    .insert(i, format!("refused by the server ({op:#06x})"));
            }
            _ => {}
        }
    }

    /// Keep hearing and sampling whether or not the panel is open, so
    /// the rates are there when it opens; take what a script or the
    /// command line asked on the board.
    fn tick(&mut self, cx: &mut Ctx) {
        if !matches!(self.source, Source::Live) {
            return;
        }
        self.local = Self::me(cx);
        if cx.index == 0 {
            self.hear(cx);
            self.take_board_requests(cx);
            let now = cx.now;
            self.starting
                .retain(|_, at| now.duration_since(*at) < STARTING_FOR);
            // A session that appeared is no longer starting.
            let running: Vec<String> = cx
                .clients
                .iter()
                .map(|c| c.config.account.to_ascii_lowercase())
                .collect();
            self.starting.retain(|a, _| !running.contains(a));
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
            Source::Live => {
                let active = cx.clients.get(cx.index);
                FleetView {
                    rows: rows(&self.seen(cx), &self.xp, cx.now),
                    on_bus: cx.board.bus_name().is_some(),
                    sessions: self.session_rows(cx),
                    lead: active.is_some_and(|c| {
                        c.autoplay.config.team.enabled && c.autoplay.config.team.lead
                    }),
                    active_account: active.map(|c| c.config.account.clone()),
                }
            }
        };
        let a = draw(egui, &v, self.compact, self.sessions_open, &mut self.form);
        if super::closed("fleet") {
            self.show = false;
        }
        if let Some(c) = a.compact {
            self.compact = c;
            cx.settings.set("fleet.compact", c);
        }
        if let Some(o) = a.sessions_open {
            self.sessions_open = o;
            cx.settings.set("fleet.sessions_open", o);
        }
        if a.items {
            super::request_open(cx.board, super::holdings::ID, super::Ask::Open);
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
        // The roster.
        let mut changed = false;
        if a.add {
            match self.form.spec() {
                Ok(spec) => {
                    self.upsert(spec);
                    self.form.clear();
                    changed = true;
                }
                Err(e) => self.form.error = Some(e),
            }
        }
        for (i, role) in a.set_role {
            if let Some(e) = self.accounts.get_mut(i) {
                e.role = role;
                changed = true;
                let account = e.account.clone();
                if let Some(s) = Self::session_of(cx, &account) {
                    if cx.clients[s].placed() {
                        Self::apply_role_now(cx, s, role);
                    }
                }
            }
        }
        for i in a.start {
            self.start(cx, i);
        }
        if a.start_followers {
            for i in 0..self.accounts.len() {
                if self.accounts[i].role == Role::Follower
                    && Self::session_of(cx, &self.accounts[i].account).is_none()
                {
                    self.start(cx, i);
                }
            }
        }
        for i in a.stop {
            if let Some(account) = self.accounts.get(i).map(|e| e.account.clone()) {
                self.stop(cx, &account);
            }
        }
        let mut remove = a.remove;
        remove.sort_unstable();
        remove.dedup();
        for i in remove.into_iter().rev() {
            if i < self.accounts.len() {
                let account = self.accounts[i].account.clone();
                self.stop(cx, &account);
                cx.board.set_local(error_key(&account), Value::Null);
                self.accounts.remove(i);
                changed = true;
            }
        }
        if let Some(on) = a.lead {
            let i = cx.index;
            if let Some(c) = cx.clients.get_mut(i) {
                let account = c.config.account.clone();
                if on {
                    apply_role(&mut c.autoplay.config, Role::Leader);
                    self.lead_account = Some(account.clone());
                    cx.log(format!("{account} leads: the followers come to it"));
                } else {
                    c.autoplay.config.team.lead = false;
                    if self
                        .lead_account
                        .as_deref()
                        .is_some_and(|l| l.eq_ignore_ascii_case(&account))
                    {
                        self.lead_account = None;
                    }
                    cx.log(format!("{account} no longer leads"));
                }
                changed = true;
            }
        }
        if changed {
            self.save_roster(cx);
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
            role: None,
        }
    }

    fn spec(account: &str, role: Role, create: bool) -> SessionSpec {
        SessionSpec {
            account: account.into(),
            password: "testpass".into(),
            character: (!create).then(|| "Reborn".to_string()),
            create: create.then(|| CreateSpec {
                name: "Fleetbot One".into(),
                template: Some("Bow Hunter".into()),
                town: Some("Holtburg".into()),
                heritage: Some("Aluvian".into()),
                sex: Some("m".into()),
            }),
            role,
        }
    }

    #[test]
    fn the_roster_round_trips_through_the_settings() {
        let mut fleet = Fleet {
            accounts: vec![
                spec("fleetbot1", Role::Follower, true),
                spec("acreborn7", Role::Leader, false),
            ],
            lead_account: Some("acreborn7".into()),
            sessions_open: true,
            ..Default::default()
        };
        let mut settings = Settings::new();
        fleet.save(&mut settings);
        let text = serde_json::to_string(settings.get_value(ROSTER_KEY).unwrap()).unwrap();
        assert!(text.contains("\"role\":\"follower\""), "{text}");
        assert!(text.contains("\"template\":\"Bow Hunter\""), "{text}");
        assert!(
            !text.contains("\"character\":null"),
            "absent choices are left out: {text}"
        );
        // Through a file and back into a fresh panel.
        let dir = std::env::temp_dir().join(format!("acswarm-fleet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("ui.json");
        settings.save(&path).unwrap();
        let back = Settings::load(&path);
        let mut fresh = Fleet::default();
        fresh.load(&back);
        assert_eq!(fresh.accounts, fleet.accounts);
        assert_eq!(fresh.lead_account.as_deref(), Some("acreborn7"));
        assert!(fresh.sessions_open);
        assert_eq!(fresh.role_for("FLEETBOT1"), Some(Role::Follower));
        assert_eq!(fresh.role_for("acreborn7"), Some(Role::Leader));
        assert_eq!(fresh.role_for("nobody"), None);
        // "I lead" alone names a leader too.
        fleet.accounts.clear();
        assert_eq!(fleet.role_for("acreborn7"), Some(Role::Leader));
        // Upserting replaces by account, case-insensitively.
        fleet.upsert(spec("Bot", Role::Manual, false));
        assert_eq!(fleet.upsert(spec("bot", Role::Follower, true)), 0);
        assert_eq!(fleet.accounts.len(), 1);
        assert_eq!(fleet.accounts[0].role, Role::Follower);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn roles_switch_the_team_rules_on() {
        let mut cfg = ac_client::autoplay::Config::default();
        apply_role(&mut cfg, Role::Manual);
        assert!(!cfg.enabled && !cfg.team.enabled);
        apply_role(&mut cfg, Role::Follower);
        assert!(cfg.enabled && cfg.team.enabled && cfg.team.follow && !cfg.team.lead);
        apply_role(&mut cfg, Role::Leader);
        assert!(cfg.team.enabled && cfg.team.lead && !cfg.team.follow);
        assert!(cfg.enabled, "leading does not switch autoplay off");
        let was = cfg.clone();
        apply_role(&mut cfg, Role::Manual);
        assert_eq!(cfg, was, "manual changes nothing");
    }

    #[test]
    fn statuses_follow_the_session() {
        assert_eq!(status_of(None, false, None), Status::Stopped);
        assert_eq!(status_of(None, true, None), Status::Starting);
        assert_eq!(
            status_of(None, false, Some("no server")),
            Status::Failed("no server".into())
        );
        let mut s = Snapshot::default();
        assert_eq!(status_of(Some(&s), false, None), Status::Connecting);
        s.creating = Some("Fleetbot One".into());
        assert_eq!(
            status_of(Some(&s), false, None),
            Status::Creating("Fleetbot One".into())
        );
        assert_eq!(
            status_of(Some(&s), false, None).label(),
            "character missing: creating Fleetbot One"
        );
        s.placed = true;
        s.name = "Fleetbot One".into();
        assert_eq!(
            status_of(Some(&s), false, None),
            Status::InWorld("Fleetbot One".into())
        );
        s.create_error = Some("that name is already in use".into());
        assert_eq!(
            status_of(Some(&s), false, None),
            Status::Failed("that name is already in use".into())
        );
        s.ended = Some("disconnected: booted".into());
        assert!(
            matches!(status_of(Some(&s), false, None), Status::Failed(w) if w.contains("booted"))
        );
        assert!(Status::Creating("x".into()).running());
        assert!(!Status::Failed("x".into()).running());
    }

    #[test]
    fn the_add_form_makes_a_spec() {
        let mut f = AddForm::default();
        assert!(f.spec().unwrap_err().contains("account"));
        f.account = "fleetbot1".into();
        assert!(f.spec().unwrap_err().contains("password"));
        f.password = "testpass".into();
        assert!(f.spec().unwrap_err().contains("character name"));
        f.character = "Fleetbot One".into();
        f.template = 1;
        f.town = 0;
        f.sex = 1;
        let s = f.spec().unwrap();
        assert_eq!(s.account, "fleetbot1");
        assert_eq!(s.character, None, "created, so not named twice");
        let c = s.create.unwrap();
        assert_eq!(c.name, "Fleetbot One");
        assert_eq!(c.template.as_deref(), Some("Bow Hunter"));
        assert_eq!(c.town.as_deref(), Some("Holtburg"));
        assert_eq!(c.heritage.as_deref(), Some("Aluvian"));
        assert_eq!(c.sex.as_deref(), Some("f"));
        assert_eq!(s.role, Role::Follower);
        // Without creation the character is just named (or left to the
        // account's first).
        f.create = false;
        let s = f.spec().unwrap();
        assert_eq!(s.character.as_deref(), Some("Fleetbot One"));
        assert!(s.create.is_none());
        f.character.clear();
        assert_eq!(f.spec().unwrap().character_name(), None);
        f.account = "a:b".into();
        assert!(f.spec().is_err());
        f.clear();
        assert!(f.account.is_empty() && f.error.is_none() && !f.create);
    }

    #[test]
    fn board_requests_start_and_stop_through_the_panel() {
        use crate::{Blackboard, IconCache};
        let mut fleet = Fleet::default();
        let mut board = Blackboard::default();
        let mut settings = Settings::new();
        let mut icons = IconCache::default();
        board.set_local(
            START_KEY,
            serde_json::json!([{
                "account": "fleetbot1", "password": "testpass",
                "create": {"name": "Fleetbot One", "template": "bow", "town": "holtburg"},
                "role": "follower"
            }]),
        );
        let now = Instant::now();
        let mut cx = Ctx {
            clients: Vec::new(),
            index: 0,
            board: &mut board,
            settings: &mut settings,
            icons: &mut icons,
            dt: 0.05,
            now,
            chat: Vec::new(),
            activate: None,
            quit: false,
            pick_data_dir: false,
            start_sessions: Vec::new(),
            stop_sessions: Vec::new(),
        };
        fleet.tick(&mut cx);
        assert_eq!(cx.start_sessions.len(), 1);
        assert_eq!(cx.start_sessions[0].account, "fleetbot1");
        assert_eq!(
            cx.start_sessions[0]
                .create
                .as_ref()
                .map(|c| c.name.as_str()),
            Some("Fleetbot One")
        );
        assert_eq!(fleet.accounts.len(), 1, "on the roster too");
        assert!(cx.board.get(START_KEY).is_some_and(Value::is_null), "taken");
        assert!(cx.settings.get::<Vec<SessionSpec>>(ROSTER_KEY).is_some());
        assert!(cx
            .chat
            .iter()
            .any(|(l, _)| l.starts_with("Starting fleetbot1")));
        // Asked again while starting: nothing more is asked.
        cx.board.set_local(
            START_KEY,
            serde_json::json!({"account": "fleetbot1", "password": "x"}),
        );
        cx.start_sessions.clear();
        fleet.tick(&mut cx);
        assert!(cx.start_sessions.is_empty());
        // Meant for another process: ignored.
        cx.board.set_local(
            START_KEY,
            serde_json::json!({"process": "bob", "account": "other", "password": "x"}),
        );
        fleet.tick(&mut cx);
        assert!(cx.start_sessions.is_empty());
        assert_eq!(fleet.accounts.len(), 1);
        // A stop names an account; with no session running it, nothing
        // is asked, but it is no longer starting.
        cx.board.set_local(STOP_KEY, serde_json::json!("fleetbot1"));
        fleet.tick(&mut cx);
        assert!(cx.stop_sessions.is_empty());
        assert!(fleet.starting.is_empty());
        assert_eq!(
            status_of(None, fleet.starting.contains_key("fleetbot1"), None),
            Status::Stopped
        );
    }

    #[test]
    fn removal_shifts_what_is_kept_by_session() {
        let now = Instant::now();
        let mut fleet = Fleet {
            local: "local".into(),
            ..Default::default()
        };
        fleet.ended.insert(0, "a".into());
        fleet.ended.insert(2, "c".into());
        fleet.role_applied.extend([0, 1, 2]);
        for i in 0..3 {
            fleet
                .xp
                .sample(&Key::new("local", i), 100, now - WINDOW / 2);
            fleet
                .xp
                .sample(&Key::new("local", i), 100 + i as i64 * 1000, now);
        }
        fleet.xp.sample(&Key::new("bob", 2), 5, now);
        fleet.session_removed(1);
        assert_eq!(
            fleet
                .ended
                .iter()
                .map(|(i, s)| (*i, s.as_str()))
                .collect::<Vec<_>>(),
            [(0, "a"), (1, "c")]
        );
        assert_eq!(
            fleet.role_applied.iter().copied().collect::<Vec<_>>(),
            [0, 1]
        );
        let rate = |k: Key| fleet.xp.rate(&k, now).map(|r| r.round() as i64);
        assert!(rate(Key::new("local", 0)).is_some_and(|r| r == 0));
        assert!(
            rate(Key::new("local", 1)).is_some_and(|r| r > 0),
            "was session 2"
        );
        assert_eq!(rate(Key::new("local", 2)), None);
        assert!(
            fleet.xp.samples.contains_key(&Key::new("bob", 2)),
            "other processes untouched"
        );
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
            ..Default::default()
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
        assert_eq!(fmt_xp(360_000.0), "360k");
        assert_eq!(fmt_xp(1_234_000.0), "1.2M");
    }

    #[test]
    fn the_demo_has_a_leader_with_rates() {
        let Source::Demo(v) = Fleet::demo().source else {
            unreachable!()
        };
        assert_eq!(v.rows.len(), 4);
        assert!(v.rows[0].leader && v.rows[0].name == "Reborn");
        assert_eq!(v.rows[0].role, Some(Role::Leader));
        assert!(v.rows.iter().take(3).all(|r| r.xp_per_hour.is_some()));
        assert_eq!(v.totals().alive, 3);
        assert_eq!(v.sessions.len(), 3);
        assert!(v.lead);
    }
}
