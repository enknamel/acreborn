//! Building the client command line, spawning it, and tracking children.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Instant;

use anyhow::{Context, Result};

use crate::config::{Account, Config, Server};

/// A resolved command: what to run, with what, from where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    /// Working directory, when it matters (the `cargo run` fallback).
    pub cwd: Option<PathBuf>,
}

impl Launch {
    /// The command as one shell-ish line, for logs and `--dry-run`.
    /// Arguments with spaces are quoted; the password is shown as typed.
    pub fn display(&self) -> String {
        std::iter::once(&self.program)
            .chain(self.args.iter())
            .map(|a| {
                if a.is_empty() || a.chars().any(char::is_whitespace) {
                    format!("{a:?}")
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Options for one launch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub character: Option<String>,
    /// Headless: adds `--mute` (and `--headless` once acviewer has it).
    pub headless: bool,
    /// Join the local cross-process bus (`--bus`).
    pub bus: bool,
    /// Frame-rate cap (`--fps N`); 0 leaves the client's default.
    pub fps: u32,
}

/// The client program and leading args: the configured one, or the
/// default described on [`Config::client_binary`].
pub fn client_binary(config: &Config) -> Vec<String> {
    if !config.client_binary.is_empty() {
        return config.client_binary.clone();
    }
    default_client_binary(std::env::current_exe().ok().as_deref())
}

/// The default client: `acviewer` next to `launcher_exe` if it exists,
/// else `cargo run -p acviewer --`.
pub fn default_client_binary(launcher_exe: Option<&Path>) -> Vec<String> {
    if let Some(dir) = launcher_exe.and_then(Path::parent) {
        let name = if cfg!(windows) {
            "acviewer.exe"
        } else {
            "acviewer"
        };
        let sibling = dir.join(name);
        if sibling.is_file() {
            return vec![sibling.to_string_lossy().into_owned()];
        }
    }
    ["cargo", "run", "-p", "acviewer", "--"]
        .into_iter()
        .map(String::from)
        .collect()
}

/// The workspace root, for the `cargo run` fallback.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Build the command for `account` on `server` with `binary` (from
/// [`client_binary`]).
pub fn build_launch(
    binary: &[String],
    data_dir: &Path,
    server: &Server,
    account: &Account,
    opts: &Options,
) -> Launch {
    let (program, lead) = match binary.split_first() {
        Some((p, rest)) => (p.clone(), rest.to_vec()),
        None => ("acviewer".to_string(), Vec::new()),
    };
    let mut args = lead;
    args.push("--data-dir".into());
    args.push(data_dir.to_string_lossy().into_owned());
    args.push("--connect".into());
    args.push(server.address());
    args.push("-a".into());
    args.push(account.account.clone());
    args.push("-v".into());
    args.push(account.password.clone());
    if let Some(c) = opts
        .character
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        args.push("--character".into());
        args.push(c.to_string());
    }
    if opts.headless {
        args.push("--mute".into());
    }
    if opts.bus {
        args.push("--bus".into());
    }
    if opts.fps > 0 {
        args.push("--fps".into());
        args.push(opts.fps.to_string());
    }
    let cwd = (program == "cargo").then(workspace_root);
    Launch { program, args, cwd }
}

/// A spawned client.
pub struct Process {
    pub pid: u32,
    pub account: String,
    pub server: String,
    pub started: Instant,
    pub log: PathBuf,
    child: Option<Child>,
    pub status: Option<ExitStatus>,
}

impl Process {
    pub fn running(&self) -> bool {
        self.child.is_some()
    }

    /// Poll the child; returns true if it just ended.
    pub fn poll(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else {
            return false;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.status = Some(status);
                self.child = None;
                true
            }
            Ok(None) => false,
            Err(e) => {
                tracing::warn!("wait pid {}: {e}", self.pid);
                self.child = None;
                true
            }
        }
    }

    pub fn kill(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
        }
    }

    pub fn status_text(&self) -> String {
        match (&self.child, self.status) {
            (Some(_), _) => format!("running {}s", self.started.elapsed().as_secs()),
            (None, Some(s)) => match s.code() {
                Some(code) => format!("exited {code}"),
                None => format!("{s}"),
            },
            (None, None) => "lost".into(),
        }
    }
}

/// Spawn `launch`, with stdout and stderr appended to `<logs>/<account>.log`.
pub fn spawn(launch: &Launch, logs: &Path, account: &Account) -> Result<Process> {
    fs::create_dir_all(logs).with_context(|| format!("create {}", logs.display()))?;
    let log = logs.join(format!("{}.log", safe_name(&account.account)));
    let out = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .with_context(|| format!("open {}", log.display()))?;
    let err: File = out.try_clone()?;
    let mut cmd = Command::new(&launch.program);
    cmd.args(&launch.args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    if let Some(cwd) = &launch.cwd {
        cmd.current_dir(cwd);
    }
    let child = cmd
        .spawn()
        .with_context(|| format!("spawn {}", launch.program))?;
    Ok(Process {
        pid: child.id(),
        account: account.account.clone(),
        server: account.server.clone(),
        started: Instant::now(),
        log,
        child: Some(child),
        status: None,
    })
}

/// An account name reduced to characters safe in a file name.
fn safe_name(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        "account".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> (Server, Account) {
        (
            Server {
                name: "Local ACE".into(),
                host: "127.0.0.1".into(),
                port: 9000,
            },
            Account {
                server: "Local ACE".into(),
                account: "alice".into(),
                password: "pa ss".into(),
                characters: vec![],
                last_character: None,
                last_used: None,
            },
        )
    }

    #[test]
    fn builds_basic_command() {
        let (server, account) = fixtures();
        let bin = vec!["/opt/acviewer".to_string()];
        let l = build_launch(
            &bin,
            Path::new("/data"),
            &server,
            &account,
            &Options::default(),
        );
        assert_eq!(l.program, "/opt/acviewer");
        assert_eq!(
            l.args,
            vec![
                "--data-dir",
                "/data",
                "--connect",
                "127.0.0.1:9000",
                "-a",
                "alice",
                "-v",
                "pa ss"
            ]
        );
        assert_eq!(l.cwd, None);
        assert_eq!(
            l.display(),
            "/opt/acviewer --data-dir /data --connect 127.0.0.1:9000 -a alice -v \"pa ss\""
        );
    }

    #[test]
    fn character_and_headless() {
        let (server, account) = fixtures();
        let bin = vec!["acviewer".to_string()];
        let opts = Options {
            character: Some(" Alice One ".into()),
            headless: true,
            bus: false,
            fps: 0,
        };
        let l = build_launch(&bin, Path::new("/data"), &server, &account, &opts);
        let tail = &l.args[l.args.len() - 3..];
        assert_eq!(tail, ["--character", "Alice One", "--mute"]);
        // A blank character is not passed.
        let opts = Options {
            character: Some("   ".into()),
            headless: false,
            bus: true,
            fps: 30,
        };
        let l = build_launch(&bin, Path::new("/data"), &server, &account, &opts);
        assert!(!l.args.iter().any(|a| a == "--character"));
        assert!(!l.args.iter().any(|a| a == "--mute"));
        let tail = &l.args[l.args.len() - 3..];
        assert_eq!(tail, ["--bus", "--fps", "30"]);
    }

    #[test]
    fn cargo_fallback_keeps_leading_args_and_sets_cwd() {
        let (server, account) = fixtures();
        let bin = default_client_binary(Some(Path::new("/nonexistent/dir/aclauncher")));
        assert_eq!(bin, ["cargo", "run", "-p", "acviewer", "--"]);
        let l = build_launch(
            &bin,
            Path::new("/data"),
            &server,
            &account,
            &Options::default(),
        );
        assert_eq!(l.program, "cargo");
        assert_eq!(&l.args[..5], ["run", "-p", "acviewer", "--", "--data-dir"]);
        let cwd = l.cwd.expect("cwd for cargo");
        assert!(cwd.join("Cargo.toml").is_file(), "{}", cwd.display());
        assert!(cwd.join("bins").join("aclauncher").is_dir());
    }

    #[test]
    fn sibling_acviewer_is_preferred() {
        let dir = std::env::temp_dir().join(format!("aclauncher-bin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let name = if cfg!(windows) {
            "acviewer.exe"
        } else {
            "acviewer"
        };
        fs::write(dir.join(name), b"").unwrap();
        let bin = default_client_binary(Some(&dir.join("aclauncher")));
        assert_eq!(bin.len(), 1);
        assert!(bin[0].ends_with(name));
        assert_eq!(default_client_binary(None)[0], "cargo");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn configured_binary_wins() {
        let mut cfg = Config::default();
        cfg.client_binary = vec!["/x/acviewer".into(), "--verbose".into()];
        assert_eq!(client_binary(&cfg), cfg.client_binary);
        let (server, account) = fixtures();
        let l = build_launch(
            &client_binary(&cfg),
            Path::new("/d"),
            &server,
            &account,
            &Options::default(),
        );
        assert_eq!(l.args[0], "--verbose");
        assert_eq!(l.args[1], "--data-dir");
    }

    #[test]
    fn safe_names() {
        assert_eq!(safe_name("alice"), "alice");
        assert_eq!(safe_name("a b/c"), "a_b_c");
        assert_eq!(safe_name(""), "account");
    }
}
