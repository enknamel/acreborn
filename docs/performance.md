# What a session costs

Measured 2026-09-09 on an Apple M3 Pro against a local ACE server.
Numbers here are a baseline to notice regressions against, not a
promise: re-run the commands before trusting them on other hardware.

## The short version

Running many sessions is cheap. The parts that are expensive -- the
world grid, the wide planner, a landblock's collision and nav graph,
the DAT archives -- are loaded once and shared, and the GPU only ever
holds the session the window is showing. What a further session adds is
well under a megabyte and a fraction of a percent of a core.

| | Value |
|---|---|
| Process, warm, no server | 66 MB footprint |
| Process, one live session standing in the world | 29 MB footprint |
| Each further live session | ~0.6 MB footprint |
| Each further idle session (no server) | ~0.2 MB footprint |
| Idle tick | ~2.4 us per session |
| Four live sessions, headless | 0.3% CPU |

## Offline floor

```
AC_DATA_DIR=~/Downloads/ac_data cargo run --release -p ac-client --example session_cost 24
```

One session warms everything shared, then 24 more are added and only
the growth from there is charged to a session. The collision, nav
graph, world grid and pathfinder rows all read 0.0 MB for those 24,
which is the sharing working.

Read the per-phase rows as the price of the *process*, not of a
session: the world grid is 24 MB and the wide planner 13 MB however
many sessions run. An earlier version of this harness divided those by
N and reported them as a per-session cost, which overstated a session
by more than twenty times.

These sessions never hear from a server, so they carry no objects and
no scenery. Treat 0.2 MB as a floor.

## Live cost

```
./target/release/acbot --data-dir ~/Downloads/ac_data --connect 127.0.0.1 \
  --client acct1:pass --client acct2:pass ... --duration 90
# then, against the pid:
vmmap --summary <pid> | grep 'Physical footprint:'
ps -o %cpu= -p <pid>
```

One session placed in the world: 29.0 MB. Four: 30.7 MB. That is about
0.6 MB for each session after the first, with its objects and its
landblock loaded. Four together held 0.3% of a core, sampled over 24
seconds.

Caveats worth keeping in mind: four is a small sample, the characters
were standing still rather than fighting or autoplaying, and the server
was local, so nothing was waiting on a network.

## Where the cost is not

Not in per-session memory, and not in the headless tick. If a machine
runs short it will be the rendered window or the server, not the
simulation.

Only the active session holds GPU state. Switching sessions drops the
old one's pickables and animation players and re-instances the new one
on the next frame (`bins/acviewer/src/main.rs`, `switch_to`), so the
window costs one session's worth of GPU whatever else is running
alongside it.

`acbot --tick-hz` sets the pace for every session in a process; 20 is
the game's, and a process of followers gets by on 10.

## Not yet measured

* Frame time in the window, with a person playing one session and
  others following. There is no headless frame-time harness.
* A session under load: fighting, casting, looting, autoplay running.
* Many sessions across separate processes rather than within one.
