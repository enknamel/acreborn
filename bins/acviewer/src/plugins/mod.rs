//! The viewer's built-in plugins and how they are registered.

pub use ac_plugin::{console, party};

pub use ac_plugin::{Host, Requests};

/// The built-in plugins. Add yours here (or load them at runtime later).
pub fn builtin() -> Host {
    let mut host = Host::new();
    host.register(Box::new(console::Console::default()));
    host.register(Box::new(party::Party::default()));
    host
}
