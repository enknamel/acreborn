//! The persistent launcher config, `~/.acreborn/launcher.json`.
//!
//! Passwords are stored in plain text: the file is only as private as the
//! user's home directory.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A game server the launcher can connect to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub name: String,
    pub host: String,
    pub port: u16,
}

impl Server {
    /// The `host:port` form `acviewer --connect` takes.
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// An account on one server. `server` is the [`Server::name`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub server: String,
    pub account: String,
    /// Plain text.
    pub password: String,
    /// Character names seen or typed for this account, most recent last.
    #[serde(default)]
    pub characters: Vec<String>,
    /// The character used on the last launch (and the one the character
    /// field shows).
    #[serde(default)]
    pub last_character: Option<String>,
    /// RFC 3339 UTC timestamp of the last launch.
    #[serde(default)]
    pub last_used: Option<String>,
}

/// Everything the launcher remembers between runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub servers: Vec<Server>,
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// Directory with `client_portal.dat` and `client_cell_1.dat`.
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    /// The client program and any leading arguments, e.g.
    /// `["/path/to/acviewer"]` or `["cargo", "run", "-p", "acviewer", "--"]`.
    /// Empty (the default) means: the `acviewer` next to the launcher
    /// binary if there is one, else `cargo run -p acviewer --`.
    #[serde(default)]
    pub client_binary: Vec<String>,
    /// The plain-text password notice has been shown and dismissed.
    #[serde(default)]
    pub password_notice_dismissed: bool,
    /// Launch every client with `--bus` so plugins in all of them share
    /// posts and blackboard values (see docs/multi-session.md).
    #[serde(default)]
    pub share_bus: bool,
    /// Frame-rate cap passed as `--fps` (0 = the client's default).
    #[serde(default)]
    pub fps: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            servers: vec![Server {
                name: "Local ACE".into(),
                host: "127.0.0.1".into(),
                port: 9000,
            }],
            accounts: Vec::new(),
            data_dir: default_data_dir(),
            client_binary: Vec::new(),
            password_notice_dismissed: false,
            share_bus: false,
            fps: 0,
        }
    }
}

/// The user's home directory, from `$HOME`.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `~/.acreborn`.
pub fn acreborn_dir() -> PathBuf {
    home_dir().join(".acreborn")
}

/// `~/.acreborn/launcher.json`.
pub fn default_path() -> PathBuf {
    acreborn_dir().join("launcher.json")
}

/// `~/.acreborn/logs`.
pub fn logs_dir() -> PathBuf {
    acreborn_dir().join("logs")
}

fn default_data_dir() -> PathBuf {
    home_dir().join("Downloads").join("ac_data")
}

impl Config {
    /// Load `path`, or the defaults when it does not exist yet.
    pub fn load(path: &Path) -> Result<Config> {
        match fs::read(path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
        }
    }

    /// Write `path` (creating its directory), replacing it atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        let json = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json.as_bytes()).with_context(|| format!("write {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
        Ok(())
    }

    pub fn server(&self, name: &str) -> Option<&Server> {
        self.servers.iter().find(|s| s.name == name)
    }

    /// Accounts on the named server, as indices into `accounts`.
    pub fn accounts_on(&self, server: &str) -> Vec<usize> {
        self.accounts
            .iter()
            .enumerate()
            .filter(|(_, a)| a.server == server)
            .map(|(i, _)| i)
            .collect()
    }

    /// Find an account by name, on `server` when given, else on any server
    /// (the first match).
    pub fn find_account(&self, account: &str, server: Option<&str>) -> Option<usize> {
        self.accounts
            .iter()
            .position(|a| a.account == account && server.is_none_or(|s| a.server == s))
    }

    /// Record a launch: `last_used` now, and the character used, if any.
    pub fn record_launch(&mut self, index: usize, character: Option<&str>) {
        let a = &mut self.accounts[index];
        a.last_used = Some(now_rfc3339());
        if let Some(c) = character.map(str::trim).filter(|c| !c.is_empty()) {
            a.characters.retain(|k| k != c);
            a.characters.push(c.to_string());
            a.last_character = Some(c.to_string());
        }
    }
}

/// The current time as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_rfc3339(secs)
}

/// Format seconds since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aclauncher-test-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        dir.join("nested").join("launcher.json")
    }

    #[test]
    fn missing_file_is_default() {
        let path = temp_path("missing");
        let cfg = Config::load(&path).unwrap();
        assert_eq!(cfg, Config::default());
        assert_eq!(cfg.servers.len(), 1);
        assert_eq!(cfg.servers[0].name, "Local ACE");
        assert_eq!(cfg.servers[0].address(), "127.0.0.1:9000");
        assert!(cfg.data_dir.ends_with("Downloads/ac_data"));
    }

    #[test]
    fn save_load_round_trip() {
        let path = temp_path("roundtrip");
        let mut cfg = Config::default();
        cfg.servers.push(Server {
            name: "Other".into(),
            host: "ace.example.org".into(),
            port: 9100,
        });
        cfg.accounts.push(Account {
            server: "Local ACE".into(),
            account: "alice".into(),
            password: "s3cret".into(),
            characters: vec!["Alice One".into(), "Alice Two".into()],
            last_character: Some("Alice Two".into()),
            last_used: Some("2026-09-04T10:00:00Z".into()),
        });
        cfg.client_binary = vec![
            "cargo".into(),
            "run".into(),
            "-p".into(),
            "acviewer".into(),
            "--".into(),
        ];
        cfg.password_notice_dismissed = true;
        cfg.save(&path).unwrap();
        let back = Config::load(&path).unwrap();
        assert_eq!(back, cfg);
        // Saved again, byte-identical.
        let first = fs::read(&path).unwrap();
        back.save(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), first);
        let _ = fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn optional_fields_default() {
        let json = r#"{"servers":[{"name":"S","host":"h","port":1}],
            "accounts":[{"server":"S","account":"a","password":"p"}]}"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.accounts[0].characters.is_empty());
        assert_eq!(cfg.accounts[0].last_character, None);
        assert_eq!(cfg.accounts[0].last_used, None);
        assert!(cfg.client_binary.is_empty());
        assert!(!cfg.password_notice_dismissed);
    }

    #[test]
    fn record_launch_updates_account() {
        let mut cfg = Config::default();
        cfg.accounts.push(Account {
            server: "Local ACE".into(),
            account: "bob".into(),
            password: "pw".into(),
            characters: vec!["Old".into(), "New".into()],
            last_character: None,
            last_used: None,
        });
        cfg.record_launch(0, Some("Old"));
        let a = &cfg.accounts[0];
        assert_eq!(a.characters, vec!["New".to_string(), "Old".to_string()]);
        assert_eq!(a.last_character.as_deref(), Some("Old"));
        let stamp = a.last_used.clone().unwrap();
        assert_eq!(stamp.len(), 20);
        assert!(stamp.ends_with('Z'));
        cfg.record_launch(0, Some("  "));
        assert_eq!(cfg.accounts[0].last_character.as_deref(), Some("Old"));
    }

    #[test]
    fn rfc3339_formatting() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_rfc3339(1_788_000_000 + 3661), "2026-08-29T11:41:01Z");
        assert_eq!(format_rfc3339(4_102_444_800), "2100-01-01T00:00:00Z");
    }

    #[test]
    fn lookups() {
        let mut cfg = Config::default();
        cfg.servers.push(Server {
            name: "B".into(),
            host: "b".into(),
            port: 2,
        });
        for (s, a) in [("Local ACE", "x"), ("B", "x"), ("B", "y")] {
            cfg.accounts.push(Account {
                server: s.into(),
                account: a.into(),
                password: String::new(),
                characters: vec![],
                last_character: None,
                last_used: None,
            });
        }
        assert_eq!(cfg.accounts_on("B"), vec![1, 2]);
        assert_eq!(cfg.find_account("x", None), Some(0));
        assert_eq!(cfg.find_account("x", Some("B")), Some(1));
        assert_eq!(cfg.find_account("z", None), None);
        assert_eq!(cfg.server("B").map(|s| s.port), Some(2));
    }
}
