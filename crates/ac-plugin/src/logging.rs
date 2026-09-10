//! Which log lines reach the terminal, per part of the client.
//!
//! Every subsystem logs through `tracing`, and what is shown is a
//! filter: a default level plus an override per target
//! (`warn,ac_client=info,ac_net=debug`). Setting that once at startup
//! is fine for a developer and useless to a player watching a character
//! do something odd, who wants to turn one part up now and back down
//! when they have seen it. These build the line; the app owns the
//! filter itself and reloads it.

/// The parts of the client worth turning up on their own, and what each
/// one covers. These are `tracing` targets: the crate a message came
/// from, which is as fine-grained as the tree goes today.
pub const SYSTEMS: &[(&str, &str)] = &[
    ("acswarm", "the app itself: startup, sessions, the window"),
    ("ac_client", "playing: fighting, looting, buffing, travel"),
    ("ac_net", "the wire: packets, retransmits, the handshake"),
    ("ac_world", "what the server says the world contains"),
    ("ac_scene", "loading and building what is drawn"),
    ("ac_plugin", "panels, settings and the fleet"),
    ("ac_script", "scripts and their commands"),
    ("ac_bus", "the bus between processes"),
];

/// The levels a system can be set to, quietest first.
pub const LEVELS: &[&str] = &["off", "error", "warn", "info", "debug", "trace"];

/// What the filter starts as when nothing says otherwise: the app's own
/// progress, and warnings from everything else.
pub const DEFAULT: &str = "warn,acswarm=info";

/// Read one system's level out of a filter line, if it names one.
pub fn level_of(filter: &str, system: &str) -> Option<String> {
    filter.split(',').find_map(|part| {
        let (target, level) = part.trim().split_once('=')?;
        (target.trim() == system).then(|| level.trim().to_string())
    })
}

/// The default level a filter line sets for everything unnamed.
pub fn base_of(filter: &str) -> String {
    filter
        .split(',')
        .map(str::trim)
        .find(|p| !p.contains('=') && !p.is_empty())
        .unwrap_or("warn")
        .to_string()
}

/// A filter line with `system` set to `level`, or dropped when `level`
/// is `None` (back to the default).
pub fn with_level(filter: &str, system: &str, level: Option<&str>) -> String {
    let mut parts: Vec<String> = filter
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .filter(|p| p.split_once('=').map(|(t, _)| t.trim()) != Some(system))
        .map(str::to_string)
        .collect();
    if let Some(level) = level {
        parts.push(format!("{system}={level}"));
    }
    if parts.is_empty() {
        return "warn".to_string();
    }
    parts.join(",")
}

/// A filter line with the default level for everything unnamed changed.
pub fn with_base(filter: &str, level: &str) -> String {
    let rest: Vec<&str> = filter
        .split(',')
        .map(str::trim)
        .filter(|p| p.contains('=') && !p.is_empty())
        .collect();
    if rest.is_empty() {
        return level.to_string();
    }
    format!("{level},{}", rest.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_systems_level_is_read_out_of_a_line() {
        let f = "warn,ac_client=info,ac_net=debug";
        assert_eq!(level_of(f, "ac_client").as_deref(), Some("info"));
        assert_eq!(level_of(f, "ac_net").as_deref(), Some("debug"));
        // Not named: it follows the default.
        assert_eq!(level_of(f, "ac_world"), None);
        assert_eq!(base_of(f), "warn");
        // A line with no default at all still answers.
        assert_eq!(base_of("ac_client=info"), "warn");
    }

    #[test]
    fn setting_a_system_replaces_rather_than_repeats() {
        let f = "warn,ac_client=info";
        let out = with_level(f, "ac_client", Some("debug"));
        assert_eq!(level_of(&out, "ac_client").as_deref(), Some("debug"));
        assert_eq!(out.matches("ac_client").count(), 1, "{out}");
        // Adding one that was not there keeps the rest.
        let more = with_level(&out, "ac_net", Some("trace"));
        assert_eq!(level_of(&more, "ac_client").as_deref(), Some("debug"));
        assert_eq!(level_of(&more, "ac_net").as_deref(), Some("trace"));
    }

    #[test]
    fn a_system_can_go_back_to_the_default() {
        let f = "warn,ac_client=info,ac_net=debug";
        let out = with_level(f, "ac_client", None);
        assert_eq!(level_of(&out, "ac_client"), None);
        assert_eq!(level_of(&out, "ac_net").as_deref(), Some("debug"));
        assert_eq!(base_of(&out), "warn");
        // Stripping everything still leaves something valid.
        let bare = with_level(&with_level("ac_client=info", "ac_client", None), "x", None);
        assert!(!bare.is_empty(), "{bare}");
    }

    #[test]
    fn the_default_level_changes_without_losing_the_overrides() {
        let f = "warn,ac_client=info";
        let out = with_base(f, "debug");
        assert_eq!(base_of(&out), "debug");
        assert_eq!(level_of(&out, "ac_client").as_deref(), Some("info"));
        // And with nothing else in the line.
        assert_eq!(with_base("warn", "trace"), "trace");
    }

    #[test]
    fn whatever_the_buttons_build_is_still_a_line() {
        // A settings box is a place to make a mess; the buttons must
        // never produce an empty or malformed one.
        let mut f = DEFAULT.to_string();
        for (system, _) in SYSTEMS {
            for level in LEVELS {
                f = with_level(&f, system, Some(level));
                assert!(!f.is_empty() && !f.starts_with(','), "{f}");
            }
            f = with_level(&f, system, None);
            assert!(!f.is_empty() && !f.starts_with(','), "{f}");
        }
        for level in LEVELS {
            f = with_base(&f, level);
            assert_eq!(base_of(&f), *level, "{f}");
        }
    }
}
