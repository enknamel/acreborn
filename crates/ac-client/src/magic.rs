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
    /// formula with this account's personal tapers
    /// (`Spell::player_formula`), or scarabs (and chorizite) plus
    /// prismatic tapers when the school's focus is carried. Empty for
    /// spells the archive does not know.
    pub fn current_formula(&self, spell: u32) -> Vec<u32> {
        let Some(sp) = self.spell(spell) else {
            return Vec::new();
        };
        if self.has_focus(sp.school) {
            let full: Vec<u32> = sp.formula().collect();
            foci_formula(&full)
        } else {
            sp.player_formula(&self.config.account)
        }
    }

    /// Every spell component carried, with counts and desired levels.
    /// Components with a desired quantity that are not carried appear
    /// with a count of 0 so the panel and `fill_components` can act on
    /// them. Sorted by the component table's category, then id.
    pub fn components(&self) -> Vec<ComponentCount> {
        let Ok(mapper) = self.assets.spell_component_ids() else {
            return Vec::new();
        };
        let table = self.assets.spell_components().ok();
        let describe = |id: u32, wcid: u32| {
            let name = table
                .as_ref()
                .and_then(|t| t.get(id).map(|c| c.name.clone()))
                .or_else(|| mapper.name_of(id).map(str::to_string))
                .unwrap_or_else(|| format!("Component {id}"));
            ComponentCount {
                component_id: id,
                name,
                wcid,
                count: 0,
                desired: 0,
            }
        };
        let mut out: Vec<ComponentCount> = Vec::new();
        for o in self.world.inventory() {
            let Some(id) = mapper.component_of_wcid(o.weenie_class_id) else {
                continue;
            };
            if id == 0 {
                continue;
            }
            let n = o.stack_size.max(1);
            match out.iter_mut().find(|c| c.component_id == id) {
                Some(c) => c.count += n,
                None => {
                    let mut c = describe(id, o.weenie_class_id);
                    c.count = n;
                    out.push(c);
                }
            }
        }
        // The server keys desired quantities by weenie class id.
        for &(wcid, desired) in &self.world.stats.options.desired_comps {
            match out.iter_mut().find(|c| c.wcid == wcid) {
                Some(c) => c.desired = desired,
                None if desired > 0 => {
                    let Some(id) = mapper.component_of_wcid(wcid) else {
                        continue;
                    };
                    let mut c = describe(id, wcid);
                    c.desired = desired;
                    out.push(c);
                }
                None => {}
            }
        }
        let category = |id: u32| {
            table
                .as_ref()
                .and_then(|t| t.get(id).map(|c| c.category))
                .unwrap_or(u32::MAX)
        };
        out.sort_by_key(|c| (category(c.component_id), c.component_id));
        out
    }

    /// Whether `spell` could be cast now, and if not why. Checked in
    /// order: known, caster wielded, components of the current formula,
    /// mana against the spell's base cost (no Mana Conversion estimate,
    /// so the real cost may be lower).
    pub fn can_cast(&self, spell: u32) -> CastCheck {
        if !self.world.stats.spells.contains(&spell) {
            return CastCheck::NotKnown;
        }
        if self.wielded_caster().is_none() {
            return CastCheck::NoCaster;
        }
        let need = self.current_formula(spell);
        if !need.is_empty() {
            let have = self.components();
            let count = |id: u32| {
                have.iter()
                    .find(|c| c.component_id == id)
                    .map_or(0, |c| c.count)
            };
            let missing = missing_components(&need, count);
            if !missing.is_empty() {
                return CastCheck::MissingComponents(missing);
            }
        }
        if let Some(sp) = self.spell(spell) {
            let have = self.world.stats.vitals[2].current;
            if have < sp.base_mana {
                return CastCheck::NotEnoughMana {
                    need: sp.base_mana,
                    have,
                };
            }
        }
        CastCheck::Ok
    }

    /// Set how many of a component the player wants to keep
    /// (SetDesiredComponentLevel), locally and on the server.
    pub fn set_desired_component(&mut self, component_id: u32, quantity: u32) {
        // The wire (and the PlayerDescription list) use the weenie class.
        let Some(wcid) = self
            .assets
            .spell_component_ids()
            .ok()
            .and_then(|m| m.component_wcid(component_id))
        else {
            tracing::warn!("no weenie for spell component {component_id}");
            return;
        };
        let list = &mut self.world.stats.options.desired_comps;
        match list.iter_mut().find(|(c, _)| *c == wcid) {
            Some(e) => e.1 = quantity,
            None => list.push((wcid, quantity)),
        }
        let mut w = ac_net::wire::Writer::new();
        w.u32(wcid);
        w.u32(quantity);
        self.session.send_action(
            ac_net::messages::action::SET_DESIRED_COMPONENT_LEVEL,
            &w.finish(),
        );
    }

    /// With a vendor open, buy components up to their desired quantities
    /// (the `@fillcomps` command): one Buy naming every stocked component
    /// that is short, for its shortfall. Returns how many kinds were
    /// ordered (0 with no vendor open or nothing to buy).
    pub fn fill_components(&mut self) -> usize {
        let Some(vendor) = self.world.open_vendor.as_ref() else {
            return 0;
        };
        let Ok(mapper) = self.assets.spell_component_ids() else {
            return 0;
        };
        tracing::debug!(
            "vendor stock: {:?}",
            vendor
                .items
                .iter()
                .map(|i| (i.desc.name.clone(), i.desc.weenie_class_id, i.stack))
                .collect::<Vec<_>>()
        );
        let orders: Vec<(u32, i32)> = self
            .components()
            .iter()
            .filter(|c| c.desired > c.count)
            .filter_map(|c| {
                let wcid = mapper.component_wcid(c.component_id)?;
                let item = vendor
                    .items
                    .iter()
                    .find(|i| i.desc.weenie_class_id == wcid)?;
                let short = c.desired - c.count;
                // A stock count below the "unlimited" marker caps the order.
                let amount = if item.stack < 0x00FF_FFFF {
                    short.min(item.stack)
                } else {
                    short
                };
                (amount > 0).then_some((item.guid, amount as i32))
            })
            .collect();
        if orders.is_empty() {
            return 0;
        }
        let vendor = vendor.vendor;
        tracing::info!(
            "fill components: {} kinds from {vendor:#010x}",
            orders.len()
        );
        self.session.send_action(
            ac_net::messages::action::BUY,
            &ac_net::messages::trade(vendor, &orders),
        );
        orders.len()
    }
}

/// Scarab component ids (SpellComponentsTable): Lead 1, Iron 2, Copper 3,
/// Silver 4, Gold 5, Pyreal 6, Diamond 110, Platinum 112, Dark 192,
/// Mana 193 (ACE `SpellFormula.Scarab`).
pub fn is_scarab(component: u32) -> bool {
    scarab_power(component) != 0
}

/// A scarab's power, which sets the taper count of the foci formula
/// (ACE `SpellFormula.ScarabPower`); 0 for anything else.
pub fn scarab_power(component: u32) -> u32 {
    match component {
        1..=6 => component, // Lead 1, Iron 2, Copper 3, Silver 4, Gold 5, Pyreal 6
        110 => 7,           // Diamond
        112 => 8,           // Platinum
        192 => 9,           // Dark
        193 => 10,          // Mana
        _ => 0,
    }
}

/// Prismatic tapers the foci formula needs for a scarab power (the
/// client's `CSpellBase::InqScarabOnlyFormula`).
pub fn taper_count(power: u32) -> u32 {
    match power {
        1 => 1,
        2 => 2,
        3 | 4 | 7 => 3,
        5 | 6 | 8 | 9 | 10 => 4,
        _ => 0,
    }
}

/// The formula a focus reduces `full` to: its scarabs (and chorizite) in
/// order, then prismatic tapers by the first component's scarab power.
pub fn foci_formula(full: &[u32]) -> Vec<u32> {
    let mut out: Vec<u32> = full
        .iter()
        .copied()
        .filter(|&c| is_scarab(c) || c == CHORIZITE)
        .collect();
    let tapers = taper_count(scarab_power(full.first().copied().unwrap_or(0)));
    out.extend(std::iter::repeat_n(PRISMATIC_TAPER, tapers as usize));
    out
}

/// What `need` (a formula, repeats counted) lacks given `have(id)`
/// carried: (component id, how many short), in the formula's order.
pub fn missing_components(need: &[u32], have: impl Fn(u32) -> u32) -> Vec<(u32, u32)> {
    let mut wanted: Vec<(u32, u32)> = Vec::new();
    for &c in need {
        match wanted.iter_mut().find(|(id, _)| *id == c) {
            Some(e) => e.1 += 1,
            None => wanted.push((c, 1)),
        }
    }
    wanted
        .into_iter()
        .filter_map(|(id, n)| {
            let short = n.saturating_sub(have(id));
            (short > 0).then_some((id, short))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scarabs_and_powers() {
        assert!(is_scarab(1) && is_scarab(6) && is_scarab(110) && is_scarab(193));
        assert!(!is_scarab(7) && !is_scarab(10) && !is_scarab(63) && !is_scarab(188));
        assert_eq!(scarab_power(6), 6);
        assert_eq!(scarab_power(110), 7);
        assert_eq!(scarab_power(112), 8);
        assert_eq!(scarab_power(192), 9);
        assert_eq!(scarab_power(193), 10);
        assert_eq!(scarab_power(111), 0);
    }

    #[test]
    fn focus_reduces_a_formula_to_scarabs_and_tapers() {
        // Heal Self I (lead scarab): scarab plus one prismatic taper.
        assert_eq!(foci_formula(&[1, 7, 26, 41, 61]), [1, PRISMATIC_TAPER]);
        // A pyreal-scarab spell needs four tapers.
        assert_eq!(
            foci_formula(&[6, 63, 15, 64, 34, 46, 65, 55]),
            [6, 188, 188, 188, 188]
        );
        // Ring spells carry several scarabs; chorizite stays too.
        assert_eq!(
            foci_formula(&[4, 4, 15, 111, 34, 46, 55]),
            [4, 4, 111, 188, 188, 188]
        );
        // Diamond (power 7): three tapers; Mana (power 10): four.
        assert_eq!(foci_formula(&[110, 7, 26, 41, 61]), [110, 188, 188, 188]);
        assert_eq!(foci_formula(&[193, 7, 26, 41, 61]).len(), 5);
        // Not scarab-led: nothing but the tapers of power 0 (none).
        assert_eq!(foci_formula(&[7, 26]), Vec::<u32>::new());
        assert_eq!(foci_formula(&[]), Vec::<u32>::new());
    }

    #[test]
    fn missing_components_arithmetic() {
        let have = |id: u32| match id {
            1 => 3,
            7 => 1,
            188 => 2,
            _ => 0,
        };
        assert!(missing_components(&[1, 7], have).is_empty());
        assert_eq!(
            missing_components(&[1, 7, 26, 41, 61], have),
            [(26, 1), (41, 1), (61, 1)]
        );
        // Repeats count: four tapers wanted, two carried.
        assert_eq!(
            missing_components(&[6, 188, 188, 188, 188], have),
            [(6, 1), (188, 2)]
        );
        assert_eq!(missing_components(&[1, 1, 1, 1], have), [(1, 1)]);
        assert!(missing_components(&[], have).is_empty());
    }

    #[test]
    fn focus_lookup() {
        use ac_formats::spell_table::school;
        assert_eq!(focus_wcid(school::LIFE), Some(15270));
        assert_eq!(focus_wcid(school::VOID), Some(43173));
        assert_eq!(focus_wcid(school::NONE), None);
    }

    /// Offline session over the real archives: no packet is ever sent
    /// (nothing calls `tick`).
    fn offline_client(assets: std::rc::Rc<ac_scene::Assets>) -> Client {
        Client::connect(
            crate::Config {
                host: "127.0.0.1:1".into(),
                account: "acreborn".into(),
                password: "x".into(),
                character: None,
                auto_enter: true,
            },
            assets,
        )
        .unwrap()
    }

    const ME: u32 = 0x5000_0001;

    fn pack_item(c: &mut Client, guid: u32, wcid: u32, stack: u32) {
        c.world.objects.insert(
            guid,
            ac_world::WorldObject {
                guid,
                weenie_class_id: wcid,
                stack_size: stack,
                container: Some(ME),
                parent: Some(ME),
                ..Default::default()
            },
        );
    }

    #[test]
    fn components_and_cast_checks_over_the_archives() {
        let Some(dir) = std::env::var_os("AC_DATA_DIR") else {
            eprintln!("AC_DATA_DIR unset; skipping");
            return;
        };
        let assets = std::rc::Rc::new(ac_scene::Assets::open(dir).unwrap());
        let mapper = assets.spell_component_ids().unwrap();
        let mut c = offline_client(assets);
        c.world.player_guid = Some(ME);
        const HEAL_SELF_I: u32 = 6;
        c.world.stats.spells = vec![HEAL_SELF_I];
        // Heal Self I: lead scarab, hyssop, powder 26, potion 41, talisman
        // 61; five stored components, so no personal tapers.
        assert_eq!(c.current_formula(HEAL_SELF_I), [1, 7, 26, 41, 61]);
        assert!(c.current_formula(999_999).is_empty());
        assert!(c.components().is_empty());
        // Two stacks of lead scarabs and some hyssop.
        pack_item(&mut c, 0x8000_0010, 691, 3);
        pack_item(&mut c, 0x8000_0011, 691, 2);
        pack_item(&mut c, 0x8000_0012, mapper.component_wcid(7).unwrap(), 1);
        pack_item(&mut c, 0x8000_0013, 12345, 1); // not a component
                                                  // Desired quantities are keyed by weenie class, as on the wire.
        c.world.stats.options.desired_comps = vec![(691, 10), (20631, 4), (774, 0)];
        let comps = c.components();
        let by_id = |id: u32| comps.iter().find(|x| x.component_id == id).cloned();
        let lead = by_id(1).unwrap();
        assert_eq!(
            (lead.name.as_str(), lead.wcid, lead.count, lead.desired),
            ("Lead Scarab", 691, 5, 10)
        );
        assert_eq!(by_id(7).unwrap().count, 1);
        // Desired but not carried: listed with a count of 0.
        let taper = by_id(188).unwrap();
        assert_eq!((taper.count, taper.desired, taper.wcid), (0, 4, 20631));
        assert_eq!(comps.len(), 3);
        // Sorted by category then id.
        let table = c.assets.spell_components().unwrap();
        let keys: Vec<(u32, u32)> = comps
            .iter()
            .map(|x| (table.get(x.component_id).unwrap().category, x.component_id))
            .collect();
        assert!(keys.windows(2).all(|w| w[0] <= w[1]), "{keys:?}");

        // Cast checks, in order.
        assert_eq!(c.can_cast(999_999), CastCheck::NotKnown);
        assert_eq!(c.can_cast(HEAL_SELF_I), CastCheck::NoCaster);
        c.world.objects.insert(
            0x8000_0020,
            ac_world::WorldObject {
                guid: 0x8000_0020,
                item_type: ac_world::item_type::CASTER,
                wielder: Some(ME),
                parent: Some(ME),
                ..Default::default()
            },
        );
        assert_eq!(c.wielded_caster(), Some(0x8000_0020));
        assert_eq!(
            c.can_cast(HEAL_SELF_I),
            CastCheck::MissingComponents(vec![(26, 1), (41, 1), (61, 1)])
        );
        // The Foci of Verdancy turns it into scarab + one prismatic taper.
        pack_item(&mut c, 0x8000_0030, 15270, 1);
        assert_eq!(c.current_formula(HEAL_SELF_I), [1, 188]);
        assert_eq!(
            c.can_cast(HEAL_SELF_I),
            CastCheck::MissingComponents(vec![(188, 1)])
        );
        pack_item(&mut c, 0x8000_0031, 20631, 4);
        assert_eq!(
            c.can_cast(HEAL_SELF_I),
            CastCheck::NotEnoughMana { need: 15, have: 0 }
        );
        c.world.stats.vitals[2].current = 15;
        assert_eq!(c.can_cast(HEAL_SELF_I), CastCheck::Ok);
        // No vendor open: nothing to fill.
        assert_eq!(c.fill_components(), 0);
        // A taper stack that got burned down is reflected through the
        // world's stack-size update.
        let mut w = ac_net::wire::Writer::new();
        w.u32(ac_net::messages::opcode::SET_STACK_SIZE)
            .u8(1)
            .u32(0x8000_0031)
            .u32(0)
            .u32(0);
        c.world.apply(&w.finish());
        // An empty stack still counts as one object until it is deleted.
        assert_eq!(
            c.components()
                .iter()
                .find(|x| x.component_id == 188)
                .unwrap()
                .count,
            1
        );
    }
}
