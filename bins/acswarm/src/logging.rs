//! Turning the log up and down while the app is running.
//!
//! Every subsystem logs through `tracing`, and which of it reaches the
//! terminal is a filter: a default level, plus an override per target
//! (`warn,ac_client=info,ac_net=debug`). Setting that once at startup
//! through `RUST_LOG` is fine for a developer and useless to a player
//! watching a character do something odd, who wants to turn one part up
//! now and back down when they have seen it.
//!
//! So the filter is reloadable. [`init`] installs it, [`set`] replaces
//! it, and [`current`] says what it is.

use std::sync::OnceLock;

use ac_plugin::logging::DEFAULT;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Registry;

type Handle = reload::Handle<EnvFilter, Registry>;

static RELOAD: OnceLock<Handle> = OnceLock::new();
static CURRENT: OnceLock<std::sync::Mutex<String>> = OnceLock::new();

/// Install the reloadable filter. `RUST_LOG` still wins when set, so a
/// developer's habits keep working; otherwise `saved` (from the
/// settings) or [`DEFAULT`].
pub fn init(saved: Option<&str>) {
    let text = std::env::var("RUST_LOG")
        .ok()
        .or_else(|| saved.map(str::to_string))
        .unwrap_or_else(|| DEFAULT.to_string());
    let filter = EnvFilter::try_new(&text).unwrap_or_else(|_| EnvFilter::new(DEFAULT));
    let (layer, handle) = reload::Layer::new(filter);
    let _ = RELOAD.set(handle);
    let _ = CURRENT.set(std::sync::Mutex::new(text));
    tracing_subscriber::registry()
        .with(layer)
        .with(tracing_subscriber::fmt::layer())
        .init();
}

/// Replace the filter. Returns what is wrong with `text` rather than
/// applying it, so a half-typed line in a settings box does not blank
/// the log.
pub fn set(text: &str) -> Result<(), String> {
    let filter = EnvFilter::try_new(text).map_err(|e| e.to_string())?;
    let handle = RELOAD.get().ok_or("logging is not set up yet")?;
    handle.reload(filter).map_err(|e| e.to_string())?;
    if let Some(cur) = CURRENT.get() {
        if let Ok(mut c) = cur.lock() {
            *c = text.to_string();
        }
    }
    Ok(())
}

/// The filter in force.
pub fn current() -> String {
    CURRENT
        .get()
        .and_then(|c| c.lock().ok().map(|c| c.clone()))
        .unwrap_or_else(|| DEFAULT.to_string())
}
