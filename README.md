# acreborn

[![CI](https://github.com/enknamel/acreborn/actions/workflows/ci.yml/badge.svg)](https://github.com/enknamel/acreborn/actions/workflows/ci.yml)

A from-scratch Rust reimplementation of the Asheron's Call client, built to
play against the [ACE](https://github.com/ACEmulator/ACE) server emulator.

Status: **Phase 4** (world). The DAT reader is verified byte-for-byte
against ACE; thirteen asset types decode every file in both archives;
`acviewer` renders outdoor landblocks with scenery, interiors and models,
and in `--connect` mode logs in to a local ACE server and walks your
character around the live world with server-accepted movement, with a
chat overlay for talking to the server and other players.
See `docs/` for the subsystem specs and `reference/README.md` for how the
reverse-engineering material is regenerated.

```
export AC_DATA_DIR=~/Downloads/ac_data     # acclient.exe + client_*.dat live here
cargo run --release -p acdat -- $AC_DATA_DIR/client_portal.dat info
cargo run --release -p acdat -- $AC_DATA_DIR/client_portal.dat ls --kind GfxObj | head
cargo run --release -p acdat -- $AC_DATA_DIR/client_cell_1.dat cat A9B4FFFF | xxd | head
cargo run --release -p acdat -- $AC_DATA_DIR/client_portal.dat decode 13000000   # Region as JSON

cargo run --release -p acviewer -- --landblock A9B4 --radius 1      # fly around Holtburg
cargo run --release -p acviewer -- --model 02000001                 # inspect a Setup
cargo run --release -p acviewer -- --emitter 3200026E --screenshot torch.png   # a particle emitter (or a 0x33 script, or a Setup's default script)
cargo run --release -p acviewer -- --landblock A9B4 --screenshot out.png   # headless render
# a new character from the CharGen table: race,gender,hair,eyes,nose,mouth,skin[,hair_color,eye_color]
cargo run --release -p acviewer -- --chargen aluvian,m,3,0,0,0,0.5 --camera 0,0.8,1.62,180,0 --screenshot face.png
```

`--chargen` dresses the race's Setup (or `--model`) the way the server would
for a freshly created character: hair style (option index; 0 is bald for
human males), eyes/nose/mouth strips and skin shade (0..1), with the
optional hair and eye colour indices. `--camera x,y,z,yaw_deg,pitch_deg`
works with any `--screenshot`; the model stands at the origin facing -Y.

Viewer controls: right-drag to look, WASD to move, Q/E down/up, Shift to
boost, Escape to quit.

Against a local ACE (`tools/ace/up.sh` starts one in Docker):

```
# headless: log in, create a character, enter the world, print messages
cargo run --release -p acclient -- -h 127.0.0.1 -a myaccount -v mypassword --create Reborn
# admin accounts can teleport: --say "@telepoi holtburg"
# play: third-person view, WASD walks the character, right-drag turns, Shift walks
# slowly, Enter opens the chat box (server commands start with @), left click
# selects and appraises an object, double-click uses it (doors, NPCs), picks
# it up (ground items) or puts it on / takes it off (in the inventory panel,
# toggled with I; K shows the skills panel, P the spellbook, where a
# double-click casts). C toggles melee combat:
# double-click a creature to attack it until it dies; double-click its corpse
# to loot (Take all / Close). Space jumps. Sounds play unless --mute.
cargo run --release -p acviewer -- --connect 127.0.0.1 -a myaccount -v mypassword
```

## Launcher

`aclauncher` is a small desktop launch manager: a list of servers on the
left, the accounts on the selected server in the middle (each with an
optional character name and Launch / Launch headless / Remove buttons, plus
Launch all), and a process log at the bottom (pid, account, exit status,
Kill all).

```
cargo run --release -p aclauncher                  # open the window
cargo run -p aclauncher -- --dump-config           # print the config with defaults applied
cargo run -p aclauncher -- --dry-run myaccount     # print the command a launch would run
```

Each launch spawns a separate client process:

```
<client_binary> --data-dir <data_dir> --connect <host:port> -a <account> -v <password> [--character <name>] [--mute]
```

with its output appended to `~/.acreborn/logs/<account>.log`. Nothing is
killed when an account is removed or the launcher exits. "Launch headless"
adds `--mute` (and will add `--headless` once acviewer has it). "Add /
create" is just adding an account: ACE creates it on the first login.

The config lives in `~/.acreborn/launcher.json`:

```json
{
  "servers": [{ "name": "Local ACE", "host": "127.0.0.1", "port": 9000 }],
  "accounts": [{ "server": "Local ACE", "account": "myaccount", "password": "mypassword",
                 "characters": ["Reborn"], "last_character": "Reborn",
                 "last_used": "2026-09-04T12:00:00Z" }],
  "data_dir": "/Users/me/Downloads/ac_data",
  "client_binary": [],
  "password_notice_dismissed": false
}
```

`client_binary` is the program plus leading arguments; empty means the
`acviewer` next to the launcher binary if there is one, else
`cargo run -p acviewer --` from the workspace root. **Passwords are stored
in plain text** in this file; it is only as private as your home directory.

## Headless sessions (acbot)

`acbot` runs many sessions in one process with no window and no GPU: the
"as many clients as possible on one computer" case. Each `--client` logs in,
enters the world and is ticked `--tick-hz` times a second (default 20; the
loop sleeps in between, and 4 Hz is enough for the server) with no keyboard
input, so plugins and the server's own move-to drive movement. The console
plugin (`ac_plugin::console::Console`, the same one the viewer registers)
answers `/commands`.

```
cargo run --release -p acbot -- --data-dir $AC_DATA_DIR --connect 127.0.0.1 \
    --client bot1:pw --client bot2:pw:Reborn --client bot3:pw \
    --tick-hz 10 --duration 300 --log-chat \
    --say "@telepoi holtburg" --say /combat --script walk.txt
```

`--say LINE` (repeatable) is typed by every session once its character is
placed, one line per second; `--script PATH` appends the lines of a text
file (blank lines and `#` comments skipped). Lines starting with `/` go to
the plugin host as commands, anything else is said to the server (`@`
commands included). `--duration 0` (the default) runs until Ctrl-C, which
disconnects every session cleanly; the run also ends when every session has
been terminated or refused. At start it prints the session count and tick
rate, and every 10 s one status line per session (placed?, cell, health,
target). `--log-chat` prints chat lines prefixed with the account;
`RUST_LOG=info` shows the connection log as well.

Workspace: `crates/ac-dat` (container), `crates/ac-formats` (asset
decoders), `crates/ac-scene` (GPU-free mesh assembly, collision),
`crates/ac-net` (protocol, sans-IO session), `crates/ac-world` (object
table and movement), `crates/ac-client` (headless game session),
`crates/ac-plugin` (plugin interface, host and console plugin), `bins/acdat`
(CLI), `bins/acviewer` (wgpu viewer and client), `bins/acclient` (headless
login client), `bins/acbot` (headless multi-session runner),
`bins/aclauncher` (launch manager).

Debugging aids: `RUST_LOG=acviewer=debug`, `ACV_HIDE_STATIC=1` (draw only
server objects), and in connected `--screenshot` mode `--walk`, `--say`,
`--click x,y`, `--use NAME`, `--attack NAME`, `--loot [NAME]`, `--buy NAME`,
`--sell NAME`, `--cast NAME`, `--jump`, `--snap-at SECS` and `--camera` to
script a session headlessly (`--say` may repeat; admin commands such as
`@create 7`, `@ci 314` or `@smite all` are handy for setting up a scene).
Double-clicking a vendor opens its shop (buy from the stock, sell from the
pack); using a scroll learns its spell.

Game data and the original executable are not distributed with this
repository and are gitignored.

## License

Dual-licensed under MIT or Apache-2.0, at your option. Game data, the
original executable and the ACE emulator sources (AGPL, used only as a
reference) are not part of this repository.
