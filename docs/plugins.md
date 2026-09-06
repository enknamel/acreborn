# Writing a plugin

A plugin is a plain Rust type implementing `ac_plugin::Plugin`, registered
with the `Host` in `bins/acviewer/src/plugins/mod.rs`. It sees every
session in the process through `Ctx`, can call any `ac_client::Client`
method, draw egui panels, take keys, and handle `/commands`. Plugins in
different sessions coordinate through the shared `Blackboard`.

The crate re-exports what you need: `ac_plugin::{Client, Event, Ctx,
Plugin, Blackboard, Message, Host, Requests, IconCache, IconLayers,
IconLoader, egui, serde_json, Value}`. The client's own panels are plugins
too (`ac_plugin::panels`, see below): read them as worked examples of a UI
plugin, or replace them.

## The `Plugin` trait

```rust
pub trait Plugin {
    fn name(&self) -> &str;

    /// A server event for `cx.client()` (chat lines, sounds, placement...).
    fn on_event(&mut self, _cx: &mut Ctx, _ev: &Event) {}

    /// Once per frame per session, after the session ticked.
    fn tick(&mut self, _cx: &mut Ctx) {}

    /// Draw panels for the active session. Runs inside the frame's egui
    /// pass; use `egui::Window`/`egui::Area` freely.
    fn ui(&mut self, _cx: &mut Ctx, _egui: &egui::Context) {}

    /// A key went down or up while no text box had focus. Return true to
    /// consume it so the client's own bindings ignore it.
    fn key(&mut self, _cx: &mut Ctx, _key: egui::Key, _pressed: bool) -> bool { false }

    /// `/name args` typed in the chat box. Return true when handled.
    fn command(&mut self, _cx: &mut Ctx, _name: &str, _args: &str) -> bool { false }
}
```

Call order each frame (`Host::frame`, once per session `i`): every
plugin's `on_event` for each of session `i`'s events, then its `tick`.
`ui`, `key` and `command` run with `index` = the active session. `key` and
`command` stop at the first plugin that returns `true`; an unhandled
command logs `Unknown command /name`.

`Event` (from `ac_client`):

```rust
pub enum Event {
    Chat { text: String, kind: u32 },   // kind = server ChatMessageType
    Sound { wave: Rc<Wave>, volume: f32 },
    Connected,
    Terminated(String),
    Refused(u32),                       // CharacterError / AccountBoot opcode
    Placed { cell: u32 },               // the character stands in the world
    SpellLearned(u32), SpellForgotten(u32),
    Characters(Vec<CharacterEntry>),    // the account's list, when not auto-entering
    CharacterCreated { id: u32, name: String },
    CharacterCreateFailed(u32),         // ACE verification code (creation::create_failure_message)
}
```

## `Ctx`

```rust
pub struct Ctx<'a> {
    pub clients: Vec<&'a mut Client>, // every session, in order
    pub index: usize,                 // the session this callback is about
    pub board: &'a mut Blackboard,
    pub icons: &'a mut IconCache,     // item and spell icons as egui textures
    pub dt: f32,                      // seconds since the last frame (0 in ui/key/command)
    pub now: Instant,
    pub chat: Vec<(String, u32)>,     // lines to add to the active chat log
    pub activate: Option<usize>,      // ask the host to switch the active session
}

impl Ctx<'_> {
    pub fn client(&mut self) -> &mut Client;   // self.clients[self.index]
    pub fn try_client(&mut self) -> Option<&mut Client>; // None when the host has no session (--demo-ui)
    pub fn client_count(&self) -> usize;
    pub fn icons(&mut self) -> &mut IconCache;
    pub fn log(&mut self, text: impl Into<String>);            // chat line, kind 0 (system)
    pub fn post(&mut self, topic: impl Into<String>, value: impl Into<Value>); // bus, from this session
}
```

### Icons

`cx.icons()` turns RenderSurface (0x06) ids into egui textures on first
use and paints them: `cx.icons().draw(ui, IconLayers::of(object),
egui::Sense::click())` draws an object's icon with its overlay and
underlay as a 24-point square and returns the `Response`;
`IconLayers::single(spell.icon_id)` for a spell. The decoder behind it is
installed once by the host (`Host::set_icon_loader`, an `Rc<dyn Fn(u32)
-> Option<Rgba>>` over `Assets::texture_rgba`); a host that never draws
installs none and `draw` paints nothing.

### `Client` fields worth reading

| field | what it is |
|---|---|
| `config: Config` | `host`, `account`, `password`, `character` of this session. |
| `world: ac_world::World` | Everything the server described: see below. |
| `assets: Rc<ac_scene::Assets>` | The DAT loader; `spell_table()`, `skill_table()`, `region()`, `setup(id)`, ... |
| `player: Option<player::Player>` | Our physics body once placed: `cell`, `local`, `heading`, `stance`, `world_position()`, `forward()`, `landblock()`, `is_indoors()`, `is_airborne()`, `busy()`, `turn(d_yaw)`. |
| `scene_block: Option<u32>` | Landblock the scene is built around; `placed()` is `scene_block.is_some()`. |
| `combat: bool`, `magic: bool` | Melee and magic combat modes. |
| `attack_target: Option<u32>` | Creature we keep swinging at until it dies or `attack_target = None`. |
| `attack_pending: bool` | A swing was sent and AttackDone has not come back. |
| `last_target_name: String` | Name of the last creature attacked (its corpse is what `/loot` opens). |
| `move_to: Option<MoveTarget>` | A server-requested MoveTo we are carrying out. |
| `selected: Option<u32>` | The selected object (target bar, appraisal, targeted casts). |
| `known_spells: HashMap<u32, String>` | Spells learnt from scrolls this session, by name. |
| `loot_queue: VecDeque<u32>`, `loot_inflight: Option<(u32, Instant)>` | Items still to take from the open container, and the one in flight. |
| `characters: Vec<CharacterEntry>` | The account's character list. |
| `session: ac_net::session::Session` | Raw access: `send_action(action, body)`, `send_message(queue, msg)` for anything `Client` has no method for. |

### `World` (`crates/ac-world/src/lib.rs`)

| item | meaning |
|---|---|
| `objects: HashMap<u32, WorldObject>` | Every object the server created, by guid. |
| `player_guid: Option<u32>`, `player()`, `player_mut()` | Our own object. |
| `stats: PlayerStats` | The character sheet (below). |
| `open_container: Option<(u32, Vec<u32>)>` | A corpse or chest we are looking into: its guid and item guids. |
| `open_vendor: Option<ApproachVendor>` | The vendor we are trading with: `vendor`, `items: Vec<VendorItem>` (`guid`, `stack`, `desc.name`, `desc.value`), `buy_rate`, `sell_rate`. |
| `generation: u64` | Bumped whenever a drawable object or position changes. |
| `inventory()`, `wielded()` | Iterators over our pack items and equipped items. |
| `drawable()` | Objects with a position and a model (what is in view). |

`WorldObject`: `guid`, `name`, `weenie_class_id`, `setup_id`,
`motion_table_id`, `position: Option<Position>` (`cell`, `local`,
`rotation`), `display` (smoothed position), `world_pos()`,
`object_desc_flags` (`object_desc_flags::{PLAYER, ATTACKABLE, VENDOR,
DOOR, CORPSE, PORTAL, STUCK, OPENABLE}`), `item_type`
(`item_type::{CREATURE, CONTAINER, WIELDABLE, ...}`), `container`,
`wielder`, `stack_size`, `value`, `spell_id`, `health: Option<f32>`
(fraction, creatures we have hit), `is_player`, `motion` (current stance
and forward command), `target: Option<MoveTarget>`. `ac_world::
landblock_origin(cell) + position.local` is the world position.

`PlayerStats` (`crates/ac-world/src/stats.rs`): `name`, `level`,
`total_xp`, `available_xp`, `skill_credits`, `attributes: [Attribute; 6]`
(Strength, Endurance, Quickness, Coordination, Focus, Self; `value()`),
`vitals: [Vital; 3]` (Health, Stamina, Mana; `current`), `vital_max(i)`,
`skills: Vec<Skill>`, `skill(id)`, `skill_value(skill, base)`, `spells:
Vec<u32>` (known spell ids), `enchantments`, `inventory`, `wielded`,
`options` (shortcuts, spell bars).

### `Client` methods (actions)

| method | effect |
|---|---|
| `tick(input: Option<player::Input>, dt, now) -> PlayerFrame` | Pump the network, apply messages, run timers and physics. The host calls this; plugins do not. |
| `drain_events() -> Vec<Event>` | Events since the last drain (the host drains and passes them to `on_event`). |
| `placed() -> bool` | The character is in the world. |
| `say(text)` | Local chat, or an `@command`. |
| `select(guid: Option<u32>)` | Set the selection (targeted spells go to it). |
| `interact(guid)` | Double-click semantics: attack if in melee and attackable, wield/unwield carried items, pick up ground items, else Use (doors, NPCs, corpses, vendors, portals). |
| `use_by_name(name) -> bool` | `interact` on the nearest object whose name starts with `name` (carried items win, exact names beat prefixes); false if nothing matches yet. |
| `toggle_combat()` | Melee mode on/off (`combat`); leaving it clears the attack target. |
| `attack(guid)` | One TargetedMeleeAttack and set `attack_target`; `tick_combat` re-swings until the target dies. Needs `combat`. |
| `cast(spell_id)` | Enter magic mode if needed, then cast: self-targeted spells untargeted, others at `selected` (or ourselves). |
| `known_spell_ids()`, `spell(id)`, `spellbook_filters()`, `set_spellbook_filters(bits)`, `forget_spell(id)` | The spellbook (`ac_client::magic`). |
| `spell_bars()`, `add_to_spell_bar(bar, pos, id)`, `remove_from_spell_bar(bar, id)` | The eight server-persisted spell bars. |
| `enchantments()`, `components()`, `has_focus(school)`, `current_formula(id)`, `can_cast(id)`, `set_desired_component(id, n)`, `fill_components()` | Buffs, components and the cast pre-check. |
| `take(guid)` | Queue an item of the open container for pickup. |
| `close_container()` | Stop viewing the open container. |
| `buy(guid)`, `sell(guid)`, `close_vendor()` | Trade with `world.open_vendor`. |
| `disconnect(now)` | Clean disconnect. |

`player::Input { forward: f32, strafe: f32, run: bool, jump: bool, jump_held: bool }` (`jump` is a full-power jump this frame, `jump_held` charges one while true and leaps when it goes false) is
what the host builds from the keys for the active session; a plugin cannot
steer the character directly yet, but `client.player.heading` and
`client.move_to` are public, and setting `move_to = Some(MoveTarget::
Position { cell, local })` makes `tick` run there.

## Blackboard and bus

```rust
pub struct Blackboard { pub values: HashMap<String, Value>, /* inbox, outbox */ }
impl Blackboard {
    pub fn get(&self, key: &str) -> Option<&Value>;
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<Value>);
    pub fn post(&mut self, from: usize, topic: impl Into<String>, value: impl Into<Value>);
    pub fn messages(&self) -> impl Iterator<Item = &Message>;
    pub fn messages_on<'a>(&'a self, topic: &'a str) -> impl Iterator<Item = &'a Message> + 'a;
}
pub struct Message { pub from: usize, pub topic: String, pub value: Value }
```

* `values` persist for the life of the process: use them for shared facts
  (`board.set("leader", 0)`, `board.get("leader")`).
* Messages posted this frame (`cx.post(topic, value)`) are readable by every
  plugin, for every session, during the next frame only. `Host::end_frame`
  rotates the queues after all sessions ran.
* Values are `serde_json::Value`; use `serde_json::json!({...})` for
  structured payloads and `.as_u64()` etc. to read them.

## Registering a plugin

`bins/acviewer/src/plugins/mod.rs`:

```rust
pub mod autoheal;
pub use ac_plugin::{console, panels};

pub fn builtin() -> Host {
    let mut host = Host::new();
    for p in panels::live() {          // vitals, radar, target, vendor, loot,
        host.register(p);              // inventory, skills, spellbook,
    }                                  // spellbar, components, buffs
    host.register(Box::new(console::Console::default()));
    host.register(Box::new(autoheal::AutoHeal::default()));
    host
}
```

Order matters for `key` and `command` (first to return `true` wins), so
put plugins that claim generic keys last; the panels go first so they draw
under everything else and own I, K, P, B, O, U and (while the spell bar is
visible) `1`..`9`, Insert, Delete, PageUp and PageDown. Any binary that drives sessions
can host plugins the same way: `Host::new()`, `register`, then
`frame`/`ui`/`key`/`command`/`end_frame` (see `crates/ac-plugin/src/host.rs`).

## The built-in panels

`crates/ac-plugin/src/panels/` holds the client's own overlay, one plugin
per file: `vitals`, `radar`, `target`, `inventory` (I; grouped by pack,
searchable with `ac_client::items::Query` lines such as `dmg>10 spell:blood
type:armor`, sortable, with stat tooltips and a background "Appraise all"),
`loot`, `vendor`,
`skills` (K), `spellbook` (P), `spellbar` (B), `components` (O), `buffs`
(U), `trade`, `fellowship` (F), `allegiance` (L), `housing` (H; a used house sign opens it), `social`
(N: title, friends, squelches), `book` (opens on a used book, sign or
plaque), `appraisal`
(opens when an assessment arrives), `salvage`
(opens when the Ust is used), `confirm` (the
server's Yes/No questions), `options` (X), `combat` (the height/power bar). Each
has the same three parts, so any of them is a template for a UI plugin:

* `view(&Client) -> View`: a plain struct of what to draw, built from the
  session each frame (`world.stats`, `inventory()`, `open_container`,
  `open_vendor`, the spell tables through `client.assets`);
* `draw(&egui::Context, ..) -> actions`: the egui code, returning what was
  clicked (guids to take or buy, spell ids to cast);
* the `Plugin` impl: `ui` builds the view, draws it, and calls
  `Client` (`interact`, `take`, `close_container`, `buy`, `sell`,
  `close_vendor`, `cast`); `key` toggles the panel.

Every panel is a borderless `egui::Window` (or `Area`) built by
`panels::window(name, pos, size, alpha, margin)`: fixed size, no title bar,
opened at `pos` and movable by dragging any part of it that no widget
claims (item and spell rows keep their own drag-and-drop, scroll areas
their drag-to-scroll). egui remembers where each was moved for the rest of
the session and keeps it inside the viewport; positions computed from the
viewport size (the centred target bar, the radar in the corner) are only
recomputed for a panel that has not been moved. "Reset window layout" in
the options panel (X) calls `egui::Memory::reset_areas`, which puts every
panel back at its default position.

A panel's data comes from a `Source<View>`: `Live` (the session) or
`Demo(view)` (canned data whose actions are dropped). `Panel::demo()`
constructors feed `acviewer --screenshot --demo-ui`, which registers
`panels::demo(assets)` instead of `builtin()` and renders the overlay with
no server; `Ctx::try_client()` is `None` there. To replace a panel,
register your own plugin instead of it in `builtin()`; the skills panel
publishes whether it is open on the blackboard (`panels::skills::OPEN_KEY`)
so the spellbook can sit beside it, a small example of panels talking. The
spellbook and spell bar do the same (`panels::spellbook::OPEN_KEY`,
`panels::spellbar::{SHOWN_KEY, VISIBLE_KEY}`).

The magic panels follow `docs/game/mechanics.md` (section 1) and build on
`ac_client::magic`:

* **spellbook** (P): known spells (`known_spell_ids`, `spell`) in display
  order, filtered by the server-side bits (`spellbook_filters`,
  `set_spellbook_filters`) with one toggle per school and level.
  Double-click or drag a spell onto the spell bar or one of its tabs to add
  it (`add_to_spell_bar`); right-click or `i` for details (school, level,
  mana, duration, description, the current formula from `current_formula`
  with each component's presence, Cast, and Delete with confirmation →
  `forget_spell`).
* **spellbar** (B; also shown while the spellbook is open or `client.magic`
  is set): the eight tabs from `spell_bars`. `1`..`9` cast the nth spell of
  the shown tab; PageUp / Insert show the next / previous tab, PageDown /
  Delete select the next / previous spell, Ctrl jumps to the last / first;
  Shift+Delete or Remove takes the selected spell off (`remove_from_spell_bar`);
  click selects, double-click or Cast casts (`cast`). Hovering shows
  `can_cast`'s verdict (no caster, missing components by name, mana); such
  spells are dimmed but still castable, the server has the last word.
* **components** (O): `components()` grouped by kind with the count and an
  editable desired quantity 0..999 (`set_desired_component`), the foci
  carried per school (`has_focus`), and "Fill from vendor"
  (`fill_components`, enabled while `world.open_vendor` is set).
* **buffs** (U, on by default): `enchantments()` split into beneficial and
  harmful by the SpellTable flag, soonest to expire first, vitae as
  "Vitae -N%". Time left is `duration + start_time - elapsed`: ACE sends
  `start_time` as the non-positive seconds the spell has already run, so
  the panel anchors each value to the local clock when it first sees it
  and needs no server time base.

## Character select and creation

`crates/ac-plugin/src/lobby/` is the screen between login and the world,
built the same way as the panels (`view` / `draw` / state, a demo mode with
no session) and driven by `ac_client::creation`:

* **select** (`lobby::select`): `view(&Client)` lists `client.characters`
  (name, "deleting in Xs" for one pending deletion); `draw` returns
  `SelectAction::{Enter, Delete, Restore, New}` and `key` handles Up/Down,
  Enter (the highlighted character; it also confirms a pending delete) and
  Escape. `Lobby` turns them into `client.enter_world(id)`,
  `delete_character(id)` (after a Yes/No strip), `restore_character(id)`
  and opens the creation screen.
* **create** (`lobby::create`): `CreateState` owns a `CharacterBuild` plus
  its `Rules` and the `CharGen` table, and walks `Step::{Heritage,
  Appearance, Attributes, Skills, Finish}` (`next`/`prev`; Left/Right or
  PageUp/PageDown). Heritage hides the Olthoi groups unless "Show all";
  appearance steps through the sex's hair, eye, nose, mouth and clothing
  option lists; attributes show `attribute_points_left` (red when
  negative, `points_color`) with -10/-1/drag/+1/+10; skills are grouped
  Specialized / Trained / Untrained / Unusable (`group_skills`) with each
  skill's train and specialize cost and Specialize / Train / Untrain / Drop
  buttons that call `build.set_skill` (refusals become the message line);
  the finish pane has the name (`valid_name` feedback), the starting town
  and a summary. Create is enabled while `build.validate(&rules)` passes
  (the first error is shown otherwise) and calls
  `client.create_character(&build)`; `Event::CharacterCreateFailed` puts
  the server's reason on the screen, `Event::CharacterCreated` returns to
  the list while the client enters the world.
* `Lobby` (`lobby::Lobby`) holds both screens, follows the events
  (`Characters` opens the list, `Placed` hides everything) and implements
  `Plugin`, but acviewer owns it directly rather than registering it: the
  viewer reads `Lobby::preview()` (the build being edited) every frame and
  draws the model beside the creation window with the `--chargen` path
  (`ac_scene::chargen::describe` → `instances_for` →
  `set_player_instances`), turning slowly or by right-drag. Clothing is
  sent but not previewed.

`acviewer --demo-select` and `--demo-create` show the two screens with no
server (three sample characters; the real CharGen table and the 3D
preview). With `--screenshot out.png` they render one frame headlessly, and
`--press ArrowRight,ArrowRight` first steps the creation screen to the
pane you want to see.

## Worked example: auto-heal

Casts Heal Self when health drops under half, at most every four seconds;
`/autoheal` toggles it. Save as `bins/acviewer/src/plugins/autoheal.rs`
and register it as above. (This file type-checks against the current
crates.)

```rust
//! Auto-heal: cast Heal Self whenever health drops under half.
//! `/autoheal` toggles it.

use std::time::{Duration, Instant};

use ac_plugin::{Client, Ctx, Plugin};

pub struct AutoHeal {
    enabled: bool,
    last_cast: Option<Instant>,
}

impl Default for AutoHeal {
    fn default() -> Self {
        AutoHeal {
            enabled: true,
            last_cast: None,
        }
    }
}

impl AutoHeal {
    /// The Heal Self the character knows: first from the spellbook (ids in
    /// `world.stats.spells`, named through the portal's SpellTable), else a
    /// scroll read this session.
    fn heal_self(c: &Client) -> Option<u32> {
        let table = c.assets.spell_table().ok()?;
        c.world
            .stats
            .spells
            .iter()
            .copied()
            .find(|id| table.get(*id).is_some_and(|sp| sp.name.starts_with("Heal Self")))
            .or_else(|| {
                c.known_spells
                    .iter()
                    .find(|(_, n)| n.starts_with("Heal Self"))
                    .map(|(id, _)| *id)
            })
    }
}

impl Plugin for AutoHeal {
    fn name(&self) -> &str {
        "autoheal"
    }

    /// Once per frame for every session: each session heals itself.
    fn tick(&mut self, cx: &mut Ctx) {
        if !self.enabled {
            return;
        }
        let c = cx.client();
        if !c.placed() {
            return;
        }
        let max = c.world.stats.vital_max(0);
        let health = c.world.stats.vitals[0].current;
        if max == 0 || health * 2 >= max {
            return;
        }
        if self.last_cast.is_some_and(|t| t.elapsed() < Duration::from_secs(4)) {
            return;
        }
        let Some(spell) = Self::heal_self(c) else {
            return;
        };
        // Heal Self is self-targeted, so `cast` sends it untargeted. It also
        // switches the character into magic mode (out of melee).
        c.cast(spell);
        self.last_cast = Some(Instant::now());
        cx.log(format!("autoheal: {health}/{max}, casting Heal Self"));
        cx.post("autoheal", health);
    }

    fn command(&mut self, cx: &mut Ctx, name: &str, _args: &str) -> bool {
        if name != "autoheal" {
            return false;
        }
        self.enabled = !self.enabled;
        cx.log(format!(
            "autoheal {}",
            if self.enabled { "on" } else { "off" }
        ));
        true
    }
}
```

Notes: `tick` runs once per session, so with three `--client`s each
character heals itself and `self.last_cast` is shared; keep per-session
state in a `Vec` indexed by `cx.index` when that matters. Health arrives
as `PrivateUpdateAttribute2ndLevel` and is applied to
`stats.vitals[0].current` before your `tick` sees it. ACE needs spell
components unless `@modifybool require_spell_comps false`.

## An agent-style plugin

A bot that plays the game is the same shape, with a perceive / decide /
act loop in `tick` and coordination over the bus:

**Perceive** from `cx.client().world`: `drawable()` for what is in view,
`object_desc_flags & ATTACKABLE` plus `item_type & CREATURE` for monsters,
`health` for ones we have hit, `player().position` and
`landblock_origin(cell) + local` for distances, `stats.vitals` for our own
state, `open_container`/`open_vendor` for what is open, and the `Event`s
in `on_event` (`Chat` lines carry server errors and `TransientString`
refusals as text).

**Decide** with plain state in the plugin struct: a small enum per session
(`Idle`, `Fighting(guid)`, `Looting`, `Healing`) indexed by `cx.index`,
with `Instant`s for timeouts, because the server answers asynchronously
and some answers never come.

**Act** through `Client`: `toggle_combat` + `attack(guid)` (or
`use_by_name`), `cast`, `interact`/`use_by_name` on corpses and doors,
`take` per item, `buy`/`sell`, `say`, and `session.send_action` for the
rest.

**Coordinate**: `cx.post("assist", json!({"target": guid}))` from the
leader; followers read `cx.board.messages_on("assist")` next frame and
`attack` the same guid. `board.set("leader", index)` for facts that must
persist. `cx.activate = Some(i)` switches the window to a session that
needs eyes on it.

Protocol facts from [subsystems/net.md](subsystems/net.md) that shape a
bot:

* **Server-driven MoveTo.** Using or attacking something out of reach makes
  ACE send a MoveTo for our guid and poll our reported position; ACE does
  not move us. `Client` runs toward `move_to` and holds MoveToState until
  the server says idle (any MoveToState cancels the chain). While
  `client.move_to.is_some()` do not re-send the action; `tick_combat`
  already waits. A move-to times out after 12 s.
* **One pickup at a time.** The server refuses a second PutItemInContainer
  while one is in flight (`YoureTooBusy`). `take(guid)` queues and
  `tick_loot` sends one at a time, waiting for the item to land or 4 s;
  check `loot_queue.is_empty() && loot_inflight.is_none()` before closing
  the container or walking off.
* **AttackDone semantics.** Every swing sequence ends with AttackDone
  carrying `ActionCancelled` (0x36); that is the normal end, not a
  failure. `attack_pending` is cleared there and `tick_combat` re-swings
  after `attack_backoff` (300 ms). Watch `world.objects[target].health`
  (fraction) and the `Chat` kill line; `attack_target` becomes `None` when
  the target is gone.
* **Lifestone protection.** After a respawn attacks are refused (WeenieError
  0x0502) or the protection is dispelled by acting; back off on the
  "cannot attack" transient strings rather than hammering.
* **Modes.** `cast` flips to magic mode and clears `combat`; call
  `toggle_combat` again before the next melee swing. `toggle_combat` off
  also drops `attack_target`.
* **Names, not guids, across sessions.** Guids are per object and stable
  while the object exists, so a guid on the bus is fine; names are for
  humans and `use_by_name` prefix matching.
