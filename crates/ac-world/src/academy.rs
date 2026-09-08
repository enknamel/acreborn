//! The Training Academy tutorial as data: the ordered steps a new
//! character goes through to earn the exit portal, from talking to the
//! Society Greeter to walking into "Exit to Holtburg".
//!
//! Every character ACE creates starts in the academy (landblock 0x8602)
//! and its exit portal wants the Sentry's quest done, so a character
//! played by the rules has to get through it. The steps are server
//! data (NPC emote scripts, drop lists, portal restrictions), so
//! `data/academy.csv` is a copy of what they say; see
//! `docs/game/mechanics.md`, "Training Academy", for the story. The
//! step machine that walks them lives in the client crate.

use std::sync::OnceLock;

use glam::Vec3;

/// The academy's landblock.
pub const LANDBLOCK: u32 = 0x8602_0000;

const DATA: &str = include_str!("../data/academy.csv");

/// What a step asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Use the NPC and listen to what it says.
    Talk,
    /// Hand the NPC a carried item.
    Give,
    /// Pick an item up off the floor.
    Pickup,
    /// Wield a carried item.
    Wear,
    /// Fight creatures around a spot until an item is carried.
    Hunt,
    /// Use a carried key on a door and open it.
    Unlock,
    /// Walk into a portal.
    Portal,
    /// Go somewhere.
    Walk,
}

impl Kind {
    fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "talk" => Kind::Talk,
            "give" => Kind::Give,
            "pickup" => Kind::Pickup,
            "wear" => Kind::Wear,
            "hunt" => Kind::Hunt,
            "unlock" => Kind::Unlock,
            "portal" => Kind::Portal,
            "walk" => Kind::Walk,
            _ => return None,
        })
    }
}

/// One step of the tutorial.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    /// The quest the step belongs to; a quest found done is skipped
    /// whole.
    pub quest: String,
    pub kind: Kind,
    /// The NPC, object, item or creature name (a creature name matches
    /// by containment: "Olthoi" takes the young ones too).
    pub target: String,
    /// Where the target stands, landblock-local; the centre of a hunt.
    pub at: Option<Vec3>,
    /// The item concerned: given, picked up, worn, hunted for, or the
    /// key of a door. 0 when none.
    pub wcid: u32,
    /// Where a portal drops the character, landblock-local; `None` when
    /// it leaves the landblock.
    pub lands: Option<Vec3>,
    /// A line from the NPC meaning the task is still open.
    pub open: String,
    /// A line from the NPC meaning the quest is already done.
    pub done: String,
}

/// The steps of the full tutorial, in order.
pub fn steps() -> &'static [Step] {
    static STEPS: OnceLock<Vec<Step>> = OnceLock::new();
    STEPS.get_or_init(|| {
        parse(DATA)
            .into_iter()
            .filter(|s| s.quest != "skip")
            .collect()
    })
}

/// The steps of the short way out: Jonathan's Exit Token.
pub fn skip_steps() -> &'static [Step] {
    static STEPS: OnceLock<Vec<Step>> = OnceLock::new();
    STEPS.get_or_init(|| {
        parse(DATA)
            .into_iter()
            .filter(|s| s.quest == "skip")
            .collect()
    })
}

/// Parse the table. Malformed rows are skipped with a log line rather
/// than failing: a bad row costs one step, not the whole tutorial.
pub fn parse(text: &str) -> Vec<Step> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| match parse_row(l) {
            Some(s) => Some(s),
            None => {
                tracing::warn!("academy.csv: bad row {l:?}");
                None
            }
        })
        .collect()
}

fn parse_row(line: &str) -> Option<Step> {
    let f = fields(line);
    if f.len() != 12 {
        return None;
    }
    let num = |s: &str| -> Option<f32> { s.trim().parse().ok() };
    let point =
        |x: &str, y: &str, z: &str| -> Option<Vec3> { Some(Vec3::new(num(x)?, num(y)?, num(z)?)) };
    let kind = Kind::parse(f[1].trim())?;
    let at = point(&f[3], &f[4], &f[5]);
    if at.is_none() && !matches!(kind, Kind::Wear) {
        return None;
    }
    let wcid = if f[6].trim().is_empty() {
        0
    } else {
        f[6].trim().parse().ok()?
    };
    Some(Step {
        quest: f[0].trim().to_string(),
        kind,
        target: f[2].trim().to_string(),
        at,
        wcid,
        lands: point(&f[7], &f[8], &f[9]),
        open: f[10].trim().to_string(),
        done: f[11].trim().to_string(),
    })
}

/// Split a CSV row on commas, honouring double quotes around a field
/// (the NPC lines have commas in them).
fn fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_fields_keep_their_commas() {
        let f = fields(r#"a,"b, c",,"d""#);
        assert_eq!(f, vec!["a", "b, c", "", "d"]);
    }

    #[test]
    fn a_row_parses_to_a_step() {
        let s = parse_row(
            r#"wasp,give,Academy Foreman,36.8,-73.7,0,13089,,,,"","Thank you! You are certainly doing your part""#,
        )
        .expect("row");
        assert_eq!(s.quest, "wasp");
        assert_eq!(s.kind, Kind::Give);
        assert_eq!(s.target, "Academy Foreman");
        assert_eq!(s.at, Some(Vec3::new(36.8, -73.7, 0.0)));
        assert_eq!(s.wcid, 13089);
        assert_eq!(s.lands, None);
        assert!(s.open.is_empty());
        assert_eq!(s.done, "Thank you! You are certainly doing your part");
        // A portal knows where it lands.
        let p =
            parse_row(r#"token,portal,Central Courtyard,70,-40,-0.1,0,50,-54,1,"","""#).unwrap();
        assert_eq!(p.kind, Kind::Portal);
        assert_eq!(p.lands, Some(Vec3::new(50.0, -54.0, 1.0)));
        // Wearing needs no place; everything else does.
        assert!(parse_row(r#"armor,wear,Leather Cap,,,,13239,,,,"","""#).is_some());
        assert!(parse_row(r#"armor,pickup,Leather Cap,,,,13239,,,,"","""#).is_none());
        // Bad kinds, counts and numbers are refused.
        assert!(parse_row("x,dance,Samuel,1,2,3,0,,,,,").is_none());
        assert!(parse_row("x,talk,Samuel,1,2").is_none());
        assert!(parse_row(r#"x,talk,Samuel,one,2,3,0,,,,"","""#).is_none());
        assert!(parse_row(r#"x,give,Samuel,1,2,3,lots,,,,"","""#).is_none());
    }

    #[test]
    fn the_table_is_whole() {
        let full = steps();
        assert!(full.len() >= 20, "{} steps", full.len());
        assert!(full.iter().all(|s| s.quest != "skip"));
        // It starts in the first room and ends at the exit portal.
        assert_eq!(full[0].target, "Society Greeter");
        let last = full.last().unwrap();
        assert_eq!(last.kind, Kind::Portal);
        assert_eq!(last.target, "Exit to Holtburg");
        assert_eq!(last.lands, None);
        // Every hunt says what it is after, and every give what to hand over.
        for s in full {
            if matches!(
                s.kind,
                Kind::Hunt | Kind::Give | Kind::Pickup | Kind::Wear | Kind::Unlock
            ) {
                assert!(s.wcid != 0, "{s:?}");
            }
        }
        // The quests come in order, each in one run.
        let mut seen: Vec<&str> = Vec::new();
        for s in full {
            if seen.last() != Some(&s.quest.as_str()) {
                assert!(!seen.contains(&s.quest.as_str()), "{} comes twice", s.quest);
                seen.push(&s.quest);
            }
        }
        assert_eq!(
            seen,
            ["start", "armor", "token", "wasp", "bellows", "sentry", "exit"]
        );
        // The short way out is two steps with Jonathan.
        let skip = skip_steps();
        assert_eq!(skip.len(), 2);
        assert!(skip.iter().all(|s| s.target == "Jonathan"));
        assert_eq!(skip[1].wcid, 29335);
    }
}
