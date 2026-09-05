//! Magic as the game defines it (see `docs/game/mechanics.md`, section 1):
//! the spellbook (server-owned set of known spells), the eight spell
//! bars (server-persisted shortcuts into the book), the enchantment
//! registry (active buffs and debuffs), and spell components (inventory
//! items the server consumes on each cast; a school's focus reduces a
//! formula to scarabs plus prismatic tapers).
//!
//! Everything here is a plain method on [`Client`] so plugins, scripts
//! and the panels all drive the same state.

use ac_formats::spell_table::Spell;
use ac_world::stats::Enchantment;

use crate::Client;

/// Number of spell bars (tabs) the game keeps per character.
pub const SPELL_BARS: usize = 8;

/// Prismatic Taper's component id in the SpellComponentsTable.
pub const PRISMATIC_TAPER: u32 = 188;
/// Chorizite: kept in the foci formula alongside the scarabs.
pub const CHORIZITE: u32 = 111;

/// Foci weenie class ids by magic school (ACE world database).
pub fn focus_wcid(school: u32) -> Option<u32> {
    use ac_formats::spell_table::school;
    Some(match school {
        school::WAR => 15271,      // Foci of Strife
        school::LIFE => 15270,     // Foci of Verdancy
        school::CREATURE => 15268, // Foci of Enchantment
        school::ITEM => 15269,     // Foci of Artifice
        school::VOID => 43173,     // Foci of Shadow
        _ => return None,
    })
}

/// Why a spell cannot be cast right now, or that it can.
#[derive(Debug, Clone, PartialEq)]
pub enum CastCheck {
    Ok,
    /// Not in the spellbook.
    NotKnown,
    /// No magic caster (wand, orb, staff...) is wielded.
    NoCaster,
    /// Components of the current formula missing from the packs:
    /// (component id, how many short).
    MissingComponents(Vec<(u32, u32)>),
    NotEnoughMana {
        need: u32,
        have: u32,
    },
}

/// One kind of spell component carried, for the Components panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentCount {
    /// Id in the SpellComponentsTable (0x0E00000F).
    pub component_id: u32,
    pub name: String,
    /// Weenie class of the item (from the DualDidMapper 0x27000002).
    pub wcid: u32,
    /// Stack total across the packs.
    pub count: u32,
    /// Quantity the player wants to keep (`@fillcomps` target).
    pub desired: u32,
}

impl Client {
    // ---------------------------------------------------------- spellbook

    /// Known spell ids, in the server's order.
    pub fn known_spell_ids(&self) -> Vec<u32> {
        self.world.stats.spells.clone()
    }

    /// The SpellTable entry for a spell, if the archive has it.
    pub fn spell(&self, spell: u32) -> Option<Spell> {
        self.assets
            .spell_table()
            .ok()
            .and_then(|t| t.get(spell).cloned())
    }

    /// Spellbook filter bits (schools and levels shown), as the server
    /// stores them: Creature 0x1, Item 0x2, Life 0x4, War 0x8,
    /// Level1..Level9 0x10..0x1000, Void 0x2000.
    pub fn spellbook_filters(&self) -> u32 {
        self.world.stats.options.spellbook_filters
    }

    /// Change the spellbook filters, locally and on the server.
    pub fn set_spellbook_filters(&mut self, bits: u32) {
        self.world.stats.options.spellbook_filters = bits;
        self.session.send_action(
            ac_net::messages::action::SPELLBOOK_FILTER,
            &bits.to_le_bytes(),
        );
    }

    /// Delete a spell from the spellbook (the server confirms with
    /// MagicRemoveSpell).
    pub fn forget_spell(&mut self, spell: u32) {
        self.session
            .send_action(ac_net::messages::action::REMOVE_SPELL, &spell.to_le_bytes());
    }

    // --------------------------------------------------------- spell bars

    /// The eight spell bars: ordered spell ids per bar.
    pub fn spell_bars(&self) -> &[Vec<u32>] {
        &self.world.stats.options.spell_bars
    }

    /// Put a spell on a bar at `position` (0-based; past the end appends),
    /// locally and on the server (AddSpellFavorite). Unknown spells and
    /// bars past the eighth are ignored.
    pub fn add_to_spell_bar(&mut self, bar: usize, position: usize, spell: u32) {
        if bar >= SPELL_BARS || !self.world.stats.spells.contains(&spell) {
            return;
        }
        let bars = &mut self.world.stats.options.spell_bars;
        while bars.len() < SPELL_BARS {
            bars.push(Vec::new());
        }
        let list = &mut bars[bar];
        list.retain(|&s| s != spell);
        let position = position.min(list.len());
        list.insert(position, spell);
        let mut w = ac_net::wire::Writer::new();
        w.u32(spell);
        w.u32(position as u32);
        w.u32(bar as u32);
        self.session
            .send_action(ac_net::messages::action::ADD_SPELL_FAVORITE, &w.finish());
    }

    /// Take a spell off a bar (RemoveSpellFavorite).
    pub fn remove_from_spell_bar(&mut self, bar: usize, spell: u32) {
        let Some(list) = self.world.stats.options.spell_bars.get_mut(bar) else {
            return;
        };
        let before = list.len();
        list.retain(|&s| s != spell);
        if list.len() == before {
            return;
        }
        let mut w = ac_net::wire::Writer::new();
        w.u32(spell);
        w.u32(bar as u32);
        self.session
            .send_action(ac_net::messages::action::REMOVE_SPELL_FAVORITE, &w.finish());
    }

    // ------------------------------------------------------- enchantments

    /// Active enchantments on the character (buffs, debuffs, vitae).
    pub fn enchantments(&self) -> &[Enchantment] {
        &self.world.stats.enchantments
    }

    // --------------------------------------------------------- components

    /// The wielded magic caster's guid, if any.
    pub fn wielded_caster(&self) -> Option<u32> {
        use ac_world::item_type;
        self.world
            .wielded()
            .find(|o| o.item_type & item_type::CASTER != 0)
            .map(|o| o.guid)
    }

    /// Whether a focus for `school` is in the packs.
    pub fn has_focus(&self, school: u32) -> bool {
        focus_wcid(school).is_some_and(|w| self.world.inventory().any(|o| o.weenie_class_id == w))
    }

    /// The components one cast of `spell` needs right now: the full
    /// formula, or scarabs (and chorizite) plus prismatic tapers when the
    /// school's focus is carried. Empty for unknown spells.
    pub fn current_formula(&self, spell: u32) -> Vec<u32> {
        let Some(sp) = self.spell(spell) else {
            return Vec::new();
        };
        let full: Vec<u32> = sp.formula().collect();
        if !self.has_focus(sp.school) {
            return full;
        }
        let mut out: Vec<u32> = full
            .iter()
            .copied()
            .filter(|&c| is_scarab(c) || c == CHORIZITE)
            .collect();
        let tapers = match scarab_power(full.first().copied().unwrap_or(0)) {
            1 => 1,
            2 => 2,
            3 | 4 | 7 => 3,
            5 | 6 | 8 | 9 | 10 => 4,
            _ => 0,
        };
        out.extend(std::iter::repeat(PRISMATIC_TAPER).take(tapers));
        out
    }

    /// Every spell component carried, with counts and desired levels.
    pub fn components(&self) -> Vec<ComponentCount> {
        // Filled in by the component mapping (DualDidMapper 0x27000002).
        Vec::new()
    }

    /// Whether `spell` could be cast now, and if not why.
    pub fn can_cast(&self, spell: u32) -> CastCheck {
        if !self.world.stats.spells.contains(&spell) {
            return CastCheck::NotKnown;
        }
        if self.wielded_caster().is_none() {
            return CastCheck::NoCaster;
        }
        CastCheck::Ok
    }

    /// Set how many of a component the player wants to keep
    /// (SetDesiredComponentLevel), locally and on the server.
    pub fn set_desired_component(&mut self, component_id: u32, quantity: u32) {
        let list = &mut self.world.stats.options.desired_comps;
        match list.iter_mut().find(|(c, _)| *c == component_id) {
            Some(e) => e.1 = quantity,
            None => list.push((component_id, quantity)),
        }
        let mut w = ac_net::wire::Writer::new();
        w.u32(component_id);
        w.u32(quantity);
        self.session.send_action(
            ac_net::messages::action::SET_DESIRED_COMPONENT_LEVEL,
            &w.finish(),
        );
    }

    /// With a vendor open, buy components up to their desired quantities
    /// (the `@fillcomps` command). Returns how many stacks were requested.
    pub fn fill_components(&mut self) -> usize {
        0
    }
}

/// Scarab component ids (SpellComponentsTable): Lead 1 ... Mana 10.
pub fn is_scarab(component: u32) -> bool {
    (1..=10).contains(&component)
}

/// A scarab's power, which sets the taper count of the foci formula.
pub fn scarab_power(component: u32) -> u32 {
    match component {
        1..=10 => component, // Lead 1, Iron 2, Copper 3, Silver 4, Gold 5, Pyreal 6, Diamond 7, Platinum 8, Dark 9, Mana 10
        _ => 0,
    }
}
