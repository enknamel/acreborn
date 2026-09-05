//! `aclauncher`: a small desktop launch manager for `acviewer`.
//!
//! Keeps servers and accounts in `~/.acreborn/launcher.json` and spawns one
//! `acviewer --connect` process per launch, logging each to
//! `~/.acreborn/logs/<account>.log`.

mod app;
mod config;
mod launch;

use std::path::PathBuf;

use anyhow::{bail, Result};
use clap::Parser;

#[derive(Parser)]
#[command(about = "Launch manager for acviewer")]
struct Cli {
    /// Config file (default: ~/.acreborn/launcher.json)
    #[arg(long)]
    config: Option<PathBuf>,
    /// Print the loaded config (with defaults applied) as JSON and exit
    #[arg(long)]
    dump_config: bool,
    /// Print the command that launching ACCOUNT would run, without running it
    #[arg(long, value_name = "ACCOUNT")]
    dry_run: Option<String>,
    /// With --dry-run: the server the account is on (default: first match)
    #[arg(long)]
    server: Option<String>,
    /// With --dry-run: the character to enter with (default: the last used)
    #[arg(long)]
    character: Option<String>,
    /// With --dry-run: build the headless command (adds --mute)
    #[arg(long)]
    headless: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aclauncher=info".into()),
        )
        .init();
    let cli = Cli::parse();
    let path = cli.config.clone().unwrap_or_else(config::default_path);
    let cfg = config::Config::load(&path)?;

    if cli.dump_config {
        println!("{}", serde_json::to_string_pretty(&cfg)?);
        println!(
            "# config file: {}\n# client (resolved): {}\n# logs: {}",
            path.display(),
            launch::client_binary(&cfg).join(" "),
            config::logs_dir().display()
        );
        return Ok(());
    }
    if let Some(name) = &cli.dry_run {
        let Some(i) = cfg.find_account(name, cli.server.as_deref()) else {
            bail!("no account {name:?} in {}", path.display());
        };
        let account = &cfg.accounts[i];
        let Some(server) = cfg.server(&account.server) else {
            bail!("account {name:?} is on unknown server {:?}", account.server);
        };
        let opts = launch::Options {
            character: cli
                .character
                .clone()
                .or_else(|| account.last_character.clone()),
            headless: cli.headless,
            bus: cfg.share_bus,
            fps: cfg.fps,
        };
        let l = launch::build_launch(
            &launch::client_binary(&cfg),
            &cfg.data_dir,
            server,
            account,
            &opts,
        );
        if let Some(cwd) = &l.cwd {
            println!("cd {}", cwd.display());
        }
        println!("{}", l.display());
        return Ok(());
    }

    app::App::new(path, cfg).run()
}
