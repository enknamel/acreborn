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
<client_binary> --data-dir <data_dir> --connect <host:port> -a <account> -v <password> [--character <name>] [--mute]
```

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

## Resources

* **DAT archives.** `ac_dat::DatArchive` mmaps the files. Within a process
  every session shares one `Rc<ac_scene::Assets>` (one mmap, one set of
  decoded-asset caches, one 32-entry assembled-landblock LRU). Across
  processes the mappings are separate but back onto the same page cache,
  so N launcher processes cost one copy of the archive pages plus N sets
  of decoded assets.
* **GPU caches are per session for now.** Each `Net` keeps its own
  `mesh_cache`, `gpu_meshes`, `palettes`, `anims`, motion `tables`,
  particle `fx` and `loaded_blocks`, so two sessions standing in Holtburg
  hold two copies of its meshes. Only the active session uploads and draws;
  the others' caches are simply retained. Sharing them across sessions is a
  planned change.
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
