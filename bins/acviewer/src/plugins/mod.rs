//! The viewer's built-in plugins and how they are registered.

pub use ac_plugin::{console, panels, party};

pub use ac_plugin::{Host, Requests};

/// The built-in plugins. Add yours here (or load them at runtime later).
/// The panels go first so they draw under, and take their keys (I, K, P)
/// before, anything registered after them.
pub fn builtin() -> Host {
    let mut host = Host::new();
    for p in panels::live() {
        host.register(p);
    }
    host.register(Box::new(console::Console));
    host.register(Box::new(party::Party::default()));
    host.register(Box::new(ac_script::ScriptPlugin::new(
        ac_script::default_dir(),
    )));
    host
}

/// `--bus ADDR`: join the local cross-process bus (or host it) as the
/// first account, or `pidN` when the viewer runs without one.
pub fn join_bus(host: &mut Host, addr: &str, account: Option<&str>) -> std::io::Result<()> {
    let name = account
        .map(str::to_string)
        .unwrap_or_else(|| format!("pid{}", std::process::id()));
    host.join_bus(Some(addr), &name)
}

/// Just the panels, filled with sample data, for `--demo-ui`: a screenshot
/// of the overlay with no server. Real icons and spells come from the
/// archives in `data_dir` when they open.
pub fn demo(data_dir: &std::path::Path) -> Host {
    let assets = match ac_scene::Assets::open(data_dir) {
        Ok(a) => Some(a),
        Err(e) => {
            tracing::warn!("demo spellbook: {e}");
            None
        }
    };
    let mut host = Host::new();
    for p in panels::demo(assets.as_ref()) {
        host.register(p);
    }
    host
}
