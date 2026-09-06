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

fn payments(list: &[ac_world::housing::Payment]) -> Array {
    list.iter()
        .map(|p| {
            let mut m = Map::new();
            m.insert("name".into(), p.name.clone().into());
            m.insert("wcid".into(), int(p.wcid));
            m.insert("needed".into(), int(p.needed));
            m.insert("paid".into(), int(p.paid));
            Dynamic::from_map(m)
        })
        .collect()
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
    // A player character (ours is never listed by objects()), not
    // `WorldObject::is_player`, which marks our own body.
    map.insert(
        "is_player".into(),
        (o.object_desc_flags & object_desc_flags::PLAYER != 0).into(),
    );
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
    map.insert(
        "material".into(),
        if o.material == 0 {
            Dynamic::UNIT
        } else {
            ac_world::material::name(o.material).into()
        },
    );
    map.insert("workmanship".into(), float(o.workmanship));
    map.insert("structure".into(), int(o.structure));
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
    // Position within the landblock, the numbers @loc and @teleloc use.
    let local = c.player.as_ref().map(|p| p.local).or_else(|| {
        let p = c.world.player()?.position?;
        Some(p.local)
    });
    let l = local.unwrap_or_default();
    map.insert("local_x".into(), float(l.x));
    map.insert("local_y".into(), float(l.y));
    map.insert("local_z".into(), float(l.z));
    // Vitae penalty in percent (5 for 95% vitae), 0 without one.
    let vitae = c
        .enchantments()
        .iter()
        .find(|e| e.spell_id == 666)
        .map(|e| ((1.0 - e.stat_mod_value) * 100.0).round() as i64)
        .unwrap_or(0);
    map.insert("vitae".into(), int(vitae));
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

    fn fellow_create(&mut self, name: &str, share_xp: bool) {
        self.client().fellowship_create(name, share_xp);
    }

    fn fellow_recruit(&mut self, player: i64) {
        self.client().fellowship_recruit(player as u32);
    }

    fn fellow_quit(&mut self, disband: bool) {
        self.client().fellowship_quit(disband);
    }

    fn swear(&mut self, patron: i64) -> bool {
        self.client().swear_allegiance(patron as u32)
    }

    fn break_allegiance(&mut self, member: i64) -> bool {
        self.client().break_allegiance(member as u32)
    }

    fn house_profile(&mut self) -> Dynamic {
        let c = self.client();
        let Some(p) = c.world.house_profile.as_ref() else {
            return Dynamic::UNIT;
        };
        let mut m = Map::new();
        m.insert("slumlord".into(), int(p.slumlord));
        m.insert("owner".into(), int(p.owner));
        m.insert("owner_name".into(), p.owner_name.clone().into());
        m.insert("kind".into(), ac_world::housing::kind_name(p.kind).into());
        m.insert("min_level".into(), int(p.min_level));
        m.insert("buy".into(), payments(&p.buy).into());
        m.insert("rent".into(), payments(&p.rent).into());
        Dynamic::from_map(m)
    }

    fn house(&mut self) -> Dynamic {
        let c = self.client();
        let Some(Some(h)) = c.world.house.as_ref() else {
            return Dynamic::UNIT;
        };
        let mut m = Map::new();
        m.insert("kind".into(), ac_world::housing::kind_name(h.kind).into());
        m.insert("cell".into(), int(h.position.map(|p| p.cell).unwrap_or(0)));
        m.insert("rent_paid".into(), h.rent_paid().into());
        m.insert("rent".into(), payments(&h.rent).into());
        Dynamic::from_map(m)
    }

    fn house_query(&mut self) {
        self.client().house_query();
    }

    fn buy_house(&mut self) -> bool {
        self.client().buy_house()
    }

    fn rent_house(&mut self) -> bool {
        self.client().rent_house()
    }

    fn abandon_house(&mut self) {
        self.client().abandon_house();
    }

    fn house_guests(&mut self) -> Dynamic {
        let c = self.client();
        let Some(a) = c.world.house_access.as_ref() else {
            return Dynamic::UNIT;
        };
        let mut m = Map::new();
        m.insert("open".into(), a.open.into());
        m.insert("allegiance_guests".into(), a.allegiance_guests.into());
        m.insert("allegiance_storage".into(), a.allegiance_storage.into());
        let guests: Array = a
            .guests
            .iter()
            .map(|g| {
                let mut gm = Map::new();
                gm.insert("guid".into(), int(g.guid));
                gm.insert("name".into(), g.name.clone().into());
                gm.insert("storage".into(), g.storage.into());
                Dynamic::from_map(gm)
            })
            .collect();
        m.insert("guests".into(), guests.into());
        Dynamic::from_map(m)
    }

    fn house_guest(&mut self, name: &str, add: bool) {
        self.client().house_guest(name, add);
    }

    fn house_storage(&mut self, name: &str, allow: bool) {
        self.client().house_storage(name, allow);
    }

    fn house_open(&mut self, open: bool) {
        self.client().house_open(open);
    }

    fn allegiance(&mut self) -> Dynamic {
        let c = self.client();
        let Some(a) = c.world.allegiance.as_ref().filter(|a| a.is_member()) else {
            return Dynamic::UNIT;
        };
        let member = |x: &ac_world::allegiance::Member| {
            let mut mm = Map::new();
            mm.insert("guid".into(), int(x.guid));
            mm.insert("name".into(), x.name.clone().into());
            mm.insert("level".into(), int(x.level));
            mm.insert("rank".into(), int(x.rank));
            mm.insert("loyalty".into(), int(x.loyalty));
            mm.insert("leadership".into(), int(x.leadership));
            mm.insert("online".into(), x.online.into());
            mm.insert("xp_cached".into(), int(x.xp_cached as i64));
            mm.insert("xp_tithed".into(), int(x.xp_tithed as i64));
            Dynamic::from_map(mm)
        };
        let opt = |x: Option<&ac_world::allegiance::Member>| x.map(member).unwrap_or(Dynamic::UNIT);
        let mut m = Map::new();
        m.insert("name".into(), a.name.clone().into());
        m.insert("rank".into(), int(a.rank));
        m.insert("total_members".into(), int(a.total_members));
        m.insert("total_vassals".into(), int(a.total_vassals));
        m.insert("motd".into(), a.motd.clone().into());
        m.insert("monarch".into(), opt(a.monarch.as_ref()));
        m.insert("patron".into(), opt(a.patron.as_ref()));
        m.insert("me".into(), opt(a.me.as_ref()));
        let vassals: Array = a.vassals.iter().map(member).collect();
        m.insert("vassals".into(), vassals.into());
        Dynamic::from_map(m)
    }

    fn salvageable(&mut self) -> Array {
        let c = self.client();
        c.salvageable()
            .into_iter()
            .filter_map(|g| c.world.objects.get(&g))
            .map(|o| Dynamic::from_map(object_map(o, None, true)))
            .collect()
    }

    fn salvage(&mut self, items: Array) -> bool {
        let guids: Vec<u32> = items
            .into_iter()
            .filter_map(|d| d.as_int().ok())
            .map(|g| g as u32)
            .collect();
        self.client().salvage(&guids)
    }

    fn allegiance_refresh(&mut self) {
        self.client().allegiance_update_request(true);
    }

    fn allegiance_name(&mut self, name: &str) {
        self.client().set_allegiance_name(name);
    }

    fn chat(&mut self, channel: &str, text: &str) -> bool {
        if let Some(room) = ac_net::messages::turbine::from_prefix(channel) {
            return self.client().turbine_say(room, text);
        }
        let Some(id) = ac_net::messages::channel::from_prefix(channel) else {
            return false;
        };
        self.client().chat_channel(id, text);
        true
    }

    fn confirmations(&mut self) -> Array {
        self.client()
            .world
            .confirmations
            .iter()
            .map(|q| {
                let mut m = Map::new();
                m.insert("kind".into(), int(q.kind));
                m.insert("context".into(), int(q.context));
                m.insert("text".into(), q.text.clone().into());
                Dynamic::from_map(m)
            })
            .collect()
    }

    fn confirm(&mut self, yes: bool) -> bool {
        let c = self.client();
        let Some(q) = c.world.confirmations.first().cloned() else {
            return false;
        };
        c.confirm(q.kind, q.context, yes);
        true
    }

    fn fellowship(&mut self) -> Dynamic {
        let c = self.client();
        let Some(f) = c.world.fellowship.as_ref() else {
            return Dynamic::UNIT;
        };
        let mut m = Map::new();
        m.insert("name".into(), f.name.clone().into());
        m.insert("leader".into(), int(f.leader));
        m.insert("share_xp".into(), f.share_xp.into());
        let members: Array = f
            .members
            .iter()
            .map(|x| {
                let mut mm = Map::new();
                mm.insert("guid".into(), int(x.guid));
                mm.insert("name".into(), x.name.clone().into());
                mm.insert("level".into(), int(x.level));
                mm.insert("health".into(), int(x.health.0));
                mm.insert("health_max".into(), int(x.health.1));
                Dynamic::from_map(mm)
            })
            .collect();
        m.insert("members".into(), members.into());
        Dynamic::from_map(m)
    }

    fn option(&mut self, name: &str, on: bool) -> bool {
        let Some(o) = ac_plugin::ac_client::options::option_by_name(name) else {
            return false;
        };
        self.client().set_option(o, on);
        true
    }

    fn use_on(&mut self, item: i64, target: i64) -> bool {
        let c = self.client();
        let target = if target == 0 {
            c.world.player_guid.unwrap_or(0)
        } else {
            target as u32
        };
        c.use_on(item as u32, target)
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

    fn appraise(&mut self, guid: i64) {
        self.client().appraise(guid as u32);
    }

    fn appraisal(&mut self, guid: i64) -> Dynamic {
        let c = self.client();
        let Some(a) = c.appraisals.get(&(guid as u32)) else {
            return Dynamic::UNIT;
        };
        let mut m = Map::new();
        let name = c
            .world
            .objects
            .get(&a.guid)
            .map(|o| o.name.clone())
            .unwrap_or_default();
        m.insert("name".into(), name.into());
        // "use" is a Rhai keyword, so the use line is "usage".
        for (key, id) in [
            ("usage", 14u32),
            ("short_desc", 15),
            ("long_desc", 16),
            ("inscription", 7),
        ] {
            m.insert(key.into(), a.string(id).unwrap_or("").into());
        }
        for (key, id) in [
            ("value", 19u32),
            ("burden", 5),
            ("workmanship", 105),
            ("armor_level", 28),
            ("spellcraft", 106),
            ("mana", 107),
            ("mana_max", 108),
            ("wield_skill", 159),
            ("wield_level", 160),
            ("level", 25),
            ("tinkers", 171),
        ] {
            m.insert(key.into(), a.int(id).map(int).unwrap_or(Dynamic::UNIT));
        }
        if let Some(w) = &a.weapon {
            m.insert("damage".into(), int(w.damage));
            m.insert(
                "damage_min".into(),
                int((w.damage as f64 * (1.0 - w.variance)).round() as i64),
            );
            m.insert("speed".into(), int(w.speed));
            m.insert("weapon_skill".into(), int(w.skill));
            m.insert("offense".into(), float(w.offense as f32));
        }
        if let Some(cp) = &a.creature {
            m.insert("health".into(), int(cp.health));
            m.insert("health_max".into(), int(cp.health_max));
        }
        let spells: Array = a.spells.iter().map(|s| int(*s)).collect();
        m.insert("spells".into(), spells.into());
        let mut ints = Map::new();
        for (k, v) in &a.ints {
            ints.insert(k.to_string().into(), int(*v));
        }
        m.insert("ints".into(), Dynamic::from_map(ints));
        let mut floats = Map::new();
        for (k, v) in &a.floats {
            floats.insert(k.to_string().into(), float(*v as f32));
        }
        m.insert("floats".into(), Dynamic::from_map(floats));
        Dynamic::from_map(m)
    }

    fn split(&mut self, item: i64, amount: i64) -> bool {
        self.client()
            .split_stack(item as u32, None, amount.max(0) as u32)
    }

    fn merge(&mut self, from: i64, to: i64) -> bool {
        self.client().merge_stacks(from as u32, to as u32, None)
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

    fn jump(&mut self, power: f64) {
        self.client().jump(power as f32);
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
