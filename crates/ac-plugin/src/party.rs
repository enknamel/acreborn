//! Party: coordinate the sessions running in this process. One session is
//! the leader; the others follow it, attack what it attacks, and every
//! session can loot its own kills. The leader's target travels over the
//! bus as `party.target`; the leader's index lives on the blackboard as
//! `party.leader`.
//!
//! The decisions (where to run, whom to attack, whether a kill is ready to
//! loot) are plain functions of plain data at the top of the file, so they
//! are unit-tested without a `Client`.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::{egui, Client, Ctx, Plugin, Value};
use ac_world::object::MoveTarget;
use glam::Vec3;

/// Followers stay within this many metres of the leader.
pub const FOLLOW_DISTANCE: f32 = 3.0;
/// How long to keep trying to open a corpse that has not appeared.
pub const CORPSE_TIMEOUT: Duration = Duration::from_secs(20);
/// Interval between attempts to open a corpse.
pub const CORPSE_RETRY: Duration = Duration::from_secs(3);

pub const LEADER_KEY: &str = "party.leader";
pub const TARGET_TOPIC: &str = "party.target";

// ---------------------------------------------------------------------------
// Decisions: plain data in, plain data out.

/// Where the leader's character stands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeaderPose {
    /// The leader's own object guid, when it has one.
    pub guid: Option<u32>,
    pub cell: u32,
    pub local: Vec3,
}

impl LeaderPose {
    pub fn world(&self) -> Vec3 {
        ac_world::landblock_origin(self.cell) + self.local
    }
}

/// What a follower standing at `me` should run toward: nothing when close
/// enough, the leader's object when the follower's world contains it
/// (the client then tracks it as it moves), else the leader's position.
pub fn follow_target(me: Vec3, leader: &LeaderPose, leader_in_view: bool) -> Option<MoveTarget> {
    let d = leader.world() - me;
    if Vec3::new(d.x, d.y, 0.0).length() <= FOLLOW_DISTANCE {
        return None;
    }
    match leader.guid {
        Some(g) if leader_in_view => Some(MoveTarget::Object(g)),
        _ => Some(MoveTarget::Position {
            cell: leader.cell,
            local: leader.local,
        }),
    }
}

/// What a follower should do about the leader's target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assist {
    Stay,
    Attack { guid: u32, enter_combat: bool },
}

/// A follower attacks the leader's target once, when it can see it, is not
/// already on it, and is not a caster (magic mode is left alone).
pub fn assist_decision(
    leader_target: Option<u32>,
    my_target: Option<u32>,
    combat: bool,
    magic: bool,
    target_in_view: bool,
) -> Assist {
    match leader_target {
        Some(guid) if target_in_view && !magic && my_target != Some(guid) => Assist::Attack {
            guid,
            enter_combat: !combat,
        },
        _ => Assist::Stay,
    }
}

/// A watched target is dead when its object is gone (`alive` is None) or
/// its health reached zero.
pub fn target_died(watched: Option<u32>, alive: Option<bool>) -> bool {
    watched.is_some() && !alive.unwrap_or(false)
}

/// The corpse a creature leaves behind, as the server names it.
pub fn corpse_name(target: &str) -> String {
    format!("Corpse of {target}")
}

/// Items of an open container not yet asked for, in container order.
pub fn items_to_take(container: &[u32], taken: &HashSet<u32>) -> Vec<u32> {
    container
        .iter()
        .copied()
        .filter(|g| !taken.contains(g))
        .collect()
}

/// `on`/`off`/empty (toggle) for the switch commands; None for anything else.
pub fn parse_switch(args: &str, current: bool) -> Option<bool> {
    match args.trim().to_ascii_lowercase().as_str() {
        "" | "toggle" => Some(!current),
        "on" | "1" | "true" | "yes" => Some(true),
        "off" | "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

/// Distance between two characters, flat (height differences are not
/// something following can fix).
pub fn flat_distance(a: Vec3, b: Vec3) -> f32 {
    let d = a - b;
    Vec3::new(d.x, d.y, 0.0).length()
}

// ---------------------------------------------------------------------------
// The plugin.

/// A corpse a session is opening and emptying.
struct Loot {
    corpse: String,
    since: Instant,
    last_try: Option<Instant>,
    /// The container guid once the server opened it for us.
    opened: Option<u32>,
    taken: HashSet<u32>,
}

#[derive(Default)]
struct SessionState {
    /// The creature this session was last attacking, with its name.
    watched: Option<(u32, String)>,
    loot: Option<Loot>,
}

#[derive(Default)]
pub struct Party {
    follow: bool,
    assist: bool,
    lootall: bool,
    /// The leader's target as last read from the bus.
    shared_target: Option<u32>,
    /// The leader's target as last posted, to post only on change.
    posted_target: Option<Option<u32>>,
    sessions: Vec<SessionState>,
}

/// One row of the party window.
struct Row {
    name: String,
    level: i32,
    health: (u32, u32),
    distance: Option<f32>,
    target: String,
    leader: bool,
}

fn leader_index(cx: &Ctx) -> Option<usize> {
    let i = cx.board.get(LEADER_KEY)?.as_u64()? as usize;
    (i < cx.client_count()).then_some(i)
}

fn position_of(c: &Client) -> Option<Vec3> {
    c.player
        .as_ref()
        .map(|p| p.world_position())
        .or_else(|| c.world.player().and_then(|o| o.world_pos()))
}

fn pose_of(c: &Client) -> Option<LeaderPose> {
    let guid = c.world.player_guid;
    if let Some(p) = c.player.as_ref() {
        return Some(LeaderPose {
            guid,
            cell: p.cell,
            local: p.local,
        });
    }
    let p = c.world.player()?.position?;
    Some(LeaderPose {
        guid,
        cell: p.cell,
        local: p.local,
    })
}

fn session_name(c: &Client) -> String {
    if !c.world.stats.name.is_empty() {
        c.world.stats.name.clone()
    } else if let Some(n) = &c.config.character {
        n.clone()
    } else {
        c.config.account.clone()
    }
}

fn target_name(c: &Client) -> String {
    match c.attack_target {
        Some(g) => c
            .world
            .objects
            .get(&g)
            .map(|o| o.name.clone())
            .unwrap_or_else(|| c.last_target_name.clone()),
        None => String::new(),
    }
}

impl Party {
    fn state(&mut self, i: usize) -> &mut SessionState {
        if self.sessions.len() <= i {
            self.sessions.resize_with(i + 1, SessionState::default);
        }
        &mut self.sessions[i]
    }

    fn rows(&self, cx: &Ctx) -> Vec<Row> {
        let leader = leader_index(cx);
        let leader_pos = leader.and_then(|l| position_of(cx.clients[l]));
        cx.clients
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let stats = &c.world.stats;
                Row {
                    name: session_name(c),
                    level: stats.level,
                    health: (
                        stats.vitals.first().map(|v| v.current).unwrap_or(0),
                        stats.vital_max(0),
                    ),
                    distance: match (leader_pos, position_of(c)) {
                        (Some(l), Some(m)) => Some(flat_distance(l, m)),
                        _ => None,
                    },
                    target: target_name(c),
                    leader: leader == Some(i),
                }
            })
            .collect()
    }

    fn print_state(&self, cx: &mut Ctx) {
        let rows = self.rows(cx);
        let leader = leader_index(cx);
        cx.log(format!(
            "party: leader {}  follow {}  assist {}  lootall {}",
            leader.map(|l| (l + 1).to_string()).unwrap_or("none".into()),
            onoff(self.follow),
            onoff(self.assist),
            onoff(self.lootall),
        ));
        for (i, r) in rows.iter().enumerate() {
            cx.log(format!(
                "#{} {}{} L{} {}/{} hp{}{}",
                i + 1,
                r.name,
                if r.leader { " (leader)" } else { "" },
                r.level,
                r.health.0,
                r.health.1,
                r.distance.map(|d| format!(" {d:.1}m")).unwrap_or_default(),
                if r.target.is_empty() {
                    String::new()
                } else {
                    format!(" -> {}", r.target)
                },
            ));
        }
    }

    /// The leader's part of a tick: publish its target when it changes.
    fn tick_leader(&mut self, cx: &mut Ctx) {
        let target = cx.client().attack_target;
        if self.posted_target != Some(target) {
            self.posted_target = Some(target);
            cx.post(TARGET_TOPIC, target.map(Value::from).unwrap_or(Value::Null));
        }
    }

    fn tick_follow(&mut self, cx: &mut Ctx, leader: usize) {
        let Some(pose) = pose_of(cx.clients[leader]) else {
            return;
        };
        let now = cx.now;
        let me = cx.client();
        let Some(my_pos) = position_of(me) else {
            return;
        };
        let in_view = pose.guid.is_some_and(|g| me.world.objects.contains_key(&g));
        if let Some(t) = follow_target(my_pos, &pose, in_view) {
            if me.move_to != Some(t) {
                tracing::debug!("follow: run to {t:?}");
            }
            me.move_to = Some(t);
            me.move_to_since = now;
        }
    }

    fn tick_assist(&mut self, cx: &mut Ctx) {
        let me = cx.client();
        let in_view = self
            .shared_target
            .is_some_and(|g| me.world.objects.contains_key(&g));
        match assist_decision(
            self.shared_target,
            me.attack_target,
            me.combat,
            me.magic,
            in_view,
        ) {
            Assist::Attack { guid, enter_combat } => {
                if enter_combat {
                    me.toggle_combat();
                }
                me.attack(guid);
            }
            Assist::Stay => {}
        }
    }

    fn tick_loot(&mut self, cx: &mut Ctx) {
        let now = cx.now;
        let index = cx.index;
        let me = cx.client();
        let st = self.state(index);
        // Remember what we fight, notice when it dies.
        if let Some(g) = me.attack_target {
            st.watched = Some((g, me.last_target_name.clone()));
        } else if let Some((g, name)) = st.watched.clone() {
            let alive = me
                .world
                .objects
                .get(&g)
                .map(|o| o.health.is_none_or(|h| h > 0.0));
            if target_died(Some(g), alive) {
                st.watched = None;
                if st.loot.is_none() {
                    tracing::info!("lootall: {name} died, looking for its corpse");
                    st.loot = Some(Loot {
                        corpse: corpse_name(&name),
                        since: now,
                        last_try: None,
                        opened: None,
                        taken: HashSet::new(),
                    });
                }
            } else {
                // Still alive: we stopped, it did not die. Not our kill.
                st.watched = None;
            }
        }
        let Some(loot) = st.loot.as_mut() else {
            return;
        };
        match loot.opened {
            None => {
                if let Some((c, _)) = &me.world.open_container {
                    let is_corpse = me
                        .world
                        .objects
                        .get(c)
                        .is_some_and(|o| o.name == loot.corpse);
                    if is_corpse {
                        loot.opened = Some(*c);
                        return;
                    }
                }
                if now.duration_since(loot.since) > CORPSE_TIMEOUT {
                    tracing::info!("lootall: gave up on {}", loot.corpse);
                    st.loot = None;
                    return;
                }
                let due = loot
                    .last_try
                    .is_none_or(|t| now.duration_since(t) >= CORPSE_RETRY);
                if due {
                    loot.last_try = Some(now);
                    if me.combat {
                        me.toggle_combat();
                    }
                    let corpse = loot.corpse.clone();
                    if !me.use_by_name(&corpse) {
                        tracing::debug!("lootall: no {corpse:?} in view yet");
                    }
                }
            }
            Some(container) => {
                let items = match &me.world.open_container {
                    Some((c, items)) if *c == container => items.clone(),
                    _ => {
                        // Closed under us (emptied, or someone else took it).
                        st.loot = None;
                        return;
                    }
                };
                for g in items_to_take(&items, &loot.taken) {
                    loot.taken.insert(g);
                    me.take(g);
                }
                let all_taken = items.iter().all(|g| loot.taken.contains(g));
                if all_taken && me.loot_queue.is_empty() && me.loot_inflight.is_none() {
                    tracing::info!("lootall: {} emptied", loot.corpse);
                    me.close_container();
                    st.loot = None;
                }
            }
        }
    }
}

fn onoff(b: bool) -> &'static str {
    if b {
        "on"
    } else {
        "off"
    }
}

impl Plugin for Party {
    fn name(&self) -> &str {
        "party"
    }

    fn tick(&mut self, cx: &mut Ctx) {
        // Everyone reads the leader's target off the bus.
        if let Some(m) = cx.board.messages_on(TARGET_TOPIC).last() {
            self.shared_target = m.value.as_u64().map(|v| v as u32);
        }
        let leader = leader_index(cx);
        if !cx.client().placed() {
            return;
        }
        if leader == Some(cx.index) {
            self.tick_leader(cx);
        } else if let Some(l) = leader {
            if self.follow {
                self.tick_follow(cx, l);
            }
            if self.assist {
                self.tick_assist(cx);
            }
        }
        if self.lootall {
            self.tick_loot(cx);
        }
    }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        let rows = self.rows(cx);
        let mut activate = None;
        let mut lead = None;
        // Below the radar, which sits in the top-right corner.
        let w = egui.viewport_rect().width();
        egui::Window::new("Party")
            .default_width(420.0)
            .default_pos(egui::pos2(
                w - 720.0,
                2.0 * crate::panels::radar::RADIUS + 40.0,
            ))
            .show(egui, |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.follow, "follow");
                    ui.checkbox(&mut self.assist, "assist");
                    ui.checkbox(&mut self.lootall, "loot all");
                });
                egui::Grid::new("party_rows").striped(true).show(ui, |ui| {
                    for h in ["#", "Name", "Lvl", "Health", "Dist", "Target", "", ""] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for (i, r) in rows.iter().enumerate() {
                        let mark = if i == cx.index { ">" } else { "" };
                        ui.label(format!("{mark}{}", i + 1));
                        ui.label(if r.leader {
                            format!("{} *", r.name)
                        } else {
                            r.name.clone()
                        });
                        ui.label(r.level.to_string());
                        ui.label(format!("{}/{}", r.health.0, r.health.1));
                        ui.label(
                            r.distance
                                .map(|d| format!("{d:.1} m"))
                                .unwrap_or_else(|| "-".into()),
                        );
                        ui.label(&r.target);
                        if i != cx.index && ui.button("Switch").clicked() {
                            activate = Some(i);
                        } else if i == cx.index {
                            ui.label("");
                        }
                        if !r.leader && ui.button("Lead").clicked() {
                            lead = Some(i);
                        } else if r.leader {
                            ui.label("");
                        }
                        ui.end_row();
                    }
                });
            });
        if let Some(i) = activate {
            cx.activate = Some(i);
        }
        if let Some(i) = lead {
            cx.board.set(LEADER_KEY, i);
        }
    }

    fn command(&mut self, cx: &mut Ctx, name: &str, args: &str) -> bool {
        match name {
            "leader" => {
                let i = match args.trim() {
                    "" => cx.index,
                    n => match n.parse::<usize>() {
                        Ok(n) if n >= 1 && n <= cx.client_count() => n - 1,
                        _ => {
                            cx.log(format!("/leader [N] with N in 1..={}", cx.client_count()));
                            return true;
                        }
                    },
                };
                cx.board.set(LEADER_KEY, i);
                self.posted_target = None;
                cx.log(format!("party: #{} leads", i + 1));
            }
            "follow" => match parse_switch(args, self.follow) {
                Some(v) => {
                    self.follow = v;
                    if !v {
                        let leader = leader_index(cx);
                        for (i, c) in cx.clients.iter_mut().enumerate() {
                            if leader != Some(i) {
                                c.move_to = None;
                            }
                        }
                    }
                    cx.log(format!("party: follow {}", onoff(v)));
                }
                None => cx.log("/follow [on|off]"),
            },
            "assist" => match parse_switch(args, self.assist) {
                Some(v) => {
                    self.assist = v;
                    cx.log(format!("party: assist {}", onoff(v)));
                }
                None => cx.log("/assist [on|off]"),
            },
            "lootall" => match parse_switch(args, self.lootall) {
                Some(v) => {
                    self.lootall = v;
                    if !v {
                        for s in &mut self.sessions {
                            s.loot = None;
                        }
                    }
                    cx.log(format!("party: lootall {}", onoff(v)));
                }
                None => cx.log("/lootall [on|off]"),
            },
            "party" => self.print_state(cx),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leader_at(x: f32, y: f32) -> LeaderPose {
        LeaderPose {
            guid: Some(0x5000_0001),
            cell: 0xA9B4_0000,
            local: Vec3::new(x, y, 0.0),
        }
    }

    #[test]
    fn follow_stays_put_when_close() {
        let leader = leader_at(50.0, 50.0);
        let me = leader.world() + Vec3::new(2.0, 1.0, 0.0);
        assert_eq!(follow_target(me, &leader, true), None);
        // Height alone never triggers a run.
        let above = leader.world() + Vec3::new(0.0, 0.0, 10.0);
        assert_eq!(follow_target(above, &leader, true), None);
    }

    #[test]
    fn follow_tracks_the_leader_object_when_visible() {
        let leader = leader_at(50.0, 50.0);
        let me = leader.world() + Vec3::new(5.0, 0.0, 0.0);
        assert_eq!(
            follow_target(me, &leader, true),
            Some(MoveTarget::Object(0x5000_0001))
        );
    }

    #[test]
    fn follow_runs_to_the_position_when_leader_is_out_of_view() {
        let leader = leader_at(50.0, 50.0);
        let me = leader.world() + Vec3::new(0.0, 40.0, 0.0);
        assert_eq!(
            follow_target(me, &leader, false),
            Some(MoveTarget::Position {
                cell: leader.cell,
                local: leader.local
            })
        );
        let no_guid = LeaderPose {
            guid: None,
            ..leader
        };
        assert!(matches!(
            follow_target(me, &no_guid, true),
            Some(MoveTarget::Position { .. })
        ));
    }

    #[test]
    fn assist_attacks_once_and_enters_combat_first() {
        let t = Some(0x8000_0001);
        assert_eq!(
            assist_decision(t, None, false, false, true),
            Assist::Attack {
                guid: 0x8000_0001,
                enter_combat: true
            }
        );
        assert_eq!(
            assist_decision(t, None, true, false, true),
            Assist::Attack {
                guid: 0x8000_0001,
                enter_combat: false
            }
        );
        // Already on it: nothing to do.
        assert_eq!(assist_decision(t, t, true, false, true), Assist::Stay);
        // Switches when the leader switches.
        assert!(matches!(
            assist_decision(Some(7), t, true, false, true),
            Assist::Attack { guid: 7, .. }
        ));
    }

    #[test]
    fn assist_leaves_casters_and_unseen_targets_alone() {
        let t = Some(0x8000_0001);
        assert_eq!(assist_decision(t, None, false, true, true), Assist::Stay);
        assert_eq!(assist_decision(t, None, false, false, false), Assist::Stay);
        assert_eq!(
            assist_decision(None, None, false, false, true),
            Assist::Stay
        );
    }

    #[test]
    fn a_kill_is_noticed_when_gone_or_at_zero_health() {
        assert!(target_died(Some(1), None));
        assert!(target_died(Some(1), Some(false)));
        assert!(!target_died(Some(1), Some(true)));
        assert!(!target_died(None, None));
        assert_eq!(corpse_name("Drudge Skulker"), "Corpse of Drudge Skulker");
    }

    #[test]
    fn take_each_item_once() {
        let container = [10, 11, 12];
        let mut taken = HashSet::new();
        assert_eq!(items_to_take(&container, &taken), vec![10, 11, 12]);
        taken.insert(11);
        assert_eq!(items_to_take(&container, &taken), vec![10, 12]);
        taken.extend([10, 12]);
        assert!(items_to_take(&container, &taken).is_empty());
    }

    #[test]
    fn switches_parse() {
        assert_eq!(parse_switch("", false), Some(true));
        assert_eq!(parse_switch("", true), Some(false));
        assert_eq!(parse_switch("on", false), Some(true));
        assert_eq!(parse_switch("OFF", true), Some(false));
        assert_eq!(parse_switch("sideways", true), None);
    }

    #[test]
    fn distance_is_flat() {
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(3.0, 4.0, 100.0);
        assert_eq!(flat_distance(a, b), 5.0);
    }
}
