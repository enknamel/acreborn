# Plugin SDK: adding features to the client

Everything a player can see or do is reachable from three surfaces, and
they all sit on the same plugin host, so what you learn on one carries
over:

| Surface | Lives in | Runs | Hot reload | UI | Language |
| --- | --- | --- | --- | --- | --- |
| **Rhai script** | `~/.acreborn/scripts/*.rhai` | inside every `acviewer`/`acbot` process, once per session per frame | yes, within a second of saving | no panels; chat log lines and `/commands` | Rhai |
| **Rust plugin** | a crate depending on `ac-plugin`, registered with the `Host` | inside the process, with the full `ac_client::Client` in hand | no (rebuild) | egui panels, keys, `/commands`, settings | Rust |
| **Separate process** | anything that speaks JSON lines over loopback TCP | on its own, next to the game processes | its own affair | its own affair | any |

The building blocks:

* `crates/ac-plugin`: the `Plugin` trait, `Ctx` (what a hook can see and
  do), the `Blackboard` (values and a message bus shared by every plugin
  in the process), `Settings` (what survives a restart), `panels` (the
  client's own panels, and the window helpers they use), `Host` (runs the
  plugins). `docs/plugins.md` is the reference.
* `crates/ac-script`: the Rhai plugin. `api.rs` lists every function a
  script can call (the `Api` trait), `bridge.rs` is its live
  implementation over `Ctx`, `testing.rs` runs scripts without a server.
  `scripts/examples/README.md` is the reference.
* `crates/ac-bus`: the cross-process hub; `--bus` on `acbot`/`acviewer`
  joins it. `docs/multi-session.md` ("Cross-process bus") is the
  reference.
* `examples/plugin-template`: a Rust plugin with a panel, a key, a
  setting and a command, an offline runner and a test. Copy it to start.

## Which one should it be?

| You want to... | Script | Rust plugin | Separate process |
| --- | --- | --- | --- |
| try an idea in a minute, no build | **yes** | | |
| draw a panel or window | | **yes** | |
| bind a key, keep a setting across restarts | keys yes, settings via `board_set` only | **yes** | |
| read something the script API does not expose (`Client`, `World`, the DAT archives) | | **yes** | |
| run a heavy computation (planning, statistics) without stalling frames | no (2 M operations per hook) | with care (it runs on the frame) | **yes** |
| coordinate characters in several processes | via the bus topics | via the bus topics | **yes**, it is what the bus is for |
| use another language or an existing tool | | | **yes** |
| ship to someone who runs the stock binaries | **yes** (a file) | needs your build | **yes** (a program) |

When in doubt start as a script; when it needs a panel or the raw
`Client`, move it to a Rust plugin: the hooks have the same names and
the blackboard is the same, so most of the logic moves as it is.

## I want to add a panel

Panels are Rust plugins. Draw in `ui`, which runs inside the frame's egui
pass for the active session; use `panels::window` (a movable, frameless
window whose position is remembered in the settings file) and
`panels::title_bar` (name, key hint, close button) so it looks and
behaves like the built-in ones.

```rust
use ac_plugin::{egui, panels, Ctx, Plugin};

#[derive(Default)]
pub struct Counter {
    show: bool,
    count: u32,
}

impl Plugin for Counter {
    fn name(&self) -> &str { "counter" }

    fn ui(&mut self, cx: &mut Ctx, egui: &egui::Context) {
        // The menu (or a script) may have asked us to open or close.
        if let Some(ask) = panels::take_open(cx.board, "counter") {
            self.show = ask.apply(self.show);
        }
        if !self.show { return; }
        // Read what you draw *before* drawing: the closure cannot borrow cx.
        let name = cx.try_client().map(|c| c.world.stats.name.clone());
        panels::window("counter", egui::pos2(300.0, 120.0), egui::vec2(220.0, 90.0), 200, 8)
            .show(egui, |ui| {
                panels::title_bar(ui, "counter", "Counter");
                ui.label(name.unwrap_or_else(|| "no session".into()));
                if ui.button(format!("clicked {}", self.count)).clicked() {
                    self.count += 1;
                }
            });
        if panels::closed("counter") {
            self.show = false;
        }
    }

    fn key(&mut self, _cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if pressed && key == egui::Key::F7 { self.show = !self.show; return true; }
        false
    }
}
```

Things the built-in panels do that yours can too:

* **Build a view first, then draw it.** Every panel in
  `crates/ac-plugin/src/panels` reads the session into a plain struct
  (`view(&Client)`), draws the struct, and turns the clicks into `Client`
  calls afterwards. The drawing code never touches the client, so it can
  run on canned data (the `Source::Demo` of `--demo-ui`) and in tests.
* **Icons.** `cx.icons().draw(ui, IconLayers::of(object), egui::Sense::click())`
  paints an item's icon; `IconLayers::single(spell.icon_id)` a spell's.
* **A key from the menu's list.** Panels the menu knows have an entry in
  `keys::ACTIONS` and ask `keys::bound("inventory", key)`; a plugin
  outside the crate compares against its own `egui::Key`, as above.
* **Register it.** Add `host.register(Box::new(Counter::default()))` to
  `bins/acviewer/src/plugins/mod.rs` (`builtin`) and, if it should run
  headless too, to `bins/acbot/src/main.rs` next to the others. Order
  matters for keys: the first plugin to return `true` takes the key.
* **Try it without a server.** `examples/plugin-template` drives a host
  with no session and a headless egui: `cargo run -p plugin-template`.
  Its test shows how to assert that the window was laid out.

A script cannot draw, but it can open and close a panel: the built-in
panels listen on the blackboard key `ui.open.<name>` (`board_set("ui.open.inventory", "toggle")`).

## I want to react to an event

Sessions report what happened as `ac_client::Event`s: chat lines (with
the server's message type), sounds, connection and placement, spellbook
changes, the character list, and, since autoplay reports its moves,
`Event::Autoplay { doing, text }` every time it switches to doing
something else ("fighting", "looting", "buffing", "healing",
"following", "idle"...).

**Rust:** `on_event` runs for every event of every session, before that
session's `tick`.

```rust
use ac_plugin::{Ctx, Event, Plugin};

impl Plugin for Watcher {
    fn name(&self) -> &str { "watcher" }

    fn on_event(&mut self, cx: &mut Ctx, ev: &Event) {
        match ev {
            Event::Chat { text, kind } if *kind == 3 => cx.log(format!("someone said: {text}")),
            Event::Autoplay { doing, text } if doing == "looting" => {
                cx.log(format!("session {} is {text}", cx.index));
            }
            Event::Placed { cell } => tracing::info!("in cell {cell:08X}"),
            _ => {}
        }
    }
}
```

**Rhai:** `on_event(ev)` with `ev.kind` and the fields of that kind:

| `ev.kind` | fields |
| --- | --- |
| `chat` | `text`, `chat_kind` |
| `sound` | `volume` |
| `connected`, `terminated` (`reason`), `refused` (`code`), `placed` (`cell`) | |
| `spell_learned`, `spell_forgotten` | `spell` |
| `characters` (`names`, `count`), `character_created` (`guid`, `name`), `character_create_failed` (`code`, `message`) | |
| `autoplay` | `doing`, `text` |

```rhai
fn on_event(ev) {
    if ev.kind == "autoplay" { log("[" + ev.doing + "] " + ev.text); }
    if ev.kind == "chat" && ev.text.contains("has been killed") { this.kills += 1; }
}
```

`scripts/examples/watch.rhai` is a complete example. Most of the world is
not an event but state: what is in view, health, the open container, the
target. Read it in `tick` (`cx.client().world`, or `me()`/`objects()` in
a script) and keep the previous value in the plugin struct or `this` to
notice a change (`autoloot.rhai` does exactly that to see a target die).

**Every process hears autoplay.** The host repeats each `Event::Autoplay`
on the blackboard topic `autoplay.event` as
`{"session", "name", "doing", "text"}`, so a plugin in another session or
another process (with `--bus`) reads it with `messages_on("autoplay.event")`
/ `messages("autoplay.event")`; a message from another process carries
`origin`.

## I want to start or stop sessions

A plugin can add sessions to the process it runs in and drop them
(what the Fleet panel's Sessions section does):

```rust
cx.start_session(ac_plugin::SessionSpec {
    account: "fleetbot1".into(),
    password: "testpass".into(),
    character: None,
    create: Some(ac_plugin::CreateSpec { name: "Fleetbot One".into(), template: Some("bow".into()), ..Default::default() }),
    role: ac_plugin::Role::Follower,
});
cx.stop_session(2);
```

The host applies both between frames: a start connects to the server
the process was started against (creating the character first when the
account lacks it) and appends the session; a stop disconnects and
removes it, the sessions after it move down one index, and every plugin
hears `session_removed(index)`, so keep per-session state in a map by
index and shift it there (`ac_plugin::shift_removed`). A script gets
the same by setting the blackboard keys `fleet.start` and `fleet.stop`
(`docs/multi-session.md`, "Starting a fleet from the client").

## I want to drive the character

Every action a player can take is a method on `ac_client::Client`, and
the script functions mirror them one to one (each does what the matching
`/command` in the console does).

**Rust**, in `tick` (once per frame per session) or in a `command`:

```rust
use ac_plugin::{Ctx, Plugin};

impl Plugin for Hunter {
    fn name(&self) -> &str { "hunter" }

    fn tick(&mut self, cx: &mut Ctx) {
        let Some(c) = cx.try_client() else { return };     // no session in --demo-ui
        if !c.placed() || c.attack_target.is_some() { return; }
        // Nearest attackable creature in view.
        let Some(me) = c.player.as_ref().map(|p| p.world_position()) else { return };
        let target = c.world.objects.values()
            .filter(|o| o.item_type & ac_world::item_type::CREATURE != 0)
            .filter(|o| o.object_desc_flags & ac_world::object_desc_flags::ATTACKABLE != 0)
            .filter(|o| o.health.unwrap_or(1.0) > 0.0)
            .filter_map(|o| {
                let p = o.position?;
                let world = ac_world::landblock_origin(p.cell) + p.local;
                Some((o.guid, world.distance(me)))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(guid, _)| guid);
        if let Some(guid) = target {
            c.attack(guid);          // moves into reach, keeps swinging until it dies
        }
    }

    fn command(&mut self, cx: &mut Ctx, name: &str, args: &str) -> bool {
        match name {
            "go" => {
                let c = cx.client();
                match ac_world::towns::parse_destination(args) {
                    Some(goal) => { c.travel_to(goal); }
                    None => cx.log(format!("go: unknown place {args}")),
                }
                true
            }
            "heal" => {
                let c = cx.client();
                if let Some(id) = c.spell_by_name("Heal Self") { c.cast(id); }
                true
            }
            _ => false,
        }
    }
}
```

The verbs to know: `use_by_name` / `interact(guid)` (double-click),
`pick_up`, `attack(guid)`, `toggle_combat`, `cast(spell)` / `cast_at`,
`say`, `take(guid)` / `close_container`, `buy` / `sell`, `give`,
`use_on(item, target)`, `travel_to(goal)`, `set_noclip`, `jump`. Read
`docs/plugins.md` ("Protocol facts") before writing a loop: actions are
asynchronous, one pickup at a time, a move-to can take seconds.

**Rhai**, the same thing:

```rhai
fn tick(dt) {
    let m = me();
    if !m.placed || m.target != () { return; }
    for o in objects() {                 // nearest first
        if o.is_creature && o.health > 0 { attack(o.guid); break; }
    }
}

fn command(name, args) {
    if name == "go" { return travel_to(args); }
    if name == "heal" { cast("Heal Self"); return true; }
    false
}
```

Or let autoplay do the fighting and steer it: `autoplay(true)`,
`fight_style("magic")`, `attack_spells(["Flame Bolt"])`, `team(true)`,
`follow(true)`, and read `autoplay_status()` or the `autoplay` events.

## I want to share state between sessions and processes

Every plugin and script in a process shares one `Blackboard`:

* **values** persist: `cx.board.set("leader", 1)` / `cx.board.get("leader")`
  (`board_set` / `board_get` in Rhai);
* **messages** live one frame: `cx.post("assist", json!({"guid": 77}))`
  this frame, `cx.board.messages_on("assist")` next frame in every
  session's hooks (`post` / `messages` in Rhai). Each message says who
  posted it: `from` is the session index.

With `--bus` on every process, the same calls reach every process on the
machine: posts and sets go out, others' come in as messages with
`from == ac_plugin::REMOTE` and `origin: Some("<process name>")`
(`m.origin` in Rhai; unit when local). Nothing else changes.

```rust
// Leader: say what we are fighting.
fn tick(&mut self, cx: &mut Ctx) {
    let target = cx.client().attack_target;
    if target != self.last { cx.post("assist", serde_json::json!({ "guid": target })); self.last = target; }
}
// Follower, any session, any process: fight the same thing.
fn tick(&mut self, cx: &mut Ctx) {
    let guid = cx.board.messages_on("assist").last().and_then(|m| m.value["guid"].as_u64());
    if let Some(g) = guid { cx.client().attack(g as u32); }
}
```

```rhai
// The same, in a script (scripts/examples/assist.rhai is the full one).
fn tick(dt) {
    for m in messages("assist") { if m.value.guid != () { attack(m.value.guid); } }
}
```

**From another process, in any language.** The hub speaks one JSON
object per line on `127.0.0.1:9500` (`$ACREBORN_BUS`). Start a game
process with `--bus` (the first one hosts), then:

```sh
# Watch what every character is doing, from a shell.
{ echo '{"kind":"hello","name":"watch"}'; cat; } | nc 127.0.0.1 9500
#   {"kind":"state","values":{...}}
#   {"kind":"post","from":"alice","topic":"autoplay.event","value":{"session":0,"name":"Alice","doing":"fighting","text":"fighting Drudge Skulker"}}
```

```python
import json, socket
s = socket.create_connection(("127.0.0.1", 9500))
s.sendall(b'{"kind":"hello","name":"py"}\n')
s.sendall(json.dumps({"kind": "post", "from": "py", "topic": "assist", "value": {"guid": 77}}).encode() + b"\n")
for line in s.makefile():
    msg = json.loads(line)
    if msg.get("topic") == "autoplay.event":
        print(msg["from"], msg["value"]["text"])
```

A `set` (`{"kind":"set","key":"leader","value":"alice"}`) is kept by the
hub and handed to everyone who joins later; a `post` is delivered to
whoever is connected now. In Rust, `ac_bus::BusClient::connect_or_host`
gives a process the same link the game processes use, and
`ac_plugin::Host::join_bus` a whole blackboard on it.

## I want a key, a setting, a command

```rust
use ac_plugin::{egui, Ctx, Plugin, Settings};

impl Plugin for Toggle {
    fn name(&self) -> &str { "toggle" }

    // A key while no text box has focus; true consumes it.
    fn key(&mut self, cx: &mut Ctx, key: egui::Key, pressed: bool) -> bool {
        if pressed && key == egui::Key::F8 { self.on = !self.on; cx.log(format!("toggle {}", self.on)); return true; }
        false
    }

    // "/toggle" or "/toggle on" in the chat box; true when handled.
    fn command(&mut self, cx: &mut Ctx, name: &str, args: &str) -> bool {
        if name != "toggle" { return false; }
        self.on = if args.is_empty() { !self.on } else { args == "on" };
        cx.log(format!("toggle {}", self.on));
        true
    }

    // Settings: one JSON file shared by every client on the machine
    // (`~/.config/acreborn/ui.json`), written every 30 s and on exit.
    fn load(&mut self, s: &Settings) { self.on = s.get("toggle.on").unwrap_or(false); }
    fn save(&self, s: &mut Settings) { s.set("toggle.on", self.on); }
}
```

Prefix your settings keys with the plugin's name. Any `serde` type goes
in (`s.set("toggle.options", &self.options)`), and `cx.settings` is the
same store if a hook needs it mid-frame.

In a script: `fn key(name, pressed)` with names like `"F8"`, and
`fn command(name, args)`. There is no settings hook, but a value kept
with `board_set` lives as long as the process, and a script can read
its own file with Rhai's standard library.

## Ship it

**As a script.** Copy the `.rhai` file into `~/.acreborn/scripts` (or
`$ACREBORN_SCRIPTS`) on any machine running the stock binaries; it
loads within a second, and `/scripts` lists it. Keep state in `this`;
top-level `let`s are not visible inside functions. Test it with the
harness below.

**As a Rust plugin.** Copy `examples/plugin-template` to a crate of your
own (a workspace member under `examples/` or `crates/`), keep the
`ac-plugin` dependency, and register your type in
`bins/acviewer/src/plugins/mod.rs` and `bins/acbot/src/main.rs`. Panels
go before the console so they see keys first. A plugin never needs to
touch the binaries beyond that one line.

**As a separate process.** Speak the bus protocol (above); start every
game process with `--bus`. Your process sees every topic the plugins and
scripts post on (`autoplay.event`, `autoplay.mate`, `assist`, your own),
and whatever it posts they read the next frame. It can be in any
language, run on its own clock, and crash without taking a session down.

## Test it

**A script, with no server.** `ac_script::testing::ScriptHarness` loads a
script and runs its hooks against a canned world (a `Recorder`
implementing the script `Api`), and records every action:

```rust
use ac_script::testing::ScriptHarness;

#[test]
fn autoloot_takes_the_corpse() {
    let mut h = ScriptHarness::from_file("scripts/examples/autoloot.rhai");
    h.me("target", 0x8000_0001i64).me("target_name", "Drudge Skulker").me("container_open", false);
    h.tick(0.1);
    h.me("target", ());                   // the target died
    h.ticks(30, 0.1);                     // the corpse takes a moment
    assert_eq!(h.calls(), ["[0] loot Corpse of Drudge Skulker"]);
    h.me("container_open", true).ticks(2, 0.1);
    assert_eq!(h.calls()[1..], ["[0] take_all", "[0] close_container"]);
    assert!(h.errors().is_empty(), "{:?}", h.logs());
}
```

`from_source("x.rhai", "fn tick(dt) { ... }")` for a script written in
the test, `from_dir` for a directory, `chat(text, kind)` and `event(&Event)`
for events, `command`, `press`/`key`, `session(i)` to switch sessions,
`deliver(from, origin, topic, value)` to hand it a bus message,
`objects(...)` for what is in view, `posted()`/`board(key)` for what it
shared, `script_logs()` for what it logged, `dir()` + `reload()` for hot
reloading. Set any other canned field on `h.api` directly (`h.api.items`,
`h.api.unappraised`, `h.api.sessions`).

**A Rust plugin, with no server.** A `Host` runs with no sessions (the
`--demo-ui` way): `host.frame(Vec::new(), 0, &events, dt, now)` delivers
events to `on_event` and runs `tick`, `host.command(Vec::new(), 0, "/hello")`
runs commands, `host.key(...)` keys, and `egui::Context::default().run_ui(RawInput::default(), |ctx| host.ui(Vec::new(), 0, ctx))`
draws panels headlessly (`egui.memory(|m| m.area_rect(Id::new("name")))`
says whether a window was laid out). `examples/plugin-template/src/lib.rs`
has the whole thing as one test. Hooks must cope with `cx.try_client()`
being `None` for this to work, which they should anyway.

**Two processes, for real.** `ac_bus::BusServer::bind("127.0.0.1:0")` in
a test gives a hub on a free port; two `Blackboard`s with
`attach_bus(BusClient::connect(addr, name))` stand in for two processes
(`crates/ac-plugin/src/lib.rs`, `blackboards_in_two_processes_share_posts_and_values`).

**Against the test server.** `docs/multi-session.md` and the project
memory describe the local ACE server; `acbot --connect ... --script
lines.txt` types commands one per second and `--log-chat` prints what
came back, so a plugin or script can be driven end to end without a
window.
