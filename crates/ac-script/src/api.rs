//! The script-facing API: the [`Api`] trait the host implements over
//! `ac_plugin::Ctx` (and tests implement with a recorder), and the Rhai
//! functions that forward to it.
//!
//! # Why a thread-local pointer
//!
//! Rhai native functions are `'static` closures: they cannot capture the
//! `&mut Ctx` a plugin callback receives, which lives only for that call.
//! Building a fresh engine (and re-registering every function) per call
//! would work but costs milliseconds per frame per session. So instead,
//! for the duration of each script call, [`Bound`] stores a raw pointer to
//! the current `Api` in a thread-local, and every registered function
//! reaches it through [`with_api`]. That accessor is the only place the
//! pointer is dereferenced, and it is safe to call at any time:
//!
//! * outside a bound call the pointer is `None` and the script gets an
//!   error instead of a dangling dereference (`Bound`'s `Drop` clears it
//!   before the borrow it was made from ends, and `Bound` carries that
//!   borrow's lifetime, so the host cannot touch the `Api` while scripts
//!   can);
//! * a `BUSY` flag rejects re-entrant access, so two `&mut dyn Api` never
//!   coexist. Functions that call back into the script (`with_session`)
//!   release the borrow before they do;
//! * the engine is single-threaded (`sync` off) and the pointer is
//!   thread-local, so no other thread can observe it.

use std::cell::Cell;
use std::marker::PhantomData;
use std::ptr::NonNull;

use ac_plugin::Message;
use rhai::serde::{from_dynamic, to_dynamic};
use rhai::{Array, Dynamic, Engine, EvalAltResult, FnPtr, Map, NativeCallContext};
use serde_json::Value;

pub type ScriptResult<T> = Result<T, Box<EvalAltResult>>;

/// Everything a script can see and do. Session indices are zero-based;
/// every action applies to the *current* session, which starts as the
/// session the callback is about and can be changed temporarily with
/// `set_session` (what the script's `with_session(i, || ...)` does).
pub trait Api {
    // ---- reads ----
    /// Summary of the current session's character (see `session`).
    fn me(&mut self) -> Map;
    /// Objects in view, nearest first: `guid, name, distance, is_creature,
    /// is_player, is_corpse, health, x, y, z, cell`.
    fn objects(&mut self) -> Array;
    /// Items in the pack (same shape as `objects`, distance 0).
    fn inventory(&mut self) -> Array;
    /// Items in the open ground container (a corpse or chest), if any.
    fn container(&mut self) -> Array;
    fn session_count(&mut self) -> i64;
    /// Summary of session `i`: `session, guid, name, level, health,
    /// health_max, stamina, stamina_max, mana, mana_max, x, y, z, cell,
    /// combat, magic, target, target_name, selected, placed`.
    fn session(&mut self, i: i64) -> Option<Map>;
    fn current_session(&mut self) -> i64;
    /// Make `i` the current session; false (and no change) when out of range.
    fn set_session(&mut self, i: i64) -> bool;

    // ---- actions on the current session ----
    fn use_name(&mut self, name: &str) -> bool;
    fn use_guid(&mut self, guid: i64) -> bool;
    /// Use an object where it is (read a book on the ground, open a chest) without picking it up.
    fn activate(&mut self, guid: i64) -> bool;
    /// Pick a loose item up into the pack.
    fn pickup(&mut self, guid: i64) -> bool;
    fn attack(&mut self, name: &str) -> bool;
    fn attack_guid(&mut self, guid: i64) -> bool;
    fn cast(&mut self, name: &str) -> bool;
    /// Why a known spell (by name prefix) cannot be cast right now: "ok",
    /// "not_known", "no_caster", "missing_components", "not_enough_mana".
    fn can_cast(&mut self, name: &str) -> String;
    /// Spell components carried: maps with `id`, `name`, `wcid`, `count`,
    /// `desired`.
    fn components(&mut self) -> Array;
    /// With a vendor open, buy components up to their desired counts;
    /// returns how many kinds were ordered.
    fn fill_components(&mut self) -> i64;
    /// Set the desired quantity of a component named by (prefix of) its name.
    fn set_desired_component(&mut self, name: &str, quantity: i64) -> bool;
    fn say(&mut self, text: &str);
    /// Open a corpse (`""` = the corpse of the last attack target).
    fn loot(&mut self, name: &str) -> bool;
    /// Queue an item of the open container to be picked up.
    fn take(&mut self, guid: i64) -> bool;
    /// Apply a carried item to a target guid (0 = yourself): kits, stones, keys.
    fn use_on(&mut self, item: i64, target: i64) -> bool;
    /// Set a character option by (prefix of) its label; false when unknown.
    fn option(&mut self, name: &str, on: bool) -> bool;
    fn fellow_create(&mut self, name: &str, share_xp: bool);
    fn fellow_recruit(&mut self, player: i64);
    fn fellow_quit(&mut self, disband: bool);
    /// Pending server questions as maps { kind, context, text }.
    fn confirmations(&mut self) -> Array;
    /// Answer the oldest pending question; false when there is none.
    fn confirm(&mut self, yes: bool) -> bool;
    /// The fellowship as a map { name, leader, members: [{guid, name, level, health, ...}] } or unit.
    fn fellowship(&mut self) -> Dynamic;
    /// Swear allegiance to a player in view (they answer a confirmation).
    fn swear(&mut self, patron: i64) -> bool;
    /// Break with your patron or one of your vassals.
    fn break_allegiance(&mut self, member: i64) -> bool;
    /// The allegiance as a map { name, rank, total_members, total_vassals,
    /// motd, monarch, patron, me, vassals: [...] } (members are maps
    /// { guid, name, level, rank, loyalty, leadership, online, xp_cached,
    /// xp_tithed }) or unit when not in one.
    fn allegiance(&mut self) -> Dynamic;
    /// Ask the server for the profile again.
    fn allegiance_refresh(&mut self);
    /// Carried items an Ust would salvage (loot with `material` and `workmanship`).
    fn salvageable(&mut self) -> Array;
    /// Salvage these guids with the carried Ust; the yields show in chat.
    fn salvage(&mut self, items: Array) -> bool;
    /// Name the allegiance (monarch), "" clears it.
    fn allegiance_name(&mut self, name: &str);
    /// The house sign last used, as a map { slumlord, owner, owner_name, kind, min_level, buy: [{name, wcid, needed, paid}], rent: [...] } or unit.
    fn house_profile(&mut self) -> Dynamic;
    /// Our house as a map { kind, cell, rent_paid, rent: [...] }, unit when we own none (or the server has not said).
    fn house(&mut self) -> Dynamic;
    fn house_query(&mut self);
    /// Buy / pay rent at the sign last used, with the pack's items; false when short.
    fn buy_house(&mut self) -> bool;
    fn rent_house(&mut self) -> bool;
    fn abandon_house(&mut self);
    /// Guest list as maps { guid, name, storage } plus flags; unit until requested.
    fn house_guests(&mut self) -> Dynamic;
    fn house_guest(&mut self, name: &str, add: bool);
    fn house_storage(&mut self, name: &str, allow: bool);
    fn house_open(&mut self, open: bool);
    /// Say something on a group channel: "v" vassals, "p" patron, "m"
    /// monarch, "c" co-vassals, "f" fellowship; or a Turbine chat room:
    /// "g"/"general", "trade", "lfg", "rp", "a"/"allegiance". False for
    /// an unknown channel or an allegiance room we are not in.
    fn chat(&mut self, channel: &str, text: &str) -> bool;
    /// Open a secure trade with a player (guid); add an item; accept/decline/reset/close.
    fn trade_open(&mut self, player: i64);
    fn trade_add(&mut self, item: i64) -> bool;
    fn trade_accept(&mut self);
    fn trade_decline(&mut self);
    fn trade_reset(&mut self);
    fn trade_close(&mut self);
    /// Items offered so far: map { mine: [..], theirs: [..], i_accepted, they_accepted }.
    fn trade(&mut self) -> Map;
    /// Spend XP on one rank of a skill / point of an attribute or vital, by name.
    fn raise(&mut self, what: &str) -> bool;
    /// Train an untrained skill with credits, by name.
    fn train(&mut self, skill: &str) -> bool;
    /// Drop a carried item on the ground.
    fn drop_item(&mut self, guid: i64) -> bool;
    /// Hand a carried item (amount 0 = whole stack) to a creature or player.
    fn give(&mut self, target: i64, item: i64, amount: i64) -> bool;
    /// Move a carried item into a pack (container 0 = main pack) or the open chest.
    fn put_in(&mut self, item: i64, container: i64) -> bool;
    /// Split `amount` off a stack into the main pack; merge one stack into another of the same kind.
    fn split(&mut self, item: i64, amount: i64) -> bool;
    /// Ask for an appraisal; `appraisal(guid)` returns the last one received for it as a map
    /// (name, usage, short_desc, long_desc, value, burden, workmanship, armor_level, damage,
    /// damage_min, speed, weapon_skill, offense, spells, spellcraft, mana, mana_max, wield_skill,
    /// wield_level, level, health, health_max, ints: #{id: v}, floats: #{id: v}) or unit.
    fn appraise(&mut self, guid: i64);
    /// The open book as a map { guid, title, author, pages: [text or ()] } or unit; `read_page(i)` asks for a page's text.
    fn book(&mut self) -> Dynamic;
    fn read_page(&mut self, index: i64);
    /// Augmentations taken, as maps { name, count, max }.
    fn augmentations(&mut self) -> Array;
    /// A soul emote by word ("wave", "bow", "cheer"...); false for unknown words.
    fn emote(&mut self, words: &str) -> bool;
    /// Friends as maps { guid, name, online }; add by name, remove by guid.
    fn friends(&mut self) -> Array;
    fn add_friend(&mut self, name: &str);
    fn remove_friend(&mut self, guid: i64);
    /// Titles as a map { current, ids: [...] }; choose one by id.
    fn titles(&mut self) -> Map;
    fn set_title(&mut self, id: i64);
    /// Squelch (or unsquelch) a player by name.
    fn squelch(&mut self, name: &str, on: bool);
    /// Squelched players as maps { guid, name, mask }.
    fn squelches(&mut self) -> Array;
    fn appraisal(&mut self, guid: i64) -> Dynamic;
    fn merge(&mut self, from: i64, to: i64) -> bool;
    /// Queue everything in the open container; returns how many.
    fn take_all(&mut self) -> i64;
    fn close_container(&mut self);
    fn buy(&mut self, name: &str) -> bool;
    fn sell(&mut self, name: &str) -> bool;
    fn combat(&mut self, on: bool);
    /// Jump with power 0..=1 (1 = a fully charged jump); stamina caps it.
    fn jump(&mut self, power: f64);
    fn select(&mut self, guid: i64);
    fn log(&mut self, text: &str);
    fn post(&mut self, topic: &str, value: Value);
    fn messages(&mut self, topic: &str) -> Vec<Message>;
    fn board_get(&mut self, key: &str) -> Option<Value>;
    fn board_set(&mut self, key: &str, value: Value);
    /// Ask the host to make session `i` the active (drawn, steered) one.
    fn switch(&mut self, i: i64);
}

thread_local! {
    static CURRENT: Cell<Option<NonNull<dyn Api>>> = const { Cell::new(None) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

/// Makes an `Api` reachable from scripts for as long as it lives.
pub struct Bound<'a> {
    previous: Option<NonNull<dyn Api>>,
    _borrow: PhantomData<&'a mut ()>,
}

impl<'a> Bound<'a> {
    pub fn new(api: &'a mut (dyn Api + 'a)) -> Self {
        let ptr: NonNull<dyn Api + 'a> = NonNull::from(api);
        // SAFETY: only the lifetime bound is erased. The pointer is never
        // dereferenced after this guard drops (`Drop` restores the previous
        // value), the guard borrows `api` for 'a, and `with_api` is the
        // sole reader, on this thread only.
        let ptr: NonNull<dyn Api + 'static> = unsafe { std::mem::transmute(ptr) };
        let previous = CURRENT.with(|c| c.replace(Some(ptr)));
        Bound {
            previous,
            _borrow: PhantomData,
        }
    }
}

impl Drop for Bound<'_> {
    fn drop(&mut self) {
        CURRENT.with(|c| c.set(self.previous));
    }
}

/// Run `f` against the bound `Api`. A script error when nothing is bound
/// or when an API call is already in progress.
pub fn with_api<R>(f: impl FnOnce(&mut dyn Api) -> R) -> ScriptResult<R> {
    let Some(ptr) = CURRENT.with(|c| c.get()) else {
        return Err("the client API is only available inside a script callback".into());
    };
    if BUSY.replace(true) {
        return Err("re-entrant client API call".into());
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            BUSY.set(false);
        }
    }
    let _reset = Reset;
    // SAFETY: a live `Bound` set the pointer on this thread and holds the
    // exclusive borrow it came from; BUSY guarantees this is the only
    // reference derived from it right now.
    Ok(f(unsafe { &mut *ptr.as_ptr() }))
}

fn to_value(d: Dynamic) -> ScriptResult<Value> {
    from_dynamic::<Value>(&d)
}

fn from_value(v: Value) -> Dynamic {
    to_dynamic(&v).unwrap_or(Dynamic::UNIT)
}

fn message_map(m: &Message) -> Map {
    let mut map = Map::new();
    map.insert("from".into(), Dynamic::from(m.from as i64));
    if let Some(origin) = &m.origin {
        map.insert("origin".into(), origin.clone().into());
    }
    map.insert("topic".into(), m.topic.clone().into());
    map.insert("value".into(), from_value(m.value.clone()));
    map
}

/// Register every script-visible function on `engine`.
pub fn register(engine: &mut Engine) {
    engine.on_print(|s| {
        let _ = with_api(|a| a.log(s));
    });
    engine.on_debug(|s, src, pos| {
        let line = match src {
            Some(src) => format!("{src} @ {pos:?}: {s}"),
            None => format!("{pos:?}: {s}"),
        };
        let _ = with_api(|a| a.log(&line));
    });

    // reads
    engine.register_fn("me", || with_api(|a| a.me()));
    engine.register_fn("objects", || with_api(|a| a.objects()));
    engine.register_fn("inventory", || with_api(|a| a.inventory()));
    engine.register_fn("container", || with_api(|a| a.container()));
    engine.register_fn("sessions", || with_api(|a| a.session_count()));
    engine.register_fn("session_index", || with_api(|a| a.current_session()));
    engine.register_fn("session", |i: i64| -> ScriptResult<Dynamic> {
        Ok(match with_api(|a| a.session(i))? {
            Some(m) => Dynamic::from_map(m),
            None => Dynamic::UNIT,
        })
    });

    // actions
    engine.register_fn("use_name", |n: &str| with_api(|a| a.use_name(n)));
    engine.register_fn("use_guid", |g: i64| with_api(|a| a.use_guid(g)));
    engine.register_fn("activate", |g: i64| with_api(|a| a.activate(g)));
    engine.register_fn("pickup", |g: i64| with_api(|a| a.pickup(g)));
    engine.register_fn("attack", |n: &str| with_api(|a| a.attack(n)));
    engine.register_fn("attack", |g: i64| with_api(|a| a.attack_guid(g)));
    engine.register_fn("cast", |n: &str| with_api(|a| a.cast(n)));
    engine.register_fn("can_cast", |n: &str| with_api(|a| a.can_cast(n)));
    engine.register_fn("components", || with_api(|a| a.components()));
    engine.register_fn("fill_components", || with_api(|a| a.fill_components()));
    engine.register_fn("set_desired_component", |n: &str, q: i64| {
        with_api(|a| a.set_desired_component(n, q))
    });
    engine.register_fn("say", |t: &str| with_api(|a| a.say(t)));
    engine.register_fn("loot", || with_api(|a| a.loot("")));
    engine.register_fn("loot", |n: &str| with_api(|a| a.loot(n)));
    engine.register_fn("take", |g: i64| with_api(|a| a.take(g)));
    engine.register_fn("use_on", |i: i64, t: i64| with_api(|a| a.use_on(i, t)));
    engine.register_fn("option", |n: &str, on: bool| with_api(|a| a.option(n, on)));
    engine.register_fn("fellow_create", |n: &str, s: bool| {
        with_api(|a| a.fellow_create(n, s))
    });
    engine.register_fn("fellow_recruit", |g: i64| with_api(|a| a.fellow_recruit(g)));
    engine.register_fn("fellow_quit", |d: bool| with_api(|a| a.fellow_quit(d)));
    engine.register_fn("confirmations", || with_api(|a| a.confirmations()));
    engine.register_fn("confirm", |y: bool| with_api(|a| a.confirm(y)));
    engine.register_fn("fellowship", || with_api(|a| a.fellowship()));
    engine.register_fn("swear", |g: i64| with_api(|a| a.swear(g)));
    engine.register_fn("break_allegiance", |g: i64| {
        with_api(|a| a.break_allegiance(g))
    });
    engine.register_fn("allegiance", || with_api(|a| a.allegiance()));
    engine.register_fn("allegiance_refresh", || {
        with_api(|a| a.allegiance_refresh())
    });
    engine.register_fn("salvageable", || with_api(|a| a.salvageable()));
    engine.register_fn("salvage", |items: Array| with_api(|a| a.salvage(items)));
    engine.register_fn("allegiance_name", |n: &str| {
        with_api(|a| a.allegiance_name(n))
    });
    engine.register_fn("house_profile", || with_api(|a| a.house_profile()));
    engine.register_fn("house", || with_api(|a| a.house()));
    engine.register_fn("house_query", || with_api(|a| a.house_query()));
    engine.register_fn("buy_house", || with_api(|a| a.buy_house()));
    engine.register_fn("rent_house", || with_api(|a| a.rent_house()));
    engine.register_fn("abandon_house", || with_api(|a| a.abandon_house()));
    engine.register_fn("house_guests", || with_api(|a| a.house_guests()));
    engine.register_fn("house_guest", |n: &str, add: bool| {
        with_api(|a| a.house_guest(n, add))
    });
    engine.register_fn("house_storage", |n: &str, on: bool| {
        with_api(|a| a.house_storage(n, on))
    });
    engine.register_fn("house_open", |on: bool| with_api(|a| a.house_open(on)));
    engine.register_fn("chat", |c: &str, t: &str| with_api(|a| a.chat(c, t)));
    engine.register_fn("trade_open", |g: i64| with_api(|a| a.trade_open(g)));
    engine.register_fn("trade_add", |g: i64| with_api(|a| a.trade_add(g)));
    engine.register_fn("trade_accept", || with_api(|a| a.trade_accept()));
    engine.register_fn("trade_decline", || with_api(|a| a.trade_decline()));
    engine.register_fn("trade_reset", || with_api(|a| a.trade_reset()));
    engine.register_fn("trade_close", || with_api(|a| a.trade_close()));
    engine.register_fn("trade", || with_api(|a| a.trade()));
    engine.register_fn("raise", |n: &str| with_api(|a| a.raise(n)));
    engine.register_fn("train", |n: &str| with_api(|a| a.train(n)));
    engine.register_fn("drop_item", |g: i64| with_api(|a| a.drop_item(g)));
    engine.register_fn("give", |t: i64, i: i64, n: i64| {
        with_api(|a| a.give(t, i, n))
    });
    engine.register_fn("put_in", |i: i64, c: i64| with_api(|a| a.put_in(i, c)));
    engine.register_fn("split", |i: i64, n: i64| with_api(|a| a.split(i, n)));
    engine.register_fn("appraise", |g: i64| with_api(|a| a.appraise(g)));
    engine.register_fn("book", || with_api(|a| a.book()));
    engine.register_fn("read_page", |i: i64| with_api(|a| a.read_page(i)));
    engine.register_fn("augmentations", || with_api(|a| a.augmentations()));
    engine.register_fn("emote", |w: &str| with_api(|a| a.emote(w)));
    engine.register_fn("friends", || with_api(|a| a.friends()));
    engine.register_fn("add_friend", |n: &str| with_api(|a| a.add_friend(n)));
    engine.register_fn("remove_friend", |g: i64| with_api(|a| a.remove_friend(g)));
    engine.register_fn("titles", || with_api(|a| a.titles()));
    engine.register_fn("set_title", |t: i64| with_api(|a| a.set_title(t)));
    engine.register_fn("squelch", |n: &str, on: bool| {
        with_api(|a| a.squelch(n, on))
    });
    engine.register_fn("squelches", || with_api(|a| a.squelches()));
    engine.register_fn("appraisal", |g: i64| with_api(|a| a.appraisal(g)));
    engine.register_fn("merge", |f: i64, t: i64| with_api(|a| a.merge(f, t)));
    engine.register_fn("take_all", || with_api(|a| a.take_all()));
    engine.register_fn("close_container", || with_api(|a| a.close_container()));
    engine.register_fn("buy", |n: &str| with_api(|a| a.buy(n)));
    engine.register_fn("sell", |n: &str| with_api(|a| a.sell(n)));
    engine.register_fn("combat", |on: bool| with_api(|a| a.combat(on)));
    engine.register_fn("jump", |p: f64| with_api(|a| a.jump(p)));
    engine.register_fn("jump", |p: i64| with_api(|a| a.jump(p as f64)));
    engine.register_fn("select", |g: i64| with_api(|a| a.select(g)));
    engine.register_fn("log", |t: &str| with_api(|a| a.log(t)));
    engine.register_fn("log", |d: Dynamic| with_api(|a| a.log(&d.to_string())));
    engine.register_fn("switch", |i: i64| with_api(|a| a.switch(i)));

    // blackboard and bus
    engine.register_fn("post", |topic: &str, v: Dynamic| -> ScriptResult<()> {
        let v = to_value(v)?;
        with_api(|a| a.post(topic, v))
    });
    engine.register_fn("messages", |topic: &str| -> ScriptResult<Array> {
        let msgs = with_api(|a| a.messages(topic))?;
        Ok(msgs
            .iter()
            .map(|m| Dynamic::from_map(message_map(m)))
            .collect())
    });
    engine.register_fn("board_get", |key: &str| -> ScriptResult<Dynamic> {
        Ok(with_api(|a| a.board_get(key))?.map_or(Dynamic::UNIT, from_value))
    });
    engine.register_fn("board_set", |key: &str, v: Dynamic| -> ScriptResult<()> {
        let v = to_value(v)?;
        with_api(|a| a.board_set(key, v))
    });

    // `with_session(i, || ...)`: run the closure with session `i` current,
    // then restore. The borrow is released before the closure runs so the
    // closure's own API calls are not re-entrant.
    engine.register_fn(
        "with_session",
        |ctx: NativeCallContext, i: i64, f: FnPtr| -> ScriptResult<Dynamic> {
            let previous = with_api(|a| {
                let previous = a.current_session();
                a.set_session(i).then_some(previous)
            })?;
            let Some(previous) = previous else {
                return Err(format!("with_session: no session {i}").into());
            };
            let result = f.call_within_context(&ctx, ());
            with_api(|a| a.set_session(previous))?;
            result
        },
    );
}
