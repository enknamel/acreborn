//! The bridge between the character and the shopping rules.
//!
//! The deciding lives in `ac-vendor`, which knows nothing of sockets,
//! packs or physics: it is handed a description of the moment and
//! answers with one thing to do. This is the half that has to touch the
//! world -- reading the pack into that description, and carrying out
//! whatever comes back.
//!
//! Keeping the two apart is what lets a whole trip be watched at a desk
//! (`cargo run -p ac-vendor --example trip`) and in the autoplay panel,
//! with the same rules running in both.

use std::collections::BTreeMap;

use ac_vendor::counter::{Counter, Item, Keep, Rules, Want, Ware};
use ac_vendor::{Act, Snapshot};

use crate::autoplay::Doing;
use crate::growth::Growth;
use crate::items::ItemStats;
use crate::Client;

impl Client {
    /// The moment as the shopping rules need to see it.
    pub fn vendor_snapshot(&self, cfg: &Growth) -> Snapshot {
        let stats = self.item_stats();
        let (carried, capacity) = self.burden();
        let items = stats.iter().map(|s| self.describe_for_sale(s)).collect();
        let mut notes: BTreeMap<u32, u32> = BTreeMap::new();
        for o in self.world.inventory() {
            if o.item_type & ac_world::item_type::PROMISSORY_NOTE != 0 {
                *notes.entry(o.value / o.stack_size.max(1)).or_insert(0) +=
                    o.stack_size.max(1);
            }
        }
        Snapshot {
            items,
            coin: self.purse(),
            notes,
            carried,
            capacity,
            slots_free: self.free_space(),
            counter: self.counter_now(),
            wants: self.vendor_wants(cfg),
            rules: Rules {
                keep_slots: self.autoplay.config.team.restock.keep_slots,
                float: ac_vendor::errand::FLOAT,
                floor_value: 0,
                reach: crate::growth::COUNTER_REACH,
            },
        }
    }

    /// One carried thing, and the reasons it may not be sold.
    ///
    /// The five refusals are settled and no profile may override them:
    /// tinkered or inscribed is somebody's work, equipped is being
    /// worn, and the last two are the server's own word.
    fn describe_for_sale(&self, s: &ItemStats) -> Item {
        let holds_anything = self
            .world
            .objects
            .values()
            .any(|o| o.container == Some(s.guid));
        Item {
            guid: s.guid,
            wcid: s.wcid,
            name: s.name.clone(),
            value: s.value,
            burden: s.burden,
            stack: s.stack.max(1),
            max_stack: s.max_stack,
            item_type: s.item_type,
            pack: (s.container != 0).then_some(s.container),
            wielded: s.wielded,
            keep: Keep {
                tinkered: s.tinks > 0,
                inscribed: s.inscribed,
                equipped: s.wielded,
                retained: false,
                unsellable: s.unsellable
                    || s.value == 0
                    || (s.item_type & ac_world::item_type::CONTAINER != 0 && holds_anything),
            },
        }
    }

    /// The counter in front of us, if its window is open.
    fn counter_now(&self) -> Option<Counter> {
        let v = self.world.open_vendor.as_ref()?;
        let name = self
            .world
            .objects
            .get(&v.vendor)
            .map(|o| o.name.clone())
            .unwrap_or_default();
        let away = match (
            self.player.as_ref().map(|p| p.world_position()),
            self.world.objects.get(&v.vendor).and_then(|o| o.world_pos()),
        ) {
            (Some(me), Some(at)) => glam::Vec2::new(at.x - me.x, at.y - me.y).length(),
            _ => 0.0,
        };
        Some(Counter {
            guid: v.vendor,
            name,
            open: true,
            buys: v.item_types,
            max_value: v.max_value,
            wares: v
                .items
                .iter()
                .map(|w| Ware {
                    wcid: w.desc.weenie_class_id,
                    name: w.desc.name.clone(),
                    // What it costs here, not what it is worth: a
                    // counter sells above an item's own value.
                    price: (w.desc.value as f32 * v.sell_rate).ceil() as u32,
                    // A stack of -1 on the wire is a shelf that never
                    // empties, which arrives here as a very large
                    // number rather than as a count.
                    stock: (w.stack != u32::MAX && w.stack != 0).then_some(w.stack),
                    burden: w.desc.burden,
                })
                .collect(),
            // Which note this counter deals in: whichever of the
            // trade notes is actually on its shelf.
            note_face: v
                .items
                .iter()
                .filter_map(|w| ac_world::shops::note_face(w.desc.weenie_class_id))
                .max(),
            away,
        })
    }

    /// What the character came to buy.
    ///
    /// A line nobody sells is left out: the Void components and, in
    /// practice, the Diamond Scarab, whose one seller is a curiosity
    /// shop. A want like that would hold a trip open for ever.
    fn vendor_wants(&self, cfg: &Growth) -> Vec<Want> {
        self.vendor_shortfall(cfg)
    }

    /// Do the one thing the rules asked for. `false` when the act was
    /// refused before it left this machine, which the rules read as a
    /// refusal rather than as silence.
    pub fn do_vendor_act(&mut self, act: &Act, saying: &str) -> bool {
        match act {
            Act::Approach { guid } => {
                let Some(at) = self.world.objects.get(guid).and_then(|o| o.world_pos()) else {
                    return false;
                };
                self.follow = Some(crate::Follow {
                    target: at,
                    stop: crate::growth::COUNTER_REACH / 2.0,
                });
                self.autoplay.say(Doing::Shopping, saying.to_string());
                true
            }
            Act::Open { guid } => {
                if self.follow.take().is_some() {
                    self.steering.reset();
                }
                self.use_object(*guid);
                self.autoplay.say(Doing::Shopping, saying.to_string());
                true
            }
            Act::Merge { from, to, amount } => {
                let sent = self.merge_stacks(*from, *to, Some(*amount));
                if sent {
                    self.autoplay.say(Doing::Tidying, saying.to_string());
                }
                sent
            }
            Act::Split { guid, amount } => {
                let sent = self.split_stack(*guid, None, *amount);
                if sent {
                    self.autoplay.say(Doing::Shopping, saying.to_string());
                }
                sent
            }
            Act::Sell { guid } => {
                self.sell(*guid);
                self.autoplay.say(Doing::Shopping, saying.to_string());
                true
            }
            Act::Buy { wcid, count } => {
                // The shelf is addressed by the guid of the thing
                // standing on it, which only this side knows.
                let line = self
                    .world
                    .open_vendor
                    .as_ref()
                    .and_then(|v| {
                        v.items
                            .iter()
                            .find(|w| w.desc.weenie_class_id == *wcid)
                            .map(|w| w.guid)
                    });
                match line {
                    Some(line) => {
                        self.buy_amount(line, *count);
                        self.autoplay.say(Doing::Shopping, saying.to_string());
                        true
                    }
                    None => false,
                }
            }
            Act::Cash { face, count } => {
                // A note is cashed by selling it back, which is what a
                // player does: the counter pays its face value.
                let note = self
                    .world
                    .inventory()
                    .find(|o| {
                        o.item_type & ac_world::item_type::PROMISSORY_NOTE != 0
                            && o.value / o.stack_size.max(1) == *face
                    })
                    .map(|o| (o.guid, o.stack_size.max(1)));
                match note {
                    Some((guid, have)) if have <= *count => {
                        self.sell(guid);
                        self.autoplay.say(Doing::Shopping, saying.to_string());
                        true
                    }
                    // More than the bill needs: cut off what it needs
                    // and sell that, rather than a fortune.
                    Some((guid, _)) => {
                        let sent = self.split_stack(guid, None, *count);
                        if sent {
                            self.autoplay.say(Doing::Shopping, saying.to_string());
                        }
                        sent
                    }
                    None => false,
                }
            }
            Act::Close => {
                self.close_vendor();
                true
            }
        }
    }
}
