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
    /// Create a character with this name if the account has none (Aluvian,
    /// first template, Holtburg). Needs --data-dir for the CharGen table.
    #[arg(long)]
    create: Option<String>,
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

/// Build a valid CharacterCreate from the CharGen table: given heritage,
/// male, the named (or first) template, default appearance, Holtburg.
fn build_character(
    data_dir: &PathBuf,
    account: &str,
    name: &str,
    heritage: u32,
) -> Result<messages::CharacterCreate> {
    use ac_formats::chargen::CharGen;
    let dat = ac_dat::DatArchive::open(data_dir.join("client_portal.dat"))?;
    let cg = CharGen::parse(CharGen::ID, &dat.read(CharGen::ID)?)?;
    let (_, hg) = cg
        .heritage_groups
        .iter()
        .find(|(id, _)| *id == heritage)
        .context("heritage not in CharGen")?;
    let tmpl = hg.templates.first().context("heritage has no templates")?;
    // Skill advancement per skill id: templates list trained ("normal") and
    // specialized ("primary") skills; other heritage skills are untrained.
    let mut skills = vec![0u32; 55];
    for s in &hg.skills {
        if (s.skill as usize) < skills.len() {
            skills[s.skill as usize] = 1;
        }
    }
    for &s in &tmpl.normal_skills {
        skills[s as usize] = 2;
    }
    for &s in &tmpl.primary_skills {
        skills[s as usize] = 3;
    }
    let start_area = cg
        .starter_areas
        .iter()
        .position(|a| a.name == "Holtburg")
        .unwrap_or(0) as u32;
    let (_, sex) = hg
        .genders
        .iter()
        .find(|(g, _)| *g == 1)
        .context("no male option")?;
    tracing::info!(
        "creating {name}: {} {} template {:?} (str {} end {} coo {} qui {} foc {} self {}), start area {}",
        hg.name,
        sex.name,
        tmpl.name,
        tmpl.strength,
        tmpl.endurance,
        tmpl.coordination,
        tmpl.quickness,
        tmpl.focus,
        tmpl.self_,
        start_area
    );
    Ok(messages::CharacterCreate {
        account: account.to_string(),
        name: name.to_string(),
        heritage,
        gender: 1,
        appearance: messages::Appearance {
            headgear_style: u32::MAX,
            skin_hue: 0.5,
            hair_hue: 0.5,
            headgear_hue: 0.5,
            shirt_hue: 0.5,
            pants_hue: 0.5,
            footwear_hue: 0.5,
            ..Default::default()
        },
        template: 0,
        strength: tmpl.strength,
        endurance: tmpl.endurance,
        coordination: tmpl.coordination,
        quickness: tmpl.quickness,
        focus: tmpl.focus,
        self_: tmpl.self_,
        slot: 0,
        skills,
        start_area,
    })
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
                            tracing::info!(
                                "<- UpdatePosition applied ({} objects)",
                                world.objects.len()
                            );
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
                                        "character creation failed with code {}",
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
                            if characters.is_empty() {
                                if let (Some(name), Some(dir)) = (&cli.create, &cli.data_dir) {
                                    let cc = build_character(dir, &cli.account, name, 1)?;
                                    tracing::info!("-> CharacterCreate {name:?}");
                                    session.send_message(queue::UI, cc.encode());
                                    continue;
                                }
                            }
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
                    }
                    let Some((op, body)) = messages::split(&msg) else {
                        continue;
                    };
                    match op {
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
