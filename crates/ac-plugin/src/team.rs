//! The wire between characters playing together.
//!
//! `ac_client::autoplay` has rules for a team: fight what the leader is
//! fighting, land the debuffs on it, hand a teammate what it is short
//! of, heal whoever is worst hurt, bring everyone into one fellowship.
//! Those rules read a [`TeamView`], and until now nothing filled it in.
//! The word each session gives about itself carries its Salvaging and
//! whether it has an Ust, so everyone agrees on who salvages.
//!
//! This plugin does. Every session with the team rules on says who it
//! is and what it is doing, a few times a second, on the blackboard:
//! to the other sessions in this process directly, and to every other
//! process through the bus when one is attached (`--bus`). Every
//! session gathers what the others said into a roster, drops anyone who
//! has gone quiet, and hands the roster to its own rules.
//!
//! The leader is not elected, it is chosen: the character whose name
//! sorts first among those on the team. Every session reaches the same
//! answer from the same roster, so there is no vote and nothing to
//! agree on, and a leader who logs off is replaced the moment the
//! others stop hearing from it.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ac_client::autoplay::{Mate, TeamView};
use serde_json::Value;

use crate::{Ctx, Plugin};

/// The topic a session's word about itself goes out on.
pub const MATE_TOPIC: &str = "autoplay.mate";
/// The topic the fleet view asks another process's session to do
/// something on: `{"process", "session", "name", "action"}`, where
/// `action` is a [`Request`] word. The team plugin in the process named
/// applies it to the session named (see [`Request::apply`]).
pub const REQUEST_TOPIC: &str = "fleet.request";
/// How often each session speaks.
const SAY_EVERY: Duration = Duration::from_millis(500);
/// A mate not heard from for this long has gone.
const FORGET_AFTER: Duration = Duration::from_secs(6);

/// A mate as last heard, and when.
#[derive(Clone, Debug)]
struct Heard {
    mate: Mate,
    at: Instant,
}

/// Everyone on the team, keyed by where they spoke from: the process
/// (this one or a name on the bus) and the session within it.
#[derive(Debug, Default)]
pub struct Roster {
    heard: BTreeMap<(String, usize), Heard>,
}

impl Roster {
    /// Take in one word from a mate.
    pub fn hear(&mut self, process: &str, session: usize, mate: Mate, now: Instant) {
        self.heard
            .insert((process.to_string(), session), Heard { mate, at: now });
    }

    /// Forget whoever has gone quiet.
    pub fn forget_quiet(&mut self, now: Instant) {
        self.heard
            .retain(|_, h| now.duration_since(h.at) < FORGET_AFTER);
    }

    /// The team as one session sees it: everyone else on the roster,
    /// and whether this session's character leads. The leader is the
    /// character that asked to lead (the one played by hand), or else
    /// the one whose name sorts first, this one included.
    pub fn view_for(&self, me: &Mate) -> TeamView {
        let mut mates: Vec<Mate> = self
            .heard
            .values()
            .map(|h| h.mate.clone())
            .filter(|m| !(m.name == me.name && m.guid == me.guid))
            .collect();
        let first = leader_name(mates.iter().chain(std::iter::once(me)));
        let leader = first.as_deref() == Some(me.name.as_str());
        for m in &mut mates {
            m.leader = first.as_deref() == Some(m.name.as_str());
        }
        TeamView { mates, leader }
    }

    /// Everyone heard, with where each spoke from (process, session).
    pub fn iter(&self) -> impl Iterator<Item = (&str, usize, &Mate)> {
        self.heard
            .iter()
            .map(|((p, s), h)| (p.as_str(), *s, &h.mate))
    }

    pub fn len(&self) -> usize {
        self.heard.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heard.is_empty()
    }
}

/// Who leads among `mates`: the first by name of those that asked to,
/// else the first by name of all. `None` when nobody has a name.
pub fn leader_name<'a>(mates: impl Iterator<Item = &'a Mate> + Clone) -> Option<String> {
    let named = |lead_only: bool| {
        mates
            .clone()
            .filter(|m| !lead_only || m.leads)
            .map(|m| m.name.as_str())
            .filter(|n| !n.is_empty())
            .min()
            .map(str::to_string)
    };
    named(true).or_else(|| named(false))
}

/// What the fleet view can ask of a session, its own or another
/// process's (over [`REQUEST_TOPIC`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Play on its own.
    AutoplayOn,
    AutoplayOff,
    /// Follow the leader about (turns the team rules on too).
    FollowOn,
    FollowOff,
    /// Come to the leader now: follow on, and whatever was being
    /// fought is let go.
    Regroup,
    /// Autoplay off, the journey and the fight cancelled.
    Stop,
}

impl Request {
    pub const ALL: [Request; 6] = [
        Request::AutoplayOn,
        Request::AutoplayOff,
        Request::FollowOn,
        Request::FollowOff,
        Request::Regroup,
        Request::Stop,
    ];

    /// The word on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Request::AutoplayOn => "autoplay_on",
            Request::AutoplayOff => "autoplay_off",
            Request::FollowOn => "follow_on",
            Request::FollowOff => "follow_off",
            Request::Regroup => "regroup",
            Request::Stop => "stop",
        }
    }

    pub fn parse(word: &str) -> Option<Request> {
        Request::ALL.into_iter().find(|r| r.as_str() == word)
    }

    /// Do it to `client`.
    pub fn apply(self, client: &mut ac_client::Client) {
        let cfg = &mut client.autoplay.config;
        match self {
            Request::AutoplayOn => cfg.enabled = true,
            Request::AutoplayOff => cfg.enabled = false,
            Request::FollowOn => {
                cfg.team.enabled = true;
                cfg.team.follow = true;
            }
            Request::FollowOff => cfg.team.follow = false,
            Request::Regroup => {
                cfg.enabled = true;
                cfg.team.enabled = true;
                cfg.team.follow = true;
                client.autoplay.drop_target();
                client.attack_target = None;
            }
            Request::Stop => {
                cfg.enabled = false;
                client.autoplay.drop_target();
                client.attack_target = None;
                client.follow = None;
                if client.traveling() {
                    client.cancel_travel();
                }
            }
        }
    }

    /// The message asking `process`'s session `session` (whose
    /// character is `name`) to do this.
    pub fn message(self, process: &str, session: usize, name: &str) -> Value {
        serde_json::json!({
            "process": process,
            "session": session,
            "name": name,
            "action": self.as_str(),
        })
    }

    /// Read a [`REQUEST_TOPIC`] message: what it asks and of whom, as
    /// `(process, session, name, request)`.
    pub fn from_message(value: &Value) -> Option<(String, usize, String, Request)> {
        let process = value.get("process")?.as_str()?.to_string();
        let session = value.get("session")?.as_u64()? as usize;
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let request = Request::parse(value.get("action")?.as_str()?)?;
        Some((process, session, name, request))
    }
}

/// The plugin: one roster shared by every session in this process, and
/// when each session last spoke.
#[derive(Default)]
pub struct Team {
    roster: Roster,
    last_said: BTreeMap<usize, Instant>,
}

/// A vital (0 health, 1 stamina, 2 mana) as a fraction of its maximum,
/// 1.0 when the sheet has not arrived.
pub fn vital_fraction(stats: &ac_world::stats::PlayerStats, i: usize) -> f32 {
    let max = stats.vital_max(i);
    if max == 0 {
        return 1.0;
    }
    (stats.vitals[i].current as f32 / max as f32).clamp(0.0, 1.0)
}

/// What a session says about itself; `None` before it is in the world
/// with a name. The fleet view builds its rows for this process's
/// sessions from the same word.
pub fn describe(client: &ac_client::Client, session: usize) -> Option<Mate> {
    let player = client.player.as_ref()?;
    let cfg = &client.autoplay.config.team;
    let name = client.world.stats.name.clone();
    if name.is_empty() {
        return None;
    }
    let stats = &client.world.stats;
    let guid = client.world.player_guid.unwrap_or(0);
    let target = client
        .attack_target
        .or(client.autoplay.casting_at())
        .filter(|g| {
            client
                .world
                .objects
                .get(g)
                .is_some_and(|o| o.health.unwrap_or(1.0) > 0.0)
        });
    let target_name = target
        .and_then(|g| client.world.objects.get(&g).map(|o| o.name.clone()))
        .unwrap_or_default();
    let in_fellowship = client
        .world
        .fellowship
        .as_ref()
        .is_some_and(|f| f.members.iter().any(|m| m.guid == guid));
    Some(Mate {
        name,
        guid,
        session,
        world: player.world_position(),
        health: client.health_fraction(),
        role: cfg.role,
        target,
        target_name,
        in_fellowship,
        wants: client.autoplay.wants.clone(),
        debuffed: client.autoplay.debuffed.clone(),
        leader: false,
        life_magic: client.life_magic(),
        can_soften: client.can_soften(),
        leads: cfg.lead,
        flying: client.noclip(),
        cell: player.cell,
        level: stats.level,
        total_xp: stats.total_xp,
        available_xp: stats.available_xp,
        stamina: vital_fraction(stats, 1),
        mana: vital_fraction(stats, 2),
        autoplay: client.autoplay.config.enabled,
        following: cfg.enabled && cfg.follow && !cfg.lead,
        salvaging: client.salvaging(),
        has_ust: client.salvage_tool().is_some(),
        supplies: client.supplies(&client.autoplay.config.growth),
        ground: client.hunting_ground(),
    })
}

impl Plugin for Team {
    fn name(&self) -> &str {
        "team"
    }

    fn session_removed(&mut self, index: usize) {
        crate::shift_removed(&mut self.last_said, index);
    }

    fn tick(&mut self, cx: &mut Ctx) {
        let now = Instant::now();
        let session = cx.index;
        // Hear everyone who spoke since last frame, other sessions of
        // this process and other processes alike.
        let me_name = cx.board.bus_name().unwrap_or("local").to_string();
        let heard: Vec<(String, usize, Mate)> = cx
            .board
            .messages_on(MATE_TOPIC)
            .filter_map(|m| {
                let mate: Mate = serde_json::from_value(m.value.clone()).ok()?;
                let process = m.origin.clone().unwrap_or_else(|| me_name.clone());
                Some((
                    process,
                    if m.origin.is_some() {
                        mate.session
                    } else {
                        m.from
                    },
                    mate,
                ))
            })
            .collect();
        for (process, from, mate) in heard {
            self.roster.hear(&process, from, mate, now);
        }
        self.roster.forget_quiet(now);
        // What another process's fleet view asked of this session. (The
        // fleet view here applies its own asks directly.)
        let asked: Vec<Request> = cx
            .board
            .messages_on(REQUEST_TOPIC)
            .filter(|m| m.is_remote())
            .filter_map(|m| Request::from_message(&m.value))
            .filter(|(process, s, _, _)| process == &me_name && *s == session)
            .map(|(_, _, _, r)| r)
            .collect();

        let Some(client) = cx.try_client() else {
            return;
        };
        for r in asked {
            r.apply(client);
            tracing::info!(request = r.as_str(), session, "fleet request applied");
        }
        if !client.autoplay.config.team.enabled {
            // Off the team: the rules see nobody.
            if !client.autoplay.team.mates.is_empty() || client.autoplay.team.leader {
                client.autoplay.team = TeamView::default();
            }
            return;
        }
        let Some(me) = describe(client, session) else {
            return;
        };
        // Hand the rules what the others said.
        let view = self.roster.view_for(&me);
        if client.autoplay.team != view {
            client.autoplay.team = view;
        }
        // And say our piece, a few times a second.
        let due = self
            .last_said
            .get(&session)
            .is_none_or(|t| now.duration_since(*t) >= SAY_EVERY);
        if due {
            self.last_said.insert(session, now);
            if let Ok(value) = serde_json::to_value(&me) {
                cx.post(MATE_TOPIC, value);
            }
        }
    }
}

/// For a panel or a script: the roster as JSON, everyone heard.
pub fn roster_json(team: &Team) -> Value {
    Value::Array(
        team.roster
            .heard
            .values()
            .filter_map(|h| serde_json::to_value(&h.mate).ok())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mate(name: &str, guid: u32) -> Mate {
        Mate {
            name: name.into(),
            guid,
            ..Default::default()
        }
    }

    #[test]
    fn the_first_name_leads_and_the_quiet_are_forgotten() {
        let now = Instant::now();
        let mut r = Roster::default();
        let me = mate("Reborn", 1);
        // Alone: I lead, and see nobody.
        let v = r.view_for(&me);
        assert!(v.leader && v.mates.is_empty());
        // Another process's session speaks, with a name that sorts first.
        r.hear("other", 0, mate("+Admin", 2), now);
        let v = r.view_for(&me);
        assert!(!v.leader);
        assert_eq!(v.mates.len(), 1);
        assert!(v.mates[0].leader, "the other is the leader");
        // My own word coming back is not a mate.
        r.hear("local", 0, me.clone(), now);
        assert_eq!(r.view_for(&me).mates.len(), 1);
        // The same session speaking again replaces, not adds.
        r.hear("other", 0, mate("+Admin", 2), now + Duration::from_secs(1));
        assert_eq!(r.len(), 2);
        // Gone quiet: gone, my own echo included.
        r.forget_quiet(now + FORGET_AFTER + Duration::from_secs(2));
        assert!(r.is_empty());
        assert!(r.view_for(&me).leader, "alone again, so leading again");
    }

    #[test]
    fn requests_round_trip_through_their_messages() {
        for r in Request::ALL {
            assert_eq!(Request::parse(r.as_str()), Some(r));
            let m = r.message("bob", 2, "Brannoc");
            assert_eq!(
                Request::from_message(&m),
                Some(("bob".to_string(), 2, "Brannoc".to_string(), r))
            );
        }
        assert_eq!(Request::parse("dance"), None);
        assert_eq!(
            Request::from_message(&serde_json::json!({"process": "bob", "action": "stop"})),
            None,
            "a request names its session"
        );
        // The word comes from the roster, whichever process spoke.
        let mut r = Roster::default();
        r.hear("bob", 1, mate("Brannoc", 5), Instant::now());
        let heard: Vec<_> = r.iter().collect();
        assert_eq!(heard.len(), 1);
        assert_eq!(
            (heard[0].0, heard[0].1, heard[0].2.name.as_str()),
            ("bob", 1, "Brannoc")
        );
    }

    #[test]
    fn the_team_view_follows_the_leaders_target() {
        let now = Instant::now();
        let mut r = Roster::default();
        let me = mate("Zed", 1);
        let mut boss = mate("Alpha", 2);
        boss.target = Some(0x77);
        boss.target_name = "Drudge".into();
        r.hear("p", 0, boss, now);
        let v = r.view_for(&me);
        assert!(!v.leader);
        assert_eq!(v.target(), Some((0x77, "Drudge".into())));
    }
}
