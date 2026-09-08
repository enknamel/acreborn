# Protocol coverage

What the client handles of the ACE wire protocol, audited on 2026-09-08
against `reference/ext/ACE/Source/ACE.Server/Network/{GameMessages,GameEvent,GameAction}`
(the opcode enums plus the `Messages/`, `Events/` and `Actions/` folders,
which say what ACE actually sends and accepts). Layouts are in
`crates/ac-net/src/messages.rs` (`opcode`, `event`, `action` modules, with
`opcode::name` / `event::name` for logs) and `crates/ac-world/src/object.rs`.

Status words:

* **handled**: parsed and applied to state (`World::apply` in
  `crates/ac-world/src/lib.rs`, the character sheet in `stats.rs`, or
  `ac_client::Client::tick` / `chat_message`).
* **parsed**: decoded, a bad body is reported, nothing reads the result.
* **named**: the constant exists so the debug log names it; not decoded.
* **never sent**: ACE has no serializer for it (an enum value only).

Seen-in-the-wild counts come from `RUST_LOG=ac_client=debug` on account
`acreborn2` ("Test Mage", level 1, stuck in the Training Academy, autoplay
on): a 5-minute session before the changes below and a 150-second one
after. Only messages that reached the "unhandled" log line are counted;
everything handled is not logged, so a zero means "handled or not seen".
The log line names each message now (`message 0xf748 UpdatePosition (40
bytes)`), so a future tally is
`grep -o 'message 0x[0-9a-f]* [A-Za-z_]*\|game event 0x[0-9a-f]* [A-Za-z_]*' LOG | sort | uniq -c | sort -rn`.

## Server to client messages

| opcode | ACE name | status | where | seen (5 min / 150 s) | affects | priority |
|---|---|---|---|---|---|---|
| 0x0024 | InventoryRemoveObject | handled | World: removes the object | | inventory | done |
| 0x0197 | SetStackSize | handled | World: stack, value | | inventory | done |
| 0x019E | PlayerKilled | **handled (new)** | World: victim health 0; chat line | | a fellow's death, PK notices | done |
| 0x01E0 | EmoteText | handled | chat_message | | chat | done |
| 0x01E2 | SoulEmote | handled | chat_message | | chat | done |
| 0x02BB | HearSpeech | handled | chat_message | | chat | done |
| 0x02BC | HearRangedSpeech | handled | chat_message | | chat | done |
| 0x02CD | PrivateUpdatePropertyInt | handled | stats (level, credits, all ints by id) | | level, burden, combat mode, deaths | done |
| 0x02CE | PublicUpdatePropertyInt | **handled (extended)** | World: StackSize, Value, Structure, MaxStructure, CurrentWieldedLocation, PlayerKillerStatus (PK description flags follow); UiEffects and RadarBlipColor parsed | | kit uses left, wield slot, PK status of players and their projectiles | done |
| 0x02CF | PrivateUpdatePropertyInt64 | handled | stats (xp) | | xp | done |
| 0x02D0 | PublicUpdatePropertyInt64 | **parsed (new)** | World | | emote-driven stat changes on other players | low |
| 0x02D1 | PrivateUpdatePropertyBool | **handled (new)** | stats.bools (`spell_components_required()`, Afk, IsAdvocate...) | | whether the server wants spell components | done |
| 0x02D2 | PublicUpdatePropertyBool | **handled (new)** | World: Locked (Openable flag follows); Open/UiHidden parsed | | locked chests and doors | done |
| 0x02D3 | PrivateUpdatePropertyFloat | **handled (new)** | stats.floats by id | | rare timers, emote stats | done |
| 0x02D4 | PublicUpdatePropertyFloat | **parsed (new)** | World | | emote-driven stat changes | low |
| 0x02D5 | PrivateUpdatePropertyString | handled | stats (name, all strings by id) | | name, titles | done |
| 0x02D6 | PublicUpdatePropertyString | **handled (new)** | World: Name | | renamed pets, corpses | done |
| 0x02D7 | PrivateUpdatePropertyDataID | **handled (new)** | stats.dids; MotionTable also updates the player object | | our animation table after a transformation | done |
| 0x02D8 | PublicUpdatePropertyDataID | **handled (new)** | World: Setup, MotionTable, Icon | | model swaps in view | done |
| 0x02D9 | PrivateUpdatePropertyInstanceID | **handled (new)** | stats.iids (`current_attacker()`) | | who is hitting us | done |
| 0x02DA | PublicUpdateInstanceId | handled | World: Container, Wielder | | inventory of others, loot | done |
| 0x02DB | PrivateUpdatePosition | handled | stats: corpse position | | corpse recovery | done |
| 0x02DC | PublicUpdatePosition | never sent | | | | n/a |
| 0x02DD | PrivateUpdateSkill | handled | stats | | skills | done |
| 0x02DE | PublicUpdateSkill | never sent | | | | n/a |
| 0x02DF | PrivateUpdateSkillLevel | handled | stats | | skills | done |
| 0x02E0 | PublicUpdateSkillLevel | never sent | | | | n/a |
| 0x02E1 | PrivateUpdateSkillAC | handled | stats (not in ACE's enum; retail only) | | skill training state | done |
| 0x02E3 | PrivateUpdateAttribute | handled | stats | | attributes | done |
| 0x02E4 | PublicUpdateAttribute | never sent | | | | n/a |
| 0x02E7 | PrivateUpdateVital | handled | stats | | vitals | done |
| 0x02E8 | PublicUpdateVital | never sent (class exists, no caller) | | | | n/a |
| 0x02E9 | PrivateUpdateAttribute2ndLevel | handled | stats: current vital | | health/stamina/mana | done |
| 0xEA60 | AdminEnvirons | named | | | admin weather and ambience | low |
| 0xF619 | PositionAndMovement | never sent | | | | n/a |
| 0xF625 | ObjDescEvent | handled | World: palettes, textures | | appearance | done |
| 0xF643 | CharacterCreateResponse | handled | Client: create/restore result | | lobby | done |
| 0xF653 | CharacterLogOff | **named (new)** | Client logs it | | a clean logoff would return to the lobby; the client never asks to log off, it disconnects | low |
| 0xF655 | CharacterDelete | handled | Client | | lobby | done |
| 0xF658 | CharacterList | handled | Client | | lobby | done |
| 0xF659 | CharacterError | handled | Client: `Event::Refused` | | lobby | done |
| 0xF6EA | ForceObjectDescSend | never sent (C2S only) | | | | n/a |
| 0xF745 | ObjectCreate | handled | World | | everything | done |
| 0xF746 | PlayerCreate | handled | World | | our guid | done |
| 0xF747 | ObjectDelete | handled | World | | everything | done |
| 0xF748 | UpdatePosition | handled | World; our own echoes are ignored on purpose (they lag a quarter second) unless a teleport/force sequence moved | 854 / 321 (own echoes) | positions | done |
| 0xF749 | ParentEvent | **handled (new)** | World: child wielded by parent, off the ground | | others' weapons and nocked arrows, creature ammo | done |
| 0xF74A | PickupEvent | **handled (new)** | World: object off the ground | | loot taken by others | done |
| 0xF74B | SetState | handled | World: physics state, hidden | | visibility | done |
| 0xF74C | MovementEvent | handled | World: motion, move-to, commands | | animation, server-driven walks | done |
| 0xF74E | VectorUpdate | **handled (new)** | World: velocity | | prediction of pushed or deflected things | done |
| 0xF750 | Sound | handled | Client: `Event::Sound` | | sound | done |
| 0xF751 | PlayerTeleport | handled | Client: takes the position, LoginComplete | | portals, recalls | done |
| 0xF753 | AutonomousPosition (S2C) | never sent (ACE's send is commented out) | | | | n/a |
| 0xF754 | PlayScriptId | never sent | | | | n/a |
| 0xF755 | PlayEffect | **handled (new)** | World `Applied::Effect`, Client `Event::Effect {guid, script, speed}`, script event "effect" | 16 / 0 | particle effects (fizzle, portal flash, level up); no state | done |
| 0xF7B0 | GameEvent | handled | see below | | | |
| 0xF7C1 | AccountBanned | **handled (new)** | Client: `Event::Refused` with the reason logged | | login | done |
| 0xF7CC | GetServerVersion | never sent | | | | n/a |
| 0xF7CD | FriendsOld | never sent | | | | n/a |
| 0xF7DB | UpdateObject | handled | World (as ObjectCreate) | | re-described objects | done |
| 0xF7DC | AccountBoot | handled | Client: `Event::Refused` | | login | done |
| 0xF7DE | TurbineChat | handled | chat_message | | chat rooms | done |
| 0xF7DF | CharacterEnterWorldServerReady | handled | Client | | login | done |
| 0xF7E0 | ServerMessage | handled | chat_message | | system chat | done |
| 0xF7E1 | ServerName | handled | Client (logged) | | none | done |
| 0xF7E2..E4, E8, E9 | DDD data/patch messages | named | | | DAT patching; the client reports matching iterations so ACE never starts one | n/a |
| 0xF7E5 | DDD_Interrogation | handled | Session answers | | login | done |
| 0xF7E7 | DDD_BeginDDD | handled | Client (logged, cannot patch) | | login | done |
| 0xF7EA | DDD_EndDDD | handled | Client | | login | done |

## Game events (0xF7B0)

| event | ACE name | status | where | seen | affects | priority |
|---|---|---|---|---|---|---|
| 0x0003 | AllegianceUpdateAborted | handled (ignored on purpose) | World | | none | done |
| 0x0004 | PopupString | handled | chat_message | | chat | done |
| 0x0013 | PlayerDescription | handled | stats (ints, int64s, strings, **bools, floats, data ids, instance ids (new)**, attributes, skills, spells, enchantments, inventory) | | the whole character | done |
| 0x0020 | AllegianceUpdate | handled | World.allegiance | | allegiance panel | done |
| 0x0021 | FriendsListUpdate | handled | World.friends | | friends | done |
| 0x0022 | InventoryPutObjInContainer | handled | World | | inventory | done |
| 0x0023 | WieldObject | handled | World | | inventory | done |
| 0x0029 | CharacterTitle | handled | World.titles | | titles | done |
| 0x002B | UpdateTitle | handled | World.titles | | titles | done |
| 0x0052 | CloseGroundContainer | handled | World | | loot | done |
| 0x0062 | ApproachVendor | handled | World.open_vendor | | vendors | done |
| 0x0075 | StartBarber | named | | | barber window | low (UI) |
| 0x00A0 | InventoryServerSaveFailed | handled | Client: clears the loot in flight | | loot, wield | done |
| 0x00A3 | FellowshipQuit | handled | World | | fellowship | done |
| 0x00A4 | FellowshipDismiss | handled | World | | fellowship | done |
| 0x00B4 | BookDataResponse | handled | chat_message / Client.book | | books, signs | done |
| 0x00B5..B7 | Book modify/add/delete page responses | named | | | writing in books | low |
| 0x00B8 | BookPageDataResponse | handled | Client.book | | books | done |
| 0x00C3 | GetInscriptionResponse | named | | | inscriptions (never asked) | low |
| 0x00C9 | IdentifyObjectResponse | handled | Client.appraisals | | appraisal | done |
| 0x0147 | ChannelBroadcast | handled | chat_message | | fellow/allegiance chat | done |
| 0x0148 | ChannelList | named | | | who is in an admin channel | low |
| 0x0149 | ChannelIndex | named | | | admin channels | low |
| 0x0196 | ViewContents | handled | World.open_container | | loot | done |
| 0x019A | InventoryPutObjectIn3D | handled | World | | drops | done |
| 0x01A7 | AttackDone | handled | Client: attack_pending | | combat | done |
| 0x01A8 | MagicRemoveSpell | handled | stats | | spellbook | done |
| 0x01AC | VictimNotification | handled | chat_message | | death | done |
| 0x01AD | KillerNotification | handled | chat_message | | kills | done |
| 0x01B1 | AttackerNotification | handled | chat_message | | combat log | done |
| 0x01B2 | DefenderNotification | handled | chat_message | | combat log | done |
| 0x01B3 | EvasionAttackerNotification | handled | chat_message | | combat log | done |
| 0x01B4 | EvasionDefenderNotification | handled | chat_message | | combat log | done |
| 0x01B8 | CombatCommenceAttack | named | | | the swing started (after the walk up); AttackDone already ends it | low |
| 0x01C0 | UpdateHealth | handled | World: health fraction | | target health; **ACE only sends it for the creature selected with QueryHealth (see C2S)** | done |
| 0x01C3 | QueryAgeResponse | named | | | /age (never asked) | low |
| 0x01C7 | UseDone | handled | Client: ends the server walk | | use | done |
| 0x01C8 | AllegianceAllegianceUpdateDone | handled (ignored on purpose) | World | 1 / 1 | none | done |
| 0x01C9 | FellowshipFellowUpdateDone | named (no body) | | | none | done |
| 0x01CA | FellowshipFellowStatsDone | never sent | | | | n/a |
| 0x01CB | ItemAppraiseDone | never sent | | | | n/a |
| 0x01E2 | Emote | never sent as an event (SoulEmote 0x01E2 is the message) | | | | n/a |
| 0x01EA | PingResponse | named (no body) | | | answer to PingRequest, never asked | low |
| 0x01F4 | SetSquelchDB | handled | World.squelches | | squelches | done |
| 0x01FD | RegisterTrade | handled | World.trade | | trade | done |
| 0x01FE | OpenTrade | never sent | | | | n/a |
| 0x01FF..0x0208 | Close/Add/Remove/Accept/Decline/Reset trade, TradeFailure, ClearTradeAcceptance | handled | World.trade | | trade | done |
| 0x021D | HouseProfile | handled | World.house_profile | | housing | done |
| 0x0225 | HouseData | handled | World.house | | housing | done |
| 0x0226 | HouseStatus | handled | World.house | | housing | done |
| 0x0227 | UpdateRentTime | handled | World.house | | housing | done |
| 0x0228 | UpdateRentPayment | handled | World.house | | housing | done |
| 0x0248 | HouseUpdateRestrictions | named | | | who may enter a house nearby; the server refuses entry itself | low |
| 0x0257 | UpdateHAR | handled | World.house_access | | guest list | done |
| 0x0259 | HouseTransaction | named | | | a notice after buying/renting | low |
| 0x0264 | QueryItemManaResponse | **handled (new)** | World: `WorldObject::mana` | | mana of an item (after QueryItemMana) | done |
| 0x0271 | AvailableHouses | named | | | house search list | low (UI) |
| 0x0274 | CharacterConfirmationRequest | handled | World.confirmations | | invitations | done |
| 0x0276 | CharacterConfirmationDone | handled | World.confirmations | | invitations | done |
| 0x027A | AllegianceLoginNotification | handled | World.allegiance | | member online | done |
| 0x027C | AllegianceInfoResponse | handled | World.allegiance_info | | allegiance | done |
| 0x0281..0x0285, 0x028C | Chess (JoinGameResponse, StartGame, MoveResponse, OpponentTurn, OpponentStalemate, GameOver) | named | | | chess boards | low |
| 0x028A | WeenieError | handled | chat_message (weenie_errors) | | errors | done |
| 0x028B | WeenieErrorWithString | handled | chat_message | 3 / 0 (was over-reported) | errors | done |
| 0x0295 | SetTurbineChatChannels | handled | Client.allegiance_room | | chat rooms | done |
| 0x02AE, 0x02B1, 0x02B3 | Admin plugin queries | never sent | | | | n/a |
| 0x02B4 | SalvageOperationsResult | handled | chat_message / Client | | salvage | done |
| 0x02BD | Tell | handled | chat_message | | tells | done |
| 0x02BE | FellowshipFullUpdate | handled | World.fellowship | | fellowship | done |
| 0x02BF | FellowshipDisband | handled | World | | fellowship | done |
| 0x02C0 | FellowshipUpdateFellow | handled | World.fellowship members; **only sent once the client says its panel is open (see C2S)** | | fellows' health | done |
| 0x02C1 | MagicUpdateSpell | handled | stats | | spellbook | done |
| 0x02C2..0x02C8, 0x0312 | Enchantment update/remove/dispel/purge events | handled | stats.enchantments | | buffs, vitae, cooldowns | done |
| 0x02C9..0x02CC | Portal storm brewing/imminent/storm/subsided | named | | | a notice; the storm's move is a PlayerTeleport | low |
| 0x02EB | CommunicationTransientString | handled | chat_message | | errors as text | done |
| 0x0314 | SendClientContractTrackerTable | named | | | quest contract panel at login | low (UI) |
| 0x0315 | SendClientContractTracker | named | | | one contract | low (UI) |

## Client to server

Everything the client sends, by ACE `GameActionType`, and what it never
sends. The transport layer (`crates/ac-net/src/session.rs`) also sends
`ConnectResponse`, `AckSequence` every 2 s, `EchoRequest` every 5 s with
real elapsed seconds (ACE's `VerifyEcho` compares client and server
deltas and boots for "speedhacking" past a threshold), `EchoResponse` to
each server echo, and retransmissions on request.

### Sent

| action | name | sent from | notes |
|---|---|---|---|
| 0x0005 | SetSingleCharacterOption | options.rs | |
| 0x0008 / 0x000A | TargetedMeleeAttack / TargetedMissileAttack | `Client::attack` | target, height, power |
| 0x000F / 0x0010 | SetAfkMode / SetAfkMessage | console | |
| 0x0015 | Talk | `say` | |
| 0x0017 / 0x0018 / 0x0025 | Remove/Add/RemoveAllFriends | social | |
| 0x0019 | PutItemInContainer | loot, store, packs | one at a time |
| 0x001A | GetAndWieldItem | wield | |
| 0x001B | DropItem | drop | |
| 0x001D / 0x001E / 0x001F | Swear/BreakAllegiance, AllegianceUpdateRequest | allegiance | update request at placement |
| 0x002C | TitleSet | social | |
| 0x0031 / 0x0033 | Clear/SetAllegianceName | allegiance | |
| 0x0035 / 0x0036 | UseWithTarget / Use | interact | |
| 0x0044..0x0047 | RaiseVital/Attribute/Skill, TrainSkill | advance.rs | |
| 0x0048 / 0x004A | CastUntargeted/TargetedSpell | magic | **a targeted cast at a creature now also sends QueryHealth** |
| 0x0053 | ChangeCombatMode | combat toggles | 1 peace, 2 melee, 4 missile, 8 magic |
| 0x0054..0x0056 | StackableMerge, SplitToContainer, SplitTo3D | items | |
| 0x0058 / 0x0059 / 0x005B | Modify character/account/global squelch | social | |
| 0x005D | Tell | console | |
| 0x005F / 0x0060 | Buy / Sell | vendors | |
| 0x0063 | TeleToLifestone | recalls | |
| 0x00A1 | LoginComplete | after PlayerTeleport and at entry | the server ignores positions before it |
| 0x00A2..0x00A5 | Fellowship create/quit/dismiss/recruit | fellowship | |
| 0x00A6 | FellowshipUpdateRequest | **new: sent once per fellowship with panel_open = 1** | without it ACE never sends FellowshipUpdateFellow vitals (`Fellowship.OnVitalUpdate` checks `FellowshipPanelOpen`), so fellows' health stayed at whatever the full update said |
| 0x00AA / 0x00AE | BookData / BookPageData | books | |
| 0x00C8 | IdentifyObject | appraise | |
| 0x00CD | GiveObjectRequest | give | |
| 0x0147 | ChatChannel | fellow/allegiance chat | |
| 0x0195 | NoLongerViewingContents | loot | |
| 0x01A8 | RemoveSpell | spellbook | |
| 0x01B7 | CancelAttack | **new: `Client::cancel_attack`** | ends the swing chain and the walk; AttackDone follows |
| 0x01BF | QueryHealth | **new: `Client::query_health`, from `attack` and targeted casts** | ACE answers UpdateHealth at once and every player heartbeat (5 s) for the selected creature; nothing else pushes a target's health between blows |
| 0x01DF / 0x01E1 | Emote / SoulEmote | emotes | |
| 0x01E3 / 0x01E4 | Add/RemoveSpellFavorite | spell bar | |
| 0x01F6 / 0x01F7 / 0x01F8 / 0x01FA / 0x01FB / 0x0204 | Trade open/close/add/accept/decline/reset | trade | |
| 0x0224 | SetDesiredComponentLevel | magic | |
| 0x021C / 0x021E / 0x021F / 0x0221 | BuyHouse, HouseQuery, AbandonHouse, RentHouse | housing | query at placement |
| 0x0245..0x024D, 0x025C..0x025F, 0x0266..0x0268 | House guest/storage/boot/hooks actions | housing | |
| 0x0254 / 0x0256 | Set/ClearMotd | allegiance | |
| 0x0262 / 0x0278 / 0x028D / 0x02AB | TeleToHouse/Mansion/MarketPlace, RecallAllegianceHometown | recalls | |
| 0x0275 | ConfirmationResponse | confirm | |
| 0x0279 | Suicide ("die") | console | |
| 0x027B | AllegianceInfoRequest | allegiance | |
| 0x027D | CreateTinkeringTool | salvage | |
| 0x0286 | SpellbookFilter | spellbook | |
| 0x028F | EnterPkLite | console | |
| 0xF61B | Jump | player.rs | extent, local velocity, sequences, two trailing u32 |
| 0xF61C | MoveToState | player.rs, on every input change | raw motion flags, stance NonCombat, position, sequences 1/0/0/0, contact |
| 0xF753 | AutonomousPosition | player.rs, 4 a second while moving | position, sequences, contact |

### Never sent

| action | name | matters for a bot? |
|---|---|---|
| 0x0026 / 0x0027 | TeleToPklArena / TeleToPkArena | no |
| 0x0030 | QueryAllegianceName | no (AllegianceUpdate carries it) |
| 0x003B..0x0042, 0x02A0..0x02A7 | Allegiance officers, titles, locks, bans, chat gag/boot | monarch tools only |
| 0x00AB..0x00AD | Book modify/add/delete page | writing in books |
| 0x00BF | SetInscription | no |
| 0x00D6 | AdvocateTeleport | advocates only |
| 0x0140 | AbuseLogRequest | no |
| 0x0145 / 0x0146 / 0x0148 / 0x0149 | Add/Remove/List/IndexChannels | admin channels |
| 0x019B | StackableSplitToWield | rarely (split straight to a hand) |
| 0x019C / 0x019D | Add/RemoveShortCut | the spell bar is kept client-side; nothing is lost |
| 0x01A1 | SetCharacterOptions | options are set one at a time with 0x0005 |
| 0x01C2 / 0x01C4 | QueryAge / QueryBirth | no |
| 0x01E9 | PingRequest | no; the transport echo measures latency |
| 0x0216..0x021A | Consent list, player permissions | corpse looting permits between players |
| 0x0255 / 0x0258 | QueryMotd / QueryLord | no |
| 0x0263 | QueryItemMana | a bot managing mana-fed gear would want it; the answer is handled |
| 0x0269..0x026E | Chess | no |
| 0x0270 | ListAvailableHouses | house hunting |
| 0x0277 | BreakAllegianceBoot | monarch tools |
| 0x0290 / 0x0291 | FellowshipAssignNewLeader / ChangeOpenness | a leader handing over; open fellowships |
| 0x02AF / 0x02B2 | Plugin query responses | admin only |
| 0x0311 | FinishBarber | barber UI |
| 0x0316 | AbandonContract | quest UI |
| 0xF61E / 0xF661 | Do/StopMovementCommand | no; MoveToState carries the same |
| 0xF649 | TurnTo | no; ACE turns us itself for casts and attacks |
| 0xF6EA | ForceObjectDescSend | no |
| 0xF752 | AutonomyLevel | no |
| 0xF7C9 | JumpNonAutonomous | no |
| 0xF653 | CharacterLogOff (message) | a clean logoff to the lobby; the client disconnects instead, which ACE treats as a logoff after ~60 s |

### C2S details checked against ACE

* **MoveToState** (`GameActionMoveToState`, `Network/Motion/MoveToState.cs`,
  `RawMotionState.cs`): flags in the low 11 bits, command count above
  them, optional hold key / stance / forward / sidestep / turn fields,
  then `(u16 command, u16 seq, f32 speed)` per command, the position,
  four u16 sequences and the contact byte, aligned. Matches. ACE reads
  the stance (`CurrentStyle`) into an unused field; the server's own
  stance comes from ChangeCombatMode, so sending NonCombat while in
  melee stance is harmless. The four sequences are read and never
  compared (`PositionPack` only writes them), so the constant 1/0/0/0 is
  fine. Any MoveToState cancels a server move-to chain, which the
  client already avoids while one runs.
* **AutonomousPosition**: position, four u16, contact byte, align.
  Matches; ACE reads it only when not teleporting. Retail sent one a
  second; the client's four a second is accepted.
* **Jump**: extent, velocity, four u16, then a u32 guid and u32 spell id
  ACE reads. Matches.
* **Echo timing**: `VerifyEcho` compares successive client echo times
  with server wall-clock deltas; a drift above `EchoThreshold` that
  keeps growing counts toward a boot. The client's echo carries real
  elapsed seconds, so it stays in step.
* **Target health**: `Player.HandleActionQueryHealth` selects the
  creature and sends UpdateHealth; `Player.Heartbeat` (every 5 s) sends
  it again for the selected target while it lives. Damage does not send
  it. Fixed by sending QueryHealth (above).
* **Fellow vitals**: `GameActionFellowshipUpdateRequest` records
  `FellowshipPanelOpen`; `Fellowship.OnVitalUpdate` sends
  FellowshipUpdateFellow only to members with it set, and the request
  itself answers with a FellowshipFullUpdate. Fixed (above).

## Live tallies

5-minute session before the changes (autoplay on, in the academy, no
fights reachable), unhandled lines only:

| count | message | verdict |
|---|---|---|
| 854 | 0xF748 UpdatePosition | our own echoes, ignored on purpose (see the table); other objects' updates are applied and never logged |
| 16 | 0xF755 PlayEffect | now `Applied::Effect` / `Event::Effect` |
| 3 | game event 0x028B WeenieErrorWithString | was already a chat line; the log over-reported it (fixed) |
| 1 | game event 0x028A WeenieError | same |
| 1 | game event 0x01C8 AllegianceUpdateDone | ignored on purpose |
| 1 each | CharacterList, ServerName, DDD_Interrogation, DDD_EndDDD, ServerMessage | handled elsewhere; the log over-reported them (fixed) |

150-second session after the changes (same account and place):

| count | message | verdict |
|---|---|---|
| 321 | 0xF748 UpdatePosition | own echoes |
| 1 | game event 0x01C8 AllegianceUpdateDone | ignored on purpose |
| 1 each | CharacterList, ServerName, DDD_Interrogation, DDD_EndDDD | logged by the binary of that run; the lobby opcodes are silenced since |

Nothing unknown was seen in either session. The academy is a quiet
place: a session that fights, trades and travels through portals
would exercise PlayEffect, VectorUpdate, ParentEvent, PickupEvent and
the PK status updates, all of which are unit-tested from ACE's layouts.

## Left alone, and why

* UI-only events (barber, contracts, chess, house lists, channel lists,
  book editing) get a name in the log and nothing else: no bot reads
  them, and the lead asked for no UI.
* PublicUpdateProperty float/int64 on other objects: decoded so a short
  body is reported, but nothing in the world model reads a stranger's
  floats.
* CombatCommenceAttack: AttackDone already closes the attack; using the
  commence event to clear `attack_pending` earlier would let the client
  re-send an attack mid-swing, which ACE refuses.
* CharacterLogOff / a logoff request: the client disconnects, which ACE
  handles; a lobby round-trip needs a driver that wants it.
