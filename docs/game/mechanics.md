# Game mechanics reference

What the game actually does, gathered from the Asheron's Call community
wiki (asheron.fandom.com, September 2026 snapshot), the ACE server source
(read as a specification, never copied) and the client's own DAT tables.
This is the reference for "does the client behave correctly", independent
of how the original UI looked: acreborn's UI may be laid out and improved
freely as long as these rules hold.

Wire opcodes are the GameAction (client to server, `0xF7B1` sub-opcode)
and GameEvent (server to client, `0xF7B0` sub-opcode) numbers from ACE.
`docs/subsystems/net.md` keeps the protocol gotchas found live.

## 1. Magic

### Schools and skills

| School | Skill (formula) | Train / specialize credits | What it does |
|---|---|---|---|
| War Magic (School of the Arm) | (Focus + Self) / 4; 16 / 12 | bolts, arcs, rings, walls, volleys, blasts, streaks in seven elements |
| Life Magic (Heart) | (Focus + Self) / 4; 12 / 8 | heal/harm/drain vitals, protections and vulnerabilities, regeneration |
| Creature Enchantment (Left Hand) | (Focus + Self) / 4; 8 / 8 | buff/debuff attributes and skills |
| Item Enchantment (Right Hand) | (Focus + Self) / 4; 8 / 8 | weapon/armor/lock enchantments, portal and lifestone tie/recall/summon |
| Void Magic (Shadow) | (Focus + Self) / 4; 16 / 12 | nether damage, curses, corruption over time |
| Summoning | (Endurance + Self) / 3; 8 / 4 | summoning devices, not spells |

Supporting skills: Mana Conversion ((Focus + Self) / 6) reduces mana cost;
Arcane Lore (Focus / 3) activates enchanted items; Magic Defense
((Focus + Self) / 7) resists spells. A spell against a creature is resisted
by comparing the caster's skill with the target's Magic Defense.

### Learning spells and the spellbook

* Spells are learned from **scrolls** (use the scroll; the check is against
  the school's skill, not Arcane Lore) or taught by NPCs. Levels I to VI
  are sold by Scriveners and drop as loot, VII comes only from loot and
  trophies, VIII scrolls are crafted (quill + mana scarab + ink + glyph).
* The server owns the spellbook. It arrives in PlayerDescription (vector
  flag `Spell`: a hash table of spell id → `2.0f`) and changes with
  MagicUpdateSpell (0x02C1: spell id, layer) and MagicRemoveSpell
  (0x01A8). A player may delete a spell from the book with RemoveSpellC2S
  (0x01A8: spell id).
* Spellbook filters (which schools and levels are shown) are per
  character and stored server side: SpellbookFilter (0x0286) with a
  bitfield Creature 0x1, Item 0x2, Life 0x4, War 0x8, Level1..Level9
  0x10..0x1000, Void 0x2000; the current value comes in PlayerDescription
  (option flag `SpellbookFilters` 0x20).
* Right-clicking a spell shows school, mana, duration, description, the
  components required and the formula. The DAT SpellTable (0x0E00000E)
  has all of it: name, description, school, icon, category (spells of one
  category do not stack, the stronger overrides), base mana, range,
  power (difficulty), component loss (burn rate), meta type, duration,
  the formula (component ids), caster/target/fizzle effects, display
  order and mana mod (extra mana per extra target).

### The spell bar (spell tabs) is not the component list

The **spell bar** holds shortcuts to spells from the spellbook, nothing
else. Facts:

* There are **8 spell bars** (tabs). Each is an ordered list of spell ids.
  The first 9 spells of the shown bar are castable with the number keys
  (`1`..`9`; with a bar open, inventory shortcuts need Ctrl+number).
* Adding a spell: drag it from the book onto the shown bar, or
  double-click it, or drag it onto another tab. Removing: select and press
  Delete. The keys cycle tabs (Insert / Page Up) and spells (Delete /
  Page Down); Ctrl with those jumps to the first or last.
* The bars are **persisted by the server**: AddSpellFavorite (0x01E3:
  spell id, position, bar id) and RemoveSpellFavorite (0x01E4: spell id,
  bar id). Bar ids and positions are 0-based on the wire (ACE rejects bar
  > 7 and unknown spells). All 8 bars arrive in PlayerDescription when
  option flag `SpellLists8` (0x400) is set: 8 × (count u32, spell ids).
  `ac_world::stats::Options::spell_bars` already parses this.
* Casting from the bar selects the spell; the cast goes to the current
  target (or the caster for self spells) via CastTargetedSpell (0x004A:
  target guid, spell id) or CastUntargetedSpell (0x0048: spell id).
* A caster with a built-in spell shows it to the left of the bar; it is
  cast the same way (its id is the caster's `Spell` DID) and never
  fizzles, but costs the item's mana (`ItemManaCost`).

### Magic mode

* Casting needs **magic mode**, which needs a wielded **magic caster**
  (wand, orb, staff, sceptre...). ChangeCombatMode (0x0053) with mode 8
  (Magic); the server reverts to peace if no caster is wielded. Wielding a
  caster while in combat switches to magic mode automatically.
* Defense is highest in any combat stance and lowest in peace mode; trades,
  crafting and (better) healing happen in peace mode.

### Components and foci (consumed by the server, listed in the Components panel)

Spell components live in the inventory as ordinary stackable items. They
are **consumed when a spell is cast**, never by the spell bar. Two systems
coexist:

1. **Full formula** (the original system): each spell's formula is 5 to 8
   components in the order scarab, taper, herb, taper, powder, potion,
   taper, talisman. The scarab sets the level: Lead I, Iron II, Copper
   III, Silver IV, Gold V, Pyreal VI, Platinum VII, Mana VIII (Diamond and
   Dark are extra scarabs for a few spells). Lead to Silver come from any
   mage shopkeeper, Gold to Platinum from Mastermages.
2. **Foci** (post-"Castling"): a Focus of Strife (war), Verdancy (life),
   Enchantment (creature), Artifice (item) or Shadow (void) takes a whole
   pack slot and cannot be dropped or sold. With the school's focus in the
   pack, any spell of that school uses the **prismatic formula**: only the
   scarab(s) of the spell (plus Chorizite where the formula has it) and a
   number of **Prismatic Tapers** set by the spell's power: 1 taper for
   power 1, 2 for power 2, 3 for power 3, 4 or 7, 4 for power 5, 6, 8, 9 or
   10. New characters get a focus for every trained school; Scriveners
   sell them; augmentations can remove the need to carry them.

Casting rules (ACE `Player_Magic`, matching retail):

* Before the cast, the server checks that every component of the current
  formula is in the inventory (`require_spell_comps` server option; the
  test server has it off). Missing components fail the cast with
  "You don't have all the components for this spell."
* On a successful cast, **each component may burn**: chance = spell's
  component loss × the component's own destruction modifier (CDM from the
  SpellComponentsTable 0x0E00000F) × min(1, spell power / caster skill).
  Burned ones are removed from the inventory (stack decrement messages)
  and reported in chat as "The spell consumed the following components:
  ...". Scarabs and tapers burn most often; a skilled caster burns fewer.
* The **Components panel** (a tab of the magic panel) lists every
  component currently carried with its count, plus a desired quantity
  0..999 per component (SetDesiredComponentLevel 0x0224; the list comes in
  PlayerDescription under option flag `DesiredComps` 0x8). With a mage
  vendor open, `@fillcomps [type] [max price]` fills the buy list up to
  the desired quantities. A component must already be carried to appear.
* Spell words are spoken by the caster (server sends them as our own
  HearSpeech); they are the components' words in formula order.
* "Peas" are concentrated components that a Splitting Tool turns into
  20 to 50 components.

### Mana, fizzle, timing

* Mana cost = spell base mana (+ mana mod × number of enchantable items
  worn for item-enchantment armor spells, + mana mod × fellows for
  fellowship spells). A trained Mana Conversion skill reduces it with two
  skill checks against half and full difficulty (ACE `GetManaCost`).
  Not enough mana: WeenieError "You don't have enough mana to cast that."
* Fizzle: success chance = 1 / (1 + e^(−0.07 × (skill − power))), and a
  skill more than 50 below the power fails outright. Level VIII spells
  have power 400 (50% at 400 skill, 80% at 420, 99% at 460). A fizzle
  still costs 5 mana, plays the fizzle effect, and reports WeenieError
  0x0402 "Your spell fizzled." Built-in item spells never fizzle.
* Casting war right after void (or void after war) within 3 to 5 s fails
  ("The Elemental energies permeating your blood cause this Void magic to
  fail").
* Cast time is the wind-up gestures (one per non-lead scarab in the
  formula) plus the cast gesture, taken from the caster's motion table in
  the Magic stance; a caster item may replace the cast gesture.
* Target validity: self-targeted spells only on self, non-item spells only
  on creatures, beneficial spells not on monsters, harmful spells on other
  players only under PK rules; the range is the spell's base range
  constant plus range mod × skill.

### Enchantments (buffs and debuffs)

* Active spells on the character are the **enchantment registry**, sent
  in PlayerDescription and updated by MagicUpdateEnchantment (0x02C2),
  MagicUpdateMultipleEnchantments (0x02C4), MagicRemoveEnchantment (0x02C3),
  MagicRemoveMultiple (0x02C5), MagicPurgeEnchantments (0x02C6),
  MagicDispel (0x02C7/0x02C8), MagicPurgeBadEnchantments (0x0312). Each
  enchantment: spell id, layer, category, power level, start time,
  duration, caster guid, degrade values, stat mod type/key/value, optional
  spell set id.
* Spells of the same category do not stack; the more powerful overrides.
  Cantrips (item spells) stack with player spells. Life protections that
  stack multiply (65% and 15% give 72%).
* The UI shows beneficial and harmful spells with remaining time; item
  spells have no timer. Vitae is an enchantment too (spell id `Vitae`).
* Level VIII spells last 90 minutes.

## 2. Combat (melee and missile)

* Three stances: **peace** (dove), **melee/missile combat**, **magic**. The
  combat bar for weapons has a power (melee) or accuracy (missile) slider
  and three attack heights (high, medium, low); the same keys that cycle
  spell tabs and spells change these.
* Power bar: damage 50% at the bottom to 150% at the top; it also picks the
  animation (stab / backhand / slash, punch / jab / kick) and, with
  Recklessness trained, the 10% to 90% band gives +10/+20 damage rating at
  the cost of taking more damage. Missile: −40% attack skill fastest to
  +50% slowest; distance costs up to about 20% skill at maximum range.
* Attack height picks a body zone at random within the zone; front and
  back are distinct (shields protect the front, Sneak Attack rewards rear
  attacks). Dirty Fighting: high attacks weaken healing and attack
  skills, low attacks weaken defense, medium bleed.
* Hit chance compares attacker skill (+ weapon attack modifier, Heart
  Seeker) with the defender's Melee or Missile Defense. Critical hits:
  10% of melee/missile hits double the damage, 5% of spells add half the
  maximum.
* Damage: melee weapons roll between (max − variance) and max, times the
  power modifier, plus a hidden attribute bonus (Strength for heavy,
  light, two-handed and thrown; Coordination for finesse, bow and
  crossbow). Missile damage = ammunition damage × weapon modifier +
  weapon bonus. Elemental weapons and rends, vulnerabilities, slayers and
  creature weaknesses multiply it; ratings add percentages.
* Speed comes from the weapon speed and Quickness; a lower power setting
  attacks faster.
* Stamina: an attack costs stamina scaled by burden and power (roughly 1
  to 6 points), being hit or evading costs 1. At 0 stamina, defenses drop
  to 0 and the character cannot attack.
* Melee attacks charge toward the target first; too far and the attack
  aborts with "You have charged too far!".
* Wire: TargetedMeleeAttack (0x0008: target, height, power),
  TargetedMissileAttack (0x000A: target, height, accuracy), CancelAttack
  (0x01B7); results as AttackerNotification / DefenderNotification
  (0x01B1/0x01B2, damage, part hit, critical), Evasion notifications
  (0x01B3/0x01B4), AttackDone (0x01A7, always with error 0x36 on a normal
  end), VictimNotification / KillerNotification (0x01AC/0x01AD),
  UpdateHealth (0x01C0 for the selected target's health fraction).

## 3. Character advancement

* **Attributes**: Strength, Endurance, Coordination, Quickness, Focus,
  Self, raised at most 190 points above the creation value (max base 290
  with augmentations). Vitals: Health max = Endurance / 2, Stamina max =
  Endurance, Mana max = Self, each raisable 196 points on its own.
  Regeneration is faster crouching and fastest lying down.
* **Skills** are Unusable (0, cannot be used), Untrained (usable at the
  formula value but cannot be raised), Trained (raised up to 208 points;
  costs skill credits) or Specialized (226 points, +10 bonus, at most 70
  credits' worth). Every skill has a formula from attributes (table in the
  wiki `Skills` page, e.g. Melee Defense (Quickness + Coordination) / 3,
  Run = Quickness, Jump (Strength + Coordination) / 2, Heavy Weapons
  (Strength + Coordination) / 3).
* **Experience** goes to the total and to the **unassigned pool**; the
  player spends it on attributes, vitals and skills. Costs come from the
  XpTable DAT (0x0E000018): attribute, vital, trained-skill and
  specialized-skill cost lists indexed by points raised, the character
  level XP list and the skill credits per level. Level 2 costs 1,000 XP,
  level 10 83,511, level 50 about 55.9 M, level 275 is the cap. Skill
  credits come with levels (one at most levels below 10, then every few
  levels) and from two quests plus two luminance augmentations.
* Wire: RaiseVital (0x0044), RaiseAttribute (0x0045), RaiseSkill
  (0x0046), TrainSkill (0x0047: skill, credits); the server answers with
  private property updates (total XP, unassigned XP, level, skill credits,
  the attribute/skill records) and system chat ("You are now level N!").
* **Vitae**: each death adds 5% vitae penalty up to 40%, cutting health,
  stamina, mana and all skills by that percentage. It is an enchantment
  (spell `Vitae`, shown as a status icon). Earning XP recovers 1% at a
  time; no XP is lost. `DeathLevel` and `VitaeCpPool` properties track
  the recovery.
* **Luminance** is a second currency from level 50+ content; ratings
  (damage, damage resist, crit...) come from augmentations, aetheria,
  trinkets and masteries.

## 4. Death and corpses

* On death the character resurrects at the last **lifestone attuned**
  (use a lifestone to attune; it drains half the stamina). Death costs
  half the pyreals (none below level 6), a number of items (none below 11;
  1 for 11 to 20; level / 20 + 0..2 above; the highest-value items drop
  first, same-type items counted at half value after the first; bonded
  items never drop; rares always drop) and 5% vitae.
* The corpse holds the dropped items, is locked to the owner unless the
  player `@permit`s someone or is a PK, and decays after level × 5 minutes
  (at least 1 hour) while its landblock is loaded; then the items fall on
  the ground.
* `/lifestone` (`/ls`) recalls to the attuned lifestone alive (long
  animation: TeleToLifestone 0x0063); `/die` kills the character. Item
  Enchantment adds Lifestone Tie / Recall / Sending and Portal Tie /
  Recall / Summon.
* Player Killer status (red) allows attacking other PKs and makes corpses
  lootable; PK Lite (pink) is PvP without death penalties.

## 5. Inventory, items, burden

* The **main pack** holds 102 items; up to 7 side packs (24 items
  typically) plus one more with an augmentation. Foci take a whole slot.
* Equipment slots: head, chest, upper/lower arms, hands (gauntlets),
  girth (abdomen), upper/lower legs, feet; shirt, pants; necklace, two
  bracelets, two rings; cloak, trinket; weapon, shield or off-hand weapon
  (Dual Wield), ammunition; aetheria at 75/150/225.
* **Burden**: capacity = Strength × 150 units at 100%. Over 100% Run,
  Jump, Melee and Missile Defense drop 10% per 10% over; at 200% no
  jumping and walking drains stamina; 300% is the hard cap.
* Picking up: `R` on an object, `F` moves the selected item into the
  preferred pack (the last opened pack, or the main pack). Stacks merge
  automatically; splitting uses a slider or amount box on the selected
  target bar (StackableMerge 0x0054, StackableSplitToContainer 0x0055,
  StackableSplitTo3D 0x0056, StackableSplitToWield 0x019B).
* **Shortcut slots**: 9 numbered slots at the bottom of the inventory
  panel, one item each, used with the number keys (Ctrl+number while a
  spell bar is up). Persisted server side: AddShortCut (0x019C) /
  RemoveShortCut (0x019D); the list arrives in PlayerDescription under
  option flag `Shortcut` 0x1. Putting the main pack in a slot means "use
  on self", so kit-then-pack heals.
* Appraising (IdentifyObject 0x00C8 → IdentifyObjectResponse 0x00C9)
  shows value and burden, description, special properties, spells and
  spellcraft, mana and mana rate, activation and wield requirements,
  armor level and per-damage-type protections, weapon damage / speed /
  modifiers, and for creatures and players level, attributes, allegiance,
  titles and armor levels.
* Mana stones charge wielded items; healing kits heal (Healing skill,
  difficulty = missing health × 2, harder in combat, 1 stamina per 5
  health); potions and food restore vitals.
* Salvaging (an Ust opens the salvage panel; drag items in; Salvage
  destroys them into bags by material and workmanship) and Tinkering
  (apply salvage to items) are the crafting systems around loot.

## 6. Vendors, trade, pyreals

* Vendors: approach (Use) → ApproachVendor/VendorInfoEvent (0x0062) with
  the stock and the buy/sell rates; Buy (0x005F) and Sell (0x0060). ACE
  names: BuyPrice is what the vendor pays, SellPrice what it charges;
  values are rounded as in `docs/subsystems/net.md`. Trade notes are
  currency items; pyreals stack to 25,000.
* Secure trade between players: OpenTradeNegotiations (0x01F6) on the
  selected player, AddToTrade (0x01F8: item, slot), AcceptTrade (0x01FA),
  DeclineTrade (0x01FB), ResetTrade (0x0204), CloseTradeNegotiations
  (0x01F7); events RegisterTrade (0x01FD), OpenTrade, CloseTrade,
  AddToTrade, RemoveFromTrade, AcceptTrade, DeclineTrade, ResetTrade,
  TradeFailure, ClearTradeAcceptance (0x01FE..0x0208). Both sides must be
  in peace mode and within range; the panel shows both offers and
  accept states.
* Giving: GiveObjectRequest (0x00CD: target, item, amount) to NPCs or
  players.

## 7. Social systems

* **Fellowship**: up to 9 members; XP shared equally when all are within
  5 levels of the founder (or all 50+), proportionally within 10 levels,
  with a group bonus (2 members 75% each, 3 60%, 9 30%); optional loot
  sharing (members may loot each other's kills). Wire: FellowshipCreate
  (0x00A2: name, share xp), FellowshipQuit (0x00A3), FellowshipDismiss
  (0x00A4), FellowshipRecruit (0x00A5: guid), FellowshipUpdateRequest
  (0x00A6); events FellowshipFullUpdate (0x02BE), FellowshipUpdateFellow
  (0x02C0), FellowshipDisband (0x02BF), Quit/Dismiss (0x00A3/0x00A4),
  FellowshipFellowUpdateDone / FellowshipFellowStatsDone (0x01C9/0x01CA).
  The panel shows every member's vitals; members are green radar blips
  (leader triangle up).
* **Allegiance**: swear to a patron of equal or higher level
  (SwearAllegiance 0x001D, BreakAllegiance 0x001E, AllegianceUpdateRequest
  0x001F → AllegianceUpdate 0x0020); vassals pass XP up by Loyalty and the
  patron's Leadership. Officers, names, motd and chat are separate
  actions (0x0030..0x0042, 0x0254).
* **Chat**: local (`@say`, emotes), `@tell` (Tell 0x005D, events Tell
  0x02BD), fellowship and allegiance channels, global channels General,
  Trade, LFG, Roleplay, Society (`@cg`, `@ct`, `@clfg`; `/join` and
  `/leave`; ChatChannel 0x0147 and the Turbine chat messages), plus
  System, Combat, Magic and Advancement message types that the UI can
  route to separate windows. Squelch lists filter characters or accounts.
* **Radar** blip colours: white other players, yellow NPCs, orange
  attackable creatures, purple portals, blue lifestones, red PKs, pink PK
  Lite, green fellows, dimmed when above or below; hollow squares for
  allegiance members.

## 8. Travel

* **Portals** are used by walking into them (collision) or Use; the server
  sends PlayerTeleport (0xF751) then the new position, and the client
  shows portal space (a short tunnel effect, `Portal_Space`) before the
  new landblock. Some have level or quest restrictions ("You are not
  powerful enough to use this portal").
* **Lifestones** attune on Use (half stamina). `/lifestone` recalls.
  Portal magic (item school) ties one primary and one secondary portal
  and one lifestone, recalls to them and can summon a portal.
* Town Network, recall gems, housing recall (`/house`, allegiance
  mansion recall for monarch-owned villas and mansions) and "portal
  storms" (moved out of crowded areas; status icons 0x02C9..0x02CC).

## 9. Housing (brief)

Apartments (level 20, 100k pyreals + Writ of Refuge), cottages, villas
(level 35) and mansions (rank 6 monarch) with hooks for items and storage
chests; bought and rented at the covenant crystal (BuyHouse 0x021C,
RentHouse 0x0221, HouseQuery 0x021E, guest and storage permissions
0x0245..0x024D; events HouseProfile, HouseData, HouseStatus,
UpdateRentTime, UpdateRentPayment, HouseUpdateRestrictions 0x021D..0x0248).

## 10. Options and UI state the server keeps

PlayerDescription carries, under option flags: character options 1 and
2 (bitfields such as auto-repeat attacks, show tooltips, side-by-side
vitals, use main pack as preferred, PK settings), shortcuts, the 8 spell
bars, desired components, spellbook filters, gameplay options blob
(0x200) and the timestamp format. SetSingleCharacterOption (0x0005) and
SetCharacterOptions (0x01A1) write them back. Titles: TitleSet (0x002C),
CharacterTitle / UpdateTitle events. AFK mode and message (0x000F/0x0010).

## What this means for acreborn

* **Spell bar ≠ components.** The spell bar panel shows the 8 bars from
  `Options::spell_bars`, lets the user add spells from the book (drag or
  double-click → AddSpellFavorite) and remove them (Delete →
  RemoveSpellFavorite), cycles bars and spells, and casts the selected
  spell with the number keys. It never shows components.
* **Components panel** is a separate view: every carried item whose
  weenie class is a spell component (the SpellComponentDIDs mapping
  0x27000002 maps component id → weenie class id), with counts and the
  desired quantity, plus a "fill from vendor" action that buys up to the
  desired counts while a mage vendor is open.
* **Casting flow in the client**: need a wielded caster → enter magic
  mode → pick spell → send Cast(Targeted|Untargeted)Spell → play the
  wind-up and cast gestures on our own model → handle HearSpeech spell
  words, the fizzle WeenieError, the "consumed components" chat and the
  stack decrements, mana update, and enchantment registry updates for
  buffs. Component availability can be pre-checked client side from the
  SpellTable formula, the foci in the pack (prismatic formula) and the
  inventory, to grey out spells the character cannot cast.
* **Spellbook panel**: known spells from PlayerDescription plus
  MagicUpdateSpell/MagicRemoveSpell, filtered by school and level with the
  server-side filter bits, sorted by DisplayOrder; details from SpellTable;
  delete sends RemoveSpellC2S.
* **Enchantment (buff) list** from the registry with remaining durations.
* **Character sheet**: unassigned XP, per-skill/attribute raise costs from
  the XpTable, train/specialize with credits, vitae indicator.
* Everything above is state on `ac_client::Client` (or `ac_world`) with
  plain methods, so plugins and scripts can drive it; the panels are just
  views. UI layout is free: it does not need to look like the 2016
  client, only to expose these mechanics correctly.

## Sources

* Community wiki (asheron.fandom.com): Magic, Magic Panel, Category:Spell
  Components, Spell Research, Combat, Skills, Attributes, Experience
  Points, Death Penalty, Lifestone, Fellowship, Allegiance, Inventory
  Panel, Examine Target Panel, Selected Target Bar, Status Panels,
  Attributes/Skills/Titles Panel, Chat Interface, Radar, Burden, Healing,
  Player Killer, Summoning, Loot, Salvaging, Housing, User Interface.
  acpedia.org has the same material without ads but sits behind a browser
  challenge that scripted fetches cannot pass.
* ACE source (reference/ext/ACE, AGPL, read only): `Player_Magic.cs`,
  `Creature_Magic.cs`, `Entity/SpellFormula.cs`, `Entity/Spell.cs`,
  `SkillCheck.cs`, `Player_Character.cs` (spell bars), `Player_Skills.cs`,
  `Player_Xp.cs`, `Player_Death.cs`, `Player_Trade.cs`,
  `GameEventPlayerDescription.cs`, `GameActionType.cs`, `GameEventType.cs`,
  `DatLoader/FileTypes/{SpellTable,SpellComponentsTable,XpTable}.cs`.
