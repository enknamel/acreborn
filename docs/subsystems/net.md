# Network protocol

Implemented in `crates/ac-net`. Sources: ACE `Source/ACE.Server/Network/*`
(the server side we must interoperate with), `ACE.Common/Cryptography`, and
the client (`FUN_00542570` header hash, `FUN_00542780` packet assembly,
`FUN_00542650` sendto over WSOCK32 ordinal 20).

## Transport

UDP. The client talks to the server's login port (9000) and, after the
handshake, sends `ConnectResponse` once to port+1 (9001). The server sends
everything from 9001 back to the client's endpoint. One client socket works
for both.

## Packet

```
u32 sequence   u32 flags   u32 checksum   u16 id   u16 time   u16 size   u16 iteration
[optional header fields, in flag order]
[fragments, if BlobFragments]
```

* `size` counts everything after the 20-byte header. Max 464.
* `id` is the server-assigned client id from `ConnectRequest` (server
  packets carry the server id, 0xB in ACE).
* `checksum = hash32(header with checksum = 0xBADD70DD) + (P ^ K)`, with
  `P = hash32(optional bytes) + Σ (hash32(fragment header) + hash32(fragment data))`
  and `K` the next ISAAC key when `EncryptedChecksum` is set, else 0.
* `hash32(data) = (len << 16) + Σ le_u32 + trailing bytes into the high
  bytes first`.

Flags (`packet::flags`): Retransmission 0x1, EncryptedChecksum 0x2,
BlobFragments 0x4, ServerSwitch 0x100, Referral 0x800, RequestRetransmit
0x1000 (u32 count + seqs), RejectRetransmit 0x2000, AckSequence 0x4000
(u32), LoginRequest 0x10000 (body = login request), WorldLoginRequest
0x20000, ConnectRequest 0x40000 (f64 time, u64 cookie, u32 client id, u32
server seed, u32 client seed, u32 pad), ConnectResponse 0x80000 (u64
cookie), NetError 0x100000 / NetErrorDisconnect 0x200000 (u32, u32),
CICMDCommand 0x400000, TimeSync 0x1000000 (f64), EchoRequest 0x2000000
(f32), EchoResponse 0x4000000 (f32, f32), Flow 0x8000000 (u32, u16).

## Fragment

```
u32 sequence  u32 id  u16 count  u16 size(incl. 16)  u16 index  u16 queue  data
```

A message larger than 448 bytes is split into `count` fragments sharing a
sequence. Fragment sequences are per direction, start at 1, and must be
delivered in order (ACE holds early ones). `queue` is the message group
(UI = 9, Weenie = 3, Database = 5, ...).

## ISAAC

Not standard ISAAC (`isaac.rs`): the key schedule mixes only the golden
ratio, then `a = b = c = seed` and one scramble; keys are read from index
255 downwards. Two streams per session: server->client seeded with the
first seed in `ConnectRequest`, client->server with the second. A receiver
searches up to 256 keys ahead for a match so lost packets do not
desynchronise it. Verified against ACE for three seeds
(`tests/golden/net/isaac_*.txt`).

## Login flow

| dir | packet / message | notes |
|---|---|---|
| C2S :9000 | `LoginRequest` seq 0, plain | body: string16 "1802", u32 len, u32 auth type 2, u32 flags 0, u32 timestamp, string16 account, string16 "", u32 len, u8 pwlen, password |
| S2C | `ConnectRequest` | cookie, client id, seeds |
| C2S :9001 | `ConnectResponse` seq 1, plain, id = client id | u64 cookie |
| S2C | `ServerName` 0xF7E1, `CharacterList` 0xF658, `DDD_Interrogation` 0xF7E5 | first encrypted data |
| C2S | `DDD_InterrogationResponse` 0xF7E6 (queue 5) | u32 language 1, i32 n, per dat: i32 type, i32 id, i32 iterations, i32 -(iterations+1); i32 0; u32 0 |
| S2C | `DDD_EndDDD` 0xF7EA (or `DDD_BeginDDD` if iterations differ and patching is on) | |
| C2S | `CharacterEnterWorldRequest` 0xF7C8 | opcode only |
| S2C | `CharacterEnterWorldServerReady` 0xF7DF | |
| C2S | `CharacterEnterWorld` 0xF657 | u32 character id, string16 account |
| S2C | `PlayerCreate` 0xF746, `ObjectCreate` 0xF745 ..., `GameEvent` 0xF7B0 (PlayerDescription 0x13 ...) | in world |

The client's first data packet is sequence 2 (ACE initialises its last
received sequence to 1). Acks: flag 0x4000 with the last processed server
sequence, sent every ~2 s. Echo: client sends `EchoRequest(f32 client
time)`; server answers `EchoResponse`; ACE also compares clocks for
speed-hack detection, so the time must advance in real seconds.

## Message framing

Every fragment payload starts with `u32 opcode`. `GameEvent` 0xF7B0 bodies
are `u32 guid, u32 sequence, u32 event type, ...`; `GameAction` 0xF7B1 are
`u32 sequence, u32 action type, ...`. Strings are `string16` (u16 length,
bytes, pad to 4).

## Verified against ACE (2026-09-04)

`acclient --create` logs in, creates a character, enters the world and
receives PlayerDescription, PlayerCreate and ~30 ObjectCreate messages.
Three ACE behaviours that are not obvious from the protocol alone:

* **First data packet is sequence 2.** ACE initialises its own
  last-received counter to 1 and skips sequence 1 when it starts encrypted
  sending; the client must also start at 1 or it buffers seq 2 forever.
* **ConnectResponse races password verification.** ACE sends the
  ConnectRequest before it has finished bcrypt-verifying the password and
  only accepts a ConnectResponse afterwards (~20-40 ms later). Replying
  within microseconds gets the response silently dropped, so the session
  resends it every 250 ms until the first data packet arrives.
* **Do not list a DAT the server lacks.** ACE dereferences its own copy of
  each archive named in `DDD_InterrogationResponse`; a server without
  `client_local_English.dat` throws a NullReferenceException and never
  answers. The client reports only portal and cell.

Debugging: set the `Packets` logger to DEBUG in
`reference/ace-run/Config/log4net.config` (picked up live) and read
`/ace/Logs/ACE_Log.txt` in the container; `<<<` lines are inbound.

## Gameplay facts verified against ACE (2026-09-05)

- **Server-driven MoveTo.** Using or attacking something out of reach makes
  ACE send a MovementEvent (0xF74C, type 6 MoveToObject / 7 MoveToPosition)
  for our own guid and then poll `WithinUseRadius` against the position we
  report. ACE does not move the player itself. Any MoveToState (0xF61C) we
  send while `IsPlayerMovingTo` cancels that chain (ActionCancelled), so the
  client must keep sending AutonomousPosition (0xF753) but hold MoveToState
  until the server describes us idle again (a MovementEvent for our guid
  with no target). Our own echoed motion states also arrive as
  MovementEvents; they carry no target.
- **Melee.** ChangeCombatMode (0x0053, u32 mode: 1 peace, 2 melee) switches
  the stance, broadcast back as our MovementEvent style. TargetedMeleeAttack
  (0x0008: target, attack height u32, power f32). Every swing sequence ends
  with AttackDone (0x01A7) carrying WeenieError 0x36 ActionCancelled: that is
  the normal end, not a failure. Blows arrive as AttackerNotification 0x01B1
  / DefenderNotification 0x01B2 (string16 name, u32 damage type, f64 percent,
  u32 damage, [u32 location], u32 critical, u64 conditions) and evasions as
  0x01B3/0x01B4 (string16). UpdateHealth 0x01C0 (guid, f32) gives the target's
  health fraction. Deaths: VictimNotification 0x01AC / KillerNotification
  0x01AD (string16).
- **Lifestone protection.** After a respawn the player cannot attack or be
  attacked for a while; attacks are refused with WeenieError 0x0502 or
  dispelled ("Your actions have dispelled the Lifestone's magic!").
- **Loot.** Use on a corpse or chest opens it: ViewContents (0x0196: container
  guid, count, [item guid, container type]) plus ObjectCreates for the items
  with their container set. PutItemInContainer (0x0019: item, container,
  placement) moves one item; the server refuses a second pickup while one is
  in flight (WeenieError 0x1D YoureTooBusy and InventoryServerSaveFailed
  0x00A0: item guid, error), so take items one at a time and wait for the
  InventoryPutObjInContainer event (0x0022). NoLongerViewingContents (0x0195,
  container guid) closes it.
- **Jump.** Jump (0xF61B): f32 extent, f32x3 velocity in the character's
  local frame, four u16 sequences, then a u32 object guid and u32 spell id
  that ACE reads even though they are unused. Launch velocity: vz =
  sqrt(19.6 * h), h from the jump skill and charge (see player.rs).
- **Errors as text.** CommunicationTransientString 0x02EB (string16) carries
  the "You cannot attack X" style messages; WeenieError 0x028A (u32) and
  WeenieErrorWithString 0x028B (u32, string16) the coded ones.
