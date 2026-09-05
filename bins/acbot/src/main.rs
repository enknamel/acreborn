//! acbot: run many game sessions in one process with no window and no GPU.
//!
//! Every `--client` becomes an `ac_client::Client`; the loop ticks each one
//! `--tick-hz` times a second with no keyboard input (plugins and the
//! server's move-to drive movement), prints what the server says, and runs
//! the plugin host once per session per frame. Lines from `--say` and
//! `--script` are typed one per second after the character is placed; those
//! starting with `/` go to the plugin host as commands. Ctrl-C disconnects
//! every session cleanly.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;

use ac_client::{Client, Config, Event};
use ac_plugin::console::Console;
use ac_plugin::Host;

#[derive(Parser, Debug)]
#[command(name = "acbot", about = "Headless runner for many game sessions")]
struct Cli {
    /// Directory with client_portal.dat and client_cell_1.dat.
    #[arg(long, env = "AC_DATA_DIR")]
    data_dir: PathBuf,
    /// Server login address: HOST or HOST:PORT (port 9000 when omitted).
    #[arg(long)]
    connect: String,
    /// A session to run: ACCOUNT:PASSWORD[:CHARACTER]. Repeat for more.
    #[arg(long = "client", required = true)]
    clients: Vec<String>,
    /// Ticks per second for every session.
    #[arg(long, default_value_t = 20)]
    tick_hz: u32,
    /// Run for this many seconds (0 = until Ctrl-C or every session ends).
    #[arg(long, default_value_t = 0)]
    duration: u64,
    /// A line every session types once placed (one per second); lines
    /// starting with `/` are plugin commands. Repeat for more.
    #[arg(long)]
    say: Vec<String>,
    /// A text file of lines typed after the --say lines, one per second,
    /// by every session. Blank lines and `#` comments are skipped.
    #[arg(long)]
    script: Option<PathBuf>,
    /// Print chat lines, prefixed with the account.
    #[arg(long)]
    log_chat: bool,
}

/// One `--client` argument, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientSpec {
    pub account: String,
    pub password: String,
    pub character: Option<String>,
}

/// Parse `ACCOUNT:PASSWORD[:CHARACTER]`. The character may contain colons;
/// the account and password may not.
pub fn parse_client_spec(spec: &str) -> Result<ClientSpec, String> {
    let mut parts = spec.splitn(3, ':');
    let (Some(account), Some(password)) = (parts.next(), parts.next()) else {
        return Err(format!(
            "--client wants ACCOUNT:PASSWORD[:CHARACTER], got {spec:?}"
        ));
    };
    if account.is_empty() {
        return Err(format!("--client {spec:?}: empty account"));
    }
    if password.is_empty() {
        return Err(format!("--client {spec:?}: empty password"));
    }
    let character = parts
        .next()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    Ok(ClientSpec {
        account: account.to_string(),
        password: password.to_string(),
        character,
    })
}

/// The lines of a script file: trimmed, without blanks and `#` comments.
pub fn parse_script(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Lines a session types after it is placed: line `k` goes out once
/// `k + 1` periods have passed (the first a period after placement, so the
/// world has settled), never twice, and in order however late the poll.
#[derive(Debug, Clone)]
pub struct Schedule {
    lines: Vec<String>,
    period: f32,
    sent: usize,
}

impl Schedule {
    pub fn new(lines: Vec<String>, period: f32) -> Self {
        Schedule {
            lines,
            period,
            sent: 0,
        }
    }

    /// How many lines are due `since_placed` seconds after placement.
    pub fn due_by(&self, since_placed: f32) -> usize {
        if since_placed < self.period || self.period <= 0.0 {
            return 0;
        }
        let n = (since_placed / self.period).floor() as usize;
        n.min(self.lines.len())
    }

    /// The lines that became due since the last poll, in order.
    pub fn poll(&mut self, since_placed: f32) -> Vec<String> {
        let due = self.due_by(since_placed);
        let out = self.lines[self.sent..due.max(self.sent)].to_vec();
        self.sent = self.sent.max(due);
        out
    }

    pub fn finished(&self) -> bool {
        self.sent >= self.lines.len()
    }
}

/// One headless session and what the loop remembers about it.
struct Session {
    client: Client,
    schedule: Schedule,
    placed_at: Option<Instant>,
    /// Terminated or refused: the connection is gone.
    ended: bool,
}

impl Session {
    fn account(&self) -> &str {
        &self.client.config.account
    }

    /// One line: placed?, cell, health, target.
    fn status(&self) -> String {
        let c = &self.client;
        let placed = if c.placed() { "yes" } else { "no" };
        let cell = match c.player.as_ref().map(|p| p.cell) {
            Some(cell) => format!("{cell:08X}"),
            None => "-".to_string(),
        };
        let st = &c.world.stats;
        let health = if st.name.is_empty() {
            "-".to_string()
        } else {
            format!("{}/{}", st.vitals[0].current, st.vital_max(0))
        };
        let target = c
            .attack_target
            .or(c.selected)
            .and_then(|g| c.world.objects.get(&g))
            .map(|o| o.name.clone())
            .unwrap_or_else(|| "-".to_string());
        let state = if self.ended { " ended" } else { "" };
        format!(
            "[{}] placed={placed} cell={cell} hp={health} target={target}{state}",
            self.account()
        )
    }
}

fn clients_of(sessions: &mut [Session]) -> Vec<&mut Client> {
    sessions.iter_mut().map(|s| &mut s.client).collect()
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    let cli = Cli::parse();
    anyhow::ensure!(cli.tick_hz > 0, "--tick-hz must be at least 1");
    let specs: Vec<ClientSpec> = cli
        .clients
        .iter()
        .map(|s| parse_client_spec(s).map_err(anyhow::Error::msg))
        .collect::<Result<_>>()?;
    let mut lines = cli.say.clone();
    if let Some(path) = &cli.script {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading script {}", path.display()))?;
        lines.extend(parse_script(&text));
    }

    let assets = Rc::new(ac_scene::Assets::open(&cli.data_dir).context("opening DAT archives")?);
    let mut sessions: Vec<Session> = Vec::with_capacity(specs.len());
    for spec in specs {
        let client = Client::connect(
            Config {
                host: cli.connect.clone(),
                account: spec.account.clone(),
                password: spec.password,
                character: spec.character,
            },
            assets.clone(),
        )
        .with_context(|| format!("connecting {} to {}", spec.account, cli.connect))?;
        sessions.push(Session {
            client,
            schedule: Schedule::new(lines.clone(), 1.0),
            placed_at: None,
            ended: false,
        });
    }

    let mut host = Host::new();
    host.register(Box::new(Console));
    host.register(Box::new(ac_plugin::party::Party::default()));
    let period = Duration::from_secs_f64(1.0 / cli.tick_hz as f64);
    println!(
        "acbot: {} session(s) to {}, {} Hz ({} ms per tick), {} scripted line(s), {}",
        sessions.len(),
        cli.connect,
        cli.tick_hz,
        period.as_millis(),
        lines.len(),
        if cli.duration == 0 {
            "running until Ctrl-C".to_string()
        } else {
            format!("running for {} s", cli.duration)
        }
    );

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst))
            .context("installing the Ctrl-C handler")?;
    }

    let start = Instant::now();
    let mut last = start;
    let mut next_tick = start;
    let mut next_status = start + Duration::from_secs(10);
    loop {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.25);
        last = now;

        for i in 0..sessions.len() {
            if sessions[i].ended {
                continue;
            }
            let _frame = sessions[i].client.tick(None, dt, now);
            let events = sessions[i].client.drain_events();
            for ev in &events {
                let account = sessions[i].account().to_string();
                match ev {
                    Event::Chat { text, .. } => {
                        if cli.log_chat {
                            println!("[{account}] {text}");
                        } else {
                            tracing::debug!("[{account}] chat: {text}");
                        }
                    }
                    Event::Connected => println!("[{account}] connected"),
                    Event::Placed { cell } => {
                        println!("[{account}] placed in cell {cell:08X}");
                        sessions[i].placed_at.get_or_insert(now);
                    }
                    Event::Terminated(reason) => {
                        println!("[{account}] terminated: {reason}");
                        sessions[i].ended = true;
                    }
                    Event::Refused(op) => {
                        println!("[{account}] refused (opcode {op:#06X})");
                        sessions[i].ended = true;
                    }
                    Event::Sound { .. } => {}
                }
            }
            let r = host.frame(clients_of(&mut sessions), i, &events, dt, now);
            for (text, _) in r.chat {
                println!("[{}] {text}", sessions[i].account());
            }

            let due = match sessions[i].placed_at {
                Some(t) if !sessions[i].ended => sessions[i].schedule.poll((now - t).as_secs_f32()),
                _ => Vec::new(),
            };
            for line in due {
                let account = sessions[i].account().to_string();
                if line.starts_with('/') {
                    println!("[{account}] {line}");
                    let r = host.command(clients_of(&mut sessions), i, &line);
                    for (text, _) in r.chat {
                        println!("[{account}] {text}");
                    }
                } else {
                    println!("[{account}] > {line}");
                    sessions[i].client.say(&line);
                }
            }
        }
        host.end_frame();

        if now >= next_status {
            for s in &sessions {
                println!("{}", s.status());
            }
            next_status += Duration::from_secs(10);
        }

        if stop.load(Ordering::SeqCst) {
            println!("acbot: interrupted, disconnecting");
            break;
        }
        if cli.duration > 0 && now - start >= Duration::from_secs(cli.duration) {
            println!("acbot: {} s elapsed, disconnecting", cli.duration);
            break;
        }
        if sessions.iter().all(|s| s.ended) {
            println!("acbot: every session ended");
            break;
        }

        next_tick += period;
        let after = Instant::now();
        if next_tick > after {
            std::thread::sleep(next_tick - after);
        } else {
            // Fell behind: don't try to catch up with a burst of ticks.
            next_tick = after;
        }
    }

    let now = Instant::now();
    for s in &mut sessions {
        if !s.ended {
            s.client.disconnect(now);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_specs_parse() {
        assert_eq!(
            parse_client_spec("bob:secret"),
            Ok(ClientSpec {
                account: "bob".into(),
                password: "secret".into(),
                character: None,
            })
        );
        assert_eq!(
            parse_client_spec("bob:secret:Reborn"),
            Ok(ClientSpec {
                account: "bob".into(),
                password: "secret".into(),
                character: Some("Reborn".into()),
            })
        );
        // The character keeps any further colons; blanks mean none.
        assert_eq!(
            parse_client_spec("bob:secret:A:B").unwrap().character,
            Some("A:B".into())
        );
        assert_eq!(parse_client_spec("bob:secret:").unwrap().character, None);
        assert!(parse_client_spec("bob").is_err());
        assert!(parse_client_spec(":secret").is_err());
        assert!(parse_client_spec("bob:").is_err());
        assert!(parse_client_spec("").is_err());
    }

    #[test]
    fn scripts_skip_blanks_and_comments() {
        let lines =
            parse_script("  /combat \n\n# a comment\nhello there\n   \n@telepoi holtburg\n");
        assert_eq!(lines, vec!["/combat", "hello there", "@telepoi holtburg"]);
        assert!(parse_script("").is_empty());
    }

    fn lines(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("line {i}")).collect()
    }

    #[test]
    fn schedule_is_one_line_per_period() {
        let mut s = Schedule::new(lines(3), 1.0);
        assert!(s.poll(0.0).is_empty());
        assert!(s.poll(0.99).is_empty());
        assert_eq!(s.poll(1.0), vec!["line 0"]);
        assert!(s.poll(1.5).is_empty(), "nothing twice");
        assert_eq!(s.poll(2.0), vec!["line 1"]);
        assert!(!s.finished());
        assert_eq!(s.poll(3.0), vec!["line 2"]);
        assert!(s.finished());
        assert!(s.poll(100.0).is_empty(), "nothing past the end");
    }

    #[test]
    fn schedule_catches_up_in_order_after_a_late_poll() {
        let mut s = Schedule::new(lines(4), 1.0);
        assert_eq!(s.poll(2.5), vec!["line 0", "line 1"]);
        assert_eq!(s.poll(10.0), vec!["line 2", "line 3"]);
        assert!(s.finished());
    }

    #[test]
    fn schedule_period_scales_and_empty_is_finished() {
        let mut s = Schedule::new(lines(2), 0.5);
        assert_eq!(s.due_by(0.4), 0);
        assert_eq!(s.due_by(0.5), 1);
        assert_eq!(s.due_by(1.0), 2);
        assert_eq!(s.due_by(9.0), 2);
        assert_eq!(s.poll(1.0), vec!["line 0", "line 1"]);
        assert!(Schedule::new(Vec::new(), 1.0).finished());
        assert_eq!(Schedule::new(lines(2), 0.0).due_by(5.0), 0);
    }
}
