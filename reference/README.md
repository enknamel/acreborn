# reference/

Everything here except `scripts/`, `notes/`, `tools/` and this file is **generated or third-party and gitignored**.

| Path | What | How to regenerate |
|---|---|---|
| `ext/ACE`, `ext/ACViewer`, `ext/aclogview` | ACEmulator repos (AGPL-3.0). Read for formats/protocol; **do not copy code into acreborn.** | `git clone --depth 1 https://github.com/ACEmulator/<name>` |
| `decomp/by_func/*.c` | One decompiled C file per function of `acclient.exe` | `scripts/ghidra/run_export.sh` |
| `dumps/*.tsv` | functions, calls, strings, imports, symbols, vtables | same |
| `index.sqlite` | FTS index over the above | `scripts/index/build_index.py` |
| `notes/` | curated, hand-written findings (address -> meaning) | committed |

Inputs (not in repo): Ghidra project `~/code/acclient.gpr`, binary and DATs in `$AC_DATA_DIR` (default `~/Downloads/ac_data`).
