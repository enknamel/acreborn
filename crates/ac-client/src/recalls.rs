//! Recall spells from the character's side: which of them it can cast
//! right now and where each would land, for the trip planner
//! (`ac_world::trip::plan_with_recalls`), and the positions those spells
//! go to.
//!
//! A named recall (Aerlinthe Recall, Ulgrim's Recall...) goes to a fixed
//! place the world data knows (`ac_world::recalls::fixed`). Lifestone
//! Sending, Lifestone Recall, Portal Recall and the two Portal Recalls
//! go to positions the server saves for the character and never sends,
//! so they are learnt here from what the character does: using a
//! lifestone (the server says the spirit is attuned), tying one or a
//! portal (it says the link succeeded), walking through a portal (the
//! journey notices). Until one of those has happened in this session the
//! spell's destination is unknown and the planner leaves it alone. A
//! recall the server refuses forgets the position it was aimed at.

use ac_world::object::Position;
use ac_world::recalls::{self, position_type};
use ac_world::trip::Recall;
use glam::Vec3;

use crate::magic::CastCheck;
use crate::Client;

/// A lifestone or portal this close to the character is the one it
/// just used or tied (the server's use radius is about as much).
const NEXT_TO: f32 = 12.0;

impl Client {
    /// The recall spells the character can cast right now (known, with
    /// a caster in hand, the components and the mana) and where each
    /// would land; the trip planner's input. A spell whose destination
    /// is not known yet is left out.
    pub fn castable_recalls(&self) -> Vec<Recall> {
        let mut out = Vec::new();
        for &id in &self.world.stats.spells {
            let (name, exit, exit_cell) = if let Some(kind) = recalls::dynamic(id) {
                let Some(p) = self.world.stats.recall_position(kind) else {
                    continue;
                };
                let at = ac_world::landblock_origin(p.cell) + p.local;
                (self.recall_name(id), at, p.cell)
            } else if let Some(r) = recalls::fixed(id) {
                (r.name.clone(), r.at, r.cell)
            } else {
                continue;
            };
            if !matches!(self.can_cast(id), CastCheck::Ok) {
                continue;
            }
            out.push(Recall {
                spell: id,
                name,
                exit: exit.truncate(),
                exit_cell,
            });
        }
        out
    }

    /// The portal gems in the pack, as ways to get somewhere.
    ///
    /// A gem needs no skill, no components and no mana: carrying it is
    /// the whole requirement, which makes it the cheapest hop a
    /// character has and often the only one to a place with no portal
    /// near it. It is spent when used, so a journey uses each once.
    pub fn carried_gems(&self) -> Vec<ac_world::trip::Gem> {
        self.world
            .inventory()
            .filter_map(|o| {
                let g = ac_world::gems::of(o.weenie_class_id)?;
                Some(ac_world::trip::Gem {
                    guid: o.guid,
                    name: g.name.clone(),
                    exit: g.xy(),
                    exit_cell: g.cell,
                    summons: g.how() == ac_world::gems::Use::Summons,
                })
            })
            .collect()
    }

    /// Where a recall spell would land the character, if known: for a
    /// map, or a script deciding whether to cast it.
    pub fn recall_destination(&self, spell: u32) -> Option<Position> {
        if let Some(kind) = recalls::dynamic(spell) {
            return self.world.stats.recall_position(kind);
        }
        let r = recalls::fixed(spell)?;
        Some(Position::new_flat(
            r.cell,
            r.at - ac_world::landblock_origin(r.cell),
        ))
    }

    fn recall_name(&self, spell: u32) -> String {
        self.spell(spell)
            .map(|s| s.name)
            .or_else(|| self.known_spells.get(&spell).cloned())
            .unwrap_or_else(|| format!("spell {spell}"))
    }

    /// Where the character stands, as a saved position.
    fn standing(&self) -> Option<Position> {
        let pl = self.player.as_ref()?;
        Some(Position::new_flat(pl.cell, pl.local))
    }

    /// Record a saved position the client has worked out.
    pub fn learn_recall_position(&mut self, kind: u32, p: Position) {
        tracing::info!(
            "recall: position {kind} is now {:#010x} {:?}",
            p.cell,
            p.local
        );
        self.world.stats.set_recall_position(kind, p);
    }

    /// A spell was sent: remember a tie, so the "successfully linked"
    /// that follows is filed under the right saved position.
    pub(crate) fn note_cast(&mut self, spell: u32) {
        if recalls::tie_sets(spell).is_some() {
            self.travel.last_tie = Some(spell);
        }
    }

    /// A line of chat from the server: the ones that mean a saved
    /// position changed. Using a lifestone attunes the spirit to it;
    /// Lifestone Tie and the Portal Ties say the link succeeded.
    pub(crate) fn recall_notice(&mut self, text: &str) {
        let lower = text.to_lowercase();
        if lower.contains("attuned your spirit") || lower.contains("resurrection point") {
            // Only next to a lifestone: the words alone could be anything.
            let Some(here) = self.standing() else {
                return;
            };
            let at = (ac_world::landblock_origin(here.cell) + here.local).truncate();
            let near = ac_world::landmarks::nearest_lifestone(at)
                .is_some_and(|l| l.xy().distance(at) <= NEXT_TO * 2.0);
            if near || self.nearest_of(ac_world::item_type::LIFESTONE).is_some() {
                self.learn_recall_position(position_type::SANCTUARY, here);
            }
        } else if lower.contains("successfully linked with the life stone") {
            if let Some(here) = self.standing() {
                self.learn_recall_position(position_type::LINKED_LIFESTONE, here);
            }
            self.travel.last_tie = None;
        } else if lower.contains("successfully linked with the portal") {
            let Some(kind) = self.travel.last_tie.take().and_then(recalls::tie_sets) else {
                return;
            };
            // The portal just tied is the one beside us; the recall
            // lands where it leads.
            let Some(mouth) = self.nearest_of(ac_world::item_type::PORTAL) else {
                tracing::info!("recall: linked with a portal, but none is in view");
                return;
            };
            let exit = ac_world::portals::near(mouth.truncate(), NEXT_TO)
                .first()
                .map(|p| (p.to_cell, p.to));
            match exit {
                Some((cell, to)) => {
                    let p = Position::new_flat(cell, to - ac_world::landblock_origin(cell));
                    self.learn_recall_position(kind, p);
                }
                None => tracing::info!("recall: the portal tied at {mouth:?} is not in the table"),
            }
        }
    }

    /// The server refused something: a recall aimed at a position it
    /// does not have, or a fizzle. The journey handles the retry; here
    /// the position the spell was aimed at is forgotten, since the
    /// server says there is nothing there.
    pub(crate) fn recall_error(&mut self, code: u32) {
        let kind = match code {
            // YouMustLinkToLifestoneToRecall
            0x049E => Some(position_type::LINKED_LIFESTONE),
            // YouMustLinkToPortalToRecall, YouCannotRecallPortal: which
            // portal recall it was is whatever the journey is casting.
            0x04A3 | 0x04AD => self.travel_recall_spell().and_then(recalls::dynamic),
            _ => None,
        };
        if let Some(kind) = kind {
            tracing::info!("recall: the server has no position {kind}; forgetting it");
            self.world.stats.clear_recall_position(kind);
            self.travel_recall_refused();
        }
        // YourSpellFizzled: try again straight away rather than wait.
        if code == 0x0402 {
            self.travel_recall_fizzled();
        }
        self.loot_refused(code);
    }

    /// The world position of the nearest object of an item type in
    /// view, within [`NEXT_TO`] of the character.
    fn nearest_of(&self, item_type: u32) -> Option<Vec3> {
        let me = self.player.as_ref()?.world_position();
        self.world
            .objects
            .values()
            .filter(|o| o.item_type & item_type != 0)
            .filter_map(|o| {
                o.position
                    .map(|p| ac_world::landblock_origin(p.cell) + p.local)
            })
            .filter(|p| p.truncate().distance(me.truncate()) <= NEXT_TO)
            .min_by(|a, b| {
                a.truncate()
                    .distance(me.truncate())
                    .total_cmp(&b.truncate().distance(me.truncate()))
            })
    }
}
