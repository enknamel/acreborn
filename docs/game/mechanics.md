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
  MagicUpdateSpell (0x02C1: u16 spell id, u16 layer) and MagicRemoveSpell
  (0x01A8: the same two u16s). A player may delete a spell from the book
  with RemoveSpellC2S (0x01A8: u32 spell id).
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
   taper, talisman. In the end-of-retail SpellTable 1,296 spells store
   only the five non-taper components (scarab, herb, powder, potion,
   talisman: no tapers at all); the other 4,970 store six to eight, and
   the tapers in slots 1, 3 and 6 are "personal": the client (and ACE's
   `SpellTable.GetSpellFormula`) replaces them with tapers picked by a
   hash of the account name, using the spell's `formula_version` (1, 2
   or 3) to choose the rotation (`Spell::player_formula`). The scarab
   sets the level: Lead I, Iron II, Copper III, Silver IV, Gold V, Pyreal
   VI, Platinum VII, Mana VIII (Diamond, level VI and power 7, and Dark,
   level VII and power 9, lead a few spells; component ids Lead 1, Iron
   2, Copper 3, Silver 4, Gold 5, Pyreal 6, Diamond 110, Platinum 112,
   Dark 192, Mana 193). Lead to Silver come from any mage shopkeeper,
   Gold to Platinum from Mastermages.
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
  TargetedMissileAttack (0x000A: target, height, accuracy; needs combat
  mode 4 Missile, a wielded launcher and ammunition in the ammo slot,
  otherwise the server answers with a weenie error; projectiles that
  meet geometry report "Your missile attack hit the environment";
  verified live), CancelAttack
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
* Raising on the wire: the client sends an XP *amount* (RaiseSkill
  0x0046: skill id, xp; RaiseAttribute 0x0045: attribute 1..6, xp;
  RaiseVital 0x0044: MaxHealth 1 / MaxStamina 3 / MaxMana 5, xp) and the
  server adds it to the stat's experience and recomputes the rank, so one
  rank costs `table[ranks + 1] - xp_spent`. TrainSkill (0x0047: skill,
  credits) trains a skill even when the sheet has no record for it yet.
  The server answers with the updated record (PrivateUpdateSkill,
  PrivateUpdateAttribute, PrivateUpdateVital), the new unassigned XP
  (PrivateUpdatePropertyInt64 AvailableExperience) and a chat line ("Your
  base Healing skill is now 38!"). Verified live on ACE.
* **Vitae**: each death adds 5% vitae penalty up to 40%, cutting health,
  stamina, mana and all skills by that percentage. It is an enchantment
  (spell `Vitae`, shown as a status icon). Earning XP recovers 1% at a
  time; no XP is lost. `DeathLevel` and `VitaeCpPool` properties track
  the recovery.
* **Luminance** is a second currency from level 50+ content; ratings
  (damage, damage resist, crit...) come from augmentations, aetheria,
  trinkets and masteries.

### Character creation and selection

Sources: ACE `ACE.Server/Factories/PlayerFactory.cs` (`Create`,
`ValidateAttributeCredits`, the skill loop, starter gear and spells,
masteries and innate augmentations), `WorldObjects/Player_Skills.cs`
(`TrainSkill`, `SpecializeSkill`, `UntrainSkill`, `AlwaysTrained`,
`AugSpecSkills`), `ACE.Entity/CharacterCreateInfo.cs` (wire layout),
`Network/Handlers/CharacterHandler.cs` (create, enter, delete and restore
handlers and their response codes),
`Network/Enum/CharacterGenerationVerificationResponse.cs`,
`Network/Enum/CharacterError.cs`, `ACE.Server/starterGear.json`, and the
CharGen (0x0E000002) and SkillTable (0x0E000004) DATs
(`ACE.DatLoader/FileTypes/{CharGen,SkillTable}.cs`).

**Choices.** A heritage (ACE `HeritageGroup`: 1 Aluvian, 2 Gharu'ndim, 3
Sho, 4 Viamontian, 5 Shadowbound ("Umbraen" in the DAT), 6 Gearknight, 7
Tumerok, 8 Lugian, 9 Empyrean, 10 Penumbraen, 11 Undead, 12 Olthoi, 13
Olthoi Acid), a sex (1 male, 2 female), an appearance (hair style, hair
colour and shade, eyes, eye colour, nose, mouth, skin shade; headgear,
shirt, pants and footwear style, colour and hue, headgear style
0xFFFFFFFF meaning none), a template, six attributes, an advancement
class for each of 55 skill slots, and a starting town.

**Attributes.** Strength, Endurance, Coordination, Quickness, Focus and
Self are each 10..=100 and their sum may not exceed the heritage's
attribute credits: 330 for every playable heritage (Olthoi 60). A 6 x 100
build is refused; 100/100/100/10/10/10 is exactly the budget, as are the
built-in templates. Vitals follow the attributes (above).

**Skills.** The SkillTable lists 38 skills; the message carries 55 slots
(slot = skill id). Slots the table lacks (0..=5, 8..=13, 17, 25, 26, 42,
53: the retired weapon skills and never-implemented ones) stay 0. Each
slot is an ACE `SkillAdvancementClass`: 0 inactive (the server leaves the
skill as the player weenie has it), 1 untrained (free; usable at the
formula value only when the table's `min_level` is 1), 2 trained, 3
specialized. The budget is 52 skill credits (Olthoi 68). Training costs
the table's `trained_cost`; specializing costs `specialized_cost -
trained_cost` more (the table's second number is the total: Melee Defense
10/20 is "train 10, specialize 10 more"; ACE
`SkillBase.UpgradeCostFromTrainedToSpecialized`). The heritage's CharGen
`skills` list replaces both numbers (`normal_cost` to train,
`primary_cost` extra to specialize); at end of retail the only override
is Arcane Lore 0/2 for every heritage. Costs (train / specialize extra):

| skill | id | cost | skill | id | cost |
|---|---|---|---|---|---|
| Melee Defense | 6 | 10 / 10 | Missile Defense | 7 | 6 / 4 |
| Arcane Lore | 14 | 0 / 2 (override; table 4 / 2) | Magic Defense | 15 | 0 / 12 |
| Mana Conversion | 16 | 6 / 6 | Item Tinkering | 18 | 2 / no |
| Assess Person | 19 | 2 / 2 | Deception | 20 | 4 / 2 |
| Healing | 21 | 6 / 4 | Jump | 22 | 0 / 4 |
| Lockpick | 23 | 6 / 4 | Run | 24 | 0 / 4 |
| Assess Creature | 27 | 4 / 2 | Weapon Tinkering | 28 | 4 / no |
| Armor Tinkering | 29 | 4 / no | Magic Item Tinkering | 30 | 4 / no |
| Creature Enchantment | 31 | 8 / 8 | Item Enchantment | 32 | 8 / 8 |
| Life Magic | 33 | 12 / 8 | War Magic | 34 | 16 / 12 |
| Leadership | 35 | 4 / 2 | Loyalty | 36 | 0 / 2 |
| Fletching | 37 | 4 / 4 | Alchemy | 38 | 6 / 6 |
| Cooking | 39 | 4 / 4 | Salvaging | 40 | 0 / no |
| Two Handed Combat | 41 | 8 / 8 | Void Magic | 43 | 16 / 12 |
| Heavy Weapons | 44 | 6 / 6 | Light Weapons | 45 | 4 / 4 |
| Finesse Weapons | 46 | 4 / 4 | Missile Weapons | 47 | 6 / 6 |
| Shield | 48 | 2 / 2 | Dual Wield | 49 | 2 / 2 |
| Recklessness | 50 | 4 / 2 | Sneak Attack | 51 | 4 / 2 |
| Dirty Fighting | 52 | 2 / 2 | Summoning | 54 | 8 / 4 |

* **Always trained** (ACE `AlwaysTrained`; `trained_cost` 0 in the table):
  Arcane Lore, Magic Defense, Jump, Run, Loyalty, Salvaging. They are sent
  Trained (2) at no cost and can still be specialized at their extra cost
  (Salvaging excepted). ACE never adds them at login, so a client that
  sent them untrained would get an untrained Run.
* **Cannot be specialized at creation** ("no" above: an extra of 999,
  ACE `AugSpecSkills`): Salvaging and the four tinkering skills; those are
  specialized later by augmentation.
* **Need training to be used** (`min_level` 2): Mana Conversion, Healing,
  Lockpick, the five magic schools, Fletching, Alchemy, Cooking,
  Recklessness, Sneak Attack, Dirty Fighting, Summoning. The rest work
  untrained at their formula value.
* ACE checks skills one by one in id order: `TrainSkill` refuses a cost
  above the credits left (and a skill already Trained), `SpecializeSkill`
  refuses a skill not Trained; any refusal, or an attribute out of
  bounds, answers Corrupt (5). A slot count other than 55 boots the
  session ("your client is not the correct version"). Skills trained at
  creation start with a bonus of 5 ranks (526 xp); specialized ones start
  at 0 ranks with the +10 specialization bonus.

**Templates** (CharGen, per heritage; the index goes on the wire and
names the starting title). The same seven for all eleven playable
heritages, each spending exactly 330 points and 52 credits:

| template | Str/End/Coo/Qui/Foc/Self | trained | specialized |
|---|---|---|---|
| Adventurer | 10/10/10/10/10/10 | (the blank sheet) | |
| Bow Hunter | 40/30/100/100/50/10 | Item Enchantment, Shield | Missile Weapons, Finesse Weapons, Arcane Lore, Melee Defense |
| Swashbuckler | 100/40/100/50/20/20 | Item Enchantment, Healing | Arcane Lore, Melee Defense, Heavy Weapons, Dual Wield |
| Life Caster | 40/50/10/30/100/100 | War Magic, Creature Enchantment, Mana Conversion | Arcane Lore, Life Magic |
| War Mage | 50/40/10/30/100/100 | Life Magic | Mana Conversion, War Magic |
| Wayfarer | 30/30/100/100/60/10 | Melee Defense, Lockpick, Healing, Missile Weapons, Item Enchantment | Finesse Weapons, Dirty Fighting, Dual Wield |
| Soldier | 100/60/100/50/10/10 | Missile Weapons, Healing | Heavy Weapons, Shield, Melee Defense, Dirty Fighting |

Olthoi has one template ("Ripper"), Olthoi Acid one ("Adventurer"), both
all 10s with no skills.

**Starting towns.** CharGen `starter_areas`: 0 Holtburg, 1 Shoushi, 2
Yaraq, 3 Sanamar, 4 OlthoiLair. Each heritage lists its home
(`primary_start_areas`) and the other three it may pick: Aluvian,
Shadowbound, Tumerok, Lugian, Empyrean, Penumbraen and Undead are from
Holtburg; Gharu'ndim and Gearknight from Yaraq; Sho from Shoushi;
Viamontian from Sanamar; the Olthoi only from their lair. ACE puts the
new character at the area's first location (the training academy), sets
the lifestone (`Sanctuary`) there, disables recalls until the academy is
left, and sets the `Instantiation` fallback to the town's "Free Ride"
spell destination (3815 Holtburg, 3813 Shoushi, 3814 Yaraq, 3535
Sanamar).

**Starter gear and spells** (ACE `starterGear.json`, granted for every
skill that is trained or specialized; heritage entries add to the common
list). Through the always-trained Jump everyone gets 10,000 pyreals (one
stack), a Sack, a Calling Stone, a Pathwarden Token, Bread, Ust and a
heritage "Letter From Home" (Gearknights also a Core Plating Integrator
and Deintegrator). Healing: Handy Healing Kit. Lockpick: Crude Lockpick.
Fletching: three bundles of arrowheads and one each of arrowshafts,
atlatl dart shafts and quarrelshafts (30 each). Alchemy: Mortar and
Pestle, three Azurite. Cooking: 6 Flour, 6 Water. Two Handed Combat:
Training Spadone. Shield: Round Shield. Summoning: Mud Golem Essence.
Heavy / Light / Finesse Weapons: one heritage training weapon each (e.g.
Aluvian Dirk / Dagger / Knife, Sho Cestus / Knuckles / Handwraps,
Gharu'ndim Stick / Staff / Bastone, Viamontian Ken / Broad Sword / Short
Sword). Missile Weapons: a Training Shortbow and 250 arrows (Aluvian,
Sho, Empyrean), Training Atlatl and 250 darts (Gharu'ndim, Tumerok,
Lugian, Undead) or Light Training Crossbow and 250 quarrels (Viamontian,
Shadowbound, Gearknight, Penumbraen). A trained Dual Wield doubles every
melee weapon granted. Each magic school trained gives a Training Wand,
the school's Foci (Enchantment, Artifice, Verdancy, Strife, Shadow), 5
Lead Scarabs, 25 Prismatic Tapers and its level I spells: trained gets
the common ones, specialized also the `specializedOnly` ones. Creature
Enchantment: Focus Self, Invulnerability Self/Other (+ Mana Conversion
Mastery Self, Willpower Self). Item Enchantment: Aura of Blood Drinker
Self, Bludgeon Bane, Aura of Swift Killer Self, Impenetrability (+ Aura
of Defender Self, Blade Bane). Life Magic: Armor Self/Other, Heal
Self/Other, Imperil Other (+ Drain Health Other, Harm Other). War Magic:
Flame, Force and Frost Bolt, Shock Wave (+ Acid Stream, Lightning Bolt,
Whirling Blade). Void Magic: Destructive Curse, Corrosion, Corruption,
Nether Bolt (+ Weakening Curse, Nether Streak, Nether Arc, Festering
Curse). Every heritage also gets a melee and a ranged weapon mastery
(Aluvian dagger/bow, Gharu'ndim staff/magic, Sho unarmed/bow, Viamontian
sword/crossbow, ...) and one innate augmentation (Jack of All Trades for
the four original heritages).

**Olthoi.** For heritages 12 and 13 (ACE `IsOlthoiPlayer`) the server
skips clothing, template, attributes, skills, starter gear and spells
entirely: the character is the `olthoiplayer` / `olthoiacidplayer`
weenie with its own stats, spawns in the lair with no Free Ride, gets no
lifestone and no recall lock. With the server's `olthoi_play_disabled`
setting a create answers Pending (2) and an enter answers CharacterError
0x14.

**Names.** ACE checks only the taboo table (NameBanned, 4), the creature
name list when `creature_name_check` is on (also 4) and uniqueness
(NameInUse, 3); it has no length or character rule. The retail client
allowed letters, spaces, hyphens and apostrophes; acreborn's
`creation::valid_name` requires 3..=32 of those with single separators
between letters. Lists show a "+" before the names of admin accounts.

**Wire.** All on the UI queue (9), before the world is entered:

* CharacterCreate 0xF656: account string16, u32 1, heritage, gender; the
  appearance as 14 u32 (eyes, nose, mouth, hair colour, eye colour, hair
  style, headgear style, headgear colour, shirt style, shirt colour, pants
  style, pants colour, footwear style, footwear colour) then 6 f64 hues
  (skin, hair, headgear, shirt, pants, footwear); i32 template; 6 u32
  attributes (Str, End, Coo, Qui, Foc, Self); u32 slot; u32 class id 0;
  u32 55 and 55 u32 advancement classes; name string16; u32 start area;
  u32 isAdmin; u32 isSentinel.
* CharacterCreateResponse 0xF643: u32 code, and on Ok the guid, the name
  string16 and a u32 0. Codes (ACE `CharacterGenerationVerificationResponse`):
  1 Ok, 2 Pending, 3 NameInUse, 4 NameBanned, 5 Corrupt, 6 DatabaseDown, 7
  AdminPrivilegeDenied. After Ok the client enters the world the normal way
  (0xF7C8 CharacterEnterWorldRequest, 0xF7DF ServerReady, 0xF657
  CharacterEnterWorld with guid and account).
* CharacterList 0xF658: u32 0, count, per character guid, name string16
  and u32 seconds until deleted (0, or >0 while a deletion is pending),
  u32 0, u32 slot count (`max_chars_per_account`, 11), account string16,
  u32 use Turbine chat, u32 has Throne of Destiny (1). Sent after login
  and again after a delete.
* CharacterDelete 0xF655: account string16, u32 slot (index in the last
  list). The server echoes an empty 0xF655 and a fresh CharacterList; the
  character is kept for `char_delete_time` (3600 s by default) and then
  purged. Failures are CharacterError 6 (Delete) or 0x15 (world closed).
* CharacterRestore 0xF7D9: u32 guid. Answered by a 0xF643 with (1, guid,
  name, u32 seconds greyed out), or 3 NameInUse if the name was taken
  meanwhile, 5 Corrupt if the save failed, or CharacterError 0xF when the
  deletion already went through.
* CharacterError 0xF659: u32 code (ACE `CharacterError`): 1 Logon, 3
  AccountLogin, 4 and 8 ServerCrash, 5 Logoff, 6 Delete, 9
  AccountInvalid, 0xA AccountDoesntExist, 0xB EnterGameGeneric, 0xC
  StressAccount, 0xD CharacterInWorld, 0xE PlayerAccountMissing, 0xF
  CharacterNotOwned (also for a character pending deletion), 0x10
  CharacterInWorldServer, 0x11 OldCharacter (back to the select screen),
  0x12 CorruptCharacter, 0x13 StartServerDown, 0x14
  CouldntPlaceCharacter, 0x15 LogonServerFull (world closed, or shutting
  down), 0x17 CharacterLocked, 0x18 SubscriptionExpired.

In acreborn: `ac_client::creation` (`rules`, `Rules`, `CharacterBuild`,
`valid_name`, `create_failure_message`), `Client::{create_character,
enter_world, delete_character, restore_character}`, the events
`Characters`, `CharacterCreated` and `CharacterCreateFailed`, and
`Config::auto_enter` (off, with no character named, the client shows the
list instead of entering). Headless: `acclient --create NAME` and `acbot
--create NAME` with `--heritage`, `--gender`, `--template`,
`--start-area`; `--show-rules` prints a heritage's credits and costs.

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
  the ground. ACE destroys the dropped pyreals outright by default
  (`corpse_destroy_pyreals`), so the corpse only holds the items.
* On the wire a death is VictimNotification 0x01AC ("You killed
  yourself", "Drudge killed you"), the vitae as MagicUpdateEnchantment
  (spell 666, `stat_mod_value` 0.95 for 95%), then MagicPurgeEnchantments
  and, after the death animation, the "You've lost 223 Pyreals, and your
  Iron Scarab!" system line and the teleport to the lifestone. The purge
  keeps the vitae: the client holds it in its own registry slot.
* Verified on ACE with acbot: suicide, corpse listed and opened at the
  death spot, item taken back, vitae shown, lifestone attune ("You have
  attuned your spirit to this Lifestone...").
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
* **Jumping** is charged: holding the jump key fills a bar (a second to
  full) and releasing it leaps with that power. The client computes the
  launch velocity itself and sends Jump 0xF61B (power, local velocity,
  sequences); height = burden mod × (Jump skill / (skill + 1300) × 22.2
  + 0.05) × power, at least 0.35 m, and the horizontal speed at take-off
  is kept in the air. Stamina cost = ceil((burden + 0.5) × power × 8 +
  2) (PK: (power + 1) × 100); the server deducts it and would send the
  character falling anyway, so the client caps the power at
  (stamina − 2) / (burden × 8 + 4) and refuses below 2 stamina.
* Moving items: DropItem (0x001B: item) puts a carried item on the
  ground in front of the character (the server answers with
  InventoryPutObjectIn3D and creates the object); PutItemInContainer
  (0x0019: item, container, placement) moves it into the main pack (the
  player's own guid), a carried side pack, or a chest the player is
  looking into (a closed or locked chest answers WeenieError 0x03EE "The
  container is closed"; corpses refuse); GiveObjectRequest (0x00CD:
  target, item, amount) hands it to an NPC (which answers with an emote
  or refuses with InventoryServerSaveFailed) or a player ("You give X 50
  Iron Scarabs" / "X gives you 50 Iron Scarabs"; the receiver must allow
  gifts in their character options and be within reach).
* Picking up: `R` on an object, `F` moves the selected item into the
  preferred pack (the last opened pack, or the main pack). Stacks:
  StackableMerge 0x0054 (from, to, amount) moves part or all of one
  stack into another of the same weenie (capped at the target's maximum
  stack size, the WeenieDesc's MaxStackSize; the rest stays),
  StackableSplitToContainer 0x0055 (stack, container, placement, amount)
  makes a new stack in a pack, StackableSplitTo3D 0x0056 (stack, amount)
  drops part of one, StackableSplitToWield 0x019B (stack, EquipMask,
  amount) wields part (ammo). The server answers with the new object and
  SetStackSize / InventoryServerSaveFailed ("Split amount not valid!").
  In the client: right-click a stack in the pack for a split slider,
  drag a stack onto another of its kind to merge; scripts `split(guid,
  n)` and `merge(from, to)`.
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
* **Appraisal** (IdentifyObject 0x00C8 on selecting; IdentifyObjectResponse
  0x00C9): a flags word, success, then by flag the int, int64, bool,
  float, string and data-id property tables (PackableHashTables of id,
  value), the spell id list, the armor profile (8 f32 multipliers of the
  armor level per damage type), the creature profile (flags, health and
  max; with flag 8 the six attributes and stamina, mana and their maxima;
  with flag 1 the buff/debuff marks), the weapon profile (damage type
  bits, speed, skill, damage, variance, damage mod, length, max
  velocity, offense multiplier, estimated range), the hook profile, the
  enchantment highlight/colour masks for armor, weapon and resistances,
  and a creature's armor by location. The client keeps every appraisal
  by guid and shows the latest in the appraisal window; scripts read it
  with `appraisal(guid)`. Verified live on a healing kit, a lifestone
  (its Use text) and a barkeeper (level, health).
* Mana stones charge wielded items; healing kits heal (Healing skill,
  difficulty = missing health × 2, harder in combat, 1 stamina per 5
  health); potions and food restore vitals.
* **Salvaging**: using the Ust (wcid 20646) opens the salvage window
  client-side; the server only hears CreateTinkeringTool 0x027D (tool
  guid, count, item guids). It skips items without a material or a
  workmanship (vendor stock) and retained ones, destroys the rest and
  merges them into salvage bags per material ("Salvaged Oak", one bag
  per 100 units; `Structure` counts the units, `ItemWorkmanship` their
  average), then answers SalvageOperationsResult 0x02B4 per skill used:
  skill id, guids it could not salvage, (material, workmanship f64,
  units) per material, augmentation bonus percent. Units per item are
  1 + floor(skill / 194 × workmanship), with the Salvaging skill or the
  best trained tinkering skill, whichever yields more (tinkering skills
  are capped at the workmanship). The item's material and workmanship
  travel in the WeenieDesc (flags 0x80000000 and 0x1000000), so the
  client knows what is salvageable without appraising.
* **Tinkering**: UseWithTarget 0x0035 with a salvage bag on an item. The
  server finds the recipe (material × item kind), computes the chance
  from the tinkering skill against the difficulty, and with the
  "crafting chance dialog" option on asks a kind 5 confirmation ("You
  determine that you have a 38 percent chance to succeed."); yes applies
  it: success raises the item's tinkered property and its tinker count
  (10 max), failure destroys the bag. Untrained skills answer "You are not
  trained in Weapon Tinkering." A bag must be full (100 units, its
  `Structure` equal to `MaxStructure`) or the recipe requirement answers
  "The material is not complete!". Salvage bags are also the ingredient
  of other recipes (keys, tokens) through the same use-on path. The
  server sends "Salvage (n)" as the bag's name; the client shows
  "Salvaged Iron (n)" from the material and structure, as retail did.
* Verified on ACE: three Rusted Maces salvaged with an Ust into
  "Salvaged Iron (6)" ("You obtain 6 Iron (workmanship 3.00) using your
  Weapon Tinkering skill."), the salvaged items removed by
  InventoryRemoveObject 0x0024 (a top-level message the client now
  handles; spent, given and corpse-dropped items use it, not
  DeleteObject); a full bag on a mace asked "You determine that you have
  a 21 percent chance to succeed." and on yes "+Admin fails to apply the
  Iron Salvage (workmanship 3.00) to the Iron Rusted Mace. The target is
  destroyed." Items the server cannot add to the pack (burden over
  three times 150 × Strength) are silently not created by `@ci`.

## 6. Vendors, trade, pyreals

* Vendors: approach (Use) → ApproachVendor/VendorInfoEvent (0x0062) with
  the stock and the buy/sell rates; Buy (0x005F) and Sell (0x0060). ACE
  names: BuyPrice is what the vendor pays, SellPrice what it charges;
  values are rounded as in `docs/subsystems/net.md`. Trade notes are
  currency items; pyreals stack to 25,000.
* Secure trade between players (verified live): double-click a player
  (the retail client sent OpenTradeNegotiations 0x01F6 itself; the server
  ignores Use on players). Both must be in peace mode and close by.
  RegisterTrade (0x01FD: initiator, partner, i64 stamp) opens the window
  for both; AddToTrade (0x01F8: item, slot) puts an item in, echoed as
  AddToTrade (0x0200: item, side 1 = own / 2 = partner, slot) to both;
  RemoveFromTrade (0x0201); AcceptTrade (0x01FA: partner, stamp as f64,
  status, initiator, initiator accepts, partner accepts; the server only
  needs it to arrive) answered by AcceptTrade (0x0202: who) plus "You
  have accepted the offer" / "X has accepted the offer"; when both have
  accepted the server swaps the items ("The items are being traded"),
  resets the window (ResetTrade 0x0205) and keeps it open. DeclineTrade
  (0x01FB / 0x0203), ResetTrade (0x0204 / 0x0205), CloseTradeNegotiations
  (0x01F7) / CloseTrade (0x01FF: reason 1 normal, 2 entered combat, 0x51
  cancelled), TradeFailure (0x0207: item, WeenieError, e.g. attuned),
  ClearTradeAcceptance (0x0208) after any change.
* Giving: GiveObjectRequest (0x00CD: target, item, amount) to NPCs or
  players.

## 7. Social systems

* **Fellowship** (verified live): up to 9 members; XP shared equally
  when all are within 5 levels of the founder (or all 50+),
  proportionally within 10 levels, with a group bonus (2 members 75%
  each, 3 60%, 9 30%); optional loot sharing. FellowshipCreate (0x00A2:
  name, share xp) answers with FellowshipFullUpdate (0x02BE: hash-table
  header u16 count/u16 buckets, then per fellow guid, cp, luminance,
  level, max health/stamina/mana, current health/stamina/mana, share
  loot, name; then name, leader, share xp, even share, open, locked,
  departed members, locks). FellowshipRecruit (0x00A5: guid) sends the
  target a ConfirmationRequest (0x0274: type 4 Fellowship, context,
  text) unless their character options ignore fellowship requests (the
  retail default; "X is not accepting fellowship requests");
  ConfirmationResponse (0x0275: type, context, yes) joins, and everyone
  gets FellowshipUpdateFellow (0x02C0: fellow record + update type).
  FellowshipQuit (0x00A3: disband) / FellowshipDismiss (0x00A4: guid)
  echo as events with the guid; FellowshipDisband (0x02BF) ends it.
  Members are green radar blips (leader triangle up).
* **Allegiance**: a tree of patrons and vassals under a monarch. Swear
  by selecting a player and sending SwearAllegiance 0x001D (their guid):
  the server walks you within 2 m, asks the patron with a kind 1
  confirmation whose text is just your name, and on yes tells both sides
  in chat ("Reborn has sworn Allegiance to you." / "+Admin has accepted
  your oath of Allegiance!"), plays the kneel motion and sends both an
  AllegianceUpdate 0x0020 then AllegianceUpdateDone 0x01C8. Refused when
  you already have a patron ("You've already sworn allegiance."), the
  patron ignores allegiance requests (character option 1), has 11
  vassals, is banned/locked, or is your own vassal. BreakAllegiance 0x001E
  (the patron's or a vassal's guid) drops the link from either side
  ("You have broken your Allegiance to X!" / "X has broken their
  Allegiance to you!").
* The profile is only sent on request (AllegianceUpdateRequest 0x001F,
  the client sends one once placed and when the panel opens): our rank,
  member and vassal counts, then the hierarchy (officers hash table,
  officer titles, broadcast counters, motd and who set it, chat room id,
  bind point, name, lock, approved vassal) and member records for the
  monarch, the patron if not the monarch, ourselves if not the monarch,
  and our direct vassals, each with rank, level, loyalty, leadership,
  online flag, XP generated/cached and a "may pass up" flag. The rest of
  the tree is never sent. AllegianceInfoRequest 0x027B (a name; officers
  only) answers with AllegianceInfoResponse 0x027C for that member;
  AllegianceLoginNotification 0x027A flips a member's online flag.
* **XP passup**: a vassal's earned XP (kills, quests; not admin grants)
  generates 50%+ for the patron by Loyalty and the patron receives by
  Leadership, but only when the patron's level was at least the vassal's
  when the oath was sworn (`ExistedBeforeAllegianceXpChanges`; the record
  shows it as the "no passup" flag). Rank is 1 + the vassal count ladder,
  capped at 10.
* Names and motd: SetAllegianceName 0x0033 / ClearAllegianceName 0x0031
  (monarch: "Your allegiance name has been set.", no profile resent, so
  the client asks for one), SetMotd 0x0254 / ClearMotd 0x0256 (officers).
  Group chat is ChatChannel 0x0147 (channel id, text): Vassals 0x1000,
  Patron 0x2000, Monarch 0x4000, CoVassals 0x01000000, Fellow 0x800, which
  everyone on the channel, sender included, gets back as ChannelBroadcast
  0x0147 (channel, sender or "" for yourself, text); `/v`, `/p`, `/m`,
  `/c`, `/f` in the chat box.
* **Emotes**: `/e text` is a free emote (Emote 0x01DF, shown to everyone
  near as "Name text" via EmoteText 0x01E0). Soul emotes are the animated
  ones: typing `*wave*` (retail) or `/wave` looks the word up in the
  emote table, plays the motion locally, sends it in the next MoveToState
  as a one-shot command (the command list length sits above the 11 flag
  bits; each item is the raw command (MotionCommand & 0xFFFF), a packed
  sequence with the autonomous bit, and speed 1.0, the only speed ACE
  accepts, and only for the motions in its `SoulEmote` list), and says
  the line with SoulEmote 0x01E1 (text), which comes back to everyone
  near as message 0x01E2 (guid, name, text) shown "Name waves". The
  server relays the command in the UpdateMotion others get, so they play
  the animation. The client's table (`ac_client::emotes`) holds about
  seventy words; verified with two sessions (wave, bow deep).
* **Friends, titles, squelch**: AddFriend 0x0018 (name) / RemoveFriend
  0x0017 (guid) / RemoveAllFriends 0x0025 answer with FriendsListUpdate
  0x0021 (count, records of guid, online flag, appear-offline flag,
  name, two empty guid lists; then the kind: 0 full list at login, 1
  added, 2 removed, 4 online status changed). Titles: CharacterTitle
  0x0029 at login (1, shown id, count, ids); TitleSet 0x002C (id) picks
  one and UpdateTitle 0x002B (id, shown flag) confirms or grants a new
  one ("You have been granted a new title."); names come from the
  portal EnumMapper 0x22000041. Squelch: ModifyCharacterSquelch 0x0058
  (flag, guid, name), ModifyAccountSquelch 0x0059 (flag, name),
  ModifyGlobalSquelch 0x005B (flag, ChatMessageType); the server drops
  the squelched player's lines before they reach us and re-sends
  SetSquelchDB 0x01F4 (account table, character table of guid to
  filter masks, name, account flag; global mask).
* **Turbine chat** (message 0xF7DE, not a game event) carries the rooms:
  General 2, Trade 3, LFG 4, Roleplay 5, the society rooms 6..9, Olthoi
  10, and each allegiance's own room whose id is the allegiance's biota
  id (SetTurbineChatChannels 0x0295 lists the ten room ids at login and
  after an oath; the "You have entered the General channel" lines are
  WeenieErrorWithString). A message is a net blob: size, blob type (1 a
  line from a room, 3 our request, 5 the server's ack), dispatch type,
  (1, id, 1, id, 0), payload size, then for a room line the room id, the
  sender and the text as counted UTF-16 strings, 0x0C, sender guid, 0,
  chat type; for our request the context id, 2, 2, room id, text, 0x0C,
  our guid, 0, chat type. The server relays a line to everyone whose
  "listen to" option for that room is on, the sender included, and
  answers the request with an ack blob. `/g`, `/trade`, `/lfg`, `/rp`
  and `/a` in the chat box; lines show as "[General] Name: text".
  Verified with two sessions on ACE in all three public rooms.
* Verified on ACE with two acbot sessions: swear + confirmation, both
  profiles, naming, `/p` and `/v` chat both ways, break from the vassal.
* **Commands**: the server (ACE `GameActionTalk`) treats only Talk lines
  starting with `@` as commands (`@acehelp`, `@myquests`, admin commands;
  unknown ones answer "Unknown command: X"). Retail's `/` commands were
  the client's own and map to game actions: `/lifestone` (`/ls`)
  TeleToLifestone 0x0063 (refused with WeenieError 0x055D while
  `RecallsDisabled` is set: from character creation until the Training
  Academy's exit portal is used), `/die` Suicide 0x0279, `/house` TeleToHouse
  0x0262, `/mansion` TeleToMansion 0x0278, `/hometown`
  RecallAllegianceHometown 0x02AB, `/marketplace` 0x028D, `/pklite`
  EnterPkLite 0x028F, `/afk [message]` SetAfkMessage 0x0010 + SetAfkMode
  0x000F, `/tell Name, text` Tell 0x005D (text, name), `/emote text`
  Emote 0x01DF. acreborn's chat box tries plugin commands first, then
  these (`Client::slash_command`), then sends the rest as `@command`.
* **Character options**: two bitfields in PlayerDescription
  (CharacterOptions1/2), changed with SetSingleCharacterOption (0x0005:
  option id from ACE `CharacterOption`, value); "ignore fellowship
  requests", "auto-repeat attacks", "let others give you items" and the
  chat channels among them. New characters ignore fellowship requests by
  default.
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
* **Dungeons have no outside.** Every collision triangle of a dungeon
  block carries its cell id, including the objects placed in cells and
  the block-level ones (portals, grates), and while the character stands
  in a dungeon cell an untagged floor keeps the current cell and the
  terrain never catches a fall. Before this, standing on a door sill or a
  staircase (untagged geometry) re-homed the character to the outdoor
  cell under the dungeon and ACE refused every move afterwards
  ("movement pre-validation failed from 012F010C ... to 012E0009").
* **Landing and headroom.** A falling character lands on any floor up
  to a step (0.6 m) above its feet, so a jump that arrives a few
  centimetres under a ledge's top stands on the ledge instead of passing
  through the slab; and while airborne the capsule may only drift to
  spots with full headroom, so it cannot slide under a porch or a
  staircase on the way down. Both came from a jump at the Shoushi
  tailor's porch that ended in the one-metre gap beneath it, blocked in
  every direction; `cargo run -p ac-client --example jump_probe` scans
  jumps from a spot, and `tests/jump.rs` keeps that porch honest.
* **Map coordinates**: the game's "42.1N, 33.6E" is the world position
  (landblock × 192 m + local) divided by 240 minus 102 on both axes,
  north and east positive, a twentieth taken off before rounding to a
  tenth; indoor cells (0x100 and up) have none. The status line shows
  them; scripts read `me().coords`.
* **Lifestones** attune on Use (half stamina). `/lifestone` recalls.
  Portal magic (item school) ties one primary and one secondary portal
  and one lifestone, recalls to them and can summon a portal.
* Town Network, recall gems, housing recall (`/house`, allegiance
  mansion recall for monarch-owned villas and mansions) and "portal
  storms" (moved out of crowded areas; status icons 0x02C9..0x02CC).

## 9. Housing

* Apartments (level 20; 100,000 pyreals + a Writ of Refuge, maintenance
  10,000 pyreals a period), cottages (level 20; the cheap ones 300,000
  pyreals + Writ of Refuge + Iron Heart, maintenance 30,000), villas
  (level 35) and mansions (rank 6 monarch), with hooks for items and
  storage chests. The sign in front (ACE `SlumLord`, named after the
  house type) is what you use: Use on it answers HouseProfile 0x021D
  (slumlord guid; dwelling id, owner guid and name, bitmask 1 active /
  2 requires monarch, min/max level and allegiance rank (-1 none),
  maintenance-free flag, type 1 cottage 2 villa 3 mansion 4 apartment,
  then the buy list and the rent list of (needed, paid, wcid, name,
  plural)).
* BuyHouse 0x021C (slumlord guid, count, item guids) pays with the
  listed stacks (larger stacks are split, the rest stays); the server
  checks level, monarch/rank, the 15-day account age (apartments exempt)
  and the 30-day cooldown since the last purchase, then says
  "Congratulations!  You now own this dwelling." and sends HouseData
  0x0225 (buy and rent timestamps, type, maintenance-free, buy and rent
  lists, position). HouseQuery 0x021E asks for it again; without a house
  the answer is HouseStatus 0x0226. RentHouse 0x0221 (same shape as buy)
  pays maintenance toward the outstanding amounts, from anyone, at a
  sign in the same landblock; UpdateRentTime 0x0227 / UpdateRentPayment
  0x0228 follow. AbandonHouse 0x021F ("You abandon your house!") boots
  everyone and answers HouseStatus.
* Guests: AddPermanentGuest 0x0245 / RemovePermanentGuest 0x0246 (name),
  ChangeStoragePermission 0x0249 (name, flag), SetOpenHouseStatus 0x0247
  (flag), ModifyAllegianceGuestPermission 0x0267 / StoragePermission
  0x0268 (flag), BootSpecificHouseGuest 0x024A (name), BootEveryone
  0x025F, RemoveAllPermanentGuests 0x025E, RemoveAllStoragePermission
  0x024C; the server answers each in chat ("Reborn has been added to
  your guest list.", "Your house is now open to the public.") and
  RequestFullGuestList 0x024D returns UpdateHAR 0x0257 (version
  0x10000002, bitmask 1 open / 2 allegiance / 4 allegiance storage,
  monarch guid, guest hash table of guid, storage flag, name, then the
  roommate guids). House objects carry a RestrictionDB in their
  WeenieDesc (flag 0x4000000: version, open, monarch, hash table of
  guid to permission), re-sent to nearby players as
  HouseUpdateRestrictions 0x0248 when it changes.
* **Hooks and storage**: both are containers (ItemType 0x200) standing
  in the house. Hooks carry one item: use the hook (it opens like a
  chest, empty) and PutItemInContainer (item, hook) hangs it; the server
  answers "The container is closed" (0x3EE) when the hook was not opened
  first, so the client opens a world container and stores once its
  contents arrive. A hooked item re-models the hook (UpdateObject with
  the item's setup and name, the item itself stays hidden inside);
  using the hook again shows the item as the hook's contents, and taking
  it puts it back in the pack. Storage chests open for the owner and
  guests with storage permission ("You do not have permission to access
  Storage" otherwise) and hold up to their capacity. Dropping a carried
  item onto a hook or chest in the world does the whole sequence; the
  script `put_in(item, container)` does too. Verified in the Holtburg
  apartment: dagger hooked and taken back, scarab stored and retrieved.
* `/house` (TeleToHouse 0x0262) recalls to the house, `/mansion`
  (0x0278) to the allegiance's; both refused while the Training Academy
  recall lock is set.
* Verified on ACE: the admin bought a Holtburg apartment at its sign
  (5 pyreal stacks and the writ consumed), read the house data, added
  Reborn as a guest with storage, opened the house, saw the sign show
  the owner and the paid price, and abandoned it. Rent payment was not
  exercised (the cooldown blocks a second purchase).

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
