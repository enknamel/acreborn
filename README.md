# acreborn

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
cargo run --release -p acviewer -- --landblock A9B4 --screenshot out.png   # headless render
```

Viewer controls: right-drag to look, WASD to move, Q/E down/up, Shift to
boost, Escape to quit.

Against a local ACE (`tools/ace/up.sh` starts one in Docker):

```
# headless: log in, create a character, enter the world, print messages
cargo run --release -p acclient -- -h 127.0.0.1 -a myaccount -v mypassword --create Reborn
# admin accounts can teleport: --say "@telepoi holtburg"
# play: third-person view, WASD walks the character, right-drag turns, Shift walks
# slowly, Enter opens the chat box (server commands start with @), left click
# selects and appraises an object, double-click uses it (doors, NPCs, items)
cargo run --release -p acviewer -- --connect 127.0.0.1 -a myaccount -v mypassword
```

Workspace: `crates/ac-dat` (container), `crates/ac-formats` (asset
decoders), `crates/ac-scene` (GPU-free mesh assembly, collision),
`crates/ac-net` (protocol, sans-IO session), `crates/ac-world` (object
table and movement), `bins/acdat` (CLI), `bins/acviewer` (wgpu viewer and
client), `bins/acclient` (headless client).

Debugging aids: `RUST_LOG=acviewer=debug`, `ACV_HIDE_STATIC=1` (draw only
server objects), and in connected `--screenshot` mode `--walk`, `--say`,
`--click x,y` and `--camera` to script a session headlessly.

Game data and the original executable are not distributed with this
repository and are gitignored.
