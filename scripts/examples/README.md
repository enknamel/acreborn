# Example scripts

Copy any of these into `~/.acreborn/scripts` (or the directory named by
`$ACREBORN_SCRIPTS`) while the client runs: it picks them up within a
second, and reloads a file whenever it changes. `/scripts` lists what is
loaded; `/scripts reload` reloads everything. Errors show in the chat log.

| Script | What it does |
| --- | --- |
| `greeter.rhai` | `/hello [name]` says hello in local chat (a `command` hook) |
| `autoloot.rhai` | When the attack target dies, opens its corpse and takes everything (a `tick` hook with state in `this`) |
| `assist.rhai` | Posts the attack target on the bus; other sessions attack it (`post` / `messages`) |

## Hooks

A script defines any of these top-level functions:

```rhai
fn on_event(ev)         // ev.kind: "chat" (ev.text, ev.chat_kind), "sound" (ev.volume),
                        // "connected", "terminated" (ev.reason), "refused" (ev.code), "placed" (ev.cell)
fn tick(dt)             // every frame, per session; dt in seconds
fn command(name, args)  // "/name args" in the chat box; return true when handled
fn key(name, pressed)   // "F5", "A", "Space"... went down (true) or up; return true to consume
```

Hooks run once per session, and everything you read or do inside refers to
that session. `this` is a map kept between calls (one per script per
session); a missing key reads as `()`. Top-level `let`s are *not* visible
inside functions (Rhai rule): use `this` or the blackboard.

## API

Reads (maps have the listed fields):

- `me()`: `session, guid, name, level, health, health_max, stamina, stamina_max, mana, mana_max, total_xp, available_xp, skill_credits, x, y, z, cell, local_x, local_y, local_z` (position within the landblock, the numbers `@loc` and `@teleloc` use), `vitae` (penalty in percent, 0 without one), `combat, magic, target, target_name, selected, placed, vendor_open, container_open` (`target`/`selected` are guids or `()`)
- `objects()`: nearest first; each `guid, name, distance, is_creature, is_player, is_corpse, health, x, y, z, cell, stack, value`
- `inventory()`, `container()` (items of the open corpse/chest): same shape
- `sessions()`: number of sessions; `session(i)`: summary of session `i` or `()`; `session_index()`

Actions (on the current session; names match by prefix, like the console):

- `use_name(name)`, `use_guid(guid)`, `attack(name)`, `attack(guid)`, `cast(spell)`, `say(text)`
- `loot()` / `loot(name)`: open the corpse of the last target / by name; `take(guid)`, `take_all()`, `close_container()`
- `buy(name)`, `sell(name)` (a vendor must be open), `combat(true|false)`, `select(guid)` (`0` clears)
- `log(text)` (and `print`): a line in the chat log, not sent to the server
- `switch(i)`: make session `i` the active one
- `with_session(i, || ...)`: run the closure with session `i` current, then restore; returns the closure's value

Shared state:

- `post(topic, value)`, `messages(topic)` (each `#{ from, topic, value }`, alive one frame)
- `board_get(key)`, `board_set(key, value)`: values persist for the process

Values cross to the other plugins as JSON: maps, arrays, strings, ints,
floats, bools and `()` go through; other Rhai types do not.

Session indices are zero-based (the console's `/switch N` is 1-based).
A hook that throws, or spins for more than about two million operations,
is stopped and reported; the rest of the client carries on.
