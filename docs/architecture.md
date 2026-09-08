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
| `ac-scene` | Assembly without a GPU: `Assets` (memoizing loader over both archives), `terrain`, `landblock`, `interior`, `scenery`, `model` (GfxObj/Setup to triangle lists), `anim`, `collision` (capsule vs. mesh, terrain sampler), `nav` (walkable-grid graph and A* per landblock), `lighting`, `particles`, `chargen`. | ac-formats |
| `ac-net` | Wire protocol, sans-IO: `packet`, `hash32`, `isaac`, `wire` (Reader/Writer), `messages` (opcodes, parsers, builders), `session::Session` (consumes datagrams and time, produces datagrams and decoded messages; the caller owns the socket). | - |
| `ac-world` | `World`: the object table built from server messages (`WorldObject`, `Position`, `MoveTarget`, `CommandQueue` in `motion`), `stats::PlayerStats` (the character sheet), open container and vendor state. | ac-net, ac-formats |
| `ac-client` | `Client`: one headless game session. Owns the socket and `Session`, applies messages to its `World`, runs the character's physics (`player::Player`), runs the gameplay timers (combat, loot), and exposes every action a player can take. Reports `Event`s to whoever drives it. | ac-net, ac-world, ac-scene |
| `ac-plugin` | `Plugin` trait, `Ctx`, `Blackboard` (named values plus a one-frame message bus), `host::Host` which fans callbacks out to every session, `icons` (item and spell icons as egui textures), and the built-in `panels` (vitals, radar, target bar, inventory, loot, vendor, skills, spellbook), each a plugin. | ac-client, egui |
| `ac-audio` | `Audio` on top of `kira`: open the default device, play a decoded `Wave` once at a volume; sound-table lookup helpers. | ac-formats |

Binaries:

| bin | role |
|---|---|
| `acviewer` | The client: wgpu renderer, egui overlay (`ui.rs`: the status line and the chat box; every other panel is a plugin), multi-session host (`Net` per session), landblock streaming, third-person camera, and the built-in plugins in `plugins/`. Also a standalone viewer for landblocks, models, particle emitters and chargen looks, and a headless `--screenshot` runner. |
| `aclauncher` | Desktop launch manager: servers and accounts in `~/.acswarm/launcher.json`, one `acviewer --connect` process per launch, logs in `~/.acswarm/logs/`. |
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
* **Shared per process, scene side**: the viewer's `mesh_cache`,
  `gpu_meshes`, `palettes`, motion `tables`, particle `fx` and
  `loaded_blocks` live on the `App`, so sessions in the same place share
  one copy of its meshes and materials.
* **Per session** (`acviewer`'s `Net`): the `ac_client::Client` (socket,
  `Session`, `World`, `Player` with its collision and navigation graphs,
  combat, loot and route state), plus the animation and picking state of
  the objects it sees (`anims`, `pickables`).
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

## Rendering cost

The goal is speed and low resource use, not fidelity: one rendered
session and up to nine followers in one process must fit on an ordinary
machine. What the renderer does about it, in `bins/acviewer/src/gpu.rs`
and the frame loop in `main.rs`:

* **Only the active session touches the GPU.** Landblocks stream, objects
  are instanced, particles simulate and animations advance for
  `App::active` alone; the other sessions tick their network, physics and
  plugins and keep no GPU state (`Net` holds only their `Client`, `anims`
  and `pickables`, and a switch clears the last two for the session left
  behind). `--render none` goes further for a follower window: nothing of
  the world reaches the GPU, only the sky and the overlay are drawn.
* **Culling.** Every static batch (one per material per landblock) carries
  a world-space box and every instance a bounding sphere; `Gpu::draw`
  tests them against the view frustum each frame. Instances farther than
  the draw distance, or too small on screen to cover a pixel and a half,
  are skipped too. The draw distance is the options panel's "draw
  distance" slider (`options.draw_distance` in the settings, published as
  `render.draw_distance` on the blackboard, 0 = no limit), read by the
  viewer each frame. `ACV_NO_CULL=1` draws everything, for comparisons.
* **Instancing.** Model matrices live in one storage buffer indexed by the
  instance index. Visible instances are sorted by mesh and every copy of a
  mesh (the same GfxObj in the same look) is one `draw_indexed` with an
  instance range; static batches read record 0 (the identity). A bind
  group already set is not set again.
* **Frame pacing.** `--fps` caps the window (default 60) through
  `ControlFlow::WaitUntil`; an unfocused window draws at 10 fps at most, a
  hidden (minimised or covered) one draws nothing, and a frame in which
  neither the camera, the GPU scene (`Gpu::is_dirty`) nor the egui output
  (`Ui::repaint_wanted`: the shapes compared with the last pass) changed
  is skipped. The sessions keep ticking at the paced rate regardless.
* **Memory.** Textures are decoded and mip-mapped once per material and
  shared by every batch, mesh and session in the process. When a landblock
  is unloaded, and every 30 s, materials and GPU meshes nothing references
  any more are dropped (`Gpu::prune_materials`, `scene::prune_gpu_meshes`).
  `--max-texture N` uploads from a smaller mip level (128 quarters the
  texture memory) for a blurrier but lighter world.

`--perf` measures it: the status line shows the last frame's time and
draw counts, and a summary is printed at exit (average, p50, p95 and max
frame time; draw calls, triangles, instances and batches drawn and culled
per frame; materials, texture MB, meshes and buffers held). A windowed run
records every presented frame; a headless `--connect --screenshot --perf`
run draws offscreen at the `--fps` rate and waits for the GPU each frame,
so its frame time is the whole cost of a windowed frame; an offline
`--landblock --screenshot --perf` draws the still scene a hundred times.

Measured on an Apple-silicon laptop at 1280x800 (release build, the
machine otherwise busy, so treat differences under half a millisecond as
noise; `before` is the same build with `ACV_NO_CULL=1` and the old
one-draw-per-instance path):

| Scene | | frame ms avg / p95 | draw calls | triangles | instances drawn | textures |
|---|---|---|---|---|---|---|
| Holtburg town, offline `--landblock A9B4 --radius 1`, overhead view | before | 2.10 / 2.32 | 256 | 154k | 0 | 91 MB |
| | after | 1.49 / 1.53 | 256 | 154k | 0 | 91 MB |
| | after, `--max-texture 128` | 1.38 / 2.07 | 256 | 154k | 0 | 65 MB |
| Open country, offline `--landblock A6B4 --radius 1` | before | 1.21 / 1.96 | 137 | 143k | 0 | 83 MB |
| | after | 1.22 / 1.96 | 133 | 143k | 0 | 83 MB |
| Connected, dungeon (Qalaba'r), 339 instances, 96 emitters, four panels open | before | 4.87 / 7.21 | 731 | 127k | 317 | 44 MB |
| | after, culling | 4.36 / 6.11 | 188 | 71k | 70 | 44 MB |
| | after, culling and instancing | 4.40 / 6.11 | 149 | 70k | 69 | 44 MB |
| | `--render none` (a follower) | 3.60 / 11.04 | 1 | 0 | 0 | 0 MB |

The offline overhead views look down on all nine blocks, so nothing
culls and the frame is the fixed cost of 250 batches. The connected
frame, headless, is about 2 ms of CPU (half of it the egui pass over the
open panels, the rest ticking the session), under 1 ms encoding the draw
calls, and 2 ms of GPU work and pass latency; in a window the GPU part
overlaps the next frame's CPU, and a frame in which nothing moved costs
only the tick and the egui pass. A follower window (`--render none`)
holds no textures, meshes or buffers at all. Pixel-for-pixel, the offline
screenshots are the same before and after (the water animates with the
clock, nothing else differs).

Left to do, in order of likely payoff: compressed textures (the DATs are
RGBA8 at up to 256x256; BC1/ASTC would cut the texture memory four to
eight times), merging the static batches of neighbouring blocks by
material (a third of the draw calls in a town), and an egui pass only
when its input or data changed (it is half the CPU time when idle).
