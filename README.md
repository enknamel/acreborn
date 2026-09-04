# acreborn

A from-scratch Rust reimplementation of the Asheron's Call client, built to
play against the [ACE](https://github.com/ACEmulator/ACE) server emulator.

Status: **Phase 3** (network). The DAT reader is verified byte-for-byte
against ACE; thirteen asset types decode every file in both archives;
`acviewer` renders outdoor landblocks with scenery and models; `acclient`
logs in to a local ACE server, creates a character and enters the world.
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

Headless login against a local ACE (`tools/ace/up.sh` starts one in Docker):

```
cargo run --release -p acclient -- -h 127.0.0.1 -a myaccount -v mypassword --create Reborn
```

Workspace: `crates/ac-dat` (container), `crates/ac-formats` (asset
decoders), `crates/ac-scene` (GPU-free mesh assembly), `bins/acdat` (CLI),
`bins/acviewer` (wgpu viewer). Later phases add `ac-net`, `ac-physics`,
`ac-world`, `acclient`.

Game data and the original executable are not distributed with this
repository and are gitignored.
