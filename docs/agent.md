# The autoplay agent

How a character decides what to do, why it is built the way it is, and
what it is being moved towards.

See also: [architecture.md](architecture.md) (the crate map),
[multi-session.md](multi-session.md) (many characters at once).

## What this has to be good at

A character is an agent in a real-time game, ticked at 20 Hz, with a
dozen or more of it running in one process. That puts four demands on
the design, and they pull in different directions:

- **Reflexes must be instant and cheap.** A spell in the air, health at
  40%, the player grabbing the keys: these preempt everything and must
  cost nothing to check, because the cost is paid by every session on
  every tick.
- **Plans must survive interruption.** A trip to town is minutes long,
  crosses landblocks, and can be broken off by a fight, a death or a
  disconnection. It has to resume, not restart.
- **Failure is the normal case.** Almost nothing a character tries is
  guaranteed: the counter will not take the item, the corpse will not
  open, the doorway will not admit it, the purse is empty, the pack is
  full, the quest is on cooldown. Most of the work is deciding what to
  do when told no.
- **A party must agree without negotiating.** Every session works out
  the group's mode from the same roster and must reach the same answer,
  because there is no authority to arbitrate and no time to vote.

## What it is now

Three things that grew separately.

**A flattened behaviour tree.** `Client::tick_autoplay` is a fixed
priority chain -- dodge, survive, recover, academy, buff, vitals, loot,
follow, team, salvage, fight, buff again, follow again, tidy, grow --
where each step returns whether it claimed the tick. That is exactly a
behaviour tree's root selector, written out by hand. It works, and the
order is genuinely load-bearing, but the order is control flow rather
than data: nothing can read it, show it, or reason about it.

**A group state machine.** `logistics` is a proper FSM over
`Hunting | Restocking{Shopping, HandOver, Away, HandOut}`, computed by
every session from the shared roster. This part is right and is not
changing: determinism is the whole point of it.

**Sixty-two fields of state.** `Autoplay` carries 62 fields and
`growth::State` another 32. Many are legitimate memory. A growing
number are not: `too_heavy`, `run_was_futile`, `stopped_in_town`,
`handed_over`, `give_tries`, `take_tries`, `refused_kinds`, `shelved`,
`walking_to`, `reaching`, `unsellable`, `skip_vendors`, `held_back`.

Each of those is the scar of one bug. Every one of them encodes the
same thing in a slightly different way: *something was refused, and
here is how long to wait before asking again.*

## The actual problem

Every failure found in a day of live testing was one of two kinds:

- **An unmodelled precondition.** Buying without checking the weight it
  would add. Offering a vendor an item it will never take. Walking to a
  counter behind a door the character cannot open. Asking for a
  component no shop in the world sells.
- **An unhandled outcome.** A give of part of a stack that the server
  drops without a word, asked 496 times. A corpse that will not open,
  asked until it rotted. An item refused for the day, asked on every
  corpse after.

Both have the same root: **an action returns `bool`.** True means it
did something. False means... it did not, this tick, for a reason the
caller cannot see and therefore cannot act on. So "keeps trying for
ever" is not a bug that was written; it is the default that falls out
of the type. Every fix has been a new flag bolted on beside the call
site, which is why there are sixty-two of them.

This is the thing to fix, and it is independent of any larger
restructuring.

## Where it is going

Four layers, because the four demands above are genuinely different
problems and one paradigm serves none of them well.

### 0. Reflexes -- fixed priority

Dodge, survive, recover, hand back to the player. An ordered list, no
scoring, no planning, O(1). This is what the current chain does well and
it stays as it is.

### 1. Goal selection -- utility

Score the standing goals each tick -- Fight, Loot, Restock, Buff,
Follow, Salvage, Idle -- from world state, and take the best.

Static priority is what makes the present code brittle: the order of
the chain silently decides a hundred questions, and every "except when"
becomes another branch. Utility says the same things out loud and in
one place. It is already being hand-written as constants: `CORPSE_URGENT`
is "looting outscores fighting when the body is nearly gone",
`go_at: 0.35` is "restocking outscores hunting below a third of stock".
Those are utility curves with the arithmetic inlined.

Scoring must stay cheap: a handful of arithmetic per goal, no
allocation, because it runs per session per tick.

### 2. Plans -- behaviour trees, and one planner

Each goal expands into a small behaviour tree: a sequence of steps with
guards, resumable, interruptible. Most goals are honestly a fixed
sequence and a tree says so plainly.

**Restocking is the exception and should be planned.** It is the one
goal with interacting preconditions over a shared resource pool --
purse, burden, slots, what the shop stocks, what it will buy, whether
the door opens, whether the place can be reached -- and it is where
every bug has been. A small domain planner over those preconditions
would have *derived* "cannot buy because too heavy, therefore sell
first" instead of it being patched in after the fact. Not general GOAP:
a handful of actions with declared preconditions and effects, searched
over a tiny state, replanned only when something changes.

### 3. The group -- keep the FSM

Unchanged. Small, deterministic, and its determinism is the point.

### Why not one paradigm

- **Pure FSM**: state explosion. Every new concern is another flag or
  another state, which is the road already travelled.
- **Pure behaviour tree**: leaves goal selection as static priority,
  which is today's problem.
- **Pure GOAP**: a replan per agent per tick is the wrong cost across a
  dozen sessions, it is hard to answer "why did it do that", and
  reflexes must never be planned at all.
- **Pure utility**: no good account of multi-step plans, which is most
  of what a trip to town is.

## The migration

Four stages, each shippable on its own, in this order. Nothing here is
a rewrite; the behaviour at each step is meant to be identical except
where a bug is being fixed.

### Stage 1 -- typed outcomes

Replace `bool` with an outcome that says what happened:

```rust
pub enum Did {
    /// Claimed the tick and is working.
    Acting,
    /// Finished; let something else have the tick.
    Done,
    /// Cannot proceed yet, for a reason that will pass by itself.
    Waiting(Because),
    /// Cannot proceed, and asking again soon will not help.
    Blocked(Because),
    /// Will never work. Do not ask again.
    Refused(Because),
}
```

`Because` carries a short reason and, where the server gave one, the
weenie error behind it. One retry policy reads that: `Waiting` retries
freely, `Blocked` backs off, `Refused` stops. That single change
subsumes `give_tries`, `take_tries`, `refused_kinds`, `shelved`,
`unsellable`, `run_was_futile`, `too_heavy` and `skip_vendors`, and it
makes every one of them visible in the log and the UI instead of
invisible in a boolean.

Done first because it is mechanical, testable, pays for itself
immediately, and makes the later stages cheaper.

### Stage 2 -- the chain becomes data

Lift the priority chain out of control flow into an ordered table of
named steps. Same order, same behaviour, but the order can be read,
logged, shown in a panel and tested.

### Stage 3 -- utility at the root

Replace the static order of the *goal* steps (not the reflexes) with
scoring. The constants that already encode the trade-offs become the
curves.

### Stage 4 -- the restock planner

Actions with preconditions and effects over `{purse, burden, slots,
stock, gate, reachable}`, replanned when one changes.

## Rules that hold whatever the structure

These are settled and are not up for redesign by a later stage.

- **Never sell** what is tinkered, inscribed, equipped, retained, or
  flagged unsellable, whatever any rule or profile says. A policy may
  not override it.
- **Nothing is given up on for good** except by an explicit `Refused`.
  A session can run for days; a daily limit is a wait, not a grudge.
- **The player's hands win.** Any manual input stops the character
  steering itself unless autoplay is on.
- **A profile is shared, live.** A rule switched off is off for every
  character reading that profile, at once.
