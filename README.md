# acreborn

A from-scratch Rust reimplementation of the Asheron's Call client, built to
play against the [ACE](https://github.com/ACEmulator/ACE) server emulator.

Status: **Phase 1** (DAT archive reader). See `docs/` for the subsystem
specs and `reference/README.md` for how the reverse-engineering material is
regenerated.

```
export AC_DATA_DIR=~/Downloads/ac_data     # acclient.exe + client_*.dat live here
cargo run --release -p acdat -- $AC_DATA_DIR/client_portal.dat info
cargo run --release -p acdat -- $AC_DATA_DIR/client_portal.dat ls --kind GfxObj | head
cargo run --release -p acdat -- $AC_DATA_DIR/client_cell_1.dat cat A9B4FFFF | xxd | head
```

Workspace: `crates/ac-dat` (container), `bins/acdat` (CLI). Later phases add
`ac-formats`, `ac-render`, `ac-net`, `ac-physics`, `ac-world`, `acviewer`,
`acclient`.

Game data and the original executable are not distributed with this
repository and are gitignored.
