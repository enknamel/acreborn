# DAT container format

Implemented in `crates/ac-dat`. Verified against the end-of-retail archives
(`client_portal.dat` iteration 2072, `client_cell_1.dat` iteration 982) and
ACE's `ACE.DatLoader` via `acdat diff`.

## Header (absolute offset 0x140, 80 bytes, little-endian)

| off | field | portal | cell |
|---|---|---|---|
| 0x00 | file_type (magic "BT") | 0x5442 | 0x5442 |
| 0x04 | block_size | 0x400 | 0x100 |
| 0x08 | file_size | archive size | archive size |
| 0x0C | data_set (1 = portal/highres, 2 = cell) | 1 | 2 |
| 0x10 | data_subset | 1 | 0 |
| 0x14 | free_head | | |
| 0x18 | free_tail | | |
| 0x1C | free_count | | |
| 0x20 | btree (root directory node offset) | | |
| 0x24 | new_lru | 0 | 0 |
| 0x28 | old_lru | 0 | 0 |
| 0x2C | use_lru (1 = true) | 0 | 0 |
| 0x30 | master_map_id | 0x25000000 | 0 |
| 0x34 | engine_pack_version | 22 | 110 |
| 0x38 | game_pack_version | 0 | 0 |
| 0x3C | version_major (16-byte GUID) | same in both | |
| 0x4C | version_minor | 6657 | 6657 |

The first 0x140 bytes of the archive are a text banner, not data. The
client's create path fills them with
`"\nFile Header Structure Default Constructor v1.3\n"` followed by a 0x1A
(DOS EOF) byte and zero padding, so `type` on a DAT prints that line.

## Blocks

Every block is `block_size` bytes. Bytes 0..4 are the absolute offset of the
next block in the chain (0 = last). Payload is the remaining `block_size - 4`
bytes. An object of `n` bytes therefore occupies `ceil(n / (block_size-4))`
blocks; the last block's trailing bytes are garbage and must not be read.

## Directory (B-tree)

A node is one chained object of 1716 bytes:

```
u32 branch[62]      child node offsets; branch[0] == 0 means leaf
u32 entry_count     <= 61
Entry entry[61]     only the first entry_count are valid
```

`Entry` (24 bytes): `u32 flags, u32 id, u32 offset, u32 size, u32 date, u32 iteration`.
`offset` is the first block of the file's chain. An interior node has
`entry_count + 1` children, visited in order. Files are sorted by id.

## Ids

* cell.dat: `LLLL CCCC` where `LLLL` is the landblock. `CCCC == 0xFFFF` is
  the LandBlock (terrain), `0xFFFE` the LandBlockInfo (buildings/objects),
  `0x0100..` EnvCells (interior cells).
* portal.dat: high byte selects the type (0x01 GfxObj, 0x02 Setup, 0x03
  Animation, 0x04 Palette, 0x05 SurfaceTexture, 0x06 Texture, 0x08 Surface,
  0x09 MotionTable, 0x0A Wave, 0x0D Environment, 0x0F PaletteSet, 0x10
  Clothing, 0x12 Scene, 0x13 Region, 0x20 SoundTable, 0x32 ParticleEmitter,
  0x33/0x34 PhysicsScript(Table), 0x78 UI layouts, ...). `0x0E00xxxx` are
  singleton tables (SpellTable 0x0E00000E, SkillTable 0x0E000004, ...).
  See `FileKind::classify` for the full list.
* Every archive has `0xFFFF0001`, the iteration file: `i32 total` then
  `(i32 consecutive, i32 start)` pairs until the running sum reaches zero.
* Most portal files begin with their own id as the first u32.

## Counts (end-of-retail)

| archive | files |
|---|---|
| client_portal.dat | 79,694 |
| client_cell_1.dat | 805,348 |

## Decompilation anchors (`reference/decomp/by_func`)

| addr | role | evidence |
|---|---|---|
| `FUN_006712d0`, `FUN_006713e0` | header struct constructor: zeroes fields, stores magic `0x5442` at +0, version fields at +0x30.. (`DAT_008f86b4`), `-1` sentinels at +0x34/+0x38 | `*param_1 = 0x5442` |
| `FUN_00677d80` | `DiskDev::Create`: builds the 0x140 banner, opens the file via `CreateFile` (`PTR_FUN_00837374`), writes banner+header (`FUN_00677cc0`, 400 = 0x190 bytes), packs header with `FUN_00677bf0` | banner string literal |
| `FUN_00675920` | reads/validates a header (checks `0x5442`) | magic compare |
| `FUN_00675040`, `FUN_00675150` | seek to `0x140` and read/write the 80-byte header | `0x140` constants |

The strings `client_portal.dat` (0x795de8) and `client_cell` (0x7c68ec)
have no code references in the Ghidra listing; they are reached through a
data table of dat descriptors, so locate callers via `FUN_00677d80` and
the `CreateFileA` import instead (`q.py import CreateFileA`).
