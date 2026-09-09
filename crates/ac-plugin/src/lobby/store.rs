//! Where the player's servers and remembered logins live on disk: one
//! JSON file in the config directory, read and written by both the
//! connect screen ([`super::connect`]) and the fleet panel
//! ([`crate::panels::fleet`]) so the two offer the same accounts.
//!
//! Two screens change the same file, so a change is made as a
//! read-modify-write ([`update_at`]): whatever the other one wrote in
//! the meantime is kept. [`changed_at`] tells a screen that is already
//! open that the file moved under it.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::servers::Servers;
use crate::Settings;

/// The file's name inside [`Settings::config_dir`].
pub const FILE_NAME: &str = "servers.json";

/// Where the servers and remembered logins are kept. Under `cargo test`
/// it is a scratch file of the test process's own: a test must never
/// read or write the player's real store.
pub fn path() -> PathBuf {
    if cfg!(test) {
        return std::env::temp_dir()
            .join(format!("acswarm-test-store-{}", std::process::id()))
            .join(FILE_NAME);
    }
    Settings::config_dir().join(FILE_NAME)
}

/// Read them from `path`; a missing or unreadable file is an empty store.
pub fn load_at(path: &Path) -> Servers {
    Servers::load(&Settings::load(path))
}

/// Read them from the usual place.
pub fn load() -> Servers {
    load_at(&path())
}

/// Write them to `path`, leaving anything else in the file alone.
pub fn save_at(path: &Path, servers: &Servers) -> std::io::Result<()> {
    let mut settings = Settings::load(path);
    servers.save(&mut settings);
    settings.save(path)
}

/// Read `path`, let `edit` change what is there, write it back, and hand
/// back the result: a change made here does not lose one another screen
/// made since. A failed write is logged and the edited value returned,
/// so the caller still shows what was asked for.
pub fn update_at(path: &Path, edit: impl FnOnce(&mut Servers)) -> Servers {
    let mut s = load_at(path);
    edit(&mut s);
    if let Err(e) = save_at(path, &s) {
        tracing::warn!("could not save the server list to {}: {e}", path.display());
    }
    s
}

/// When the file was last written, or `None` when there is none: an
/// open screen re-reads once this changes.
pub fn changed_at(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::servers::Server;

    /// A scratch file of this test's own.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("acswarm-store-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(FILE_NAME)
    }

    #[test]
    fn an_edit_keeps_what_another_screen_wrote() {
        let path = scratch("edit");
        assert_eq!(load_at(&path).logins.len(), 0, "no file yet: empty");
        assert_eq!(changed_at(&path), None);
        // One screen remembers an account.
        update_at(&path, |s| s.remember("h:9000", "alice", "pw", ""));
        let seen = changed_at(&path);
        assert!(seen.is_some(), "the file is there now");
        // Another, holding a stale copy, adds a server and its own
        // account: alice is still there afterwards.
        let stale = Servers::default();
        assert!(stale.logins.is_empty());
        let after = update_at(&path, |s| {
            s.add(Server {
                name: "Home".into(),
                host: "h".into(),
                port: 9000,
            });
            s.remember("h:9000", "bob", "", "");
        });
        assert_eq!(after.accounts_for("h:9000").len(), 2);
        assert_eq!(load_at(&path).accounts_for("h:9000").len(), 2);
        assert_eq!(after.custom.len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
