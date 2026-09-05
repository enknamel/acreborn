//! acclient: connect to an ACE server, log in, enter the world, and print
//! every message the server sends. Rendering comes later.
//!
//!   acclient -h 127.0.0.1 -a account -v password [--character NAME] [--duration 30]
//!   acclient ... --create NAME [--heritage sho --gender f --template bow --start-area yaraq]
//!   acclient --show-rules [--heritage NAME] --data-dir DIR

use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ac_client::creation::{self, CharacterBuild};
use ac_net::messages::{self, opcode, queue, DatIteration};
use ac_net::session::{Config, Event, Port, Session, State};
use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser)]
#[command(version, about, disable_help_flag = true)]
struct Cli {
    /// Print help
    #[arg(long, action = clap::ArgAction::Help)]
    help: Option<bool>,
    /// Server host
    #[arg(short = 'h', long, default_value = "127.0.0.1")]
    host: String,
    /// Server login port (the +1 port is derived)
    #[arg(short = 'p', long, default_value_t = 9000)]
    port: u16,
    #[arg(
        short = 'a',
        long,
        required_unless_present = "show_rules",
        default_value = ""
    )]
    account: String,
    #[arg(
        short = 'v',
        long,
        required_unless_present = "show_rules",
        default_value = ""
    )]
    password: String,
    /// Character name to enter the world with (default: first in the list)
    #[arg(long)]
    character: Option<String>,
    /// Create a character with this name when the account has none of
    /// that name (see --heritage, --gender, --template, --start-area),
    /// then enter the world with it. Needs --data-dir for the CharGen table.
    #[arg(long)]
    create: Option<String>,
    /// Heritage for --create: a name (aluvian, gharu, sho, viamontian,
    /// ...) or id 1..=13. Default Aluvian.
    #[arg(long)]
    heritage: Option<String>,
    /// Sex for --create: m or f. Default m.
    #[arg(long)]
    gender: Option<String>,
    /// Template for --create: a name (adventurer, bow, swash, life, war,
    /// wayfarer, soldier) or index. Default the first (Adventurer).
    #[arg(long)]
    template: Option<String>,
    /// Starting town for --create: holtburg, shoushi, yaraq or sanamar.
    /// Default the heritage's home town.
    #[arg(long)]
    start_area: Option<String>,
    /// Print the creation rules for --heritage (credits, skill costs,
    /// templates, towns) and exit without connecting.
    #[arg(long)]
    show_rules: bool,
    /// Send this chat line 2 s after entering the world (e.g. "@telepoi holtburg")
    #[arg(long)]
    say: Option<String>,
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
        // The language DAT (id 3) is deliberately not reported: ACE
        // dereferences its own copy when a client lists one, and this
        // server has no client_local_English.dat.
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

/// Build a valid CharacterCreate through `ac_client::creation`: the
/// --heritage / --gender / --template / --start-area choices (Aluvian,
/// male, the first template and the home town when absent), default
/// appearance, validated against the heritage's rules.
fn build_character(
    assets: &ac_scene::Assets,
    cli: &Cli,
    name: &str,
    slot: u32,
) -> Result<messages::CharacterCreate> {
    let (build, rules) = CharacterBuild::from_options(
        assets,
        name,
        cli.heritage.as_deref(),
        cli.gender.as_deref(),
        cli.template.as_deref(),
        cli.start_area.as_deref(),
    )
    .map_err(anyhow::Error::msg)?;
    build
        .validate(&rules)
        .map_err(|e| anyhow::anyhow!("{name:?} is not a valid character: {e}"))?;
    tracing::info!(
        "creating {name}: {} {} template {:?} (str {} end {} coo {} qui {} foc {} self {}), {} of {} credits, start area {}",
        rules.heritage_name,
        if build.look.gender == 2 { "female" } else { "male" },
        rules.templates.get(build.template).map(String::as_str).unwrap_or("?"),
        build.attributes[0],
        build.attributes[1],
        build.attributes[2],
        build.attributes[3],
        build.attributes[4],
        build.attributes[5],
        build.credits_used(&rules),
        rules.skill_credits,
        build.start_area
    );
    Ok(build.to_message(&cli.account, slot))
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
        opcode::SERVER_MESSAGE => {
            let mut r = ac_net::wire::Reader::new(body);
            format!("ServerMessage {:?}", r.string16().unwrap_or_default())
        }
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
        opcode::MOVEMENT_EVENT => match ac_world::MovementEvent::parse(body) {
            Ok(ev) => format!(
                "MovementEvent guid={:#010x} type={} forward={:#x} target={:?}",
                ev.guid, ev.movement_type, ev.forward, ev.target
            ),
            Err(e) => format!("MovementEvent (bad: {e})"),
        },
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

fn flush_disconnect(
    session: &mut Session,
    socket: &UdpSocket,
    primary: SocketAddr,
    secondary: SocketAddr,
) -> Result<()> {
    session.disconnect(Instant::now());
    for (port, dg) in session.outgoing() {
        let to = if port == Port::Primary {
            primary
        } else {
            secondary
        };
        socket.send_to(&dg, to)?;
    }
    Ok(())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();
    if cli.show_rules {
        let dir = cli
            .data_dir
            .as_ref()
            .context("--show-rules needs --data-dir")?;
        let assets = ac_scene::Assets::open(dir).context("opening DAT archives")?;
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
    // The character to enter with: --character, else the one --create names.
    let wanted: Option<String> = cli.character.clone().or_else(|| cli.create.clone());
    let primary: SocketAddr = format!("{}:{}", cli.host, cli.port).parse().or_else(|_| {
        std::net::ToSocketAddrs::to_socket_addrs(&(cli.host.as_str(), cli.port))
            .and_then(|mut a| a.next().context("resolve").map_err(std::io::Error::other))
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

    let mut world = ac_world::World::default();
    let mut buf = [0u8; 2048];
    let mut characters: Vec<messages::CharacterEntry> = Vec::new();
    let mut characters_known = false;
    let mut ddd_done = false;
    let mut entered_at: Option<Instant> = None;
    let mut said = false;
    let mut enter_requested = false;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let now = Instant::now();
        for (port, dg) in session.outgoing() {
            let to = match port {
                Port::Primary => primary,
                Port::Secondary => secondary,
            };
            if let Some(h) = ac_net::packet::Header::parse(&dg) {
                tracing::debug!(
                    "-> {to} seq={} flags={:#x} id={} size={}",
                    h.sequence,
                    h.flags,
                    h.id,
                    h.size
                );
            }
            socket.send_to(&dg, to)?;
        }
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                if let Some(h) = ac_net::packet::Header::parse(&buf[..n]) {
                    tracing::debug!(
                        "<- {from} seq={} flags={:#x} id={} size={} frags={}",
                        h.sequence,
                        h.flags,
                        h.id,
                        h.size,
                        h.has(ac_net::packet::flags::BLOB_FRAGMENTS)
                    );
                }
                session.receive(&buf[..n], now)
            }
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
                    flush_disconnect(&mut session, &socket, primary, secondary)?;
                    return Ok(());
                }
                Event::Message(msg) => {
                    match world.apply(&msg) {
                        ac_world::Applied::Created => {
                            let (op, body) = messages::split(&msg).unwrap();
                            let _ = op;
                            let guid = u32::from_le_bytes(body[..4].try_into().unwrap());
                            let o = &world.objects[&guid];
                            tracing::info!(
                                "<- ObjectCreate {:#010x} {:?} setup={:#010x} wcid={} pos={}",
                                o.guid,
                                o.name,
                                o.setup_id,
                                o.weenie_class_id,
                                o.position
                                    .map(|p| format!(
                                        "{:#010x} ({:.1},{:.1},{:.1})",
                                        p.cell, p.local.x, p.local.y, p.local.z
                                    ))
                                    .unwrap_or_else(|| "carried".into())
                            );
                            continue;
                        }
                        ac_world::Applied::PlayerSet => {
                            session.send_action(messages::action::LOGIN_COMPLETE, &[]);
                        }
                        ac_world::Applied::Moved => {
                            tracing::info!("<- {} (applied)", describe(&msg));
                            continue;
                        }
                        ac_world::Applied::Failed => {
                            tracing::warn!("world: failed to apply {}", describe(&msg))
                        }
                        _ => {}
                    }
                    tracing::info!("<- {}", describe(&msg));
                    let Some((op, body)) = messages::split(&msg) else {
                        continue;
                    };
                    match op {
                        opcode::CHARACTER_LIST => {
                            if let Ok(cl) = messages::CharacterList::parse(body) {
                                characters = cl.characters;
                                characters_known = true;
                            }
                        }
                        opcode::CHARACTER_CREATE_RESPONSE => {
                            if let Ok(r) = messages::CharacterCreateResponse::parse(body) {
                                if r.response == 1 {
                                    characters.push(messages::CharacterEntry {
                                        id: r.guid,
                                        name: r.name,
                                        seconds_until_deleted: 0,
                                    });
                                    tracing::info!(
                                        "-> CharacterEnterWorldRequest for new character"
                                    );
                                    session
                                        .send_message(queue::UI, messages::enter_world_request());
                                    enter_requested = true;
                                } else {
                                    tracing::error!(
                                        "character creation failed: {} (code {})",
                                        creation::create_failure_message(r.response),
                                        r.response
                                    );
                                    return Ok(());
                                }
                            }
                        }
                        opcode::DDD_END_DDD => {
                            ddd_done = true;
                        }
                        _ => {}
                    }
                    // Enter (or create) once both the DDD exchange is done and
                    // the character list has arrived, whichever comes last.
                    if ddd_done && characters_known && !enter_requested {
                        ddd_done = false;
                        {
                            if let Some(name) = &cli.create {
                                if !characters.iter().any(|c| c.name.eq_ignore_ascii_case(name)) {
                                    let dir = cli
                                        .data_dir
                                        .as_ref()
                                        .context("--create needs --data-dir")?;
                                    let assets = ac_scene::Assets::open(dir)
                                        .context("opening DAT archives")?;
                                    let cc = build_character(
                                        &assets,
                                        &cli,
                                        name,
                                        characters.len() as u32,
                                    )?;
                                    tracing::info!("-> CharacterCreate {name:?}");
                                    session.send_message(queue::UI, cc.encode());
                                    continue;
                                }
                            }
                            if !enter_requested {
                                let pick = match &wanted {
                                    Some(name) => characters
                                        .iter()
                                        .find(|c| c.name.eq_ignore_ascii_case(name)),
                                    None => characters.first(),
                                };
                                match pick {
                                    Some(c) => {
                                        tracing::info!(
                                            "-> CharacterEnterWorldRequest for {} ({:#010x})",
                                            c.name,
                                            c.id
                                        );
                                        session.send_message(
                                            queue::UI,
                                            messages::enter_world_request(),
                                        );
                                        enter_requested = true;
                                    }
                                    None => tracing::warn!(
                                        "no character to enter the world with (use --create NAME)"
                                    ),
                                }
                            }
                        }
                    }
                    let Some((op, _body)) = messages::split(&msg) else {
                        continue;
                    };
                    if op == opcode::CHARACTER_ENTER_WORLD_SERVER_READY {
                        let pick = match &wanted {
                            Some(name) => characters
                                .iter()
                                .find(|c| c.name.eq_ignore_ascii_case(name)),
                            None => characters.first(),
                        };
                        if let Some(c) = pick {
                            tracing::info!("-> CharacterEnterWorld {}", c.name);
                            session
                                .send_message(queue::UI, messages::enter_world(c.id, &cli.account));
                            entered_at = Some(Instant::now());
                        }
                    }
                }
            }
        }
        if session.state() == State::Terminated {
            return Ok(());
        }
        if let (Some(t), Some(text)) = (entered_at, cli.say.as_deref()) {
            if t.elapsed() > Duration::from_secs(2) && !said {
                said = true;
                tracing::info!("-> Talk {text:?}");
                let mut w = ac_net::wire::Writer::new();
                w.string16(text);
                session.send_action(messages::action::TALK, &w.finish());
            }
        }
        if let Some(t) = entered_at {
            if cli.duration > 0 && t.elapsed() > Duration::from_secs(cli.duration) {
                tracing::info!("done");
                flush_disconnect(&mut session, &socket, primary, secondary)?;
                return Ok(());
            }
        } else if Instant::now() > deadline {
            tracing::warn!("timed out before entering the world");
            flush_disconnect(&mut session, &socket, primary, secondary)?;
            return Ok(());
        }
    }
}
