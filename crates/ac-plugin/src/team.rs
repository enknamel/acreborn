//! The wire between characters playing together.
//!
//! `ac_client::autoplay` has rules for a team: fight what the leader is
//! fighting, land the debuffs on it, hand a teammate what it is short
//! of, heal whoever is worst hurt, bring everyone into one fellowship.
//! Those rules read a [`TeamView`], and until now nothing filled it in.
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
        let named = |lead_only: bool| {
            mates
                .iter()
                .chain(std::iter::once(me))
                .filter(|m| !lead_only || m.leads)
                .map(|m| m.name.as_str())
                .filter(|n| !n.is_empty())
                .min()
                .map(str::to_string)
        };
        let first = named(true).or_else(|| named(false));
        let leader = first.as_deref() == Some(me.name.as_str());
        for m in &mut mates {
            m.leader = first.as_deref() == Some(m.name.as_str());
        }
        TeamView { mates, leader }
    }

    pub fn len(&self) -> usize {
        self.heard.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heard.is_empty()
    }
}

/// The plugin: one roster shared by every session in this process, and
/// when each session last spoke.
#[derive(Default)]
pub struct Team {
    roster: Roster,
    last_said: BTreeMap<usize, Instant>,
}

/// What a session says about itself.
fn describe(client: &ac_client::Client, session: usize) -> Option<Mate> {
    let player = client.player.as_ref()?;
    let cfg = &client.autoplay.config.team;
    let name = client.world.stats.name.clone();
    if name.is_empty() {
        return None;
    }
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
    })
}

impl Plugin for Team {
    fn name(&self) -> &str {
        "team"
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

        let Some(client) = cx.try_client() else {
            return;
        };
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
