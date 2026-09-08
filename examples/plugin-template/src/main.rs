//! An offline runner for the template plugin: a plugin host with no
//! session, the way `acviewer --demo-ui` runs the panels, driven for a
//! few frames from the command line. It feeds the plugin a chat line and
//! an autoplay event, presses its key, types its command, draws its panel
//! through a headless egui, and writes the settings file, so every hook
//! can be seen working without a server or a window.
//!
//! ```text
//! cargo run -p plugin-template -- [--frames N] [--bus [ADDR]]
//! ```
//!
//! With `--bus` the host joins the local cross-process bus (or hosts
//! it), so an `acbot --bus` or `acviewer --bus` running alongside hears
//! the greeting on the `template.hello` topic and this runner prints
//! what they post on `autoplay.event`.

use std::time::{Duration, Instant};

use ac_plugin::{egui, Event, Host, AUTOPLAY_TOPIC};
use plugin_template::{Template, HELLO_TOPIC};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let mut frames = 3usize;
    let mut bus: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--frames" => frames = args.next().and_then(|n| n.parse().ok()).unwrap_or(frames),
            "--bus" => bus = Some(String::new()),
            "--help" | "-h" => {
                println!("usage: plugin-template [--frames N] [--bus]");
                return;
            }
            other if bus == Some(String::new()) && !other.starts_with("--") => {
                bus = Some(other.to_string());
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }

    let mut host = Host::new();
    host.register(Box::new(Template::new()));
    // The settings file: `$ACREBORN_CONFIG_DIR/ui.json`, shared with the
    // real client, so what this run saves the viewer will load.
    host.load_settings(ac_plugin::Settings::default_path());
    println!("plugins: {:?}", host.names());
    if let Some(addr) = &bus {
        let addr = (!addr.is_empty()).then_some(addr.as_str());
        match host.join_bus(addr, "plugin-template") {
            Ok(()) => println!("bus: joined as plugin-template"),
            Err(e) => println!("bus: cannot join: {e}"),
        }
    }

    // What a session would have produced.
    let events = vec![
        Event::Chat {
            text: "Drudge Skulker says, \"Grr\"".into(),
            kind: 3,
        },
        Event::Autoplay {
            doing: "fighting".into(),
            text: "fighting Drudge Skulker".into(),
        },
    ];
    let egui = egui::Context::default();
    let mut last = Instant::now();
    for frame in 0..frames {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32();
        last = now;
        // Frame 0 carries the events; the key, the command and the
        // panel follow on later frames so their effects can be seen.
        let evs = if frame == 0 { events.as_slice() } else { &[] };
        let r = host.frame(Vec::new(), 0, evs, dt, now);
        say(frame, &r.chat);
        match frame {
            1 => {
                let r = host.key(Vec::new(), 0, egui::Key::F7, true);
                println!("[{frame}] F7 consumed: {}", r.consumed);
                let r = host.command(Vec::new(), 0, "/hello Asheron");
                say(frame, &r.chat);
            }
            2 => {
                let mut requests = None;
                let _ = egui.run_ui(egui::RawInput::default(), |ctx| {
                    requests = Some(host.ui(Vec::new(), 0, ctx));
                });
                let rect = egui.memory(|m| m.area_rect(egui::Id::new("template")));
                println!("[{frame}] panel drawn at {rect:?}");
            }
            _ => {}
        }
        for m in host.board.messages_on(HELLO_TOPIC) {
            println!(
                "[{frame}] bus {HELLO_TOPIC}: {} (from {:?})",
                m.value, m.origin
            );
        }
        for m in host.board.messages_on(AUTOPLAY_TOPIC) {
            println!(
                "[{frame}] bus {AUTOPLAY_TOPIC}: {} (from {:?})",
                m.value, m.origin
            );
        }
        host.end_frame();
        if bus.is_some() {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    let written = host.save_settings();
    println!(
        "settings {} at {}",
        if written { "written" } else { "unchanged" },
        host.settings_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    );
}

fn say(frame: usize, chat: &[(String, u32)]) {
    for (text, _) in chat {
        println!("[{frame}] chat: {text}");
    }
}
