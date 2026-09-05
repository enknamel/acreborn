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

with stdout/stderr appended to `~/.acreborn/logs/<account>.log`.
"Launch headless" adds `--mute` (and will add `--headless` once acviewer
has it). The launcher never kills children on its own: removing an account
or closing the window leaves them running; Kill all is explicit.

Config, `~/.acreborn/launcher.json` (`config::Config`), written atomically
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
ACREBORN_BUS=127.0.0.1:9600 cargo run -p acbot -- ... --bus          # another bus
```

* `ADDR` is `HOST:PORT` or a bare port; empty means `$ACREBORN_BUS` or
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

## Resources

* **DAT archives.** `ac_dat::DatArchive` mmaps the files. Within a process
  every session shares one `Rc<ac_scene::Assets>` (one mmap, one set of
  decoded-asset caches, one 32-entry assembled-landblock LRU). Across
  processes the mappings are separate but back onto the same page cache,
  so N launcher processes cost one copy of the archive pages plus N sets
  of decoded assets.
* **Scene caches are per process.** The viewer keeps one `mesh_cache`,
  `gpu_meshes`, `palettes`, motion `tables`, particle `fx` and
  `loaded_blocks` on the `App`, shared by every session; only the active
  session's surroundings are streamed and drawn, and only per-session
  state (`anims`, `pickables`) lives on each `Net`. Two sessions standing
  in Holtburg therefore hold one copy of its meshes.
* **Memory, measured** (release build, Apple Silicon, one session in the
  Academy dungeon, `vmmap`): graphics allocations 11-15 MB (about 400
  materials, 44 MB of texture data before compression by the driver),
  heap 50-80 MB, process footprint 230-440 MB of which the bulk is the
  DAT archive pages, file-backed and shared by every process on the
  machine. A headless `acbot` session is about 30 MB private. Before
  `Gpu::flush` a headless `acviewer --screenshot` run leaked every
  per-tick particle upload until the final submit (2.7 GB after 30 s);
  windowed sessions were never affected.
* **Audio.** One `ac_audio::Audio` device per process, cloned into every
  session; only the active session's sounds play. `--mute` skips opening
  the device (and `--screenshot` implies it).
* **Tick rates.** Windowed, every session ticks once per presented frame
  (vsync, `PresentMode::AutoVsync`), so a 60 Hz display gives 60 ticks/s
  per session; `dt` is clamped to 0.1 s so a stall does not teleport the
  character. Headless `--screenshot` loops with a 1 ms sleep and logs
  `ticks/s`. On the wire a moving character sends AutonomousPosition four
  times a second, MoveToState on input changes, an echo every 5 s and an
  ack every 2 s, so network cost per session is small.
* **CPU.** `Client::tick` is cheap; scene assembly (`build_landblock`) is
  the expensive step and runs for the active session only, one block per
  frame.

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
* All sessions in one process must be on the same host (`--connect`); use
  the launcher for several servers.
* GPU-side caches are duplicated per session (above); memory grows with
  the number of sessions that have seen distinct areas.
* One window, one active view: there is no split screen. Run several
  launcher processes for several windows.
* The launcher's "Launch headless" only mutes; a truly windowless
  `acviewer --headless` does not exist yet.
