//! [`Api`] over `ac_plugin::Ctx`: the real client behind the script
//! functions. Each action mirrors what the console plugin does for the
//! same `/command`, so scripts and typed commands behave alike.

use ac_plugin::{Client, Ctx, Message};
use ac_world::{item_type, object_desc_flags, WorldObject};
use rhai::{Array, Dynamic, Map};
use serde_json::Value;

use crate::api::Api;

pub struct CtxApi<'c, 'a> {
    pub cx: &'c mut Ctx<'a>,
}

fn int(v: impl Into<i64>) -> Dynamic {
    Dynamic::from_int(v.into())
}

fn float(v: f32) -> Dynamic {
    Dynamic::from_float(v as f64)
}

fn opt_guid(g: Option<u32>) -> Dynamic {
    g.map_or(Dynamic::UNIT, int)
}

/// A spellbook spell by name prefix, or one learnt from a scroll this
/// session.
fn spell_by_name(c: &Client, name: &str) -> Option<u32> {
    let table = c.assets.spell_table().ok();
    c.world
        .stats
        .spells
        .iter()
        .copied()
        .find(|id| {
            table
                .as_ref()
                .and_then(|t| t.get(*id))
                .is_some_and(|sp| sp.name.starts_with(name))
        })
        .or_else(|| {
            c.known_spells
                .iter()
                .find(|(_, n)| n.starts_with(name))
                .map(|(id, _)| *id)
        })
}

fn player_position(c: &Client) -> Option<[f32; 3]> {
    let p = match c.player.as_ref() {
        Some(p) => p.world_position(),
        None => c.world.player()?.world_pos()?,
    };
    Some([p.x, p.y, p.z])
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn object_map(o: &WorldObject, me: Option<[f32; 3]>, carried: bool) -> Map {
    let pos = o.world_pos().map(|p| [p.x, p.y, p.z]);
    let dist = match (pos, me) {
        _ if carried => 0.0,
        (Some(p), Some(m)) => distance(p, m),
        _ => f32::INFINITY,
    };
    let mut map = Map::new();
    map.insert("guid".into(), int(o.guid));
    map.insert("name".into(), o.name.clone().into());
    map.insert("distance".into(), float(dist));
    map.insert(
        "is_creature".into(),
        (o.item_type & item_type::CREATURE != 0).into(),
    );
    map.insert("is_player".into(), o.is_player.into());
    map.insert(
        "is_corpse".into(),
        (o.object_desc_flags & object_desc_flags::CORPSE != 0).into(),
    );
    map.insert("health".into(), o.health.map_or(Dynamic::UNIT, float));
    let p = pos.unwrap_or([0.0; 3]);
    map.insert("x".into(), float(p[0]));
    map.insert("y".into(), float(p[1]));
    map.insert("z".into(), float(p[2]));
    map.insert("cell".into(), int(o.position.map(|p| p.cell).unwrap_or(0)));
    map.insert("stack".into(), int(o.stack_size));
    map.insert("value".into(), int(o.value));
    map
}

fn summary(c: &Client, index: usize) -> Map {
    let stats = &c.world.stats;
    let mut map = Map::new();
    map.insert("session".into(), int(index as i64));
    map.insert("guid".into(), opt_guid(c.world.player_guid));
    let name = if stats.name.is_empty() {
        c.world.player().map(|o| o.name.clone()).unwrap_or_default()
    } else {
        stats.name.clone()
    };
    map.insert("name".into(), name.into());
    map.insert("level".into(), int(stats.level));
    map.insert("total_xp".into(), int(stats.total_xp));
    map.insert("available_xp".into(), int(stats.available_xp));
    map.insert("skill_credits".into(), int(stats.skill_credits));
    for (i, vital) in ["health", "stamina", "mana"].into_iter().enumerate() {
        map.insert(vital.into(), int(stats.vitals[i].current));
        map.insert(format!("{vital}_max").into(), int(stats.vital_max(i)));
    }
    let p = player_position(c).unwrap_or([0.0; 3]);
    map.insert("x".into(), float(p[0]));
    map.insert("y".into(), float(p[1]));
    map.insert("z".into(), float(p[2]));
    let cell = c
        .player
        .as_ref()
        .map(|p| p.cell)
        .or_else(|| c.world.player()?.position.map(|p| p.cell))
        .unwrap_or(0);
    map.insert("cell".into(), int(cell));
    map.insert("combat".into(), c.combat.into());
    map.insert("magic".into(), c.magic.into());
    map.insert("target".into(), opt_guid(c.attack_target));
    map.insert("target_name".into(), c.last_target_name.clone().into());
    map.insert("selected".into(), opt_guid(c.selected));
    map.insert("placed".into(), c.placed().into());
    map.insert("vendor_open".into(), c.world.open_vendor.is_some().into());
    map.insert(
        "container_open".into(),
        c.world.open_container.is_some().into(),
    );
    map
}

impl CtxApi<'_, '_> {
    fn client(&mut self) -> &mut Client {
        self.cx.client()
    }

    fn set_combat(&mut self, on: bool) {
        let c = self.client();
        if c.combat != on {
            c.toggle_combat();
        }
    }
}

impl Api for CtxApi<'_, '_> {
    fn me(&mut self) -> Map {
        let index = self.cx.index;
        summary(self.client(), index)
    }

    fn objects(&mut self) -> Array {
        let c = self.client();
        let me = player_position(c);
        let mine = c.world.player_guid;
        let mut objs: Vec<(f32, Map)> = c
            .world
            .drawable()
            .filter(|o| Some(o.guid) != mine)
            .map(|o| {
                let m = object_map(o, me, false);
                let d = m.get("distance").and_then(|d| d.as_float().ok());
                (d.unwrap_or(f64::INFINITY) as f32, m)
            })
            .collect();
        objs.sort_by(|a, b| a.0.total_cmp(&b.0));
        objs.into_iter()
            .map(|(_, m)| Dynamic::from_map(m))
            .collect()
    }

    fn inventory(&mut self) -> Array {
        let c = self.client();
        c.world
            .inventory()
            .map(|o| Dynamic::from_map(object_map(o, None, true)))
            .collect()
    }

    fn container(&mut self) -> Array {
        let c = self.client();
        let Some((_, items)) = c.world.open_container.as_ref() else {
            return Array::new();
        };
        items
            .iter()
            .filter_map(|g| c.world.objects.get(g))
            .map(|o| Dynamic::from_map(object_map(o, None, true)))
            .collect()
    }

    fn session_count(&mut self) -> i64 {
        self.cx.client_count() as i64
    }

    fn session(&mut self, i: i64) -> Option<Map> {
        let i = usize::try_from(i).ok()?;
        let c = self.cx.clients.get(i)?;
        Some(summary(c, i))
    }

    fn current_session(&mut self) -> i64 {
        self.cx.index as i64
    }

    fn set_session(&mut self, i: i64) -> bool {
        match usize::try_from(i) {
            Ok(i) if i < self.cx.client_count() => {
                self.cx.index = i;
                true
            }
            _ => false,
        }
    }

    fn use_name(&mut self, name: &str) -> bool {
        self.client().use_by_name(name)
    }

    fn use_guid(&mut self, guid: i64) -> bool {
        let Ok(guid) = u32::try_from(guid) else {
            return false;
        };
        let c = self.client();
        if !c.world.objects.contains_key(&guid) {
            return false;
        }
        c.selected = Some(guid);
        c.interact(guid);
        true
    }

    fn attack(&mut self, name: &str) -> bool {
        self.set_combat(true);
        self.client().use_by_name(name)
    }

    fn attack_guid(&mut self, guid: i64) -> bool {
        let Ok(guid) = u32::try_from(guid) else {
            return false;
        };
        if !self.client().world.objects.contains_key(&guid) {
            return false;
        }
        self.set_combat(true);
        let c = self.client();
        c.selected = Some(guid);
        c.interact(guid);
        true
    }

    fn cast(&mut self, name: &str) -> bool {
        let c = self.client();
        let id = spell_by_name(c, name);
        match id {
            Some(id) => {
                c.cast(id);
                true
            }
            None => false,
        }
    }

    fn say(&mut self, text: &str) {
        self.client().say(text);
    }

    fn loot(&mut self, name: &str) -> bool {
        let corpse = if name.is_empty() {
            format!("Corpse of {}", self.client().last_target_name)
        } else {
            name.to_string()
        };
        self.set_combat(false);
        self.client().use_by_name(&corpse)
    }

    fn take(&mut self, guid: i64) -> bool {
        let Ok(guid) = u32::try_from(guid) else {
            return false;
        };
        let c = self.client();
        let held = c
            .world
            .open_container
            .as_ref()
            .is_some_and(|(_, items)| items.contains(&guid));
        if held {
            c.take(guid);
        }
        held
    }

    fn raise(&mut self, what: &str) -> bool {
        use ac_plugin::ac_client::advance::{ATTRIBUTE_NAMES, VITAL_NAMES};
        let c = self.client();
        let w = what.trim().to_lowercase();
        if let Some(i) = ATTRIBUTE_NAMES.iter().position(|n| n.to_lowercase() == w) {
            return c.raise_attribute(i);
        }
        if let Some(i) = VITAL_NAMES.iter().position(|n| n.to_lowercase() == w) {
            return c.raise_vital(i);
        }
        match skill_by_name(c, what) {
            Some(id) => c.raise_skill(id),
            None => false,
        }
    }

    fn train(&mut self, skill: &str) -> bool {
        let c = self.client();
        match skill_by_name(c, skill) {
            Some(id) => c.train_skill(id),
            None => false,
        }
    }

    fn trade_open(&mut self, player: i64) {
        self.client().open_trade(player as u32);
    }

    fn trade_add(&mut self, item: i64) -> bool {
        self.client().add_to_trade(item as u32)
    }

    fn trade_accept(&mut self) {
        self.client().accept_trade();
    }

    fn trade_decline(&mut self) {
        self.client().decline_trade();
    }

    fn trade_reset(&mut self) {
        self.client().reset_trade();
    }

    fn trade_close(&mut self) {
        self.client().close_trade();
    }

    fn trade(&mut self) -> Map {
        let c = self.client();
        let mut m = Map::new();
        match c.world.trade.as_ref() {
            Some(t) => {
                m.insert("open".into(), true.into());
                m.insert("partner".into(), int(t.partner));
                m.insert(
                    "mine".into(),
                    t.mine.iter().map(|&g| int(g)).collect::<Array>().into(),
                );
                m.insert(
                    "theirs".into(),
                    t.theirs.iter().map(|&g| int(g)).collect::<Array>().into(),
                );
                m.insert("i_accepted".into(), t.i_accepted.into());
                m.insert("they_accepted".into(), t.they_accepted.into());
            }
            None => {
                m.insert("open".into(), false.into());
            }
        }
        m
    }

    fn drop_item(&mut self, g: i64) -> bool {
        self.client().drop_item(g as u32)
    }

    fn give(&mut self, target: i64, item: i64, amount: i64) -> bool {
        let amount = (amount > 0).then_some(amount as u32);
        self.client().give(target as u32, item as u32, amount)
    }

    fn put_in(&mut self, item: i64, container: i64) -> bool {
        let c = self.client();
        let container = if container == 0 {
            c.world.player_guid.unwrap_or(0)
        } else {
            container as u32
        };
        c.put_in_container(item as u32, container)
    }

    fn take_all(&mut self) -> i64 {
        let c = self.client();
        let items: Vec<u32> = c
            .world
            .open_container
            .as_ref()
            .map(|(_, items)| items.clone())
            .unwrap_or_default();
        for g in &items {
            c.take(*g);
        }
        items.len() as i64
    }

    fn close_container(&mut self) {
        self.client().close_container();
    }

    fn can_cast(&mut self, name: &str) -> String {
        let c = self.client();
        let Some(id) = spell_by_name(c, name) else {
            return "not_known".into();
        };
        use ac_plugin::ac_client::magic::CastCheck;
        match c.can_cast(id) {
            CastCheck::Ok => "ok",
            CastCheck::NotKnown => "not_known",
            CastCheck::NoCaster => "no_caster",
            CastCheck::MissingComponents(_) => "missing_components",
            CastCheck::NotEnoughMana { .. } => "not_enough_mana",
        }
        .into()
    }

    fn components(&mut self) -> Array {
        self.client()
            .components()
            .into_iter()
            .map(|c| {
                let mut m = Map::new();
                m.insert("id".into(), int(c.component_id));
                m.insert("name".into(), c.name.into());
                m.insert("wcid".into(), int(c.wcid));
                m.insert("count".into(), int(c.count));
                m.insert("desired".into(), int(c.desired));
                Dynamic::from_map(m)
            })
            .collect()
    }

    fn fill_components(&mut self) -> i64 {
        self.client().fill_components() as i64
    }

    /// Set the desired quantity of a component by (prefix of) its name;
    /// false when no such component is in the table.
    fn set_desired_component(&mut self, name: &str, quantity: i64) -> bool {
        let c = self.client();
        let Some(id) = c
            .assets
            .spell_components()
            .ok()
            .and_then(|t| t.find_by_name(name))
        else {
            return false;
        };
        c.set_desired_component(id, quantity.max(0) as u32);
        true
    }

    fn buy(&mut self, name: &str) -> bool {
        let c = self.client();
        let guid = c
            .world
            .open_vendor
            .as_ref()
            .and_then(|v| v.items.iter().find(|i| i.desc.name.starts_with(name)))
            .map(|i| i.guid);
        match guid {
            Some(g) => {
                c.buy(g);
                true
            }
            None => false,
        }
    }

    fn sell(&mut self, name: &str) -> bool {
        let c = self.client();
        if c.world.open_vendor.is_none() {
            return false;
        }
        let guid = c
            .world
            .inventory()
            .find(|o| o.name.starts_with(name))
            .map(|o| o.guid);
        match guid {
            Some(g) => {
                c.sell(g);
                true
            }
            None => false,
        }
    }

    fn combat(&mut self, on: bool) {
        self.set_combat(on);
    }

    fn select(&mut self, guid: i64) {
        let guid = u32::try_from(guid).ok().filter(|g| *g != 0);
        self.client().select(guid);
    }

    fn log(&mut self, text: &str) {
        self.cx.log(text);
    }

    fn post(&mut self, topic: &str, value: Value) {
        self.cx.post(topic, value);
    }

    fn messages(&mut self, topic: &str) -> Vec<Message> {
        self.cx.board.messages_on(topic).cloned().collect()
    }

    fn board_get(&mut self, key: &str) -> Option<Value> {
        self.cx.board.get(key).cloned()
    }

    fn board_set(&mut self, key: &str, value: Value) {
        self.cx.board.set(key, value);
    }

    fn switch(&mut self, i: i64) {
        if let Ok(i) = usize::try_from(i) {
            if i < self.cx.client_count() {
                self.cx.activate = Some(i);
            }
        }
    }
}

/// A skill id from (a prefix of) its name, case-insensitive.
fn skill_by_name(c: &ac_plugin::ac_client::Client, name: &str) -> Option<u32> {
    let want = name.trim().to_lowercase();
    if want.is_empty() {
        return None;
    }
    // Every skill in the table, not just the ones on the sheet.
    let ids: Vec<u32> = match c.assets.skill_table() {
        Ok(t) => t.skills.iter().map(|(id, _)| *id).collect(),
        Err(_) => c.world.stats.skills.iter().map(|s| s.id).collect(),
    };
    ids.iter()
        .copied()
        .find(|&id| ac_world::stats::skill_name(id).to_lowercase() == want)
        .or_else(|| {
            ids.iter().copied().find(|&id| {
                ac_world::stats::skill_name(id)
                    .to_lowercase()
                    .starts_with(&want)
            })
        })
}
