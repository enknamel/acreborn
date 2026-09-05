# Architecture

How the workspace is cut up, how data flows from the DAT archives and the
server to the screen, and the two design goals every crate is shaped by:
many sessions per machine, and plugin-driven extensibility.

See also: [plugins.md](plugins.md) (writing a plugin),
[multi-session.md](multi-session.md) (running several clients),
[subsystems/dat.md](subsystems/dat.md) and
[subsystems/net.md](subsystems/net.md) (formats and protocol facts).

## Crate map

Libraries, in dependency order (each depends only on the ones above it):

| crate | role | depends on |
|---|---|---|
| `ac-dat` | DAT container: `DatArchive` mmaps `client_portal.dat` / `client_cell_1.dat`, walks the B-tree directory, reads a file by id. Knows nothing about file contents. | memmap2 |
| `ac-formats` | One decoder module per asset type (`gfxobj`, `setup`, `animation`, `motion_table`, `landblock`, `environment`, `region`, `scene`, `surface`, `texture`, `palette`, `particle_emitter`, `physics_script`, `sound_table`, `wave`, `spell_table`, `skill_table`, `chargen`, ...). Every `parse` takes the raw bytes and must consume them exactly. | ac-dat |
| `ac-scene` | Assembly without a GPU: `Assets` (memoizing loader over both archives), `terrain`, `landblock`, `interior`, `scenery`, `model` (GfxObj/Setup to triangle lists), `anim`, `collision` (capsule vs. mesh, terrain sampler), `lighting`, `particles`, `chargen`. | ac-formats |
| `ac-net` | Wire protocol, sans-IO: `packet`, `hash32`, `isaac`, `wire` (Reader/Writer), `messages` (opcodes, parsers, builders), `session::Session` (consumes datagrams and time, produces datagrams and decoded messages; the caller owns the socket). | - |
| `ac-world` | `World`: the object table built from server messages (`WorldObject`, `Position`, `MoveTarget`, `CommandQueue` in `motion`), `stats::PlayerStats` (the character sheet), open container and vendor state. | ac-net, ac-formats |
| `ac-client` | `Client`: one headless game session. Owns the socket and `Session`, applies messages to its `World`, runs the character's physics (`player::Player`), runs the gameplay timers (combat, loot), and exposes every action a player can take. Reports `Event`s to whoever drives it. | ac-net, ac-world, ac-scene |
| `ac-plugin` | `Plugin` trait, `Ctx`, `Blackboard` (named values plus a one-frame message bus), `host::Host` which fans callbacks out to every session, `icons` (item and spell icons as egui textures), and the built-in `panels` (vitals, radar, target bar, inventory, loot, vendor, skills, spellbook), each a plugin. | ac-client, egui |
| `ac-audio` | `Audio` on top of `kira`: open the default device, play a decoded `Wave` once at a volume; sound-table lookup helpers. | ac-formats |

Binaries:

| bin | role |
|---|---|
| `acviewer` | The client: wgpu renderer, egui overlay (`ui.rs`: the status line and the chat box; every other panel is a plugin), multi-session host (`Net` per session), landblock streaming, third-person camera, and the built-in plugins in `plugins/`. Also a standalone viewer for landblocks, models, particle emitters and chargen looks, and a headless `--screenshot` runner. |
| `aclauncher` | Desktop launch manager: servers and accounts in `~/.acreborn/launcher.json`, one `acviewer --connect` process per launch, logs in `~/.acreborn/logs/`. |
| `acclient` | The older headless CLI built directly on `ac-net`/`ac-world`: log in, optionally `--create` a character, enter the world, print messages. Still the quickest way to create a character. |
| `acdat` | DAT CLI: `info`, `ls`, `cat`, `extract`, `decode` (asset as JSON), `wav`, `manifest` and `diff` (against an ACE-generated manifest). |

## Data flow

```
client_portal.dat / client_cell_1.dat
        |  ac-dat (mmap, B-tree lookup by id)
        v
   ac-formats  (bytes -> typed asset)
        |  ac-scene::Assets (Rc<...> caches, 32-landblock LRU)
        v
   ac-scene    (meshes, collision, lighting, particles, chargen)
        |
        +----------------------------+
        |                            |
   ac-client::player (physics)   acviewer::scene (GPU batches)

UDP <-> ac-net::Session <-> ac-client::Client::tick <-> ac-world::World
                                   |
                                   +--> Vec<Event> --> plugins, UI, audio
```

* The archives are opened once per process (`Assets::open`) and shared by
  every session as `Rc<Assets>`.
* `Session` never touches a socket. `Client::tick` sends what the session
  has queued, drains the socket into it, then applies each decoded message
  to `World::apply`, which returns an `Applied` tag the client uses to react
  (login-complete after placement, server move-to, stance changes).
* Everything above `ac-client` is a consumer of `Client`: the renderer reads
  `client.world` and `client.player`, plugins read and call `Client`,
  scripts do the same.

## Design goal: many sessions per machine

The client is meant to run a handful of characters at once, in one process
or several (see [multi-session.md](multi-session.md)). That fixes what is
shared and what is per session:

* **Shared per process**: `Rc<ac_scene::Assets>` (the DAT mmaps and every
  decoded asset cache), the `ac_audio::Audio` device (it is `Clone`), the
  `plugins::Host` with its `Blackboard`, the window, the wgpu device.
* **Per session** (`acviewer`'s `Net`): the `ac_client::Client` (socket,
  `Session`, `World`, `Player`, combat and loot state), plus the GPU-side
  caches for what that session has seen: `loaded_blocks`, `mesh_cache`,
  `gpu_meshes`, `palettes`, `anims`, `tables`, `fx`.
* **Drawn**: only the active session (`App::active`). Keys steer it, its
  chat and sounds reach the overlay, landblocks stream around its
  character. Every other session still ticks its network, physics and
  plugins every frame with `player::Input::default()`.

`ac-client` renders nothing and has no `winit`/`wgpu` dependency, so a
session is cheap and a headless host (a bot runner, a test) can drive
several `Client`s the same way the viewer does.

## Design goal: plugin-driven extensibility

Everything the UI can do is a method on `ac_client::Client` or an `Event`
it emits; the UI has no private channel to the server. The panels
themselves (vitals, radar, target bar, inventory, loot, vendor, skills,
spellbook) are plugins in `ac_plugin::panels`: each reads `cx.client()`
and turns its clicks into `client.buy/sell/take/interact/cast`, so they
double as examples and can be swapped for your own. `acviewer` keeps only
the status line and the chat box; `apply_ui_commands` sends chat lines to
`client.say` or to the plugins' `/commands`, and `tick_client` turns
`Event::Chat`/`Sound`/`Placed` into chat lines, audio and a scene rebuild.
A plugin gets the same
`&mut Client` (for every session) plus the same event list, so anything a
person can do at the keyboard a plugin can do programmatically, and the
`/commands` in `plugins/console.rs` are one-line wrappers over `Client`.

## One frame in the viewer

`App::window_event(RedrawRequested)` in `bins/acviewer/src/main.rs`:

1. **Input**: winit key and mouse events were collected since the last
   frame. egui gets first refusal (typing in the chat box); then
   `Host::key` offers the key to plugins; then the viewer's own bindings
   (Tab switch, C combat, I/K/P panels, Enter chat, Space jump). Held
   movement keys sit in `App::keys`.
2. **UI commands**: `apply_ui_commands` drains the chat box. Lines
   starting with `/` go to `Host::command`; the rest to `client.say`.
   (Panel clicks need no relay: the panel plugins called `Client` during
   the previous frame's `ui` pass.)
3. **Tick every session**: for each `Net`, `tick_client` builds a
   `player::Input` from the keys (active session only) and calls
   `Client::tick(input, dt, now)`, which pumps the socket, applies messages,
   runs `tick_combat`/`tick_loot`, builds the `Player` on first placement,
   advances `World::tick` and the character physics, and reports movement
   to the server. `drain_events` collects that session's `Event`s; chat and
   sound are shown only for the active one.
4. **Plugins**: one `Host::frame(clients, i, events, dt, now)` batch per
   session, in order: every plugin's `on_event` for each event, then its
   `tick`. Requests (chat lines, a session switch) are applied, then
   `Host::end_frame` rotates the bus so this frame's posts are readable next
   frame.
5. **Streaming** (active session): the landblock under the character
   first, then its eight neighbours outdoors, one new block built per frame
   (`scene::build_landblock`, uploaded with `gpu.add_block`), stale blocks
   removed. Static-object particle emitters (`world_fx`) update. When
   `world.generation` changed or something is animating, dynamic objects
   are re-instanced (`scene::object_instances`).
6. **Camera and character**: third-person camera behind `client.player`,
   clamped against walls; the character's model is re-instanced when
   `PlayerFrame::dirty`.
7. **Render**: `refresh_status` fills the status line; `run_overlay` runs
   the egui pass (`Ui::begin`), inside which `Host::ui` lets plugins draw:
   the panels first, reading the active `Client` and acting on it, then
   the rest; the host's status line and chat box go on top. `gpu.render`
   draws the scene and paints the overlay; `request_redraw` schedules the
   next frame (vsync). Offline, `--screenshot --demo-ui` runs the same
   pass with the panels on canned data (`plugins::demo`).

## Headless testing

`acviewer --screenshot out.png` renders one frame without a window. With
`--connect` it becomes a scripted session (`main()` in `main.rs`): the same
`App` and `tick_net` run in a loop with a 1 ms sleep until the character is
placed, then a small state machine drives actions on timers and the frame
is written when they settle:

* `--walk SECS` holds W; `--jump` jumps once; `--say LINE` (repeatable,
  3 s apart; `@commands` for admin accounts) goes through the chat path;
  `--click x,y` double-clicks a pixel.
* `--use NAME` uses the nearest object with that name (retried once a
  second for 60 s while it is not in view yet).
* `--attack NAME` enters melee, attacks until the target dies (90 s cap);
  `--loot [NAME]` then opens the corpse (`Corpse of <last target>` by
  default) and takes everything, one item at a time.
* `--buy NAME` / `--sell NAME` act once a vendor window is open (after
  `--use` on the vendor); `--cast NAME` casts a spellbook spell or one
  learnt from a scroll.
* `--snap-at SECS` also writes `<out>.mid.png` mid-action; `--camera
  x,y,z,yaw,pitch` overrides the final viewpoint; `--show-skills` opens the
  skills and spellbook panels; `--mute` is implied.

Together with `tools/ace/up.sh` (a local ACE in Docker) this is the
integration test: a scene is set up with admin commands (`@create 7`,
`@ci 314`, `@smite all`, `@telepoi holtburg`), the client acts, and the
PNG plus the `RUST_LOG=acviewer=debug` log are checked. Unit tests that
need the archives skip themselves when `AC_DATA_DIR` is unset; golden
files for the DAT reader and ISAAC live in `tests/golden/`.
