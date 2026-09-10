//! What the team does between fights: the mode it is all in, and the
//! money it needs to get back on its feet.
//!
//! A party is in one of two modes at a time. It is **hunting**, or it
//! is **restocking**. The switch is not made by each character on its
//! own -- a mage that walks off to town alone leaves the archer with
//! nothing to hide behind -- so the mode is decided from what the whole
//! roster says about itself, and every session reaches the same answer
//! from the same roster. There is no vote and nothing to agree on.
//!
//! The trigger is deliberately early. Running a party until someone
//! fires their last arrow means the trip to town starts from a losing
//! fight; instead the party leaves for town while everyone still has
//! enough to fight their way out. That is what [`Restock::go_at`] is:
//! the fraction of a full load that sends the whole party shopping.
//! Coming back needs a higher bar, [`Restock::full_at`], so a party
//! cannot leave town and immediately decide to return.
//!
//! There are two shapes of trip. The party can all walk to town, which
//! is simple and slow; or it can send one character -- the
//! quartermaster -- with everyone's sale loot and everyone's shopping
//! list, and have it hand the goods back out on its return. The second
//! is much faster and keeps the party together at the hunting ground,
//! at the cost of needing one pack big enough for the lot.
//!
//! Money is a team matter too. Loot sells for pyreals, pyreals are
//! heavy and capped, and the useful form to carry is trade notes; and
//! the character that looted the good armour is not always the one that
//! needs to buy four hundred tapers. So after selling, whoever is over
//! their own bill hands the difference to whoever is under theirs.

use std::fmt;

use serde::{Deserialize, Serialize};

/// What the party is doing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupMode {
    /// Out in the field, fighting.
    #[default]
    Hunting,
    /// Selling, buying and getting back together.
    Restocking(Stage),
}

/// Where a restocking trip has got to. A trip everyone makes together
/// has only one stage; a quartermaster run has three, because the party
/// has to load the runner, wait for it, and be given the goods back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stage {
    /// Everyone is walking to town and buying for themselves.
    #[default]
    Shopping,
    /// Everyone is handing the quartermaster their sale loot and their
    /// order.
    HandOver,
    /// The quartermaster is in town; the rest hold at the hunting
    /// ground.
    Away,
    /// The quartermaster is back and giving out what it bought.
    HandOut,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Stage::Shopping => "shopping",
            Stage::HandOver => "loading the quartermaster",
            Stage::Away => "waiting for the quartermaster",
            Stage::HandOut => "handing out supplies",
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl GroupMode {
    pub fn label(self) -> &'static str {
        match self {
            GroupMode::Hunting => "hunting",
            GroupMode::Restocking(s) => s.label(),
        }
    }

    /// Whether the party is off fighting.
    pub fn hunting(self) -> bool {
        matches!(self, GroupMode::Hunting)
    }

    /// The stage of the trip, if one is under way.
    pub fn stage(self) -> Option<Stage> {
        match self {
            GroupMode::Hunting => None,
            GroupMode::Restocking(s) => Some(s),
        }
    }
}

impl fmt::Display for GroupMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// How a restocking trip is made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Plan {
    /// Everyone walks to town and shops for themselves. Slow, but it
    /// needs nothing of anybody and cannot strand the party.
    #[default]
    Everyone,
    /// One character carries the party's sale loot and its shopping
    /// list to town and brings the goods back. Much faster, and the
    /// party keeps its place at the hunting ground.
    Quartermaster,
}

impl Plan {
    pub const ALL: [Plan; 2] = [Plan::Everyone, Plan::Quartermaster];

    pub fn label(self) -> &'static str {
        match self {
            Plan::Everyone => "everyone goes to town",
            Plan::Quartermaster => "one character shops for the party",
        }
    }
}

/// How the party keeps itself supplied. How *much* of each thing to
/// carry lives in `growth::Growth` alongside the other amounts; this is
/// the policy: when to go, how to go, and what to do about money.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Restock {
    /// Restock as a party rather than one at a time.
    pub together: bool,
    /// How the party makes the trip.
    pub plan: Plan,
    /// The fewest free pack slots a quartermaster needs before the
    /// party will trust it with the run. This is a floor, not a
    /// requirement to carry the whole order at once: a runner that
    /// cannot fit everything makes another trip. Below the floor there
    /// is no point sending anybody, and the party walks to town.
    pub runner_space: u32,
    /// The whole party goes shopping once any one member is down to
    /// this fraction of a full load. Well before empty: the trip to
    /// town has to be survivable.
    pub go_at: f32,
    /// A character counts as stocked again at this fraction. Higher
    /// than `go_at` so a party that has just shopped does not turn
    /// straight round.
    pub full_at: f32,
    /// Pyreals to keep in hand after shopping, for the next trip.
    pub float: u32,
    /// Turn what is left over into trade notes rather than carrying
    /// coin, and share the notes out so everyone can pay their own way.
    pub share_money: bool,
    /// How many trips the quartermaster may make before the party
    /// settles for what it has. One pack does not always hold a whole
    /// party's shopping, and a vendor does not always have it all, so
    /// more than one round is normal rather than a failure.
    pub max_rounds: u32,
    /// Give up on a trip that has taken this many seconds and go back
    /// to hunting.
    ///
    /// A trip can fail to finish for reasons no rule here can see: the
    /// vendor is out of tapers, the money ran out, a character died on
    /// the way. Without a limit the party would wait at the hunting
    /// ground for ever, which is worse than hunting undersupplied.
    pub give_up_after: f32,
}

impl Default for Restock {
    fn default() -> Self {
        Restock {
            together: true,
            plan: Plan::default(),
            runner_space: 10,
            go_at: 0.35,
            full_at: 0.9,
            float: 5_000,
            share_money: true,
            max_rounds: 4,
            give_up_after: 900.0,
        }
    }
}

impl Restock {
    /// Guard against a config that would make the party thrash: the
    /// bar for coming home must sit above the bar for leaving.
    pub fn sane(&self) -> Restock {
        let go = self.go_at.clamp(0.0, 0.95);
        let full = self.full_at.clamp(go + 0.05, 1.0);
        Restock {
            go_at: go,
            full_at: full,
            ..self.clone()
        }
    }
}

/// What one character says about its own supplies, for the party to
/// decide on. Filled in by each session about itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Supplies {
    /// Who it is.
    pub name: String,
    /// Its worst-off supply, as a fraction of what it wants to carry:
    /// 0.0 out, 1.0 full. A character that wants nothing is 1.0.
    pub level: f32,
    /// It cannot pick anything else up.
    pub pack_full: bool,
    /// It has been to the vendors it needed and is ready to go back.
    pub stocked: bool,
    /// What its remaining shopping will cost, in pyreals.
    pub bill: u32,
    /// What it can spend: coin and notes together.
    pub purse: u32,
    /// It has bought everything it can pay for. A character short of
    /// something it cannot afford is not going to become less short by
    /// standing at the counter, so it counts as done: it goes back to
    /// hunting with what it has and earns the rest.
    pub broke: bool,
    /// Free slots in its pack: what decides who can be quartermaster.
    pub free_space: u32,
    /// It has given the quartermaster its sale loot and its order.
    pub handed_over: bool,
    /// The quartermaster still has goods in its pack that belong to
    /// somebody else. While this is true the party is still being
    /// unloaded; once it is false and anyone is still short, the run
    /// was not big enough and another round is needed.
    pub holding_orders: bool,
    /// What it wants brought back, by name and count. The quartermaster
    /// adds these up into one shopping list.
    pub order: Vec<(String, u32)>,
}

impl Supplies {
    /// Whether this character alone is reason enough to go shopping.
    fn wants_town(&self, cfg: &Restock) -> bool {
        self.pack_full || self.level < cfg.go_at
    }

    /// Whether the party need wait for it any longer.
    fn settled(&self) -> bool {
        self.stocked || self.broke
    }
}

/// Why the party is going shopping, for the log and the UI.
#[derive(Clone, Debug, PartialEq)]
pub struct Switch {
    pub mode: GroupMode,
    pub because: String,
}

/// The mode the party should be in, given what everyone has said.
///
/// `now` is the mode it is in. `mates` is the whole roster, this
/// character included; an empty roster means nobody has spoken yet, in
/// which case nothing changes. The answer depends only on the
/// arguments, so every session in the party works out the same one.
pub fn decide(now: GroupMode, mates: &[Supplies], cfg: &Restock, round: u32) -> Option<Switch> {
    let cfg = cfg.sane();
    if mates.is_empty() {
        return None;
    }
    match now {
        GroupMode::Hunting => start_trip(mates, &cfg),
        GroupMode::Restocking(stage) => advance_trip(stage, mates, &cfg, round),
    }
}

/// Whether the party should stop hunting, and what shape of trip it
/// should make.
fn start_trip(mates: &[Supplies], cfg: &Restock) -> Option<Switch> {
    // One member short is enough. Splitting the party so that one can
    // shop is worse than all of it going.
    let mut short: Vec<&Supplies> = mates.iter().filter(|m| m.wants_town(cfg)).collect();
    if short.is_empty() {
        return None;
    }
    short.sort_by(|a, b| a.level.total_cmp(&b.level));
    let because = short
        .iter()
        .map(|m| {
            if m.pack_full {
                format!("{}'s pack is full", m.name)
            } else {
                format!("{} is down to {:.0}% supplies", m.name, m.level * 100.0)
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(Switch {
        mode: GroupMode::Restocking(opening_stage(mates, cfg)),
        because,
    })
}

/// The stage a trip starts in. A quartermaster run needs somebody able
/// to carry the party's loot out and its order back, and needs somebody
/// else to carry it for; without either, the party walks to town.
fn opening_stage(mates: &[Supplies], cfg: &Restock) -> Stage {
    if cfg.plan != Plan::Quartermaster || mates.len() < 2 {
        return Stage::Shopping;
    }
    match quartermaster(mates) {
        Some(q) if q.free_space >= cfg.runner_space => Stage::HandOver,
        _ => Stage::Shopping,
    }
}

/// Whether the trip has reached its next stage.
fn advance_trip(stage: Stage, mates: &[Supplies], cfg: &Restock, round: u32) -> Option<Switch> {
    let done = |m: &Supplies| m.settled();
    match stage {
        // Everyone shopping for themselves: one stage, and nobody goes
        // back until everybody is ready. A party that trickles back to
        // the hunting ground arrives in pieces.
        Stage::Shopping => mates.iter().all(done).then(|| Switch {
            mode: GroupMode::Hunting,
            because: "everyone is restocked".to_string(),
        }),
        // Loading the runner. It leaves once everyone else has given it
        // their sale loot and their order.
        Stage::HandOver => {
            let runner = quartermaster(mates)?.name.clone();
            let loaded = mates.iter().all(|m| m.name == runner || m.handed_over);
            loaded.then(|| Switch {
                mode: GroupMode::Restocking(Stage::Away),
                because: format!("{runner} has the party's loot and orders"),
            })
        }
        // The runner is in town. The rest hold until it says it is done.
        Stage::Away => {
            let runner = quartermaster(mates)?;
            done(runner).then(|| Switch {
                mode: GroupMode::Restocking(Stage::HandOut),
                because: format!("{} is back with the supplies", runner.name),
            })
        }
        // Handing the goods out. Over when everyone, runner included,
        // has what it asked for -- or when the runner's pack is empty
        // and somebody still is not, in which case one trip was not
        // enough and it goes back for another. A pack does not always
        // hold a whole party's shopping and a vendor does not always
        // have it all, so this is normal rather than a failure.
        Stage::HandOut => {
            if mates.iter().all(done) {
                return Some(Switch {
                    mode: GroupMode::Hunting,
                    because: "everyone has their supplies".to_string(),
                });
            }
            let runner = quartermaster(mates)?;
            if runner.holding_orders {
                return None;
            }
            if round + 1 >= cfg.max_rounds {
                let short: Vec<&str> = still_shopping(mates);
                return Some(Switch {
                    mode: GroupMode::Hunting,
                    because: format!(
                        "back to hunting after {} trips; {} still short",
                        round + 1,
                        short.join(", ")
                    ),
                });
            }
            Some(Switch {
                mode: GroupMode::Restocking(Stage::HandOver),
                because: format!("one trip was not enough; {} is going back", runner.name),
            })
        }
    }
}

/// Who makes the run for the party: the one with the most room in its
/// pack, ties settled by name so that every session picks the same
/// character without having to agree on one.
pub fn quartermaster(mates: &[Supplies]) -> Option<&Supplies> {
    mates.iter().filter(|m| !m.name.is_empty()).max_by(|a, b| {
        a.free_space
            .cmp(&b.free_space)
            .then_with(|| b.name.cmp(&a.name))
    })
}

/// Everything the party wants bought, gathered into one list for the
/// quartermaster to shop from. Orders for the same thing are added up,
/// so one trip to the archmage buys every mage's tapers at once.
pub fn merged_order(mates: &[Supplies]) -> Vec<(String, u32)> {
    let mut total: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    for m in mates {
        for (name, count) in &m.order {
            if name.trim().is_empty() || *count == 0 {
                continue;
            }
            *total.entry(name.clone()).or_default() += count;
        }
    }
    total.into_iter().collect()
}

/// How to split what the quartermaster actually brought home.
///
/// A run does not always fill every order -- a vendor runs out, the
/// money runs out -- so what came back is shared in proportion to what
/// each character asked for, and the remainder goes to the largest
/// order. Nobody is given more than they asked for.
pub fn hand_out(mates: &[Supplies], item: &str, brought: u32) -> Vec<(String, u32)> {
    let asked: Vec<(&str, u32)> = mates
        .iter()
        .filter_map(|m| {
            m.order
                .iter()
                .find(|(n, _)| n == item)
                .filter(|(_, c)| *c > 0)
                .map(|(_, c)| (m.name.as_str(), *c))
        })
        .collect();
    let wanted: u32 = asked.iter().map(|(_, c)| *c).sum();
    if wanted == 0 || brought == 0 {
        return Vec::new();
    }
    if brought >= wanted {
        return asked.iter().map(|(n, c)| (n.to_string(), *c)).collect();
    }
    let mut out: Vec<(String, u32)> = asked
        .iter()
        .map(|(n, c)| {
            let share = (*c as u64 * brought as u64 / wanted as u64) as u32;
            (n.to_string(), share)
        })
        .collect();
    // Integer division loses a few; give them to the biggest order.
    let mut left = brought - out.iter().map(|(_, c)| *c).sum::<u32>();
    let mut order: Vec<usize> = (0..asked.len()).collect();
    order.sort_by(|&a, &b| {
        asked[b]
            .1
            .cmp(&asked[a].1)
            .then_with(|| asked[a].0.cmp(asked[b].0))
    });
    for i in order {
        if left == 0 {
            break;
        }
        let room = asked[i].1 - out[i].1;
        let give = room.min(left);
        out[i].1 += give;
        left -= give;
    }
    out.retain(|(_, c)| *c > 0);
    out
}

/// Whoever is still shopping, for the ones who have finished to wait on.
pub fn still_shopping(mates: &[Supplies]) -> Vec<&str> {
    mates
        .iter()
        .filter(|m| !m.settled())
        .map(|m| m.name.as_str())
        .collect()
}

/// One character handing money to another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub from: String,
    pub to: String,
    pub amount: u32,
}

/// Who should hand money to whom so that everyone can pay their own
/// bill.
///
/// Selling is not shared out evenly -- the one who looted the good
/// armour comes out of town rich while the mage who burnt four hundred
/// tapers comes out broke -- so the surplus is moved to cover the
/// shortfalls. The richest surplus covers the largest shortfall first,
/// which settles the party in as few handovers as it can. Nobody is
/// asked to give away money it needs itself, and a shortfall nobody can
/// cover is simply left: the character buys what it can afford.
pub fn share_money(mates: &[Supplies]) -> Vec<Transfer> {
    let mut givers: Vec<(String, u32)> = mates
        .iter()
        .filter_map(|m| {
            m.purse
                .checked_sub(m.bill)
                .filter(|s| *s > 0)
                .map(|s| (m.name.clone(), s))
        })
        .collect();
    let mut takers: Vec<(String, u32)> = mates
        .iter()
        .filter_map(|m| {
            m.bill
                .checked_sub(m.purse)
                .filter(|s| *s > 0)
                .map(|s| (m.name.clone(), s))
        })
        .collect();
    // Largest first on both sides, ties by name so that every session
    // in the party works out the same list.
    givers.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    takers.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut moves = Vec::new();
    let mut g = 0;
    for (to, mut need) in takers {
        while need > 0 && g < givers.len() {
            let spare = givers[g].1;
            if spare == 0 {
                g += 1;
                continue;
            }
            let amount = spare.min(need);
            moves.push(Transfer {
                from: givers[g].0.clone(),
                to: to.clone(),
                amount,
            });
            givers[g].1 -= amount;
            need -= amount;
        }
        if g >= givers.len() {
            break;
        }
    }
    moves
}

/// How full a supply line is: `have` against `want`. A line that wants
/// nothing is full, not empty -- an archer carries no tapers and is not
/// thereby out of supplies.
pub fn line_level(have: u32, want: u32) -> f32 {
    if want == 0 {
        return 1.0;
    }
    (have as f32 / want as f32).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mate(name: &str, level: f32) -> Supplies {
        Supplies {
            name: name.into(),
            level,
            stocked: level >= 0.9,
            free_space: 50,
            ..Supplies::default()
        }
    }

    const SHOPPING: GroupMode = GroupMode::Restocking(Stage::Shopping);

    #[test]
    fn a_full_party_keeps_hunting() {
        let cfg = Restock::default();
        let party = [mate("Aldric", 1.0), mate("Bryn", 0.8)];
        assert_eq!(decide(GroupMode::Hunting, &party, &cfg, 0), None);
    }

    #[test]
    fn one_member_running_low_takes_the_whole_party_to_town() {
        let cfg = Restock::default();
        // Bryn is below go_at while everyone else is comfortable.
        let party = [mate("Aldric", 1.0), mate("Bryn", 0.2), mate("Caius", 0.9)];
        let s = decide(GroupMode::Hunting, &party, &cfg, 0).expect("a switch");
        assert_eq!(s.mode, SHOPPING);
        assert!(s.because.contains("Bryn"), "{}", s.because);
    }

    #[test]
    fn the_party_leaves_before_anyone_is_empty() {
        // The point of the early trigger: at the moment the party turns
        // round, its worst-off member still has a third of a load.
        let cfg = Restock::default();
        let party = [mate("Aldric", 0.34)];
        assert!(decide(GroupMode::Hunting, &party, &cfg, 0).is_some());
        assert!(party[0].level > 0.0);
    }

    #[test]
    fn a_full_pack_is_reason_enough() {
        let cfg = Restock::default();
        let mut m = mate("Aldric", 1.0);
        m.pack_full = true;
        let s = decide(GroupMode::Hunting, &[m], &cfg, 0).expect("a switch");
        assert_eq!(s.mode, SHOPPING);
        assert!(s.because.contains("pack is full"), "{}", s.because);
    }

    #[test]
    fn nobody_goes_back_until_everybody_is_ready() {
        let cfg = Restock::default();
        let mut party = [mate("Aldric", 1.0), mate("Bryn", 1.0)];
        party[1].stocked = false;
        assert_eq!(decide(SHOPPING, &party, &cfg, 0), None);
        assert_eq!(still_shopping(&party), vec!["Bryn"]);
        party[1].stocked = true;
        let s = decide(SHOPPING, &party, &cfg, 0).expect("a switch");
        assert_eq!(s.mode, GroupMode::Hunting);
    }

    #[test]
    fn a_party_that_just_shopped_does_not_turn_straight_round() {
        // Coming home needs full_at, leaving needs go_at, so the level
        // that ends a trip cannot also start one.
        let cfg = Restock::default().sane();
        let party = [mate("Aldric", cfg.full_at)];
        assert_eq!(
            decide(SHOPPING, &party, &cfg, 0).map(|s| s.mode),
            Some(GroupMode::Hunting)
        );
        assert_eq!(decide(GroupMode::Hunting, &party, &cfg, 0), None);
    }

    #[test]
    fn a_config_with_the_bars_the_wrong_way_round_is_straightened_out() {
        let bad = Restock {
            go_at: 0.9,
            full_at: 0.1,
            ..Restock::default()
        };
        let ok = bad.sane();
        assert!(ok.full_at > ok.go_at, "{ok:?}");
    }

    #[test]
    fn nobody_speaking_yet_changes_nothing() {
        let cfg = Restock::default();
        assert_eq!(decide(GroupMode::Hunting, &[], &cfg, 0), None);
        assert_eq!(decide(SHOPPING, &[], &cfg, 0), None);
    }

    fn purse(name: &str, purse: u32, bill: u32) -> Supplies {
        Supplies {
            name: name.into(),
            purse,
            bill,
            ..Supplies::default()
        }
    }

    #[test]
    fn the_one_who_sold_the_armour_pays_for_the_mages_tapers() {
        let party = [purse("Aldric", 100_000, 5_000), purse("Bryn", 500, 20_000)];
        let moves = share_money(&party);
        assert_eq!(
            moves,
            vec![Transfer {
                from: "Aldric".into(),
                to: "Bryn".into(),
                amount: 19_500,
            }]
        );
    }

    #[test]
    fn a_party_that_can_all_pay_moves_nothing() {
        let party = [purse("Aldric", 10_000, 5_000), purse("Bryn", 9_000, 9_000)];
        assert!(share_money(&party).is_empty());
    }

    #[test]
    fn one_purse_can_cover_two_shortfalls() {
        let party = [
            purse("Aldric", 50_000, 0),
            purse("Bryn", 0, 10_000),
            purse("Caius", 0, 4_000),
        ];
        let moves = share_money(&party);
        assert_eq!(moves.len(), 2);
        assert!(moves.iter().all(|m| m.from == "Aldric"));
        // Largest shortfall first, so the party settles in as few
        // handovers as it can.
        assert_eq!(moves[0].to, "Bryn");
        assert_eq!(moves[0].amount, 10_000);
        assert_eq!(moves[1].to, "Caius");
        assert_eq!(moves[1].amount, 4_000);
    }

    #[test]
    fn two_purses_are_pooled_for_one_shortfall() {
        let party = [
            purse("Aldric", 6_000, 0),
            purse("Bryn", 5_000, 0),
            purse("Caius", 0, 10_000),
        ];
        let moves = share_money(&party);
        assert_eq!(moves.iter().map(|m| m.amount).sum::<u32>(), 10_000);
        assert!(moves.iter().all(|m| m.to == "Caius"));
    }

    #[test]
    fn a_shortfall_nobody_can_cover_is_left_alone() {
        // Not an error: the character buys what it can afford. What it
        // must not do is take money someone else needs.
        let party = [purse("Aldric", 1_000, 0), purse("Bryn", 0, 90_000)];
        let moves = share_money(&party);
        assert_eq!(moves.iter().map(|m| m.amount).sum::<u32>(), 1_000);
    }

    #[test]
    fn nobody_is_asked_to_give_away_money_it_needs_itself() {
        let party = [purse("Aldric", 10_000, 10_000), purse("Bryn", 0, 5_000)];
        assert!(share_money(&party).is_empty());
    }

    #[test]
    fn an_archer_carrying_no_tapers_is_not_out_of_supplies() {
        assert_eq!(line_level(0, 0), 1.0);
        assert_eq!(line_level(50, 100), 0.5);
        // More than asked for is still just full.
        assert_eq!(line_level(400, 100), 1.0);
    }

    // ---- the quartermaster run ------------------------------------

    fn qm_cfg() -> Restock {
        Restock {
            plan: Plan::Quartermaster,
            ..Restock::default()
        }
    }

    #[test]
    fn a_quartermaster_run_starts_by_loading_the_runner() {
        let mut party = [mate("Aldric", 1.0), mate("Bryn", 0.2)];
        party[0].free_space = 90;
        party[1].free_space = 10;
        let s = decide(GroupMode::Hunting, &party, &qm_cfg(), 0).expect("a switch");
        assert_eq!(s.mode, GroupMode::Restocking(Stage::HandOver));
        // The roomiest pack carries for the party.
        assert_eq!(
            quartermaster(&party).map(|m| m.name.as_str()),
            Some("Aldric")
        );
    }

    #[test]
    fn a_party_with_no_room_to_spare_walks_to_town_instead() {
        // Sending a runner that cannot carry the order home would
        // strand the party, so it falls back to everyone going.
        let mut party = [mate("Aldric", 1.0), mate("Bryn", 0.2)];
        party[0].free_space = 3;
        party[1].free_space = 2;
        let s = decide(GroupMode::Hunting, &party, &qm_cfg(), 0).expect("a switch");
        assert_eq!(s.mode, SHOPPING);
    }

    #[test]
    fn a_character_on_its_own_has_nobody_to_send() {
        let mut party = [mate("Aldric", 0.1)];
        party[0].free_space = 90;
        let s = decide(GroupMode::Hunting, &party, &qm_cfg(), 0).expect("a switch");
        assert_eq!(s.mode, SHOPPING);
    }

    #[test]
    fn the_runner_leaves_once_everyone_else_has_loaded_it() {
        let mut party = [mate("Aldric", 1.0), mate("Bryn", 0.2)];
        party[0].free_space = 90;
        party[1].free_space = 10;
        let at = GroupMode::Restocking(Stage::HandOver);
        // Bryn has not handed anything over yet.
        assert_eq!(decide(at, &party, &qm_cfg(), 0), None);
        party[1].handed_over = true;
        // The runner does not wait on itself.
        let s = decide(at, &party, &qm_cfg(), 0).expect("a switch");
        assert_eq!(s.mode, GroupMode::Restocking(Stage::Away));
        assert!(s.because.contains("Aldric"), "{}", s.because);
    }

    #[test]
    fn the_party_waits_at_the_hunting_ground_until_the_runner_is_done() {
        let mut party = [mate("Aldric", 0.5), mate("Bryn", 0.2)];
        party[0].free_space = 90;
        party[1].free_space = 10;
        party[0].stocked = false;
        party[1].stocked = false;
        let at = GroupMode::Restocking(Stage::Away);
        assert_eq!(decide(at, &party, &qm_cfg(), 0), None);
        // Only the runner's word matters here: the others cannot be
        // stocked until it hands their goods over.
        party[0].stocked = true;
        let s = decide(at, &party, &qm_cfg(), 0).expect("a switch");
        assert_eq!(s.mode, GroupMode::Restocking(Stage::HandOut));
    }

    #[test]
    fn hunting_resumes_when_the_last_order_is_handed_out() {
        let mut party = [mate("Aldric", 1.0), mate("Bryn", 0.2)];
        party[0].free_space = 90;
        party[0].stocked = true;
        party[1].stocked = false;
        let at = GroupMode::Restocking(Stage::HandOut);
        // The runner still has goods in its pack: it is mid-handout.
        party[0].holding_orders = true;
        assert_eq!(decide(at, &party, &qm_cfg(), 0), None);
        party[0].holding_orders = false;
        party[1].stocked = true;
        let s = decide(at, &party, &qm_cfg(), 0).expect("a switch");
        assert_eq!(s.mode, GroupMode::Hunting);
    }

    #[test]
    fn one_trip_that_was_not_enough_becomes_a_second() {
        // The runner's pack is empty and Bryn is still short, so one
        // trip did not cover the party and it goes back for more.
        let mut party = [mate("Aldric", 1.0), mate("Bryn", 0.2)];
        party[0].free_space = 90;
        party[0].stocked = true;
        party[1].stocked = false;
        party[0].holding_orders = false;
        let at = GroupMode::Restocking(Stage::HandOut);
        let s = decide(at, &party, &qm_cfg(), 0).expect("a switch");
        assert_eq!(s.mode, GroupMode::Restocking(Stage::HandOver));
        assert!(s.because.contains("not enough"), "{}", s.because);
    }

    #[test]
    fn the_party_stops_going_back_eventually() {
        // Some things simply cannot be bought: no vendor has them, or
        // the money has run out. After the allowed rounds the party
        // settles for what it has rather than shuttling for ever.
        let cfg = Restock {
            max_rounds: 3,
            ..qm_cfg()
        };
        let mut party = [mate("Aldric", 1.0), mate("Bryn", 0.2)];
        party[0].free_space = 90;
        party[0].stocked = true;
        party[1].stocked = false;
        let at = GroupMode::Restocking(Stage::HandOut);
        // Rounds 0 and 1 go back for more.
        for round in 0..2 {
            let s = decide(at, &party, &cfg, round).expect("a switch");
            assert_eq!(
                s.mode,
                GroupMode::Restocking(Stage::HandOver),
                "round {round}"
            );
        }
        // The last one gives up and says who is still short.
        let s = decide(at, &party, &cfg, 2).expect("a switch");
        assert_eq!(s.mode, GroupMode::Hunting);
        assert!(s.because.contains("Bryn"), "{}", s.because);
        assert!(s.because.contains("3 trips"), "{}", s.because);
    }

    #[test]
    fn every_session_picks_the_same_runner_from_the_same_roster() {
        // Equal packs: the tie has to break the same way everywhere, or
        // two characters both set off for town.
        let mut party = [mate("Bryn", 1.0), mate("Aldric", 1.0)];
        party[0].free_space = 40;
        party[1].free_space = 40;
        assert_eq!(
            quartermaster(&party).map(|m| m.name.as_str()),
            Some("Aldric")
        );
        party.reverse();
        assert_eq!(
            quartermaster(&party).map(|m| m.name.as_str()),
            Some("Aldric")
        );
    }

    // ---- the shopping list ----------------------------------------

    fn orders(name: &str, order: &[(&str, u32)]) -> Supplies {
        Supplies {
            name: name.into(),
            order: order.iter().map(|(n, c)| (n.to_string(), *c)).collect(),
            ..Supplies::default()
        }
    }

    #[test]
    fn one_trip_to_the_archmage_buys_every_mages_tapers() {
        let party = [
            orders("Aldric", &[("Prismatic Taper", 300), ("Lead Scarab", 50)]),
            orders("Bryn", &[("Prismatic Taper", 200)]),
            orders("Caius", &[("Arrowhead", 250)]),
        ];
        assert_eq!(
            merged_order(&party),
            vec![
                ("Arrowhead".to_string(), 250),
                ("Lead Scarab".to_string(), 50),
                ("Prismatic Taper".to_string(), 500),
            ]
        );
    }

    #[test]
    fn an_empty_order_line_is_ignored() {
        let party = [orders("Aldric", &[("", 5), ("Prismatic Taper", 0)])];
        assert!(merged_order(&party).is_empty());
    }

    #[test]
    fn a_full_run_gives_everyone_what_they_asked_for() {
        let party = [
            orders("Aldric", &[("Prismatic Taper", 300)]),
            orders("Bryn", &[("Prismatic Taper", 200)]),
        ];
        let out = hand_out(&party, "Prismatic Taper", 500);
        assert_eq!(
            out,
            vec![("Aldric".to_string(), 300), ("Bryn".to_string(), 200)]
        );
    }

    #[test]
    fn a_short_run_is_shared_out_in_proportion() {
        // The vendor only had half of what the party wanted.
        let party = [
            orders("Aldric", &[("Prismatic Taper", 300)]),
            orders("Bryn", &[("Prismatic Taper", 200)]),
        ];
        let out = hand_out(&party, "Prismatic Taper", 250);
        assert_eq!(
            out,
            vec![("Aldric".to_string(), 150), ("Bryn".to_string(), 100)]
        );
    }

    #[test]
    fn nothing_brought_back_is_handed_out_twice() {
        // Whatever the split, the party cannot be given more than the
        // runner is carrying, and nobody gets more than they asked for.
        let party = [
            orders("Aldric", &[("Prismatic Taper", 7)]),
            orders("Bryn", &[("Prismatic Taper", 5)]),
            orders("Caius", &[("Prismatic Taper", 3)]),
        ];
        for brought in 0..=20u32 {
            let out = hand_out(&party, "Prismatic Taper", brought);
            let given: u32 = out.iter().map(|(_, c)| c).sum();
            assert_eq!(given, brought.min(15), "brought {brought}");
            for (who, got) in &out {
                let asked = party
                    .iter()
                    .find(|m| &m.name == who)
                    .and_then(|m| m.order.first())
                    .map(|(_, c)| *c)
                    .unwrap();
                assert!(*got <= asked, "{who} got {got} of {asked}");
            }
        }
    }

    #[test]
    fn nobody_is_handed_something_they_did_not_ask_for() {
        let party = [orders("Aldric", &[("Prismatic Taper", 100)])];
        assert!(hand_out(&party, "Lead Scarab", 50).is_empty());
    }
    #[test]
    fn a_character_that_cannot_afford_the_rest_stops_shopping() {
        // Standing at the counter with an empty purse does not make it
        // less short. It goes back to hunting and earns the rest, which
        // is the only thing that will actually help.
        let cfg = qm_cfg();
        let mut party = [mate("Aldric", 0.2), mate("Bryn", 1.0)];
        party[0].stocked = false;
        party[1].stocked = true;
        let at = SHOPPING;
        assert_eq!(decide(at, &party, &cfg, 0), None, "still hoping");
        party[0].broke = true;
        let s = decide(at, &party, &cfg, 0).expect("a switch");
        assert_eq!(s.mode, GroupMode::Hunting);
        assert!(still_shopping(&party).is_empty());
    }
}
