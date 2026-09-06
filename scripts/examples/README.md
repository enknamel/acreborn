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
- `objects()`: nearest first; each `guid, name, distance, is_creature, is_player` (another player character), `is_corpse, health, x, y, z, cell, stack, value`
- `inventory()`, `container()` (items of the open corpse/chest): same shape, plus `material` (name or `()`), `workmanship`, `structure` (a salvage bag's units)
- `sessions()`: number of sessions; `session(i)`: summary of session `i` or `()`; `session_index()`

Actions (on the current session; names match by prefix, like the console):

- `use_name(name)`, `use_guid(guid)`, `attack(name)`, `attack(guid)`, `cast(spell)`, `say(text)`
- `loot()` / `loot(name)`: open the corpse of the last target / by name; `take(guid)`, `take_all()`, `close_container()`
- `buy(name)`, `sell(name)` (a vendor must be open), `combat(true|false)`, `jump(power)` (0..1, capped by stamina), `select(guid)` (`0` clears)
- `log(text)` (and `print`): a line in the chat log, not sent to the server
- Social: `fellow_create(name, share_xp)`, `fellow_recruit(guid)`, `fellow_quit(disband)`, `fellowship()`; `trade_open(guid)`, `trade_add(item)`, `trade_accept()`, `trade_decline()`, `trade_reset()`, `trade_close()`, `trade()`; `confirmations()`, `confirm(yes)` (the oldest pending question); `swear(guid)`, `break_allegiance(guid)`, `allegiance()` (`name, rank, total_members, total_vassals, motd, monarch, patron, me, vassals`, members with `guid, name, level, rank, loyalty, leadership, online, xp_cached, xp_tithed`), `allegiance_refresh()`, `allegiance_name(name)`; `chat("v"|"p"|"m"|"c"|"f", text)` for the vassals, patron, monarch, co-vassals and fellowship channels, `chat("g"|"trade"|"lfg"|"rp"|"a", text)` for the Turbine chat rooms
- Housing: `house_profile()` after using a sign (`slumlord, owner, owner_name, kind, min_level, buy, rent` with `{name, wcid, needed, paid}` items), `house()` (`kind, cell, rent_paid, rent`) or `()`, `house_query()`, `buy_house()`, `rent_house()` (pay with the pack's items; false when short), `abandon_house()`, `house_guests()` (`open, allegiance_guests, allegiance_storage, guests: [{guid, name, storage}]`; the list is refreshed after every guest change), `house_guest(name, add)`, `house_storage(name, on)`, `house_open(on)`
- `appraise(guid)` asks the server; `appraisal(guid)` is the last answer as a map (`name, usage, short_desc, long_desc, value, burden, workmanship, armor_level, damage, damage_min, speed, weapon_skill, offense, spells, spellcraft, mana, mana_max, wield_skill, wield_level, level, health, health_max, ints, floats`) or `()`
- `split(guid, n)` takes `n` off a stack into the main pack; `merge(from, to)` pours one stack into another of the same kind
- `salvageable()` (carried loot an Ust can salvage), `salvage([guids])` (needs an Ust in the pack; yields show in chat); tinkering is `use_on(bag, item)` then `confirm(true)`
- `option(name, on)` sets a character option by label prefix; `raise(what)`, `train(skill)` spend XP and credits
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
