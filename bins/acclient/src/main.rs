//! acclient: connect to an ACE server, log in, enter the world, and print
//! every message the server sends. Rendering comes later.
//!
//!   acclient -h 127.0.0.1 -a account -v password [--character NAME] [--duration 30]

use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ac_net::messages::{self, opcode, queue, DatIteration};
use ac_net::session::{Config, Event, Port, Session, State};
use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Server host
    #[arg(short = 'h', long, default_value = "127.0.0.1")]
    host: String,
    /// Server login port (the +1 port is derived)
    #[arg(short = 'p', long, default_value_t = 9000)]
    port: u16,
    #[arg(short = 'a', long)]
    account: String,
    #[arg(short = 'v', long)]
    password: String,
    /// Character name to enter the world with (default: first in the list)
    #[arg(long)]
    character: Option<String>,
    /// Seconds to stay connected after entering the world (0 = forever)
    #[arg(long, default_value_t = 20)]
    duration: u64,
    /// DAT directory, used to report iterations to the server
    #[arg(long, env = "AC_DATA_DIR")]
    data_dir: Option<PathBuf>,
}

fn dat_iterations(dir: Option<&PathBuf>) -> Vec<DatIteration> {
    let mut v = vec![
        DatIteration {
            dat_file_id: 1,
            dat_file_type: 0,
            iterations: 2072,
        },
        DatIteration {
            dat_file_id: 2,
            dat_file_type: 0,
            iterations: 982,
        },
        DatIteration {
            dat_file_id: 3,
            dat_file_type: 0,
            iterations: 994,
        },
    ];
    if let Some(dir) = dir {
        for (i, name) in [(0usize, "client_portal.dat"), (1, "client_cell_1.dat")] {
            if let Ok(dat) = ac_dat::DatArchive::open(dir.join(name)) {
                if let Ok(b) = dat.read(ac_dat::ITERATION_FILE_ID) {
                    if let Some(it) = ac_dat::Iteration::parse(&b) {
                        v[i].iterations = it.total;
                    }
                }
            }
        }
    }
    v
}

fn describe(msg: &[u8]) -> String {
    let Some((op, body)) = messages::split(msg) else {
        return format!("{} bytes (no opcode)", msg.len());
    };
    match op {
        opcode::SERVER_NAME => match messages::ServerName::parse(body) {
            Ok(s) => format!(
                "ServerName {:?} ({}/{} online)",
                s.name, s.current_connections, s.max_connections
            ),
            Err(e) => format!("ServerName (bad: {e})"),
        },
        opcode::CHARACTER_LIST => match messages::CharacterList::parse(body) {
            Ok(cl) => format!(
                "CharacterList account={:?} slots={} chars={:?}",
                cl.account,
                cl.slot_count,
                cl.characters
                    .iter()
                    .map(|c| format!("{:#010x} {}", c.id, c.name))
                    .collect::<Vec<_>>()
            ),
            Err(e) => format!("CharacterList (bad: {e})"),
        },
        opcode::DDD_INTERROGATION => "DDD_Interrogation".into(),
        opcode::DDD_END_DDD => "DDD_EndDDD".into(),
        opcode::DDD_BEGIN_DDD => "DDD_BeginDDD (server wants to patch our DATs)".into(),
        opcode::CHARACTER_ENTER_WORLD_SERVER_READY => "CharacterEnterWorldServerReady".into(),
        opcode::CHARACTER_ERROR => format!(
            "CharacterError {:#x}",
            body.get(..4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .unwrap_or(0)
        ),
        opcode::ACCOUNT_BOOT => "AccountBoot".into(),
        opcode::OBJECT_CREATE => format!(
            "ObjectCreate guid={:#010x} ({} bytes)",
            body.get(..4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .unwrap_or(0),
            body.len()
        ),
        opcode::PLAYER_CREATE => format!(
            "PlayerCreate guid={:#010x}",
            body.get(..4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                .unwrap_or(0)
        ),
        opcode::UPDATE_POSITION => "UpdatePosition".into(),
        opcode::GAME_EVENT => match messages::split_game_event(body) {
            Some((guid, seq, ev, rest)) => format!(
                "GameEvent {ev:#06x} guid={guid:#010x} seq={seq} ({} bytes)",
                rest.len()
            ),
            None => "GameEvent (short)".into(),
        },
        _ => format!("opcode {op:#06x} ({} bytes)", body.len()),
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
        )
        .init();
    let cli = Cli::parse();
    let primary: SocketAddr = format!("{}:{}", cli.host, cli.port).parse().or_else(|_| {
        std::net::ToSocketAddrs::to_socket_addrs(&(cli.host.as_str(), cli.port)).and_then(
            |mut a| {
                a.next()
                    .context("resolve")
                    .map_err(|e| std::io::Error::other(e))
            },
        )
    })?;
    let secondary = SocketAddr::new(primary.ip(), primary.port() + 1);
    let socket = UdpSocket::bind("0.0.0.0:0").context("bind")?;
    socket.set_read_timeout(Some(Duration::from_millis(20)))?;
    tracing::info!("connecting to {primary} from {}", socket.local_addr()?);

    let now = Instant::now();
    let mut session = Session::new(
        Config {
            account: cli.account.clone(),
            password: cli.password.clone(),
            dats: dat_iterations(cli.data_dir.as_ref()),
            echo_interval: Duration::from_secs(5),
            ack_interval: Duration::from_secs(2),
        },
        now,
    );
    session.login(now);

    let mut buf = [0u8; 2048];
    let mut characters: Vec<messages::CharacterEntry> = Vec::new();
    let mut entered_at: Option<Instant> = None;
    let mut enter_requested = false;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let now = Instant::now();
        for (port, dg) in session.outgoing() {
            let to = match port {
                Port::Primary => primary,
                Port::Secondary => secondary,
            };
            socket.send_to(&dg, to)?;
        }
        match socket.recv_from(&mut buf) {
            Ok((n, _from)) => session.receive(&buf[..n], now),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(e) => return Err(e.into()),
        }
        session.poll(now);
        for ev in session.events() {
            match ev {
                Event::Connected { client_id } => {
                    tracing::info!("connected, client id {client_id}")
                }
                Event::Terminated(why) => {
                    tracing::warn!("terminated: {why}");
                    return Ok(());
                }
                Event::Message(msg) => {
                    tracing::info!("<- {}", describe(&msg));
                    let Some((op, body)) = messages::split(&msg) else {
                        continue;
                    };
                    match op {
                        opcode::CHARACTER_LIST => {
                            if let Ok(cl) = messages::CharacterList::parse(body) {
                                characters = cl.characters;
                            }
                        }
                        opcode::DDD_END_DDD => {
                            if !enter_requested {
                                let pick = match &cli.character {
                                    Some(name) => characters
                                        .iter()
                                        .find(|c| c.name.eq_ignore_ascii_case(name)),
                                    None => characters.first(),
                                };
                                match pick {
                                    Some(c) => {
                                        tracing::info!("-> CharacterEnterWorldRequest for {} ({:#010x})", c.name, c.id);
                                        session.send_message(queue::UI, messages::enter_world_request());
                                        enter_requested = true;
                                    }
                                    None => tracing::warn!("no character to enter the world with (create one in the retail client first)"),
                                }
                            }
                        }
                        opcode::CHARACTER_ENTER_WORLD_SERVER_READY => {
                            let pick = match &cli.character {
                                Some(name) => characters
                                    .iter()
                                    .find(|c| c.name.eq_ignore_ascii_case(name)),
                                None => characters.first(),
                            };
                            if let Some(c) = pick {
                                tracing::info!("-> CharacterEnterWorld {}", c.name);
                                session.send_message(
                                    queue::UI,
                                    messages::enter_world(c.id, &cli.account),
                                );
                                entered_at = Some(Instant::now());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if session.state() == State::Terminated {
            return Ok(());
        }
        if let Some(t) = entered_at {
            if cli.duration > 0 && t.elapsed() > Duration::from_secs(cli.duration) {
                tracing::info!("done");
                return Ok(());
            }
        } else if Instant::now() > deadline {
            tracing::warn!("timed out before entering the world");
            return Ok(());
        }
    }
}
