# Running several clients

Two ways: several sessions inside one `acviewer` process, or several
processes started by `aclauncher`. They compose: the launcher starts
processes, each of which may hold several sessions.

## Several sessions in one process

```
cargo run --release -p acviewer -- --connect 127.0.0.1 -a alice -v pw1 --character Alice \
    --client bob:pw2:Bob --client carol:pw3
```

* `--connect`, `-a`, `-v`, `--character` describe session 1.
* `--client ACCOUNT:PASSWORD[:CHARACTER]` adds a session on the same host;
  repeat it for more. Without a character name the first on the account
  is used.
* All sessions log in at once (`App::start_connect` calls
  `ac_client::Client::connect` for each) and tick every frame.

Switching which session the window shows:

* **Tab** cycles to the next session.
* `/switch N` (1-based) in the chat box picks one; `/clients` prints how
  many there are and which is shown; a plugin can set `cx.activate`.
* On a switch the camera pitch resets and the chat log gets a
  `Now showing session N (account)` line. Streaming then builds the
  blocks around the new character, one per frame, and the object
  instances are re-generated on that session's next `world.generation`
  change.

What the active session gets that the others do not: keys and the mouse
(`player::Input` from WASD/Shift/Space; right-drag turns), chat lines in
the overlay, sounds, the target bar and panels, landblock streaming and
drawing. Inactive sessions run `Client::tick` with
`player::Input::default()`: they keep their connection alive (echo and
acks), apply the server's messages, finish a server move-to, keep
swinging at their `attack_target`, drain their loot queue, and run every
plugin's `on_event` and `tick`. Their chat lines still go to the log at
`info` level, prefixed with the account.
They keep nothing on the GPU: no landblocks, meshes, particles or
animation state are built for them (a switch clears what the session
left behind and re-instances the new one). A process whose window is
only a follower's can pass `--render none` and draw no world at all, and
`--fps` caps the frame rate of any window; see "Rendering cost" in
[architecture.md](architecture.md).

## Several processes: the launcher

`aclauncher` (`bins/aclauncher`) is a small egui window: servers on the
left, the selected server's accounts in the middle (character name field,
Launch, Launch headless, Remove; Launch all), a process log at the bottom
(pid, account, exit status; Kill all).

```
cargo run --release -p aclauncher                  # open the window
cargo run -p aclauncher -- --dump-config           # config with defaults applied, resolved client, log dir
cargo run -p aclauncher -- --dry-run alice [--server NAME] [--character NAME] [--headless]
```

Each launch spawns one process (`launch::build_launch`):

```
<client_binary> --data-dir <data_dir> --connect <host:port> -a <account> -v <password> [--character <name>] [--mute] [--bus] [--fps N]
```

Two settings under the server list apply to every launch: **Share state
between clients** adds `--bus` (the first client hosts the loopback hub,
the rest join it, see below) and **Frame cap** adds `--fps N`. Both are
saved in `launcher.json` (`share_bus`, `fps`).

with stdout/stderr appended to `~/.acswarm/logs/<account>.log`.
"Launch headless" adds `--mute` (and will add `--headless` once acviewer
has it). The launcher never kills children on its own: removing an account
or closing the window leaves them running; Kill all is explicit.

Config, `~/.acswarm/launcher.json` (`config::Config`), written atomically
on every change:

```json
{
  "servers": [{ "name": "Local ACE", "host": "127.0.0.1", "port": 9000 }],
  "accounts": [{ "server": "Local ACE", "account": "alice", "password": "pw1",
                 "characters": ["Alice"], "last_character": "Alice",
                 "last_used": "2026-09-04T12:00:00Z" }],
  "data_dir": "/Users/me/Downloads/ac_data",
  "client_binary": [],
  "password_notice_dismissed": false
}
```

`client_binary` is the program plus leading arguments (`["/path/to/
acviewer"]` or `["cargo","run","-p","acviewer","--"]`); empty means the
`acviewer` next to the launcher binary if there is one, else `cargo run -p
acviewer --` from the workspace root. Passwords are plain text. "Add /
create" only adds an account: ACE creates it on first login, and
`acclient --create NAME` makes the first character.

## Cross-process bus

Sessions in one process share the plugin blackboard (`docs/plugins.md`);
processes do not, so a party split across several `acviewer`/`acbot`
processes could not coordinate. `--bus [ADDR]` links them through
`crates/ac-bus`, a local hub on loopback TCP:

```
cargo run -p acbot -- --connect HOST --client alice:pw1 --bus
cargo run -p acviewer -- --connect HOST -a bob -v pw2 --bus          # joins alice's hub
ACSWARM_BUS=127.0.0.1:9600 cargo run -p acbot -- ... --bus          # another bus
```

* `ADDR` is `HOST:PORT` or a bare port; empty means `$ACSWARM_BUS` or
  `127.0.0.1:9500`. The flag is off by default and processes without it
  are unaffected.
* **Auto-hosting.** `BusClient::connect_or_host` connects to the hub at
  the address or, when the connection is refused, starts a `BusServer` in
  this process and connects to it: the first process up is the hub, later
  ones join. When the hub's process exits, every client reconnects with
  backoff (0.1 s doubling to 2 s) and each also tries to bind the address;
  the one that wins re-hosts, seeded with the values it last saw, and the
  others' next attempt connects to it. Two processes can race for the port;
  the loser's bind fails with "address in use" and it simply connects a
  moment later. Posts made while a process has no link are dropped; `set`s
  are kept (latest per key) and sent on rejoin.
* **Protocol.** One JSON object per line: `{"kind":"hello","name":..}`
  from a joining client, answered by `{"kind":"state","values":{..}}`;
  then `{"kind":"post","from":..,"topic":..,"value":..}` and
  `{"kind":"set","key":..,"value":..}` in both directions. The hub forwards
  each post and set to every *other* connection (no echo) and keeps the
  values map. Anything that speaks this (a script over `nc`, say) can join.
* **In the blackboard.** `Host::attach_bus(client, name)` (or
  `Host::join_bus(addr, name)`, which does the connect-or-host too) hooks
  the client into `Blackboard::end_frame`: this frame's local posts go out
  tagged with the process name (the first account, or `pidN`), posts from
  other processes come in as messages readable next frame with
  `from == ac_plugin::REMOTE` and `origin: Some(process name)`,
  `Blackboard::set` publishes, and incoming sets and the join-time state
  update `values`. A plugin that already reads `messages_on("party.target")`
  therefore sees the whole party's posts without change; one that must not
  act on other processes' posts checks `Message::is_remote()`. Rhai scripts
  see `origin` on the message map.
* **Latency and ordering.** Local posts are readable at home the next
  frame and elsewhere one hub hop later (sub-millisecond on loopback, so
  usually the next frame there too). Messages from different processes are
  ordered per sender only. Sockets live on background threads; the main
  loop only drains a channel.

## Playing one, the rest following

The team rules (`ac_client::autoplay`, Autoplay panel → Team) know a
leader. Without one it is whoever's name sorts first and nobody follows
anyone; tick **lead** on the client you play by hand (or call
`team_lead(true)` from a script) and every other session on the team
comes to you, keeps within a few metres (the "follow" distance), fights
what turns up within the "fighting within" radius of you (25 m by
default; both in the Autoplay panel's Team section, or
`follow_distances(keep, fight)` from a script), and:

- flies when you fly (Y) and lands when you land -- a follower's
  no-clip follows the leader's, and it flies straight at you;
- walks straight after you within 120 m, letting the steering find
  the way round walls, and plans a journey to you (portals included)
  when you have got further than that, planning again as you move on;
- drops a fight once you are more than 40 m away;
- accepts your fellowship invitations, which your client sends even
  with autoplay off (the leader's fellowship housekeeping runs whenever
  the team is on).

`scripts/examples/follow.rhai` sets a follower up with one `/follow`.
Every process has to be on the bus (`--bus`) for the sessions to hear
each other; sessions in one process hear each other anyway.

## Loot rules and who salvages

Autoplay's Loot section (and `loot_rules()` / `loot_rule_add(query,
action)` / `loot_rules_clear()` from a script) holds ordered rules: a
search in the inventory's language and what to do with an item it
matches. The first rule that matches decides; nothing matching is left
on the corpse. Names under "always take" and "never take" come before
the rules. The rules are part of the autoplay config, so they are saved
with it and handed to every session of the process as it appears.

| action | what happens |
|---|---|
| keep | picked up and kept |
| salvage | picked up, tagged, then salvaged by the team's salvager |
| sell | picked up and tagged for the next run to town |
| skip | left where it is |

The search language, in full (the fold under the rule list shows the
same):

- a **word** matches the name, material, kind, a spell name or a slot
  word (`ring`, `bracelet`, `hauberk`); `"epic life magic"` is a phrase;
- a space means **and**; `or`; `not x` or `-x`; parentheses group:
  `a b or c` is `(a and b) or c`;
- **numbers**: `value>250`, `al>=200`, `dmg>10`, `ws<6`, `wield<=150`
  (`level` and `req` say the same), `burden`, `speed`, `mana`, `sc`,
  `uses`, `tinks`, `stack`, `atk`, `def`;
- **cantrips** by tier, read off the spell names on the item:
  `epics>=2`, `legendaries>=1`, `majors`, `moderates`, `minors`,
  `cantrips` (any tier), `spells>=3` (everything on it), `tier:epic`
  (at least one);
- **slots** from where the item is worn: `slot:ring`, `slot:neck`,
  `slot:bracelet`, `slot:head`, `slot:chest`, `slot:abdomen`, `arms`,
  `hands`, `legs`, `feet`, `trinket`, `cloak`, `sigil`, `melee`,
  `shield`, `missile`, `ammo`, `wand`, `twohanded`;
- one field: `spell:blood`, `type:armor`, `mat:iron`, `skill:sword`;
  flags `wielded`, `unappraised`.

Examples: a Hauberk with Epic Life Mastery is `hauberk "epic life"`
(spelt the way the cantrip is on the item, `spell:"epic life magic"`
also does); any ring with multiple epics is `slot:ring epics>=2`; more
than two epics is `epics>2`. A rule the parser cannot make sense of is
shown in red with what is wrong (`Query::check`); it still runs, meaning
as much as it can.

**Who salvages.** Every session on the team says its Salvaging skill
(buffs counted) and whether it carries an Ust, and everyone reaches the
same answer: the highest Salvaging with an Ust, ties to the name that
sorts first (`salvager()` from a script says who; the Autoplay panel
shows it). Between fights, the salvager salvages what it carries tagged
`salvage`; everyone else walks to the salvager when it is within 30 m
and not fighting and hands its tagged items over, one every few
seconds. The receiver runs the rules again on what arrives, so a handed
item that matches a salvage rule is salvaged on the next pass. Salvage
bags are never salvaged again and only handed on when a rule names
them. An item refused three times (the server would not take it, or
would not salvage it) is kept instead, and the log says so.

The server only lets one player give another an item when the receiver
has "Let other players give you items" on (ACE `AllowGive`); the team
rules switch it on for every teammate, along with the fellowship
options, and the giver has to be within use range. The `salvage` and
`hand off` switches in the Loot section turn either half off.

## Fleet view

The Fleet panel (menu → Fleet; no key out of the box, bindable there;
`crates/ac-plugin/src/panels/fleet.rs`) is the leader's overview: one
row per character being played, in this process and in every other
process on the bus, with the controls a leader needs.

Each row shows the name (a gold star marks the leader, "me" the
session the window shows, "here" or the process name says where it is
played), level, health, stamina and mana bars, XP an hour, where it is
(the nearest town, the distance to it when out of town, and the
landmark stood at), how far it is from the leader, what its rules are
doing ("fighting Drudge Skulker"), and flags: following, flying, in a
fellowship, dead. The leader sorts first, the rest by name. A row of
this process is clickable and switches the window to that session
(what Tab does, chosen by name). The header sums it up: sessions,
alive, mean health, XP an hour over everyone. **Compact** (a checkbox
in the header, kept in the settings as `fleet.compact`) draws one line
per character instead of the table, small enough to leave open on the
leader's screen.

Controls, per row and for everyone at once: **autoplay** on or off,
**follow** on or off (turns the team rules on too), **regroup**
(everyone: autoplay and follow on, and whatever they were fighting let
go, so they come to the leader now) and **stop** (autoplay off, the
journey and the fight cancelled). On a session of this process they act
at once. On another process's session the panel posts
`{"process", "session", "name", "action"}` on the bus topic
`fleet.request`, and the team plugin in that process applies it to the
session named (`ac_plugin::team::Request`: `autoplay_on`,
`autoplay_off`, `follow_on`, `follow_off`, `regroup`, `stop`). A script
or anything else on the bus can post the same.

Where the rows come from: this process's sessions are read directly
(`team::describe`, the same word the team plugin speaks), whether or
not their team rules are on. Other processes' sessions are heard on
`autoplay.mate`, which a session only speaks with its team rules on
(Autoplay panel → Team, or `team_enabled(true)` from a script), so a
bot that is not on the team does not appear; what each is doing comes
from `autoplay.event`. The mate word carries, besides what the rules
need, the level, total and unspent XP, stamina and mana fractions, and
whether autoplay and follow are on (all `#[serde(default)]`, so older
speakers still parse). XP an hour is not on the wire: every process
works it out itself from the totals it has seen, a sample every 5 s
kept for 15 minutes, and quotes nothing for a character it has watched
less than 30 s. A process that goes quiet for 6 s drops off the panel.

`acviewer --demo-ui` opens the panel on four sample characters (two
here, two in a process called `bob`, one of them dead), with a roster
of three accounts under them.

### Starting a fleet from the client

Everything above assumes the followers were started on the command
line. The Fleet panel's **Sessions** section starts them from the
client being played, and remembers them, so the next launch is one
click:

1. Play as usual (`acviewer --connect HOST -a ACCOUNT -v PASSWORD`, or
   from the launcher). Open the menu, click **Fleet**, unfold
   **Sessions**.
2. **Add an account.** Type the follower's account and password, the
   character's name, pick its role (follower, leader, manual) and, with
   **create if missing** ticked, the template, heritage, sex and town
   to make it with when the account has no character of that name (the
   choices `acbot --create` accepts: Adventurer, Bow Hunter,
   Swashbuckler, Life Caster, War Mage, Wayfarer, Soldier; Holtburg,
   Shoushi, Yaraq, Sanamar). Click **Add**. ACE creates the account
   itself on its first login, so a new name is fine. Without **create
   if missing** the name is the character to enter with, or, left
   blank, the account's first.
3. **Start** the row (or **Start all followers**). The client logs the
   account in as another session of this process, against the same
   server. The status column follows it: *starting*, *connecting*,
   *character missing: creating NAME*, *in world as NAME*, or *failed:
   why* (the server's answer, or what went wrong before it).
4. Tick **I lead**. The session being played gets the team rules on
   with lead (what `team_lead(true)` does); each follower, the moment
   its character stands in the world, gets team, follow and autoplay
   on (what `scripts/examples/follow.rhai` does) and comes to you. A
   leader row does the same as **I lead**; a manual row gets nothing.
   Rows for sessions in the world are the same characters as the table
   above (their role is captioned next to the name); click a running
   account to switch the window to it, **Stop** to disconnect and drop
   it, **Remove** to forget it.

The roster is kept in the settings file (`~/.config/acswarm/ui.json`,
`fleet.roster`, one entry per account: `account`, `password`,
`character`, `create` `{name, template, town, heritage, sex}`, `role`)
and the leading account under `fleet.lead_account`, so on the next
launch the rows are there and **Start all followers** brings the fleet
back with the roles applied. **Passwords are stored in plain text**, as
the launcher stores them: a convenience for a private server, not a
place for a password that matters.

Under the hood a plugin asks the host for sessions through
`Ctx::start_session(SessionSpec)` / `Ctx::stop_session(index)`
(`ac_plugin::Requests::{start_sessions, stop_sessions}`); the viewer
connects (`ac_client::Client::connect` with the `--connect` host and
`Client::create_when_missing` for the creation) and appends a session,
or disconnects and removes one, between frames. A removed session's
successors move down one index and every plugin hears
`Plugin::session_removed(index)` (the team, party, autoplay, fleet and
script plugins shift what they keep by session). `acbot` applies starts
the same way and ignores stops with a warning. A script or the command
line drives the same path through two blackboard keys: `fleet.start`
(a session spec or a list of them, each added to the roster and
started) and `fleet.stop` (an account or a list); with `--bus`, add
`"process": NAME` so only that process acts. `acviewer --fleet-start
ACCOUNT:PASSWORD:CHARACTER[:TEMPLATE[:TOWN[:HERITAGE[:SEX]]]]` (headless,
with `--screenshot`) sets `fleet.start` once session 1 is placed and
`fleet.stop` `--fleet-stop-after` seconds later, which is how the flow
is tested:

```
acviewer --connect 127.0.0.1:9000 -a LEADER -v PASSWORD --mute --screenshot out.png \
    --fleet-start fleetbot1:testpass:"Fleetbot One":bow:holtburg --fleet-stop-after 45
```

## Items across characters

The Items window (menu → **Items (all characters)**; no key out of the
box, bindable there; the **items** button in the Fleet window;
`crates/ac-plugin/src/panels/holdings.rs`) searches every character's
inventory on every account at once: the sessions of this process, the
sessions of every other process on the bus, and characters that are
not logged in at all, so "which character has a Hauberk with Epic Life
Magic Aptitude" or "any ring with two or more epics" is one search.

**What is published.** Each session takes a snapshot of its own items
(`ac_client::holdings::CharacterHoldings`: account, character, guid,
`taken_at` in unix seconds, `online`, and one `HoldingRecord` per item,
the owned form of `ItemStats` with the appraised numbers and spells
when the item has been appraised, and only its name, kind, value and
burden when it has not; nothing is appraised just for the snapshot)
when its inventory has arrived, whenever an item is added, removed,
moved, wielded or appraised (at most once every 2 s), every 30 s
regardless, and once more with `online: false` when the session ends
(a logout, a dropped connection, a stopped session). The snapshot is
`set` on the blackboard, and so on the bus, under
`holdings.<account>/<character>` (the account in lower case), which the
hub keeps and hands every process on joining, so a client started
later sees everyone already playing. Without `--bus` the snapshots stay
in the process and the files.

**Where the files live.** Every snapshot is also written to
`<cache dir>/holdings/<account>/<character>.json` (the cache dir is
`$ACSWARM_CACHE_DIR` or `~/.cache/acswarm`, the same as the world
grid's; names are made file-system safe). Every process reads the
whole directory once at start, so a character that is not logged in
is searchable from its last snapshot; the newer `taken_at` wins
wherever two snapshots of one character meet (a file and a bus value,
two processes). A snapshot counts as **online** while it says so and
is under 90 s old, so a process that died without saying goodbye fades
out after its last heartbeat; the Updated column says "online" or how
long ago the snapshot was taken.

**Searching.** The search line is the inventory's own
(`ac_client::items::Query`: words, `spell:`, `type:`, `mat:`, `skill:`,
`slot:ring`, `tier:epic`, `wielded`, `unappraised`, numbers such as
`al>=300` and `epics>=2`, `or`, `not`, parentheses and quoted phrases;
the `?` beside the box lists them). The table shows the character
(green when online), account, item, where it sits (worn, the main
pack, or the side pack's name), value, spells and when the snapshot was
taken; a click on a column sorts by it, a click on a row shows the
item's summary underneath, and the header counts characters, items and
how many are not yet appraised. A search that needs appraised numbers
(damage, armor, spells, cantrip counts) cannot match an unappraised
item, and the window says so; **Refresh** publishes this process's
snapshots again and posts `holdings.request` (`{"all": true}`, or
`{"account", "character"}` for one), on which every session appraises
what it has not (`Client::appraise_all`, one item at a time) and
publishes as the answers come in. Both are rate-limited to once every
10 s. From code, `Client::holdings_search(line)` searches the same
store (`ac_client::holdings::store()`) and returns the hits with the
account, character, online flag, place and `ItemStats`; from a script,
`find_items_everywhere(query)` returns the same hits as maps of
`account`, `character`, `online`, `place`, `taken_at` and `stats` (the
`find_items` item map), e.g.

```rhai
for hit in find_items_everywhere("slot:ring epics>=2") {
    log(hit.character + " (" + hit.place + "): " + hit.stats.name);
}
```

## Resources

What one process shares between its sessions, and what the Nth session
costs. `crates/ac-client/examples/session_cost.rs` measures it without a
server:

```
AC_DATA_DIR=... cargo run --release -p ac-client --example session_cost [N] [BLOCK]
```

It opens the archives once, starts N `Client`s (to a port nobody
listens on), stands each character in a landblock (Holtburg by default)
and makes each do what allocates: walk into a wall (the block's
collision), plan around it (the nav graph), plan a journey (the world
grid) and ask the wide planner for a route (the pathfinder thread), then
100 idle ticks each. After each phase it prints the process RSS (`ps`)
and, on macOS, the physical footprint (`vmmap`), which leaves out the
file-backed archive pages.

* **DAT archives.** `ac_dat::DatArchive` mmaps the files and walks the
  directory into a sorted entry table (about 1M entries for the cell
  archive: 23 MB private, and the directory nodes it touched, some 350 MB
  of file-backed pages that the OS shares with every process on the
  machine and drops under pressure). Within a process every session
  shares one `Rc<ac_scene::Assets>`: one mapping, one entry table, one
  set of decoded-asset caches, one 32-entry assembled-landblock LRU.
  `Assets` now holds the archives as `Arc<DatArchive>`, so another
  thread's `Assets` (`Assets::with_archives(assets.archives())`) shares
  the mapping and the entry table too, with caches of its own.
* **Pathfinder.** One planner thread per process (`ac_client::pathfinder`),
  started by the first session that asks for a route. Every session's
  `Pathfinder` handle sends asks down the same channel and gets answers
  on its own; the thread keeps the newest ask per handle and the last
  four assembled neighbourhoods (nine blocks of collision each, about
  13 MB), so a party walking together plans on one. Before, each session
  started a thread that opened the archives again: about 390 MB of RSS
  per session, of which 23 MB was a second entry table.
* **Block collision and nav graphs.** `Assets::block_collision(block)`
  (`ac_scene::blockcache`) holds the process's `CollisionWorld` per
  landblock, most recent 48, with the nav graphs built on it, one per
  capsule shape. A `Player` keeps `Rc`s to the blocks it stands in or
  next to and lets go of the rest as it walks on, so a session's own
  hold on blocks is bounded (it was not before: a character that
  crossed the map kept every block's collision it had built).
* **World grid.** `Assets::world_grid()` reads the cached
  `~/.cache/acswarm/worldgrid.bin` (about 16 MB in memory) once per
  process; journeys share it.
* **Scene caches are per process.** The viewer keeps one `mesh_cache`,
  `gpu_meshes`, `palettes`, motion `tables`, particle `fx` and
  `loaded_blocks` on the `App`, shared by every session; only the active
  session's surroundings are streamed and drawn, and only per-session
  state (`anims`, `pickables`) lives on each `Net`. Two sessions standing
  in Holtburg therefore hold one copy of its meshes.
* **Memory, measured.** `session_cost` (release, Apple Silicon, 16 KB
  pages, Holtburg), RSS after the last phase:

  | sessions | before (RSS) | after (RSS) | after (footprint) |
  |---:|---:|---:|---:|
  | 1 | 795 MB | 421 MB | 66 MB |
  | 4 | 2019 MB | 422 MB | -- |
  | 8 | 3653 MB | 422 MB | 67 MB |
  | 16 | -- | 424 MB | 69 MB |

  Before, the Nth session cost about 410 MB of RSS (390 MB of it the
  pathfinder's own archives, 16 MB its world grid, 2-3 MB its block
  collision and graph). After, the first session still pays for the
  world grid (24 MB), the planner's neighbourhood (13 MB) and the
  block's collision (4 MB), once, and the Nth session costs about
  0.2-0.3 MB: its `Client`, `Player` and per-session channel ends. The
  372 MB the archives cost the RSS is shared file-backed pages; the
  private footprint of the whole process with 16 sessions is 69 MB.
  The earlier `vmmap` figures for the windowed viewer (graphics
  allocations 11-15 MB, heap 50-80 MB, process footprint 230-440 MB) are
  in addition to this for `acviewer`.
* **Audio.** One `ac_audio::Audio` device per process, cloned into every
  session; only the active session's sounds play. `--mute` skips opening
  the device (and `--screenshot` implies it).
* **Tick rates.** Windowed, every session ticks once per presented frame
  (vsync, `PresentMode::AutoVsync`), so a 60 Hz display gives 60 ticks/s
  per session; `dt` is clamped to 0.1 s so a stall does not teleport the
  character. `acbot` ticks every session `--hz N` times a second
  (`--tick-hz`; default 20, the game's pace); a process of followers
  gets by on `--hz 10`, which halves its share of the CPU. Headless
  `--screenshot` loops with a 1 ms sleep and logs `ticks/s`. On the wire
  a moving character sends AutonomousPosition four times a second,
  MoveToState on input changes, an echo every 5 s and an ack every 2 s,
  so network cost per session is small.
* **CPU.** An idle `Client::tick` is 2-4 us per session (`session_cost`),
  so 20 sessions at 20 Hz idle in well under a millisecond a second;
  what costs is walking (collision per step, microseconds) and planning:
  scene assembly (`build_landblock`, ~0.5 s a block) runs once per
  process per block now, on the planner thread for neighbourhoods and on
  the session thread for the block a character first walks into.

## Known limits

* Closing the window (or Escape) sends a clean disconnect for the active
  session only; the others just stop, and ACE drops them on its own
  timeout.
* `switch_to` does not clear the GPU: landblocks the previous session
  streamed stay uploaded (and drawn, if in view) until that session is
  active again and unloads them, since `gpu.blocks` is keyed by block id
  while each `Net` only tracks its own `loaded_blocks`. Switching between
  characters in the same area is seamless; far-apart ones leave stray
  geometry.
* Keys steer only the active session; a plugin cannot yet hand a
  `player::Input` to an inactive one (it can set `client.move_to`).
* The headless `--screenshot` script (`--use`, `--attack`, ...) acts on
  session 1; extra `--client`s connect and tick but are not scripted.
* A session stopped from the Fleet panel leaves what it streamed on the
  GPU like a switch does, and `acbot` cannot stop sessions at all.
* All sessions in one process must be on the same host (`--connect`); use
  the launcher for several servers.
* GPU-side caches are duplicated per session (above); memory grows with
  the number of sessions that have seen distinct areas.
* One window, one active view: there is no split screen. Run several
  launcher processes for several windows.
* The launcher's "Launch headless" only mutes; a truly windowless
  `acviewer --headless` does not exist yet.
