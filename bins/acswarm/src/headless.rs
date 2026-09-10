//! Headless mode: run many game sessions in one process with no window
//! and no GPU. What used to be a separate headless mode binary.
//!
//! Every `--client` becomes an `ac_client::Client`; the loop ticks each one
//! `--hz` (`--tick-hz`) times a second with no keyboard input (plugins and the
//! server's move-to drive movement), prints what the server says, and runs
//! the plugin host once per session per frame. Lines from `--say` and
//! `--script` are typed one per second after the character is placed; those
//! starting with `/` go to the plugin host as commands. Ctrl-C disconnects
//! every session cleanly.

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use ac_client::creation::{self, CreateSpec};
use ac_client::{Client, Config, Event};
use ac_plugin::console::Console;
use ac_plugin::Host;

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

    /// Whether every line has gone out. The loop stops on the duration
    /// or on every session ending, so only the tests ask this.
    #[cfg(test)]
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

/// Log `account` in: enter with `character` (else the account's first,
/// or the `create` name), and with `create` make the character when
/// the account lacks it (`Client::create_when_missing`).
fn connect_session(
    connect: &str,
    assets: &Rc<ac_scene::Assets>,
    account: &str,
    password: &str,
    character: Option<&str>,
    create: Option<&CreateSpec>,
) -> Result<Client> {
    let mut client = Client::connect(
        Config {
            host: connect.to_string(),
            account: account.to_string(),
            password: password.to_string(),
            character: character
                .map(str::to_string)
                .or_else(|| create.map(|c| c.name.clone())),
            auto_enter: true,
        },
        assets.clone(),
    )
    .with_context(|| format!("connecting {account} to {connect}"))?;
    if let Some(c) = create {
        client.create_when_missing(c.clone());
        // A named character is still the one to enter with; the create
        // only fills in for it when it is missing.
        if let Some(name) = character {
            client.config.character = Some(name.to_string());
        }
    }
    Ok(client)
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
            format!("{}/{}", st.vitals[0].current, st.vital_max_current(0))
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

pub fn run(cli: crate::Cli) -> Result<()> {
    // The app set up logging before it worked out which mode to run in.
    anyhow::ensure!(cli.tick_hz > 0, "--tick-hz must be at least 1");
    if cli.show_rules {
        let assets = ac_scene::Assets::open(cli.data_dir.as_ref().expect("data dir"))
            .context("opening DAT archives")?;
        let cg = assets.chargen().context("reading the CharGen table")?;
        let heritage = match &cli.heritage {
            Some(h) => ac_scene::chargen::heritage_id(&cg, h)
                .with_context(|| format!("unknown heritage {h:?}"))?,
            None => creation::HERITAGE_ALUVIAN,
        };
        let rules = creation::rules(&assets, heritage).map_err(|e| anyhow::anyhow!("{e}"))?;
        print!("{}", rules.summary());
        return Ok(());
    }
    let connect = cli.connect.clone().context("--connect is required")?;
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

    let assets = Rc::new(
        ac_scene::Assets::open(cli.data_dir.as_ref().expect("data dir"))
            .context("opening DAT archives")?,
    );
    // What --create makes on a session whose account lacks the character.
    let create: Option<CreateSpec> = cli.create.as_ref().map(|name| CreateSpec {
        name: name.clone(),
        heritage: cli.heritage.clone(),
        sex: cli.gender.clone(),
        template: cli.template.clone(),
        town: cli.start_area.clone(),
    });
    let mut sessions: Vec<Session> = Vec::with_capacity(specs.len());
    for spec in specs {
        let client = connect_session(
            &connect,
            &assets,
            &spec.account,
            &spec.password,
            spec.character.as_deref(),
            create.as_ref(),
        )?;
        sessions.push(Session {
            client,
            schedule: Schedule::new(lines.clone(), 1.0),
            placed_at: None,
            ended: false,
        });
    }

    let mut host = Host::new();
    if let Some(bus) = &cli.bus {
        let name = sessions.first().map(|s| s.client.config.account.clone());
        host.join_bus(
            Some(bus),
            &name.unwrap_or_else(|| format!("pid{}", std::process::id())),
        )
        .context("joining the bus")?;
    }
    // The panels draw nothing here but their commands (/bar ...) work.
    for p in ac_plugin::panels::live() {
        host.register(p);
    }
    host.register(Box::new(Console));
    host.register(Box::new(ac_plugin::party::Party::default()));
    host.register(Box::new(ac_plugin::team::Team::default()));
    host.register(Box::new(ac_script::ScriptPlugin::new(
        ac_script::default_dir(),
    )));
    let period = Duration::from_secs_f64(1.0 / cli.tick_hz as f64);
    println!(
        "headless: {} session(s) to {}, {} Hz ({} ms per tick), {} scripted line(s), {}",
        sessions.len(),
        connect,
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
                    Event::Characters(list) => {
                        let names: Vec<&str> = list.iter().map(|c| c.name.as_str()).collect();
                        println!("[{account}] characters: {names:?}");
                        // The client creates a missing --create character
                        // itself (`create_when_missing`); the list comes
                        // here for the record.
                        let c = &sessions[i].client;
                        if let Some(name) = c.creating() {
                            println!(
                                "[{account}] creating {name}{}",
                                create
                                    .as_ref()
                                    .map(|c| {
                                        let s = c.summary();
                                        if s.is_empty() {
                                            s
                                        } else {
                                            format!(" ({s})")
                                        }
                                    })
                                    .unwrap_or_default()
                            );
                        } else if let Some(why) = c.create_error() {
                            println!("[{account}] cannot create: {why}");
                            sessions[i].ended = true;
                        } else if c.entering.is_none() {
                            println!("[{account}] no character to enter the world with (use --create NAME)");
                            sessions[i].ended = true;
                        }
                    }
                    Event::CharacterCreated { id, name } => {
                        println!("[{account}] created {name} ({id:#010x}); entering the world");
                    }
                    Event::CharacterCreateFailed(code) => {
                        println!(
                            "[{account}] character creation failed: {} (code {code})",
                            creation::create_failure_message(*code)
                        );
                        sessions[i].ended = true;
                    }
                    Event::Sound { .. }
                    | Event::Effect { .. }
                    | Event::SpellLearned(_)
                    | Event::SpellForgotten(_)
                    | Event::Autoplay { .. } => {}
                }
            }
            let r = host.frame(clients_of(&mut sessions), i, &events, dt, now);
            for (text, _) in r.chat {
                println!("[{}] {text}", sessions[i].account());
            }
            // A plugin (the fleet panel's roster, a script through
            // `fleet.start`) asked for sessions: start them here too.
            // Stopping is not supported headless; every session runs
            // until the end.
            for spec in r.start_sessions {
                if sessions
                    .iter()
                    .any(|s| s.client.config.account.eq_ignore_ascii_case(&spec.account))
                {
                    println!("[{}] already running", spec.account);
                    continue;
                }
                match connect_session(
                    &connect,
                    &assets,
                    &spec.account,
                    &spec.password,
                    spec.character.as_deref(),
                    spec.create.as_ref(),
                ) {
                    Ok(client) => {
                        println!("[{}] session {} started", spec.account, sessions.len() + 1);
                        sessions.push(Session {
                            client,
                            schedule: Schedule::new(lines.clone(), 1.0),
                            placed_at: None,
                            ended: false,
                        });
                    }
                    Err(e) => {
                        println!("[{}] cannot start: {e:#}", spec.account);
                        host.board.set_local(
                            ac_plugin::panels::fleet::error_key(&spec.account),
                            e.to_string(),
                        );
                    }
                }
            }
            if !r.stop_sessions.is_empty() {
                tracing::warn!(
                    "headless: a plugin asked to stop sessions {:?}; not supported headless (Ctrl-C ends the run)",
                    r.stop_sessions
                );
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
                    if !r.consumed {
                        sessions[i].client.slash_command(&line);
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
            println!("headless: interrupted, disconnecting");
            break;
        }
        if cli.duration > 0 && now - start >= Duration::from_secs(cli.duration) {
            println!("headless: {} s elapsed, disconnecting", cli.duration);
            break;
        }
        if sessions.iter().all(|s| s.ended) {
            println!("headless: every session ended");
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
