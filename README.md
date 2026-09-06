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
See [`docs/architecture.md`](docs/architecture.md) for the crate map and
data flow, [`docs/plugins.md`](docs/plugins.md) for writing a plugin,
[`docs/multi-session.md`](docs/multi-session.md) for running several
clients, [`docs/game/mechanics.md`](docs/game/mechanics.md) for the game
rules the client must follow (magic, combat, advancement, death, trade),
`docs/subsystems/` for the format and protocol specs, and
`reference/README.md` for how the reverse-engineering material is
regenerated.

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

With several sessions in one process, the `party` plugin coordinates them
from the chat box: `/leader` makes the current session the leader (`/leader
N` picks another), `/follow` makes every other session run after it whenever
it is more than 3 m away (around corners too: a blocked line plans a route
on the landblock's walkable grid, see `ac_scene::nav`), `/assist` makes them attack whatever the leader
attacks (entering melee mode first), and `/lootall` makes each session open
the corpse of its last kill and take everything in it; each switch takes
`on`/`off` or toggles. `/party` prints the state, and the Party window lists
every session (name, level, health, distance to the leader, target) with
Switch and Lead buttons. The leader's target is broadcast on the bus as
`party.target`; the leader's index is the blackboard value `party.leader`.
Sessions in different processes coordinate the same way when each is
started with `--bus`: the first process hosts a loopback hub, the others
join it, and posts and blackboard values flow between them (see
[docs/multi-session.md](docs/multi-session.md)).

Against a local ACE (`tools/ace/up.sh` starts one in Docker):

```
# headless: log in, create a character, enter the world, print messages
cargo run --release -p acclient -- -h 127.0.0.1 -a myaccount -v mypassword --create Reborn
# admin accounts can teleport: --say "@telepoi holtburg"
# play: third-person view, WASD walks the character, right-drag turns, Shift walks
# slowly, Enter opens the chat box (server commands start with @), left click
# selects and appraises an object, double-click uses it (doors, NPCs), picks
# it up (ground items) or puts it on / takes it off (in the inventory panel,
# toggled with I; K shows the character sheet with Raise buttons that spend
# unassigned XP on attributes, vitals and skills and Train buttons that
# spend skill credits; drag an item from the inventory onto a side pack or the
# Pack header to move it (a stack onto another of its kind merges them;
# right-click a stack for the split slider), onto the target bar or an NPC/player in the world
# Double-click picks a loose item up and uses anything else; with an
# object selected, R uses it where it is (read a book or sign on the
# ground, open a chest) and G picks it up, retail's separate actions.
# Using a sign, plaque or book (in the world or in the pack) opens the
# book window with its pages. The status line shows the map coordinates
# (42.1N, 33.6E) outdoors.
# (clicking anything in the world or a single click on an item in the
# pack, a container, a vendor's list or the trade window selects and
# appraises it: the appraisal window shows its value, damage, armor,
# spells and requirements)
# to hand it over, onto an open chest to store it, or onto empty ground to
# drop it, onto a chest, house hook or storage chest to put it there;
# K shows the skills panel, P the spellbook, B the spell bar
# (1..9 cast its spells, Insert/PageUp cycle tabs, Delete/PageDown spells),
# O the components panel, U the buffs, F the fellowship, L the allegiance
# (swear to the selected player, break, name it; /v /p /m /c chat to
# vassals, patron, monarch, co-vassals; /g /trade /lfg /rp /a for the
# General, Trade, LFG, Roleplay and allegiance rooms; *wave*, *bow*,
# *cheer* and some seventy other emotes typed between asterisks, or as
# /wave, animate and say the line), H housing (your house, guests,
# recall; use a house sign to see its price and buy it), N the social
# panel (your title, friends with who is online, squelches), X the character
# options; server
# questions such as a fellowship invitation or an oath pop up with Yes/No;
# double-click the Ust to open the salvage window, drag a salvage bag onto
# an item in the pack to tinker it). C toggles combat in the stance of
# the wielded weapon (a bow, crossbow or atlatl with ammunition in the
# ammo slot gives missile mode; anything else melee):
# the combat bar then shows the attack height and the power/accuracy
# slider (Insert/PageUp step power, Delete/PageDown height) and auto-repeat;
# double-click another player (in peace mode) to open a secure trade: drag
# items into your offer, Accept on both sides swaps them;
# double-click a creature to attack it until it dies; double-click its corpse
# to loot (Take all / Close). Hold Space to charge a jump (a green bar
# under the vitals; a second fills it) and release to leap; stamina caps
# the power. Sounds play unless --mute.
# Character select and creation: without --character the window shows the
# account's characters after login (Up/Down highlight, Enter enters; Delete
# asks first and the server keeps the character for a while with Restore
# beside it; New character opens the creation screen). Creation walks five
# panes (Left/Right or PageUp/PageDown: heritage and sex, appearance with a
# turntable preview of the model, right-drag turns it, template and
# attributes, skills with the credits left, name and starting town) and
# Create sends it when the rules pass; Escape returns to the list.
# Offline: --demo-select and --demo-create show both screens with no server
# (--press ArrowRight steps the creation panes; add --screenshot out.png).
# Chat lines starting with / go to the plugins first (/help lists them),
# then to the game's own commands (/lifestone, /die, /house, /tell Name,
# text, /emote, /afk), and anything else to the server as @command
# (/acehelp lists those).
cargo run --release -p acviewer -- --connect 127.0.0.1 -a myaccount -v mypassword
# several characters in one window: --client ACCOUNT:PASSWORD[:CHARACTER]
# per extra session; Tab (or /switch N) picks the one shown and steered
cargo run --release -p acviewer -- --connect 127.0.0.1 -a alice -v pw1 --client bob:pw2:Bob
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
decoders), `crates/ac-scene` (GPU-free assembly, collision, lighting,
particles, chargen), `crates/ac-net` (protocol, sans-IO session),
`crates/ac-world` (object table, character sheet, motions),
`crates/ac-client` (headless game session: connect, tick, actions, events,
player physics), `crates/ac-plugin` (`Plugin` trait, `Ctx`, blackboard and
bus, host, the console and party plugins), `crates/ac-audio` (sound
playback), `bins/acdat` (CLI), `bins/acviewer` (wgpu viewer and
multi-session client), `bins/acbot` (headless multi-session runner),
`bins/aclauncher` (launch manager), `bins/acclient` (old headless CLI).
See `docs/architecture.md`.

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

## Scripting

The viewer runs every `*.rhai` file in `~/.acreborn/scripts` (or
`$ACREBORN_SCRIPTS`) through an embedded [Rhai](https://rhai.rs) engine
and reloads a file when it changes, so the client can be extended without
recompiling. A script defines `on_event(ev)`, `tick(dt)`,
`command(name, args)` and/or `key(name, pressed)`, and calls into the game
with the same verbs as the console: `me()`, `objects()`, `attack(name)`,
`cast(spell)`, `loot()`, `say(text)`, `post(topic, value)` /
`messages(topic)` for talking to other sessions, `with_session(i, || ...)`
to act as another one. `/scripts` lists what is loaded; script errors go
to the chat log and never stop the client. Examples and the full API are
in [`scripts/examples/`](scripts/examples/README.md); the plugin is
`crates/ac-script`.

## License

Dual-licensed under MIT or Apache-2.0, at your option. Game data, the
original executable and the ACE emulator sources (AGPL, used only as a
reference) are not part of this repository.
