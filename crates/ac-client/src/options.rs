//! Character options: the per-character switches the server keeps
//! (accept fellowship / allegiance / trade requests, let others give
//! you items, auto-repeat attacks, run by default, chat channels...).
//! They arrive as two bitfields in PlayerDescription and change with
//! SetSingleCharacterOption (0x0005: option id, value), ACE
//! `CharacterOption` mapped onto `CharacterOptions1/2` bits.

use crate::Client;

/// Which of the two option words a bit lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Word {
    One,
    Two,
}

/// One option: its wire id, the bit it sets, and a label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharacterOption {
    pub id: u32,
    pub word: Word,
    pub bit: u32,
    pub label: &'static str,
    /// The bit means "ignore"/"disable"; the label is phrased positively
    /// and the panel shows the inverse.
    pub inverted: bool,
}

/// The options a player is likely to touch, in panel order.
pub const OPTIONS: [CharacterOption; 21] = [
    CharacterOption {
        id: 0x1A,
        word: Word::One,
        bit: 0x8000_0000,
        label: "Ask before tinkering (chance of success dialog)",
        inverted: false,
    },
    CharacterOption {
        id: 0x02,
        word: Word::One,
        bit: 0x0000_0008,
        label: "Accept fellowship requests",
        inverted: true,
    },
    CharacterOption {
        id: 0x12,
        word: Word::One,
        bit: 0x2000_0000,
        label: "Automatically accept fellowship requests",
        inverted: false,
    },
    CharacterOption {
        id: 0x0F,
        word: Word::One,
        bit: 0x0004_0000,
        label: "Share fellowship experience",
        inverted: false,
    },
    CharacterOption {
        id: 0x11,
        word: Word::One,
        bit: 0x0010_0000,
        label: "Share fellowship loot",
        inverted: false,
    },
    CharacterOption {
        id: 0x01,
        word: Word::One,
        bit: 0x0000_0004,
        label: "Accept allegiance requests",
        inverted: true,
    },
    CharacterOption {
        id: 0x03,
        word: Word::One,
        bit: 0x0002_0000,
        label: "Accept trade requests",
        inverted: true,
    },
    CharacterOption {
        id: 0x06,
        word: Word::One,
        bit: 0x0000_0040,
        label: "Let other players give you items",
        inverted: false,
    },
    CharacterOption {
        id: 0x10,
        word: Word::One,
        bit: 0x0008_0000,
        label: "Accept corpse looting permissions",
        inverted: false,
    },
    CharacterOption {
        id: 0x00,
        word: Word::One,
        bit: 0x0000_0002,
        label: "Auto-repeat attacks",
        inverted: false,
    },
    CharacterOption {
        id: 0x19,
        word: Word::One,
        bit: 0x1000_0000,
        label: "Use charge attack",
        inverted: false,
    },
    CharacterOption {
        id: 0x0D,
        word: Word::One,
        bit: 0x0000_2000,
        label: "Auto-target",
        inverted: false,
    },
    CharacterOption {
        id: 0x0A,
        word: Word::One,
        bit: 0x0000_0400,
        label: "Run as default movement",
        inverted: false,
    },
    CharacterOption {
        id: 0x1B,
        word: Word::One,
        bit: 0x4000_0000,
        label: "Listen to allegiance chat",
        inverted: false,
    },
    CharacterOption {
        id: 0x23,
        word: Word::Two,
        bit: 0x0000_0100,
        label: "Listen to General chat",
        inverted: false,
    },
    CharacterOption {
        id: 0x24,
        word: Word::Two,
        bit: 0x0000_0200,
        label: "Listen to Trade chat",
        inverted: false,
    },
    CharacterOption {
        id: 0x25,
        word: Word::Two,
        bit: 0x0000_0400,
        label: "Listen to LFG chat",
        inverted: false,
    },
    CharacterOption {
        id: 0x26,
        word: Word::Two,
        bit: 0x0000_0800,
        label: "Listen to Roleplay chat",
        inverted: false,
    },
    CharacterOption {
        id: 0x27,
        word: Word::Two,
        bit: 0x0000_1000,
        label: "Appear offline",
        inverted: false,
    },
    CharacterOption {
        id: 0x1C,
        word: Word::Two,
        bit: 0x0000_0002,
        label: "Show your date of birth",
        inverted: false,
    },
    CharacterOption {
        id: 0x20,
        word: Word::Two,
        bit: 0x0000_0010,
        label: "Show your number of deaths",
        inverted: false,
    },
];

/// The option whose label starts with `name` (case-insensitive).
pub fn option_by_name(name: &str) -> Option<&'static CharacterOption> {
    let want = name.trim().to_lowercase();
    OPTIONS
        .iter()
        .find(|o| o.label.to_lowercase() == want)
        .or_else(|| {
            OPTIONS
                .iter()
                .find(|o| o.label.to_lowercase().starts_with(&want))
        })
}

impl Client {
    /// Whether an option is on, as the panel phrases it (an inverted
    /// "ignore" bit reads as "accept" when clear).
    pub fn option_enabled(&self, o: &CharacterOption) -> bool {
        let word = match o.word {
            Word::One => self.world.stats.options.options1,
            Word::Two => self.world.stats.options.options2,
        };
        let set = word & o.bit != 0;
        if o.inverted {
            !set
        } else {
            set
        }
    }

    /// Turn an option on or off (SetSingleCharacterOption 0x0005), as the
    /// panel phrases it, and mirror it locally.
    pub fn set_option(&mut self, o: &CharacterOption, enabled: bool) {
        let bit_on = if o.inverted { !enabled } else { enabled };
        let word = match o.word {
            Word::One => &mut self.world.stats.options.options1,
            Word::Two => &mut self.world.stats.options.options2,
        };
        if bit_on {
            *word |= o.bit;
        } else {
            *word &= !o.bit;
        }
        tracing::info!("option {}: {}", o.label, if enabled { "on" } else { "off" });
        let mut w = ac_net::wire::Writer::new();
        w.u32(o.id).u32(u32::from(bit_on));
        self.session.send_action(
            ac_net::messages::action::SET_SINGLE_CHARACTER_OPTION,
            &w.finish(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_resolve_by_prefix() {
        assert_eq!(
            option_by_name("accept fellowship").map(|o| o.id),
            Some(0x02)
        );
        assert_eq!(option_by_name("Run").map(|o| o.id), Some(0x0A));
        assert!(option_by_name("zzz").is_none());
    }
}
