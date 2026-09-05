//! The viewer's built-in plugins and how they are registered.

pub use ac_plugin::{console, party};

pub use ac_plugin::{Host, Requests};

/// The built-in plugins. Add yours here (or load them at runtime later).
pub fn builtin() -> Host {
    let mut host = Host::new();
    host.register(Box::new(console::Console::default()));
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
