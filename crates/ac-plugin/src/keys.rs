//! Key bindings: which key opens which panel, changeable by the player
//! from the menu and kept in the settings file, so every client on the
//! machine shares them.
//!
//! Only the most used panels have a key out of the box; the rest are
//! opened from the menu (Escape). A panel asks whether a key is its own
//! with [`bound`]; the menu lists and changes the bindings.

use std::collections::BTreeMap;
use std::sync::RwLock;

use crate::{egui, Settings};

/// Something a key can be bound to: a panel to toggle, or an action.
#[derive(Clone, Copy, Debug)]
pub struct Action {
    /// The id panels and the menu use ("inventory").
    pub id: &'static str,
    /// What the menu shows.
    pub label: &'static str,
    /// The key out of the box, by egui name ("I", "F5"), or none.
    pub default: Option<&'static str>,
}

/// Every panel and action a key can open, in menu order.
pub const ACTIONS: &[Action] = &[
    Action {
        id: "menu",
        label: "Menu",
        default: Some("Escape"),
    },
    Action {
        id: "inventory",
        label: "Inventory",
        default: Some("I"),
    },
    Action {
        id: "map",
        label: "Map",
        default: Some("M"),
    },
    Action {
        id: "skills",
        label: "Skills",
        default: Some("K"),
    },
    Action {
        id: "spellbook",
        label: "Spellbook",
        default: Some("P"),
    },
    Action {
        id: "spellbar",
        label: "Spell bar",
        default: Some("B"),
    },
    Action {
        id: "buffs",
        label: "Buffs",
        default: None,
    },
    Action {
        id: "components",
        label: "Components",
        default: None,
    },
    Action {
        id: "autoplay",
        label: "Play on its own",
        default: None,
    },
    Action {
        id: "fleet",
        label: "Fleet",
        default: None,
    },
    Action {
        id: "holdings",
        label: "Items (all characters)",
        default: None,
    },
    Action {
        id: "fellowship",
        label: "Fellowship",
        default: None,
    },
    Action {
        id: "allegiance",
        label: "Allegiance",
        default: None,
    },
    Action {
        id: "social",
        label: "Social",
        default: None,
    },
    Action {
        id: "housing",
        label: "Housing",
        default: None,
    },
    Action {
        id: "appraisal",
        label: "Appraisal",
        default: None,
    },
    Action {
        id: "nameplates",
        label: "Nameplates",
        default: None,
    },
    Action {
        id: "options",
        label: "Options",
        default: None,
    },
];

/// Keys the viewer itself answers to, not bindable here, listed so the
/// menu can say what they are.
pub const FIXED: &[(&str, &str)] = &[
    ("W A S D", "walk (Shift: slow)"),
    ("Space", "jump: hold to charge, wheel to place the landing"),
    ("C", "combat on or off"),
    ("R / G", "use / pick up the selected object"),
    ("T", "show the route on the ground"),
    ("Y", "fly (Space up, Control down)"),
    ("Tab", "next session"),
    ("Enter", "chat"),
];

/// Bindings as `action id -> key name`; an action missing here has its
/// default, one with `None` has no key.
static BINDINGS: RwLock<BTreeMap<String, Option<String>>> = RwLock::new(BTreeMap::new());

fn key_name(k: egui::Key) -> String {
    k.symbol_or_name().to_string()
}

/// The key bound to `action`, if any.
pub fn binding(action: &str) -> Option<egui::Key> {
    let name = {
        let b = BINDINGS.read().ok()?;
        match b.get(action) {
            Some(k) => k.clone(),
            None => ACTIONS
                .iter()
                .find(|a| a.id == action)
                .and_then(|a| a.default.map(str::to_string)),
        }
    };
    name.and_then(|n| egui::Key::from_name(&n))
}

/// Whether `key` is the one bound to `action`.
pub fn bound(action: &str, key: egui::Key) -> bool {
    binding(action) == Some(key)
}

/// Bind `action` to `key` (or to nothing); any other action that had the
/// key loses it.
pub fn rebind(action: &str, key: Option<egui::Key>) {
    let Ok(mut b) = BINDINGS.write() else { return };
    if let Some(k) = key {
        let taken: Vec<String> = ACTIONS
            .iter()
            .filter(|a| a.id != action)
            .filter(|a| {
                let current = match b.get(a.id) {
                    Some(c) => c.clone(),
                    None => a.default.map(str::to_string),
                };
                current.as_deref() == Some(key_name(k).as_str())
            })
            .map(|a| a.id.to_string())
            .collect();
        for t in taken {
            b.insert(t, None);
        }
    }
    b.insert(action.to_string(), key.map(key_name));
}

/// The key's name as the menu shows it ("I", "F5", "none").
pub fn binding_label(action: &str) -> String {
    binding(action)
        .map(key_name)
        .unwrap_or_else(|| "none".to_string())
}

/// Back to the keys out of the box.
pub fn reset() {
    if let Ok(mut b) = BINDINGS.write() {
        b.clear();
    }
}

/// Read the bindings from the settings (`keys.<action>`: a key name, or
/// "" for none).
pub fn load(settings: &Settings) {
    let Ok(mut b) = BINDINGS.write() else { return };
    b.clear();
    for a in ACTIONS {
        if let Some(name) = settings.get::<String>(&format!("keys.{}", a.id)) {
            b.insert(a.id.to_string(), (!name.is_empty()).then_some(name));
        }
    }
}

/// Write the bindings that differ from the defaults.
pub fn save(settings: &mut Settings) {
    let Ok(b) = BINDINGS.read() else { return };
    for a in ACTIONS {
        let key = format!("keys.{}", a.id);
        match b.get(a.id) {
            Some(k) => settings.set(key, k.clone().unwrap_or_default()),
            None => {
                if settings.get::<String>(&key).is_some() {
                    settings.set(key, a.default.unwrap_or("").to_string());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_bind_and_rebinding_moves_a_key() {
        reset();
        assert!(bound("inventory", egui::Key::I));
        assert!(!bound("buffs", egui::Key::I));
        assert_eq!(binding("buffs"), None);
        // Give the buffs I: the inventory loses it.
        rebind("buffs", Some(egui::Key::I));
        assert!(bound("buffs", egui::Key::I));
        assert_eq!(binding("inventory"), None);
        assert_eq!(binding_label("inventory"), "none");
        // Round trip through the settings.
        let mut s = Settings::default();
        save(&mut s);
        reset();
        assert!(bound("inventory", egui::Key::I));
        load(&s);
        assert!(bound("buffs", egui::Key::I));
        assert_eq!(binding("inventory"), None);
        reset();
    }
}
