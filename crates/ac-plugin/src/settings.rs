//! Settings that survive a restart: a flat JSON object on disk
//! (`~/.config/acreborn/ui.json` by default) that the host loads at
//! startup, hands to every plugin ([`crate::Plugin::load`]) and writes
//! back on exit and every 30 s when something changed
//! ([`crate::Plugin::save`]). Plugins read and write it through
//! `cx.settings` too. Keys are free-form strings; keep them prefixed with
//! the plugin's name (`inventory.show`) so plugins do not collide.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{Map, Value};

/// The settings file's name inside the config directory.
pub const FILE_NAME: &str = "ui.json";

/// A typed view over a JSON object, with a dirty flag so the host writes
/// the file only when something changed.
#[derive(Debug, Default, Clone)]
pub struct Settings {
    values: Map<String, Value>,
    /// A value changed since the last [`Settings::save`] (or load).
    pub dirty: bool,
}

impl Settings {
    pub fn new() -> Self {
        Self::default()
    }

    /// The config directory: `$ACREBORN_CONFIG_DIR`, else
    /// `~/.config/acreborn`.
    pub fn config_dir() -> PathBuf {
        if let Some(d) = std::env::var_os("ACREBORN_CONFIG_DIR") {
            return PathBuf::from(d);
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        home.join(".config").join("acreborn")
    }

    /// Where the UI settings live: [`Settings::config_dir`]`/ui.json`.
    pub fn default_path() -> PathBuf {
        Self::config_dir().join(FILE_NAME)
    }

    /// Read `path`. A missing file is an empty store; an unreadable or
    /// malformed one is logged and treated the same (the next save
    /// overwrites it).
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::new(),
            Err(e) => {
                tracing::warn!("settings: cannot read {}: {e}", path.display());
                return Self::new();
            }
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(values)) => Settings {
                values,
                dirty: false,
            },
            Ok(_) => {
                tracing::warn!("settings: {} is not a JSON object", path.display());
                Self::new()
            }
            Err(e) => {
                tracing::warn!("settings: {} is malformed: {e}", path.display());
                Self::new()
            }
        }
    }

    /// Write `path` as pretty JSON, creating its directory. Clears the
    /// dirty flag on success.
    pub fn save(&mut self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)?;
            }
        }
        let text = serde_json::to_string_pretty(&Value::Object(self.values.clone()))
            .map_err(std::io::Error::other)?;
        std::fs::write(path, text + "\n")?;
        self.dirty = false;
        Ok(())
    }

    /// The value under `key` as a `T`, or `None` when it is missing or
    /// does not deserialize as one.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let v = self.values.get(key)?;
        serde_json::from_value(v.clone()).ok()
    }

    /// The raw value under `key`.
    pub fn get_value(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// Store `value` under `key`. Marks the store dirty only when the
    /// stored JSON actually changes, so plugins can `set` everything they
    /// own on every save without forcing a write.
    pub fn set(&mut self, key: impl Into<String>, value: impl Serialize) {
        let key = key.into();
        let value = match serde_json::to_value(value) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("settings: cannot serialize {key}: {e}");
                return;
            }
        };
        if self.values.get(&key) != Some(&value) {
            self.values.insert(key, value);
            self.dirty = true;
        }
    }

    /// Drop `key`; returns what was there.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let old = self.values.remove(key);
        if old.is_some() {
            self.dirty = true;
        }
        old
    }

    pub fn contains(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Every key and value.
    pub fn values(&self) -> &Map<String, Value> {
        &self.values
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    fn temp_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("acreborn-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("nested").join(FILE_NAME)
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Filter {
        search: String,
        kind: usize,
        folded: Vec<u32>,
    }

    #[test]
    fn typed_get_set_tracks_changes() {
        let mut s = Settings::new();
        assert!(!s.dirty);
        assert_eq!(s.get::<bool>("inventory.show"), None);
        s.set("inventory.show", true);
        assert!(s.dirty);
        assert_eq!(s.get::<bool>("inventory.show"), Some(true));
        // The wrong type reads as missing.
        assert_eq!(s.get::<String>("inventory.show"), None);
        s.dirty = false;
        // Setting the same value again is not a change.
        s.set("inventory.show", true);
        assert!(!s.dirty);
        s.set("inventory.show", false);
        assert!(s.dirty);
        let f = Filter {
            search: "dmg>10".into(),
            kind: 2,
            folded: vec![0x8000_0001, 0x8000_0002],
        };
        s.set("inventory.filter", &f);
        assert_eq!(s.get::<Filter>("inventory.filter"), Some(f));
        assert!(s.remove("inventory.filter").is_some());
        assert!(!s.contains("inventory.filter"));
        assert!(s.remove("inventory.filter").is_none());
    }

    #[test]
    fn round_trips_through_a_file() {
        let path = temp_path("roundtrip");
        // Nothing there yet: an empty store.
        let s = Settings::load(&path);
        assert!(s.is_empty());
        assert!(!s.dirty);

        let mut s = Settings::new();
        s.set("map.show", true);
        s.set(
            "map.view",
            serde_json::json!({"tab": "world", "zoom_world": 2.5}),
        );
        s.set("windows", serde_json::json!({"inventory": [12.0, 34.0]}));
        s.save(&path).unwrap();
        assert!(!s.dirty);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"map.show\": true"), "{text}");

        let back = Settings::load(&path);
        assert!(!back.dirty);
        assert_eq!(back.get::<bool>("map.show"), Some(true));
        assert_eq!(
            back.get::<std::collections::BTreeMap<String, [f32; 2]>>("windows")
                .unwrap()["inventory"],
            [12.0, 34.0]
        );
        assert_eq!(back.values(), s.values());

        // A malformed file is an empty store, not a crash.
        std::fs::write(&path, "{ not json").unwrap();
        assert!(Settings::load(&path).is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn default_path_is_ui_json_under_acreborn() {
        // `config_dir` reads the process environment; only check the
        // shape without touching it, as tests run in parallel.
        let p = Settings::default_path();
        assert_eq!(p.file_name().unwrap(), FILE_NAME);
        assert!(p.parent().unwrap().ends_with("acreborn"));
    }
}
