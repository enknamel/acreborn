//! World state built from server messages. `World::apply` consumes decoded
//! messages (`ObjectCreate`, `PlayerCreate`, `UpdatePosition`,
//! `DeleteObject`) and keeps a table of objects with their model ids and
//! positions.

pub mod academy;
pub mod allegiance;
pub mod buffs;
pub mod elements;
pub mod fletching;
pub mod housing;
pub mod hunting;
pub mod landmarks;
pub mod material;
pub mod motion;
pub mod object;
pub mod portals;
pub mod recalls;
pub mod shops;
pub mod social;
pub mod stats;
pub mod towns;
pub mod trip;

use std::collections::HashMap;

use ac_net::messages::{self, event, opcode};
use ac_net::wire::Reader;
use glam::{Mat4, Quat, Vec3};

pub use motion::{CommandQueue, PendingCommand};
pub use object::{MoveTarget, MovementEvent, ObjectCreate, Position};

/// What an object is currently doing, for animation and prediction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    /// Forward motion command (Ready, WalkForward, RunForward, ...).
    pub forward: u32,
    pub forward_speed: f32,
    pub style: u16,
    pub run_rate: f32,
}

impl Default for Motion {
    fn default() -> Self {
        Motion {
            forward: object::motion_cmd::READY,
            forward_speed: 1.0,
            style: 0,
            run_rate: 1.0,
        }
    }
}

/// One object the server has placed in the world.
pub mod object_desc_flags {
    pub const OPENABLE: u32 = 0x0000_0001;
    /// Cannot be picked up (NPCs, doors, signs, furniture).
    pub const STUCK: u32 = 0x0000_0004;
    pub const PLAYER: u32 = 0x0000_0008;
    pub const ATTACKABLE: u32 = 0x0000_0010;
    pub const VENDOR: u32 = 0x0000_0200;
    pub const DOOR: u32 = 0x0000_1000;
    pub const CORPSE: u32 = 0x0000_2000;
    pub const PORTAL: u32 = 0x0004_0000;
    /// A player killer (PlayerKillerStatus PK); see `object::pk_status`.
    pub const PLAYER_KILLER: u32 = 0x0000_0020;
    pub const FREE_PK_STATUS: u32 = 0x0020_0000;
    pub const PK_LITE_STATUS: u32 = 0x0200_0000;
}

/// ItemType bits (ACE `ItemType`). There is no "healer" type: a healing
/// kit is `MISC`, and what an item may be used on comes from its
/// [`usable`] word, not from these.
pub mod item_type {
    pub const MELEE_WEAPON: u32 = 0x1;
    pub const ARMOR: u32 = 0x2;
    pub const CLOTHING: u32 = 0x4;
    pub const JEWELRY: u32 = 0x8;
    pub const CREATURE: u32 = 0x10;
    pub const FOOD: u32 = 0x20;
    pub const MONEY: u32 = 0x40;
    pub const MISC: u32 = 0x80;
    pub const MISSILE_WEAPON: u32 = 0x100;
    pub const CONTAINER: u32 = 0x200;
    pub const USELESS: u32 = 0x400;
    pub const GEM: u32 = 0x800;
    pub const SPELL_COMPONENTS: u32 = 0x1000;
    /// Books, plaques, signs, scrolls: anything with pages or an
    /// inscription.
    pub const WRITABLE: u32 = 0x2000;
    pub const KEY: u32 = 0x4000;
    pub const CASTER: u32 = 0x8000;
    pub const PORTAL: u32 = 0x1_0000;
    /// A thing that can be locked (a chest, a door), not a lockpick.
    pub const LOCKABLE: u32 = 0x2_0000;
    pub const PROMISSORY_NOTE: u32 = 0x4_0000;
    pub const MANA_STONE: u32 = 0x8_0000;
    pub const SERVICE: u32 = 0x10_0000;
    pub const MAGIC_WIELDABLE: u32 = 0x20_0000;
    pub const CRAFT_COOKING_BASE: u32 = 0x40_0000;
    pub const CRAFT_ALCHEMY_BASE: u32 = 0x80_0000;
    pub const CRAFT_FLETCHING_BASE: u32 = 0x200_0000;
    pub const CRAFT_ALCHEMY_INTERMEDIATE: u32 = 0x400_0000;
    pub const CRAFT_FLETCHING_INTERMEDIATE: u32 = 0x800_0000;
    pub const LIFESTONE: u32 = 0x1000_0000;
    /// The Ust and its kin.
    pub const TINKERING_TOOL: u32 = 0x2000_0000;
    /// Salvage bags.
    pub const TINKERING_MATERIAL: u32 = 0x4000_0000;
    pub const GAMEBOARD: u32 = 0x8000_0000;
    /// Item types that go on the body: wield them rather than use them.
    pub const WIELDABLE: u32 = MELEE_WEAPON | ARMOR | CLOTHING | JEWELRY | MISSILE_WEAPON | CASTER;
}

/// The `Usable` word of an object (ACE `Usable`): the low byte says where
/// the item must be to be used (wielded, contained, in view, remote), the
/// bits above 0xFFFF say what it can be used on. An item with any target
/// bit is applied to something (a kit to a player, a stone to an item, a
/// key to a chest) rather than used by itself.
/// Where an item can be worn or wielded: the `valid_locations` word,
/// and the `wielded_location` of something already equipped. These are
/// the slots the character description sends.
pub mod equip {
    pub const MELEE_WEAPON: u32 = 0x0010_0000;
    pub const SHIELD: u32 = 0x0020_0000;
    pub const MISSILE_WEAPON: u32 = 0x0040_0000;
    /// Arrows, bolts and quarrels: a bow needs one of these wielded as
    /// well as the bow itself.
    pub const MISSILE_AMMO: u32 = 0x0080_0000;
    /// The hand slot a wand, orb or staff goes in.
    pub const HELD: u32 = 0x0100_0000;
    pub const TWO_HANDED: u32 = 0x0200_0000;
}

/// What a thing asks of whoever would wield it. An appraisal reports up
/// to four of these as `(kind, what, difficulty)`, and every one must be
/// met: the top wands ask for a base War Magic of 275, and the server
/// simply refuses to arm anyone who falls short.
pub mod wield {
    /// The skill as it stands, buffs counted.
    pub const SKILL: u32 = 1;
    /// The skill without buffs, which is what most weapons ask for.
    pub const RAW_SKILL: u32 = 2;
    /// An attribute (1 Strength .. 6 Self), buffed and unbuffed.
    pub const ATTRIB: u32 = 3;
    pub const RAW_ATTRIB: u32 = 4;
    /// Health, stamina or mana (1, 3, 5), buffed and unbuffed.
    pub const SECONDARY_ATTRIB: u32 = 5;
    pub const RAW_SECONDARY_ATTRIB: u32 = 6;
    /// The character's level.
    pub const LEVEL: u32 = 7;
    /// The skill trained or specialised (see `sac`).
    pub const TRAINING: u32 = 8;
}

pub mod vitals;

pub mod usable {
    /// `Usable::No`: the thing cannot be used at all.
    pub const NO: u32 = 1;
    pub const TARGET_SELF: u32 = 0x2_0000;
    pub const TARGET_WIELDED: u32 = 0x4_0000;
    pub const TARGET_CONTAINED: u32 = 0x8_0000;
    pub const TARGET_VIEWED: u32 = 0x10_0000;
    pub const TARGET_REMOTE: u32 = 0x20_0000;
    /// The item itself (a fellowship stone).
    pub const TARGET_OBJSELF: u32 = 0x80_0000;
    pub const TARGET_MASK: u32 = 0xFFFF_0000;

    /// Whether using the item means applying it to a target.
    pub fn needs_target(usable: u32) -> bool {
        usable & TARGET_MASK & !TARGET_OBJSELF != 0
    }

    /// Whether the item may be applied to the user (a healing kit, yes; a
    /// mana stone, also yes; a key, no).
    pub fn on_self(usable: u32) -> bool {
        usable & TARGET_SELF != 0
    }
}

#[derive(Debug, Clone, Default)]
pub struct WorldObject {
    pub guid: u32,
    pub name: String,
    pub weenie_class_id: u32,
    /// Setup (0x02) id, or 0 when unknown.
    pub setup_id: u32,
    pub motion_table_id: u32,
    /// SoundTable (0x20) id, or 0 when unknown.
    pub sound_table_id: u32,
    pub scale: f32,
    /// Absent for objects carried by another object (inventory, wielded).
    pub position: Option<Position>,
    /// Parent object when carried/wielded.
    pub parent: Option<u32>,
    pub no_draw: bool,
    pub is_player: bool,
    /// ObjectDescriptionFlag bits (0x8 = another player, 0x1000 = door...).
    pub object_desc_flags: u32,
    pub item_type: u32,
    pub icon_id: u32,
    /// RenderSurface drawn over / under the icon, 0 when none.
    pub icon_overlay: u32,
    pub icon_underlay: u32,
    pub stack_size: u32,
    pub value: u32,
    pub spell_id: u32,
    /// ACE `MaterialType`, 0 when none (see `material::name`).
    pub material: u32,
    /// Loot workmanship 1..=10, 0 when none; what salvaging yields.
    pub workmanship: f32,
    /// Uses left (a salvage bag's units), and the maximum.
    pub structure: u32,
    pub max_structure: u32,
    /// What ammunition it takes or is, and what it is for in a fight,
    /// from the header (see `fletching`), 0 unless sent.
    pub ammo_type: u32,
    pub combat_use: u32,
    /// Largest stack of this item, 1 when it does not stack.
    pub max_stack_size: u32,
    /// How the item is used and on what (see `usable`).
    pub usable: u32,
    /// Burden units, 0 unless sent.
    pub burden: u32,
    /// Item slots and side-pack slots of a container (or of the player).
    pub items_capacity: u32,
    pub containers_capacity: u32,
    /// Container holding this item (a pack, or a creature's inventory).
    pub container: Option<u32>,
    /// Creature wielding this item.
    pub wielder: Option<u32>,
    pub valid_locations: u32,
    pub wielded_location: u32,
    /// Health fraction, once the server has told us (UpdateHealth).
    pub health: Option<f32>,
    pub palette_id: u32,
    pub sub_palettes: Vec<(u32, u8, u8)>,
    pub texture_changes: Vec<(u8, u32, u32)>,
    pub anim_part_changes: Vec<(u8, u32)>,
    pub motion: Motion,
    /// One-shot motions (attacks, emotes, door open/close) the server has
    /// asked for, newest last; repeats across events are filtered out.
    pub commands: CommandQueue,
    /// Where the object is being drawn: eases toward `position` so
    /// server updates don't snap.
    pub display: Option<Position>,
    /// Server-issued move-to target, predicted locally until an update.
    pub target: Option<MoveTarget>,
    /// The velocity the server last sent, in metres a second (world
    /// axes). A spell projectile is created with the one it flies at
    /// and gets no position updates on the way: `position` and this
    /// are its whole path.
    pub velocity: Vec3,
    /// `PhysicsState` bits as last sent (see `object::PHYSICS_STATE_*`).
    pub physics_state: u32,
    /// Whether a chest or door is locked, once the server has said
    /// (PublicUpdatePropertyBool Locked); the Openable description flag
    /// follows it.
    pub locked: Option<bool>,
    /// ACE `PlayerKillerStatus` as last broadcast (see
    /// `object::pk_status`); 0 until an update arrives. The PK
    /// description flags follow it.
    pub pk_status: u32,
    /// Mana fraction of an item, once asked (QueryItemManaResponse).
    pub mana: Option<f32>,
}

impl WorldObject {
    /// Something the server is flying at a target: a spell projectile,
    /// an arrow (`PhysicsState::Missile`).
    pub fn is_missile(&self) -> bool {
        self.physics_state & object::PHYSICS_STATE_MISSILE != 0
    }
}

impl WorldObject {
    /// World-space transform (landblock origin + local frame), if placed.
    /// Uses the smoothed display position when available.
    pub fn transform(&self) -> Option<Mat4> {
        let p = self.display.or(self.position)?;
        let origin = landblock_origin(p.cell);
        Some(Mat4::from_scale_rotation_translation(
            Vec3::splat(self.scale),
            p.rotation,
            origin + p.local,
        ))
    }

    pub fn world_pos(&self) -> Option<Vec3> {
        let p = self.position?;
        Some(landblock_origin(p.cell) + p.local)
    }
}

/// Outdoor cell id for a landblock-local point: cells are 24 m, numbered
/// `x * 8 + y + 1`.
pub fn outdoor_cell(landblock: u32, local: Vec3) -> u32 {
    let cx = (local.x / 24.0).floor().clamp(0.0, 7.0) as u32;
    let cy = (local.y / 24.0).floor().clamp(0.0, 7.0) as u32;
    (landblock & 0xFFFF_0000) | (cx * 8 + cy + 1)
}

/// World origin of a cell's landblock: block x/y from the high 16 bits.
pub fn landblock_origin(cell: u32) -> Vec3 {
    let bx = (cell >> 24) as f32;
    let by = ((cell >> 16) & 0xFF) as f32;
    Vec3::new(bx * 192.0, by * 192.0, 0.0)
}

/// Decide what to do with a server position echo for our own character:
/// `true` means take it (resync to where the server has us), `false`
/// means keep our own prediction. `gap` is how far the server's position
/// is from ours; `server_step` is how far the server's own position moved
/// since its last echo; `moved_by_server` is a teleport or forced move.
/// `streak` counts consecutive echoes that looked like a refused move.
///
/// A normal echo lags a few metres behind a running character but keeps
/// pace, so it is ignored. A move the server refused (flying through a
/// wall, a jump it would not allow) leaves the server's position frozen
/// somewhere else: it stops advancing while a real gap remains. That
/// holds whether we are still running or standing still being hit, so
/// after a few echoes confirm it we snap back to the server.
fn reconcile(gap: f32, server_step: f32, moved_by_server: bool, streak: &mut u8) -> bool {
    /// A gap this big is wrong however it arose; snap at once.
    const FAR: f32 = 40.0;
    /// Past this the server is plainly not where we are. It is small on
    /// purpose: walking into a wall the server will not let us through
    /// strands us only a few metres away, and that still breaks casting
    /// and looting.
    const APART: f32 = 3.0;
    /// The server's own position barely moved: stuck, not merely lagging.
    const STUCK: f32 = 2.0;
    /// Echoes in a row that must agree before we snap, so one late packet
    /// is not mistaken for a refusal.
    const CONFIRM: u8 = 3;

    if moved_by_server || gap > FAR {
        *streak = 0;
        return true;
    }
    // A server that is still advancing is following us, just late: leave
    // our prediction alone. One that has stopped while a real gap remains
    // has stopped taking our positions, whether or not we are moving --
    // standing still in sync leaves no gap to see.
    if server_step < STUCK && gap > APART {
        *streak = streak.saturating_add(1);
        if *streak >= CONFIRM {
            *streak = 0;
            return true;
        }
        return false;
    }
    *streak = 0;
    false
}

/// The map coordinates the game shows (42.1N, 33.6E): world position
/// over 240 minus 102 on both axes, north and east positive. None
/// indoors (cells from 0x100 up have no map position).
pub fn map_coords(p: &object::Position) -> Option<(f32, f32)> {
    if p.cell & 0xFFFF >= 0x100 {
        return None;
    }
    let g = landblock_origin(p.cell) + p.local;
    Some((g.y / 240.0 - 102.0, g.x / 240.0 - 102.0))
}

/// `42.1N, 33.6E` (ACE `GetMapCoordStr`: a twentieth is taken off before
/// rounding, as retail did), or "indoors".
pub fn map_coord_str(p: &object::Position) -> String {
    match map_coords(p) {
        Some((ns, ew)) => format!(
            "{:.1}{}, {:.1}{}",
            (ns.abs() - 0.05).max(0.0),
            if ns >= 0.0 { "N" } else { "S" },
            (ew.abs() - 0.05).max(0.0),
            if ew >= 0.0 { "E" } else { "W" }
        ),
        None => "indoors".into(),
    }
}

#[derive(Debug, Default)]
pub struct World {
    pub objects: HashMap<u32, WorldObject>,
    pub player_guid: Option<u32>,
    /// The teleport and forced-position sequences last seen on our own
    /// position updates: a change means the server moved us itself.
    pub player_move_seqs: Option<(u16, u16)>,
    /// The last position the server echoed for us, and how many echoes in
    /// a row have shown the server holding still while our own position
    /// ran away from it: the sign of a move the server refused, so we
    /// resync to where it says we are (see the UpdatePosition handler).
    player_echo: Option<Position>,
    desync_streak: u8,
    /// Bumped whenever the set of drawable objects or a position changes.
    pub generation: u64,
    /// The player's character sheet.
    pub stats: stats::PlayerStats,
    /// A ground container we are looking into: its guid and item guids.
    pub open_container: Option<(u32, Vec<u32>)>,
    /// The vendor we are trading with.
    pub open_vendor: Option<object::ApproachVendor>,
    /// The secure trade in progress, if any.
    pub trade: Option<Trade>,
    /// The fellowship we belong to, if any.
    pub fellowship: Option<Fellowship>,
    /// Questions the server asked (recruit into a fellowship, swear
    /// allegiance...), until answered with `confirm`.
    pub confirmations: Vec<Confirmation>,
    /// Our allegiance as last described by AllegianceUpdate; `None`
    /// until one arrives, `Some` with no members when we have none.
    pub allegiance: Option<allegiance::Allegiance>,
    /// Another player's allegiance profile (AllegianceInfoResponse to
    /// an officer's request), keyed by that player's guid.
    pub allegiance_info: Option<(u32, allegiance::Allegiance)>,
    /// The house sign we last used (HouseProfile), and a counter bumped
    /// each time one arrives (the panel opens on a new one).
    pub house_profile: Option<housing::HouseProfile>,
    pub house_profile_seq: u64,
    /// Our own house (HouseData); `Some(None)` once the server said we
    /// have none, `None` until it answered a HouseQuery at all.
    pub house: Option<Option<housing::HouseData>>,
    /// Our house's guest list (UpdateHAR), on request.
    pub house_access: Option<housing::HouseAccess>,
    pub friends: Vec<social::Friend>,
    pub titles: social::Titles,
    pub squelches: social::Squelches,
}

/// What `apply` did with a message, for logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    Created,
    Moved,
    /// The server moved our own character somewhere else (a teleport
    /// or a correction of more than a few metres).
    PlayerMoved,
    Deleted,
    PlayerSet,
    /// Name, level, attributes or vitals changed.
    Stats,
    /// A spell entered (`known`) or left our spellbook.
    Spellbook {
        spell: u32,
        known: bool,
    },
    /// Our enchantment registry changed (a buff, debuff, dispel or purge).
    Enchantments,
    /// An item moved between the world, a container and a wielder.
    Inventory,
    /// The trade window changed (opened, items, acceptance, closed).
    Trade,
    /// Fellowship membership or vitals changed.
    Fellowship,
    /// A confirmation request arrived or was resolved.
    Confirmation,
    /// The allegiance profile changed (ours, or one asked about).
    Allegiance,
    /// A house profile, our house data or its guest list arrived.
    House,
    /// Friends, titles or squelches changed.
    Social,
    /// An object's look changed (equipment).
    Appearance,
    /// A creature's health fraction changed.
    Health,
    /// A vendor opened its stock to us.
    Vendor,
    /// The server told our own character to move to a target.
    PlayerMoveTo,
    /// The server described our own character's motion (no target): a
    /// server-driven move is over, or our own state was echoed.
    PlayerMotion,
    /// An object plays a particle script (PlayEffect): a fizzle, a
    /// portal flash, a level-up.
    Effect {
        guid: u32,
        script: u32,
    },
    Ignored,
    Failed,
}

impl World {
    pub fn player(&self) -> Option<&WorldObject> {
        self.objects.get(&self.player_guid?)
    }

    pub fn player_mut(&mut self) -> Option<&mut WorldObject> {
        self.objects.get_mut(&self.player_guid?)
    }

    /// Items the player carries: the main pack's contents and everything
    /// inside carried side packs (an item's `container` then names the
    /// pack, whose own `container` is the player).
    pub fn inventory(&self) -> impl Iterator<Item = &WorldObject> {
        let me = self.player_guid;
        self.objects.values().filter(move |o| {
            me.is_some()
                && o.container.is_some()
                && (o.container == me
                    || o.container
                        .and_then(|c| self.objects.get(&c))
                        .is_some_and(|c| c.container == me))
        })
    }

    /// Whether `guid` is in the character's packs, the main one or a
    /// side pack. Anything wielded is not "carried" in this sense.
    pub fn is_carried(&self, guid: u32) -> bool {
        let me = self.player_guid;
        me.is_some()
            && self.objects.get(&guid).is_some_and(|o| {
                o.container == me
                    || o.container
                        .and_then(|c| self.objects.get(&c))
                        .is_some_and(|c| c.container == me)
            })
    }

    /// Items directly in the main pack (not inside side packs).
    pub fn main_pack(&self) -> impl Iterator<Item = &WorldObject> {
        let me = self.player_guid;
        self.objects
            .values()
            .filter(move |o| me.is_some() && o.container == me)
    }

    /// Items the player is wielding.
    pub fn wielded(&self) -> impl Iterator<Item = &WorldObject> {
        let me = self.player_guid;
        self.objects
            .values()
            .filter(move |o| me.is_some() && o.wielder == me)
    }

    pub fn apply(&mut self, msg: &[u8]) -> Applied {
        let Some((op, body)) = messages::split(msg) else {
            return Applied::Ignored;
        };
        match op {
            // UpdateObject carries the same description as CreateObject:
            // the object re-described (a hook that now shows the item hung
            // on it, a changed appearance). Runtime state stays.
            opcode::OBJECT_CREATE | opcode::UPDATE_OBJECT => match ObjectCreate::parse(body) {
                Ok(oc) => {
                    let is_player = self.player_guid == Some(oc.guid);
                    let previous = self.objects.remove(&oc.guid);
                    if oc.object_desc_flags & object_desc_flags::PLAYER != 0 {
                        tracing::debug!(
                            "player object {} ({:#010x}): position {:?} setup {:#010x} no_draw {} parent {:?}",
                            oc.name, oc.guid, oc.position.map(|p| p.cell), oc.setup_id, oc.no_draw, oc.parent
                        );
                    }
                    let obj = WorldObject {
                        guid: oc.guid,
                        // Salvage bags are sent as "Salvage"; the client
                        // names them by material, as retail did.
                        name: if oc.name.starts_with("Salvage") && oc.material != 0 {
                            format!("Salvaged {}", material::name(oc.material))
                        } else {
                            oc.name
                        },
                        weenie_class_id: oc.weenie_class_id,
                        setup_id: oc.setup_id,
                        motion_table_id: oc.motion_table_id,
                        sound_table_id: oc.sound_table_id,
                        scale: if oc.scale > 0.0 { oc.scale } else { 1.0 },
                        position: oc.position,
                        parent: oc.parent,
                        no_draw: oc.no_draw,
                        is_player,
                        object_desc_flags: oc.object_desc_flags,
                        item_type: oc.item_type,
                        icon_id: oc.icon_id,
                        icon_overlay: oc.icon_overlay,
                        icon_underlay: oc.icon_underlay,
                        stack_size: oc.stack_size,
                        value: oc.value,
                        spell_id: oc.spell_id,
                        material: oc.material,
                        workmanship: oc.workmanship,
                        structure: oc.structure,
                        max_structure: oc.max_structure,
                        ammo_type: oc.ammo_type,
                        combat_use: oc.combat_use,
                        max_stack_size: oc.max_stack_size,
                        usable: oc.usable,
                        burden: oc.burden,
                        items_capacity: oc.items_capacity,
                        containers_capacity: oc.containers_capacity,
                        container: oc.container,
                        wielder: oc.wielder,
                        valid_locations: oc.valid_locations,
                        wielded_location: oc.wielded_location,
                        health: previous.as_ref().and_then(|o| o.health),
                        palette_id: oc.palette_id,
                        sub_palettes: oc.sub_palettes,
                        texture_changes: oc.texture_changes,
                        anim_part_changes: oc.anim_part_changes,
                        motion: previous.as_ref().map(|o| o.motion).unwrap_or_default(),
                        commands: previous
                            .as_ref()
                            .map(|o| o.commands.clone())
                            .unwrap_or_default(),
                        display: oc.position,
                        target: None,
                        velocity: oc.velocity,
                        physics_state: oc.physics_state,
                        locked: previous.as_ref().and_then(|o| o.locked),
                        pk_status: previous.as_ref().map(|o| o.pk_status).unwrap_or(0),
                        mana: previous.as_ref().and_then(|o| o.mana),
                    };
                    self.objects.insert(obj.guid, obj);
                    self.generation += 1;
                    Applied::Created
                }
                Err(e) => {
                    tracing::warn!("ObjectCreate: {e}");
                    Applied::Failed
                }
            },
            opcode::PLAYER_CREATE => {
                if body.len() >= 4 {
                    let guid = u32::from_le_bytes(body[..4].try_into().unwrap());
                    self.player_guid = Some(guid);
                    if let Some(o) = self.objects.get_mut(&guid) {
                        o.is_player = true;
                    }
                    self.generation += 1;
                    Applied::PlayerSet
                } else {
                    Applied::Failed
                }
            }
            opcode::UPDATE_POSITION => match object::UpdatePosition::parse(body) {
                Ok(up) => {
                    let is_player = self.player_guid == Some(up.guid);
                    if let Some(o) = self.objects.get_mut(&up.guid) {
                        if is_player {
                            // Our own positions come back as echoes, a
                            // quarter of a second old: at a run that is
                            // metres behind where we are, and taking them
                            // dragged a fast character back every echo.
                            // The server counts its teleport and forced
                            // position sequences up when it moves us
                            // itself; only those moves are taken, plus
                            // anything so far off that something is wrong.
                            let seqs = (up.teleport_seq, up.force_seq);
                            let moved_by_server = match self.player_move_seqs {
                                Some(seen) => seen != seqs,
                                None => true,
                            };
                            self.player_move_seqs = Some(seqs);
                            // Where the server says we are, versus where we
                            // have run to. A normal echo lags a few metres
                            // and keeps pace with us; a move the server
                            // refused (flying through a wall, a jump it
                            // rejected) leaves its position frozen while
                            // ours runs away, so the gap grows and the
                            // server's own position barely moves.
                            let server_pos = landblock_origin(up.position.cell) + up.position.local;
                            let gap = match o.position {
                                Some(cur) => {
                                    (landblock_origin(cur.cell) + cur.local - server_pos).length()
                                }
                                None => f32::INFINITY,
                            };
                            let server_step = self
                                .player_echo
                                .map(|e| (landblock_origin(e.cell) + e.local - server_pos).length())
                                .unwrap_or(f32::INFINITY);
                            self.player_echo = Some(up.position);
                            if !reconcile(
                                gap,
                                server_step,
                                moved_by_server,
                                &mut self.desync_streak,
                            ) {
                                return Applied::Ignored;
                            }
                            tracing::info!(
                                "server moved the player to {:#010x} {:?} (gap {gap:.0} m)",
                                up.position.cell,
                                up.position.local
                            );
                        }
                        o.position = Some(up.position);
                        if o.display.is_none() {
                            o.display = Some(up.position);
                        }
                        if up.flags & 0x01 != 0 {
                            o.velocity = up.velocity;
                        }
                        // The server's own position supersedes local prediction.
                        o.target = None;
                        self.generation += 1;
                        if is_player {
                            Applied::PlayerMoved
                        } else {
                            Applied::Moved
                        }
                    } else {
                        Applied::Ignored
                    }
                }
                Err(e) => {
                    tracing::warn!("UpdatePosition: {e}");
                    Applied::Failed
                }
            },
            // SetState (0xF74B): guid, physics state, two sequences. A
            // player logs in hidden and is shown this way; NoDraw and
            // Hidden objects stay off the scene.
            opcode::SET_STATE => {
                let mut r = Reader::new(body);
                match (r.u32(), r.u32()) {
                    (Ok(guid), Ok(state)) => {
                        let Some(o) = self.objects.get_mut(&guid) else {
                            return Applied::Ignored;
                        };
                        let hidden = state
                            & (object::PHYSICS_STATE_NO_DRAW | object::PHYSICS_STATE_HIDDEN)
                            != 0;
                        o.physics_state = state;
                        if o.no_draw != hidden {
                            o.no_draw = hidden;
                            self.generation += 1;
                            Applied::Created
                        } else {
                            Applied::Ignored
                        }
                    }
                    _ => Applied::Failed,
                }
            }
            opcode::MOVEMENT_EVENT => match MovementEvent::parse(body) {
                Ok(ev) => {
                    let Some(o) = self.objects.get_mut(&ev.guid) else {
                        return Applied::Ignored;
                    };
                    o.motion = Motion {
                        forward: ev.forward,
                        forward_speed: ev.forward_speed,
                        style: ev.style,
                        run_rate: ev.run_rate,
                    };
                    o.target = ev.target;
                    for &(cmd, seq, speed) in &ev.commands {
                        if o.commands.push(cmd, seq, speed) {
                            tracing::debug!(
                                "{:#010x} plays command {cmd:#06x} (seq {seq:#x}, speed {speed})",
                                ev.guid
                            );
                        }
                    }
                    if let (Some(h), Some(p)) = (ev.desired_heading, o.position.as_mut()) {
                        if !matches!(ev.target, Some(MoveTarget::Position { .. })) {
                            p.rotation = heading_quat(h);
                        }
                    }
                    self.generation += 1;
                    if Some(ev.guid) == self.player_guid {
                        if ev.target.is_some() {
                            Applied::PlayerMoveTo
                        } else {
                            Applied::PlayerMotion
                        }
                    } else {
                        Applied::Moved
                    }
                }
                Err(e) => {
                    tracing::warn!("MovementEvent: {e}");
                    Applied::Failed
                }
            },
            opcode::OBJECT_DELETE | opcode::INVENTORY_REMOVE_OBJECT => {
                // InventoryRemoveObject: a spent/given item is gone from
                // the pack; the server sends no DeleteObject for it.
                if body.len() >= 4 {
                    let guid = u32::from_le_bytes(body[..4].try_into().unwrap());
                    if self.objects.remove(&guid).is_some() {
                        self.generation += 1;
                        return Applied::Deleted;
                    }
                }
                Applied::Ignored
            }
            opcode::GAME_EVENT => match messages::split_game_event(body) {
                Some((_, _, event::INVENTORY_PUT_OBJ_IN_CONTAINER, rest)) => {
                    let mut r = Reader::new(rest);
                    match (r.u32(), r.u32(), r.u32()) {
                        (Ok(item), Ok(container), Ok(_placement)) => {
                            if let Some(o) = self.objects.get_mut(&item) {
                                o.container = Some(container);
                                o.wielder = None;
                                o.wielded_location = 0;
                                o.position = None;
                                o.display = None;
                                o.parent = Some(container);
                                self.generation += 1;
                            }
                            if let Some((c, items)) = &mut self.open_container {
                                if *c != container {
                                    items.retain(|g| *g != item);
                                } else if !items.contains(&item) {
                                    // Stored into the chest (or hook) we are
                                    // looking into: it shows up there.
                                    items.push(item);
                                }
                            }
                            Applied::Inventory
                        }
                        _ => Applied::Failed,
                    }
                }
                Some((_, _, event::WIELD_OBJECT, rest)) => {
                    let mut r = Reader::new(rest);
                    match (r.u32(), r.u32()) {
                        (Ok(item), Ok(location)) => {
                            if let Some(o) = self.objects.get_mut(&item) {
                                o.wielder = self.player_guid;
                                o.wielded_location = location;
                                o.container = None;
                                o.position = None;
                                o.display = None;
                                o.parent = self.player_guid;
                                self.generation += 1;
                            }
                            Applied::Inventory
                        }
                        _ => Applied::Failed,
                    }
                }
                Some((_, _, event::UPDATE_HEALTH, rest)) => {
                    match messages::parse_update_health(rest) {
                        Ok((guid, h)) => {
                            if let Some(o) = self.objects.get_mut(&guid) {
                                o.health = Some(h);
                            }
                            Applied::Health
                        }
                        Err(_) => Applied::Failed,
                    }
                }
                Some((_, _, event::FELLOWSHIP_FULL_UPDATE, rest)) => {
                    let mut r = Reader::new(rest);
                    let parsed = (|| {
                        let n = r.u16().ok()? as usize;
                        let _buckets = r.u16().ok()?;
                        let mut members = Vec::with_capacity(n.min(16));
                        for _ in 0..n {
                            members.push(parse_fellow(&mut r)?);
                        }
                        let name = r.string16().ok()?;
                        let leader = r.u32().ok()?;
                        let share_xp = r.u32().ok()? != 0;
                        let even_share = r.u32().ok()? != 0;
                        let open = r.u32().ok()? != 0;
                        let locked = r.u32().ok()? != 0;
                        Some(Fellowship {
                            name,
                            leader,
                            share_xp,
                            even_share,
                            open,
                            locked,
                            members,
                        })
                    })();
                    match parsed {
                        Some(f) => {
                            self.fellowship = Some(f);
                            self.generation += 1;
                            Applied::Fellowship
                        }
                        None => Applied::Failed,
                    }
                }
                Some((_, _, event::FELLOWSHIP_UPDATE_FELLOW, rest)) => {
                    let mut r = Reader::new(rest);
                    match (parse_fellow(&mut r), self.fellowship.as_mut()) {
                        (Some(f), Some(fs)) => {
                            match fs.members.iter_mut().find(|m| m.guid == f.guid) {
                                Some(m) => *m = f,
                                None => fs.members.push(f),
                            }
                            Applied::Fellowship
                        }
                        _ => Applied::Ignored,
                    }
                }
                Some((_, _, event::FELLOWSHIP_DISBAND, _)) => {
                    self.fellowship = None;
                    self.generation += 1;
                    Applied::Fellowship
                }
                Some((_, _, event::FELLOWSHIP_QUIT | event::FELLOWSHIP_DISMISS, rest)) => {
                    let who = Reader::new(rest).u32().unwrap_or(0);
                    if Some(who) == self.player_guid {
                        self.fellowship = None;
                    } else if let Some(fs) = self.fellowship.as_mut() {
                        fs.members.retain(|m| m.guid != who);
                    }
                    self.generation += 1;
                    Applied::Fellowship
                }
                Some((_, _, event::CONFIRMATION_REQUEST, rest)) => {
                    let mut r = Reader::new(rest);
                    match (r.u32(), r.u32(), r.string16()) {
                        (Ok(kind), Ok(context), Ok(text)) => {
                            self.confirmations.push(Confirmation {
                                kind,
                                context,
                                text,
                            });
                            Applied::Confirmation
                        }
                        _ => Applied::Failed,
                    }
                }
                Some((_, _, event::ALLEGIANCE_UPDATE, rest)) => {
                    let mut r = Reader::new(rest);
                    let me = self.player_guid.unwrap_or(0);
                    match r
                        .u32()
                        .ok()
                        .and_then(|rank| allegiance::parse_profile(&mut r, me, rank))
                    {
                        Some(a) => {
                            self.allegiance = Some(a);
                            Applied::Allegiance
                        }
                        None => Applied::Failed,
                    }
                }
                Some((_, _, event::ALLEGIANCE_INFO_RESPONSE, rest)) => {
                    let mut r = Reader::new(rest);
                    match r
                        .u32()
                        .ok()
                        .and_then(|who| Some((who, allegiance::parse_profile(&mut r, who, 0)?)))
                    {
                        Some(info) => {
                            self.allegiance_info = Some(info);
                            Applied::Allegiance
                        }
                        None => Applied::Failed,
                    }
                }
                Some((_, _, event::ALLEGIANCE_LOGIN_NOTIFICATION, rest)) => {
                    let mut r = Reader::new(rest);
                    if let (Ok(who), Ok(on)) = (r.u32(), r.u32()) {
                        if let Some(m) = self.allegiance.as_mut().and_then(|a| a.member_mut(who)) {
                            m.online = on != 0;
                        }
                    }
                    Applied::Allegiance
                }
                Some((
                    _,
                    _,
                    event::ALLEGIANCE_UPDATE_DONE | event::ALLEGIANCE_UPDATE_ABORTED,
                    _,
                )) => Applied::Ignored,
                Some((_, _, event::HOUSE_PROFILE, rest)) => match housing::parse_profile(rest) {
                    Some(p) => {
                        self.house_profile = Some(p);
                        self.house_profile_seq += 1;
                        Applied::House
                    }
                    None => Applied::Failed,
                },
                Some((_, _, event::HOUSE_DATA, rest)) => match housing::parse_data(rest) {
                    Some(d) => {
                        self.house = Some(Some(d));
                        Applied::House
                    }
                    None => Applied::Failed,
                },
                Some((_, _, event::HOUSE_STATUS, _)) => {
                    // The answer to HouseQuery without a house.
                    self.house = Some(None);
                    self.house_access = None;
                    Applied::House
                }
                Some((_, _, event::UPDATE_HAR, rest)) => match housing::parse_access(rest) {
                    Some(a) => {
                        self.house_access = Some(a);
                        Applied::House
                    }
                    None => Applied::Failed,
                },
                Some((_, _, event::UPDATE_RENT_TIME, rest)) => {
                    if let (Ok(t), Some(Some(h))) = (Reader::new(rest).u32(), self.house.as_mut()) {
                        h.rent_time = t;
                    }
                    Applied::House
                }
                Some((_, _, event::UPDATE_RENT_PAYMENT, rest)) => {
                    let mut r = Reader::new(rest);
                    if let (Some(rent), Some(Some(h))) =
                        (housing::read_payments_pub(&mut r), self.house.as_mut())
                    {
                        h.rent = rent;
                    }
                    Applied::House
                }
                Some((_, _, event::FRIENDS_LIST_UPDATE, rest)) => match social::parse_friends(rest)
                {
                    Some(u) => {
                        match u.kind {
                            0 => self.friends = u.friends,
                            2 => {
                                for f in &u.friends {
                                    self.friends.retain(|x| x.guid != f.guid);
                                }
                            }
                            _ => {
                                for f in u.friends {
                                    match self.friends.iter_mut().find(|x| x.guid == f.guid) {
                                        Some(x) => {
                                            x.online = f.online;
                                            if !f.name.is_empty() {
                                                x.name = f.name;
                                            }
                                        }
                                        None => self.friends.push(f),
                                    }
                                }
                            }
                        }
                        Applied::Social
                    }
                    None => Applied::Failed,
                },
                Some((_, _, event::CHARACTER_TITLE, rest)) => match social::Titles::parse(rest) {
                    Some(t) => {
                        self.titles = t;
                        Applied::Social
                    }
                    None => Applied::Failed,
                },
                Some((_, _, event::UPDATE_TITLE, rest)) => {
                    if self.titles.apply_update(rest) {
                        Applied::Social
                    } else {
                        Applied::Failed
                    }
                }
                Some((_, _, event::SET_SQUELCH_DB, rest)) => match social::parse_squelches(rest) {
                    Some(sq) => {
                        self.squelches = sq;
                        Applied::Social
                    }
                    None => Applied::Failed,
                },
                Some((_, _, event::CONFIRMATION_DONE, rest)) => {
                    let mut r = Reader::new(rest);
                    if let (Ok(kind), Ok(context)) = (r.u32(), r.u32()) {
                        self.confirmations
                            .retain(|c| !(c.kind == kind && c.context == context));
                    }
                    Applied::Confirmation
                }
                Some((_, _, event::REGISTER_TRADE, rest)) => {
                    let mut r = Reader::new(rest);
                    match (r.u32(), r.u32()) {
                        (Ok(initiator), Ok(partner)) => {
                            let stamp = r.u64().unwrap_or(0) as i64;
                            let me = self.player_guid.unwrap_or(0);
                            let other = if initiator == me { partner } else { initiator };
                            self.trade = Some(Trade {
                                partner: other,
                                initiator,
                                stamp,
                                ..Default::default()
                            });
                            self.generation += 1;
                            Applied::Trade
                        }
                        _ => Applied::Failed,
                    }
                }
                Some((_, _, event::CLOSE_TRADE, _)) => {
                    self.trade = None;
                    self.generation += 1;
                    Applied::Trade
                }
                Some((_, _, event::ADD_TO_TRADE, rest)) => {
                    let mut r = Reader::new(rest);
                    match (r.u32(), r.u32()) {
                        (Ok(item), Ok(side)) => {
                            if let Some(t) = self.trade.as_mut() {
                                let list = if side == 1 {
                                    &mut t.mine
                                } else {
                                    &mut t.theirs
                                };
                                if !list.contains(&item) {
                                    list.push(item);
                                }
                                t.i_accepted = false;
                                t.they_accepted = false;
                            }
                            Applied::Trade
                        }
                        _ => Applied::Failed,
                    }
                }
                Some((_, _, event::REMOVE_FROM_TRADE, rest)) => {
                    let mut r = Reader::new(rest);
                    if let (Ok(item), Some(t)) = (r.u32(), self.trade.as_mut()) {
                        t.mine.retain(|&g| g != item);
                        t.theirs.retain(|&g| g != item);
                        t.i_accepted = false;
                        t.they_accepted = false;
                    }
                    Applied::Trade
                }
                Some((_, _, event::ACCEPT_TRADE, rest)) => {
                    let who = Reader::new(rest).u32().unwrap_or(0);
                    let me = self.player_guid;
                    if let Some(t) = self.trade.as_mut() {
                        if Some(who) == me {
                            t.i_accepted = true;
                        } else {
                            t.they_accepted = true;
                        }
                    }
                    Applied::Trade
                }
                Some((_, _, event::DECLINE_TRADE, rest)) => {
                    let who = Reader::new(rest).u32().unwrap_or(0);
                    let me = self.player_guid;
                    if let Some(t) = self.trade.as_mut() {
                        if Some(who) == me {
                            t.i_accepted = false;
                        } else {
                            t.they_accepted = false;
                        }
                    }
                    Applied::Trade
                }
                Some((_, _, event::RESET_TRADE, _)) => {
                    if let Some(t) = self.trade.as_mut() {
                        t.mine.clear();
                        t.theirs.clear();
                        t.i_accepted = false;
                        t.they_accepted = false;
                    }
                    Applied::Trade
                }
                Some((_, _, event::CLEAR_TRADE_ACCEPTANCE, _)) => {
                    if let Some(t) = self.trade.as_mut() {
                        t.i_accepted = false;
                        t.they_accepted = false;
                    }
                    Applied::Trade
                }
                Some((_, _, event::TRADE_FAILURE, rest)) => {
                    let mut r = Reader::new(rest);
                    if let (Ok(item), Ok(reason), Some(t)) = (r.u32(), r.u32(), self.trade.as_mut())
                    {
                        t.failure = Some((item, reason));
                    }
                    Applied::Trade
                }
                Some((_, _, event::VIEW_CONTENTS, rest)) => {
                    match messages::parse_view_contents(rest) {
                        Ok((c, items)) => {
                            for (g, _) in &items {
                                if let Some(o) = self.objects.get_mut(g) {
                                    o.container = Some(c);
                                    o.parent = Some(c);
                                }
                            }
                            // Our own side packs are listed the same way at
                            // login; only a container in the world opens the
                            // loot window.
                            let me = self.player_guid;
                            let carried = me.is_some()
                                && self.objects.get(&c).is_some_and(|o| o.container == me);
                            if !carried {
                                self.open_container =
                                    Some((c, items.into_iter().map(|(g, _)| g).collect()));
                            }
                            Applied::Inventory
                        }
                        Err(_) => Applied::Failed,
                    }
                }
                Some((_, _, event::APPROACH_VENDOR, rest)) => {
                    match object::ApproachVendor::parse(rest) {
                        Ok(v) => {
                            self.open_vendor = Some(v);
                            Applied::Vendor
                        }
                        Err(e) => {
                            tracing::warn!("ApproachVendor: {e}");
                            tracing::debug!(
                                "ApproachVendor payload: {}",
                                rest.iter().map(|b| format!("{b:02x}")).collect::<String>()
                            );
                            Applied::Failed
                        }
                    }
                }
                Some((_, _, event::CLOSE_GROUND_CONTAINER, _)) => {
                    self.open_container = None;
                    Applied::Inventory
                }
                Some((_, _, event::QUERY_ITEM_MANA_RESPONSE, rest)) => {
                    let mut r = Reader::new(rest);
                    match (r.u32(), r.f32(), r.u32()) {
                        (Ok(item), Ok(mana), Ok(success)) => {
                            if let Some(o) = self.objects.get_mut(&item) {
                                o.mana = (success != 0).then_some(mana);
                            }
                            Applied::Inventory
                        }
                        _ => Applied::Failed,
                    }
                }
                Some((_, _, event::INVENTORY_PUT_OBJECT_IN_3D, rest)) => {
                    if let Ok(item) = Reader::new(rest).u32() {
                        if let Some(o) = self.objects.get_mut(&item) {
                            o.container = None;
                            o.wielder = None;
                            o.parent = None;
                            self.generation += 1;
                        }
                    }
                    Applied::Inventory
                }
                _ => self.apply_stats(op, body),
            },
            opcode::SET_STACK_SIZE => match messages::parse_set_stack_size(body) {
                Ok((guid, stack, value)) => {
                    if let Some(o) = self.objects.get_mut(&guid) {
                        o.stack_size = stack;
                        o.value = value;
                        self.generation += 1;
                        Applied::Inventory
                    } else {
                        Applied::Ignored
                    }
                }
                Err(_) => Applied::Failed,
            },
            opcode::PUBLIC_UPDATE_PROPERTY_INT => {
                let Ok(u) = object::PropertyUpdate::parse_public_int(body) else {
                    return Applied::Failed;
                };
                let Some(o) = self.objects.get_mut(&u.guid) else {
                    return Applied::Ignored;
                };
                let value = u.value.max(0) as u32;
                let applied = match u.key {
                    messages::property_int::STACK_SIZE => {
                        o.stack_size = value;
                        Applied::Inventory
                    }
                    messages::property_int::VALUE => {
                        o.value = value;
                        Applied::Inventory
                    }
                    // Uses left on a healing kit or a lockpick.
                    object::property_int::STRUCTURE => {
                        o.structure = value;
                        Applied::Inventory
                    }
                    object::property_int::MAX_STRUCTURE => {
                        o.max_structure = value;
                        Applied::Inventory
                    }
                    // Sent with the wield and unwield events; the
                    // location is what tells a two-handed weapon from a
                    // shield hand.
                    object::property_int::CURRENT_WIELDED_LOCATION => {
                        o.wielded_location = value;
                        Applied::Inventory
                    }
                    // Broadcast when a player (or the projectile they
                    // fired) changes PK status: the description flags
                    // follow ACE's UpdateObjectDescriptionFlag.
                    object::property_int::PLAYER_KILLER_STATUS => {
                        o.pk_status = value;
                        let mut f = o.object_desc_flags
                            & !(object_desc_flags::PLAYER_KILLER
                                | object_desc_flags::FREE_PK_STATUS
                                | object_desc_flags::PK_LITE_STATUS);
                        if value == object::pk_status::PK {
                            f |= object_desc_flags::PLAYER_KILLER;
                        } else if value == object::pk_status::FREE {
                            f |= object_desc_flags::FREE_PK_STATUS;
                        } else if value == object::pk_status::PK_LITE {
                            f |= object_desc_flags::PK_LITE_STATUS;
                        }
                        o.object_desc_flags = f;
                        Applied::Appearance
                    }
                    _ => return Applied::Ignored,
                };
                self.generation += 1;
                applied
            }
            // Locked/unlocked chests and doors, and UiHidden. The
            // Openable description flag follows Locked as ACE computes
            // it (`openable = !IsLocked`).
            opcode::PUBLIC_UPDATE_PROPERTY_BOOL => {
                let Ok(u) = object::PropertyUpdate::parse_public_bool(body) else {
                    return Applied::Failed;
                };
                let Some(o) = self.objects.get_mut(&u.guid) else {
                    return Applied::Ignored;
                };
                match u.key {
                    object::property_bool::LOCKED => {
                        o.locked = Some(u.value);
                        if u.value {
                            o.object_desc_flags &= !object_desc_flags::OPENABLE;
                        } else {
                            o.object_desc_flags |= object_desc_flags::OPENABLE;
                        }
                        self.generation += 1;
                        Applied::Inventory
                    }
                    _ => Applied::Ignored,
                }
            }
            // A renamed object (a pet, a corpse that gained a suffix).
            opcode::PUBLIC_UPDATE_PROPERTY_STRING => {
                let Ok(u) = object::PropertyUpdate::parse_public_string(body) else {
                    return Applied::Failed;
                };
                let Some(o) = self.objects.get_mut(&u.guid) else {
                    return Applied::Ignored;
                };
                if u.key == messages::property_string::NAME {
                    o.name = u.value;
                    self.generation += 1;
                    Applied::Appearance
                } else {
                    Applied::Ignored
                }
            }
            // A model, motion table or icon swap on an object in view.
            opcode::PUBLIC_UPDATE_PROPERTY_DATA_ID => {
                let Ok(u) = object::PropertyUpdate::parse_public_did(body) else {
                    return Applied::Failed;
                };
                let Some(o) = self.objects.get_mut(&u.guid) else {
                    return Applied::Ignored;
                };
                match u.key {
                    object::property_did::SETUP => o.setup_id = u.value,
                    object::property_did::MOTION_TABLE => o.motion_table_id = u.value,
                    object::property_did::ICON => o.icon_id = u.value,
                    _ => return Applied::Ignored,
                }
                self.generation += 1;
                Applied::Appearance
            }
            // Parsed so a bad body is reported; nothing in the world
            // reads an object's floats or 64-bit ints (emote-driven
            // stat changes on other players).
            opcode::PUBLIC_UPDATE_PROPERTY_FLOAT => {
                match object::PropertyUpdate::parse_public_float(body) {
                    Ok(_) => Applied::Ignored,
                    Err(_) => Applied::Failed,
                }
            }
            opcode::PUBLIC_UPDATE_PROPERTY_INT64 => {
                match object::PropertyUpdate::parse_public_int64(body) {
                    Ok(_) => Applied::Ignored,
                    Err(_) => Applied::Failed,
                }
            }
            // Our own motion table changed (a mount, a transformation):
            // the player object animates from the new one.
            opcode::PRIVATE_UPDATE_PROPERTY_DATA_ID => {
                if let Ok(u) = object::PropertyUpdate::parse_private_u32(body) {
                    if u.key == object::property_did::MOTION_TABLE {
                        if let Some(o) = self.player_mut() {
                            o.motion_table_id = u.value;
                            self.generation += 1;
                        }
                    }
                }
                self.apply_stats(op, body)
            }
            // A player in view died: their health is gone until the
            // server describes them again. The message itself goes to
            // the chat log.
            opcode::PLAYER_KILLED => match messages::parse_player_killed(body) {
                Ok((_, victim, _)) => {
                    if let Some(o) = self.objects.get_mut(&victim) {
                        o.health = Some(0.0);
                    }
                    Applied::Health
                }
                Err(_) => Applied::Failed,
            },
            // A velocity without a position: prediction takes it.
            opcode::VECTOR_UPDATE => match object::VectorUpdate::parse(body) {
                Ok(v) => {
                    let Some(o) = self.objects.get_mut(&v.guid) else {
                        return Applied::Ignored;
                    };
                    o.velocity = v.velocity;
                    self.generation += 1;
                    Applied::Moved
                }
                Err(e) => {
                    tracing::warn!("VectorUpdate: {e}");
                    Applied::Failed
                }
            },
            // Something in view took an item in hand: another player's
            // weapon, an arrow nocked before a shot, a creature's ammo.
            opcode::PARENT_EVENT => match object::ParentEvent::parse(body) {
                Ok(ev) => {
                    let Some(o) = self.objects.get_mut(&ev.child) else {
                        return Applied::Ignored;
                    };
                    o.wielder = Some(ev.parent);
                    o.parent = Some(ev.parent);
                    o.container = None;
                    o.position = None;
                    o.display = None;
                    self.generation += 1;
                    Applied::Inventory
                }
                Err(e) => {
                    tracing::warn!("ParentEvent: {e}");
                    Applied::Failed
                }
            },
            // An item left the ground (picked up by someone, or a
            // wielded item being introduced); its owner follows in an
            // instance id update or a ParentEvent.
            opcode::PICKUP_EVENT => match object::parse_pickup_event(body) {
                Ok((guid, _, _)) => {
                    let Some(o) = self.objects.get_mut(&guid) else {
                        return Applied::Ignored;
                    };
                    o.position = None;
                    o.display = None;
                    o.target = None;
                    self.generation += 1;
                    Applied::Inventory
                }
                Err(_) => Applied::Failed,
            },
            opcode::PLAY_EFFECT => match messages::parse_play_effect(body) {
                Ok((guid, script, _)) => Applied::Effect { guid, script },
                Err(_) => Applied::Failed,
            },
            opcode::OBJ_DESC_EVENT => match object::ObjDescEvent::parse(body) {
                Ok(ev) => {
                    if let Some(o) = self.objects.get_mut(&ev.guid) {
                        o.palette_id = ev.desc.palette_id;
                        o.sub_palettes = ev.desc.sub_palettes;
                        o.texture_changes = ev.desc.texture_changes;
                        o.anim_part_changes = ev.desc.anim_part_changes;
                        self.generation += 1;
                        Applied::Appearance
                    } else {
                        Applied::Ignored
                    }
                }
                Err(e) => {
                    tracing::warn!("ObjDescEvent: {e}");
                    Applied::Failed
                }
            },
            opcode::PUBLIC_UPDATE_INSTANCE_ID => {
                let mut r = Reader::new(body);
                let _seq = r.u8();
                match (r.u32(), r.u32(), r.u32()) {
                    (Ok(guid), Ok(prop), Ok(value)) => {
                        if let Some(o) = self.objects.get_mut(&guid) {
                            let v = (value != 0).then_some(value);
                            match prop {
                                2 => o.container = v,
                                3 => o.wielder = v,
                                _ => {}
                            }
                            o.parent = o.wielder.or(o.container);
                            self.generation += 1;
                        }
                        Applied::Inventory
                    }
                    _ => Applied::Failed,
                }
            }
            _ => self.apply_stats(op, body),
        }
    }

    /// Hand a message to the character sheet and report what it changed.
    fn apply_stats(&mut self, op: u32, body: &[u8]) -> Applied {
        use stats::StatsApplied;
        match self.stats.apply(op, body) {
            Some(StatsApplied::Stats) => Applied::Stats,
            Some(StatsApplied::Spellbook { spell, known }) => {
                self.generation += 1;
                Applied::Spellbook { spell, known }
            }
            Some(StatsApplied::Enchantments) => {
                self.generation += 1;
                Applied::Enchantments
            }
            None => Applied::Ignored,
        }
    }

    /// Advance smoothing and local prediction by `dt` seconds. Returns true
    /// if any drawn object moved.
    pub fn tick(&mut self, dt: f32) -> bool {
        let mut moved = false;
        let player = self.player_guid;
        for o in self.objects.values_mut() {
            if Some(o.guid) == player {
                continue;
            }
            let Some(mut pos) = o.position else { continue };
            // Predict move-to-position: walk toward the target at the
            // motion's speed until the server says otherwise.
            if let Some(MoveTarget::Position { cell, local }) = o.target {
                let here = landblock_origin(pos.cell) + pos.local;
                let there = landblock_origin(cell) + local;
                let d = there - here;
                let flat = Vec3::new(d.x, d.y, 0.0);
                let dist = flat.length();
                if dist > 0.05 {
                    // Events carry only the low 16 bits of a motion command.
                    let running =
                        o.motion.forward & 0xFFFF == object::motion_cmd::RUN_FORWARD & 0xFFFF;
                    let base = if running { 6.0 } else { 2.5 };
                    let speed = base * o.motion.run_rate.max(0.1);
                    let step = (speed * dt).min(dist);
                    let dir = flat / dist;
                    let next = here + dir * step + Vec3::new(0.0, 0.0, d.z * (step / dist));
                    pos.local = next - landblock_origin(pos.cell);
                    pos.rotation = heading_quat_from_dir(dir);
                    o.position = Some(pos);
                } else {
                    o.target = None;
                    o.motion.forward = object::motion_cmd::READY;
                }
            }
            // Ease the display toward the authoritative position.
            let disp = o.display.get_or_insert(pos);
            let a = landblock_origin(disp.cell) + disp.local;
            let b = landblock_origin(pos.cell) + pos.local;
            let gap = b - a;
            let dist = gap.length();
            if dist > 0.001 {
                // Snap for teleports, ease otherwise (reach the target in ~0.25 s).
                let t = if dist > 15.0 {
                    1.0
                } else {
                    (dt / 0.25).min(1.0)
                };
                let np = a + gap * t;
                *disp = Position {
                    cell: pos.cell,
                    local: np - landblock_origin(pos.cell),
                    rotation: disp.rotation.slerp(pos.rotation, t),
                };
                moved = true;
            } else if disp.rotation != pos.rotation {
                disp.rotation = disp.rotation.slerp(pos.rotation, (dt / 0.25).min(1.0));
                moved = true;
            }
        }
        if moved {
            self.generation += 1;
        }
        moved
    }

    /// Objects that have a world position and a model.
    pub fn drawable(&self) -> impl Iterator<Item = &WorldObject> {
        self.objects
            .values()
            .filter(|o| o.position.is_some() && o.setup_id != 0 && !o.no_draw && o.parent.is_none())
    }
}

/// A secure trade with another player (ACE `Player_Trade`): both sides
/// offer items, both accept, the server swaps them. Any change to an
/// offer clears both acceptances; entering combat or walking away ends
/// it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trade {
    pub partner: u32,
    /// Who asked (from RegisterTrade), echoed back in AcceptTrade.
    pub initiator: u32,
    /// The server's trade timestamp (RegisterTrade's i64), echoed back.
    pub stamp: i64,
    /// Items we put in the window.
    pub mine: Vec<u32>,
    /// Items the partner put in (created for us as objects).
    pub theirs: Vec<u32>,
    pub i_accepted: bool,
    pub they_accepted: bool,
    /// Last failure from the server (item, WeenieError), for the panel.
    pub failure: Option<(u32, u32)>,
}

/// One member of the fellowship (ACE `Fellow`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fellow {
    pub guid: u32,
    pub name: String,
    pub level: u32,
    pub health: (u32, u32),
    pub stamina: (u32, u32),
    pub mana: (u32, u32),
    pub share_loot: bool,
}

/// A fellowship: up to nine players sharing XP and, optionally, loot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fellowship {
    pub name: String,
    pub leader: u32,
    pub share_xp: bool,
    pub even_share: bool,
    pub open: bool,
    pub locked: bool,
    pub members: Vec<Fellow>,
}

/// A yes/no question from the server (ConfirmationRequest 0x0274):
/// `kind` is ACE `ConfirmationType` (1 swear allegiance, 2 alter skill,
/// 3 alter attribute, 4 fellowship, 5 craft, 6 augmentation, 7 yes/no).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmation {
    pub kind: u32,
    pub context: u32,
    pub text: String,
}

fn parse_fellow(r: &mut Reader) -> Option<Fellow> {
    let guid = r.u32().ok()?;
    let _cp = r.u32().ok()?;
    let _lum = r.u32().ok()?;
    let level = r.u32().ok()?;
    let hmax = r.u32().ok()?;
    let smax = r.u32().ok()?;
    let mmax = r.u32().ok()?;
    let h = r.u32().ok()?;
    let st = r.u32().ok()?;
    let m = r.u32().ok()?;
    let share = r.u32().ok()?;
    let name = r.string16().ok()?;
    Some(Fellow {
        guid,
        name,
        level,
        health: (h, hmax),
        stamina: (st, smax),
        mana: (m, mmax),
        share_loot: share != 0,
    })
}

/// Convenience for callers building a camera.
pub fn quat_forward(q: Quat) -> Vec3 {
    q * Vec3::Y
}

/// Client heading (degrees, 0 = north, clockwise) to an orientation.
pub fn heading_quat(deg: f32) -> Quat {
    Quat::from_rotation_z(-deg.to_radians())
}

/// Orientation facing a horizontal direction.
pub fn heading_quat_from_dir(dir: Vec3) -> Quat {
    Quat::from_rotation_z((-dir.x).atan2(dir.y))
}

#[cfg(test)]
mod coord_tests {
    use super::*;

    #[test]
    fn holtburg_lifestone_reads_like_retail() {
        // The Holtburg lifestone sits at 0xA9B40019 (81.3, 11.8), which
        // ACE reports as 42.0N, 33.5E.
        let p = object::Position::new_flat(0xA9B4_0019, Vec3::new(81.33, 11.8, 94.0));
        assert_eq!(map_coord_str(&p), "42.0N, 33.5E");
        let south_west = object::Position::new_flat(0x0101_0001, Vec3::ZERO);
        assert_eq!(map_coord_str(&south_west), "101.1S, 101.1W");
        let inside = object::Position::new_flat(0x7200_018C, Vec3::ZERO);
        assert_eq!(map_coord_str(&inside), "indoors");
        assert!(map_coords(&inside).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ac_net::wire::Writer;

    const ME: u32 = 0x5000_0001;

    #[test]
    fn normal_echoes_keep_our_prediction() {
        // A run keeps the server a few metres behind but moving with us,
        // so we never snap and movement stays smooth.
        let mut streak = 0u8;
        for _ in 0..20 {
            assert!(!reconcile(5.0, 4.0, false, &mut streak));
            assert_eq!(streak, 0);
        }
    }

    #[test]
    fn standing_still_in_sync_is_left_alone() {
        // Nobody is moving and the server agrees where we are: no gap to
        // see, so we never snap.
        let mut streak = 0u8;
        for _ in 0..10 {
            assert!(!reconcile(0.5, 0.0, false, &mut streak));
            assert_eq!(streak, 0);
        }
    }

    #[test]
    fn a_desync_is_caught_while_we_stand_still() {
        // Stranded and then stopped (being hit where the server thinks we
        // are): nothing moves, but the gap is real and must still resync.
        let mut streak = 0u8;
        assert!(!reconcile(7.0, 0.0, false, &mut streak));
        assert!(!reconcile(7.0, 0.0, false, &mut streak));
        assert!(
            reconcile(7.0, 0.0, false, &mut streak),
            "a standing desync still resyncs"
        );
    }

    #[test]
    fn a_refused_move_resyncs_even_when_the_gap_is_small() {
        // A wall the server will not let us through: it holds still a few
        // metres away while we keep going. Three echoes confirm it.
        let mut streak = 0u8;
        assert!(!reconcile(4.0, 0.1, false, &mut streak));
        assert!(!reconcile(6.0, 0.1, false, &mut streak));
        assert!(
            reconcile(9.0, 0.1, false, &mut streak),
            "third echo resyncs"
        );
        assert_eq!(streak, 0, "streak clears after the snap");
    }

    #[test]
    fn a_single_late_packet_is_not_mistaken_for_a_desync() {
        // One echo looks stuck, then the server catches up: no snap.
        let mut streak = 0u8;
        assert!(!reconcile(18.0, 0.5, false, &mut streak));
        assert_eq!(streak, 1);
        assert!(!reconcile(4.0, 14.0, false, &mut streak));
        assert_eq!(streak, 0);
    }

    #[test]
    fn a_teleport_or_a_huge_gap_snaps_at_once() {
        let mut streak = 0u8;
        assert!(
            reconcile(3.0, 3.0, true, &mut streak),
            "server teleport is taken"
        );
        let mut streak = 0u8;
        assert!(
            reconcile(80.0, 0.0, false, &mut streak),
            "a huge gap snaps at once"
        );
    }

    /// A whole GameEvent message: opcode, our guid, sequence, event, body.
    fn game_event(ev: u32, rest: &[u8]) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32(opcode::GAME_EVENT).u32(ME).u32(1).u32(ev).bytes(rest);
        w.finish()
    }

    fn world_with_stack(guid: u32, stack: u32) -> World {
        let mut world = World {
            player_guid: Some(ME),
            ..Default::default()
        };
        world.objects.insert(
            guid,
            WorldObject {
                guid,
                name: "Lead Scarab".into(),
                weenie_class_id: 691,
                stack_size: stack,
                value: stack * 5,
                container: Some(ME),
                parent: Some(ME),
                ..Default::default()
            },
        );
        world
    }

    #[test]
    fn stack_size_updates_reach_pack_items() {
        let mut world = world_with_stack(0x8000_0010, 10);
        let gen = world.generation;
        // SetStackSize: sequence, guid, stack, value.
        let mut w = Writer::new();
        w.u32(opcode::SET_STACK_SIZE)
            .u8(3)
            .u32(0x8000_0010)
            .u32(9)
            .u32(45);
        assert_eq!(world.apply(&w.finish()), Applied::Inventory);
        let o = &world.objects[&0x8000_0010];
        assert_eq!((o.stack_size, o.value), (9, 45));
        assert!(world.generation > gen);
        // PublicUpdatePropertyInt StackSize on the same item.
        let mut w = Writer::new();
        w.u32(opcode::PUBLIC_UPDATE_PROPERTY_INT)
            .u8(4)
            .u32(0x8000_0010)
            .u32(messages::property_int::STACK_SIZE)
            .i32(7);
        assert_eq!(world.apply(&w.finish()), Applied::Inventory);
        assert_eq!(world.objects[&0x8000_0010].stack_size, 7);
        // Other properties and unknown objects are ignored.
        let mut w = Writer::new();
        w.u32(opcode::PUBLIC_UPDATE_PROPERTY_INT)
            .u8(5)
            .u32(0x8000_0010)
            .u32(99)
            .i32(1);
        assert_eq!(world.apply(&w.finish()), Applied::Ignored);
        let mut w = Writer::new();
        w.u32(opcode::SET_STACK_SIZE)
            .u8(6)
            .u32(0x8000_0099)
            .u32(1)
            .u32(1);
        assert_eq!(world.apply(&w.finish()), Applied::Ignored);
        assert_eq!(world.inventory().count(), 1);
    }

    #[test]
    fn spellbook_and_enchantment_events_bump_generation() {
        let mut world = World::default();
        let gen = world.generation;
        let mut w = Writer::new();
        w.u16(2091).u16(0);
        assert_eq!(
            world.apply(&game_event(event::MAGIC_UPDATE_SPELL, &w.finish())),
            Applied::Spellbook {
                spell: 2091,
                known: true
            }
        );
        assert_eq!(world.stats.spells, vec![2091]);
        assert!(world.generation > gen);
        let mut w = Writer::new();
        w.u16(2091).u16(0);
        assert_eq!(
            world.apply(&game_event(event::MAGIC_REMOVE_SPELL, &w.finish())),
            Applied::Spellbook {
                spell: 2091,
                known: false
            }
        );
        assert!(world.stats.spells.is_empty());
        // A dispel of nothing is still an enchantment event.
        let mut w = Writer::new();
        w.u16(1).u16(1);
        assert_eq!(
            world.apply(&game_event(event::MAGIC_DISPEL_ENCHANTMENT, &w.finish())),
            Applied::Enchantments
        );
        assert_eq!(
            world.apply(&game_event(event::MAGIC_PURGE_ENCHANTMENTS, &[])),
            Applied::Enchantments
        );
        assert_eq!(
            world.apply(&game_event(event::USE_DONE, &[0, 0, 0, 0])),
            Applied::Ignored
        );
    }

    /// A world with our player and one creature in view.
    fn world_with_creature(guid: u32) -> World {
        let mut world = World {
            player_guid: Some(ME),
            ..Default::default()
        };
        world.objects.insert(
            ME,
            WorldObject {
                guid: ME,
                name: "Me".into(),
                is_player: true,
                motion_table_id: 0x0900_0001,
                ..Default::default()
            },
        );
        world.objects.insert(
            guid,
            WorldObject {
                guid,
                name: "Drudge Skulker".into(),
                item_type: item_type::CREATURE,
                object_desc_flags: object_desc_flags::ATTACKABLE,
                position: Some(object::Position::new_flat(0xA9B4_0019, Vec3::ZERO)),
                health: Some(0.5),
                ..Default::default()
            },
        );
        world
    }

    #[test]
    fn public_property_updates_apply_to_objects() {
        let mut world = world_with_stack(0x8000_0010, 3);
        // CurrentWieldedLocation comes with the wield/unwield events.
        let mut w = Writer::new();
        w.u32(opcode::PUBLIC_UPDATE_PROPERTY_INT)
            .u8(1)
            .u32(0x8000_0010)
            .u32(object::property_int::CURRENT_WIELDED_LOCATION)
            .i32(0x0010_0000);
        assert_eq!(world.apply(&w.finish()), Applied::Inventory);
        assert_eq!(world.objects[&0x8000_0010].wielded_location, 0x0010_0000);
        // Structure: uses left on a kit.
        let mut w = Writer::new();
        w.u32(opcode::PUBLIC_UPDATE_PROPERTY_INT)
            .u8(2)
            .u32(0x8000_0010)
            .u32(object::property_int::STRUCTURE)
            .i32(4);
        assert_eq!(world.apply(&w.finish()), Applied::Inventory);
        assert_eq!(world.objects[&0x8000_0010].structure, 4);
        // PlayerKillerStatus flips the description flags like ACE.
        world.objects.insert(
            ME,
            WorldObject {
                guid: ME,
                name: "Me".into(),
                is_player: true,
                object_desc_flags: object_desc_flags::PLAYER,
                ..Default::default()
            },
        );
        let mut w = Writer::new();
        w.u32(opcode::PUBLIC_UPDATE_PROPERTY_INT)
            .u8(3)
            .u32(ME)
            .u32(object::property_int::PLAYER_KILLER_STATUS)
            .i32(object::pk_status::PK as i32);
        assert_eq!(world.apply(&w.finish()), Applied::Appearance);
        let me = &world.objects[&ME];
        assert_eq!(me.pk_status, object::pk_status::PK);
        assert_ne!(me.object_desc_flags & object_desc_flags::PLAYER_KILLER, 0);
        let mut w = Writer::new();
        w.u32(opcode::PUBLIC_UPDATE_PROPERTY_INT)
            .u8(4)
            .u32(ME)
            .u32(object::property_int::PLAYER_KILLER_STATUS)
            .i32(object::pk_status::NPK as i32);
        assert_eq!(world.apply(&w.finish()), Applied::Appearance);
        assert_eq!(
            world.objects[&ME].object_desc_flags & object_desc_flags::PLAYER_KILLER,
            0
        );
        // Locked: the chest cannot be opened until it is unlocked.
        world.objects.insert(
            0x8000_0020,
            WorldObject {
                guid: 0x8000_0020,
                name: "Chest".into(),
                object_desc_flags: object_desc_flags::OPENABLE | object_desc_flags::STUCK,
                ..Default::default()
            },
        );
        let mut w = Writer::new();
        w.u32(opcode::PUBLIC_UPDATE_PROPERTY_BOOL)
            .u8(1)
            .u32(0x8000_0020)
            .u32(object::property_bool::LOCKED)
            .u32(1);
        assert_eq!(world.apply(&w.finish()), Applied::Inventory);
        let chest = &world.objects[&0x8000_0020];
        assert_eq!(chest.locked, Some(true));
        assert_eq!(chest.object_desc_flags & object_desc_flags::OPENABLE, 0);
        let mut w = Writer::new();
        w.u32(opcode::PUBLIC_UPDATE_PROPERTY_BOOL)
            .u8(2)
            .u32(0x8000_0020)
            .u32(object::property_bool::LOCKED)
            .u32(0);
        assert_eq!(world.apply(&w.finish()), Applied::Inventory);
        let chest = &world.objects[&0x8000_0020];
        assert_eq!(chest.locked, Some(false));
        assert_ne!(chest.object_desc_flags & object_desc_flags::OPENABLE, 0);
        // A rename: key before guid, then the aligned string.
        let mut w = Writer::new();
        w.u32(opcode::PUBLIC_UPDATE_PROPERTY_STRING)
            .u8(1)
            .u32(messages::property_string::NAME)
            .u32(0x8000_0020)
            .align4()
            .string16("Old Chest");
        assert_eq!(world.apply(&w.finish()), Applied::Appearance);
        assert_eq!(world.objects[&0x8000_0020].name, "Old Chest");
        // A motion table swap on an object in view.
        let mut w = Writer::new();
        w.u32(opcode::PUBLIC_UPDATE_PROPERTY_DATA_ID)
            .u8(1)
            .u32(ME)
            .u32(object::property_did::MOTION_TABLE)
            .u32(0x0900_020D);
        assert_eq!(world.apply(&w.finish()), Applied::Appearance);
        assert_eq!(world.objects[&ME].motion_table_id, 0x0900_020D);
        // Floats and int64s parse and change nothing; a short body fails.
        let mut w = Writer::new();
        w.u32(opcode::PUBLIC_UPDATE_PROPERTY_FLOAT)
            .u8(1)
            .u32(ME)
            .u32(7)
            .f64(1.5);
        assert_eq!(world.apply(&w.finish()), Applied::Ignored);
        let mut w = Writer::new();
        w.u32(opcode::PUBLIC_UPDATE_PROPERTY_INT64).u8(1).u32(ME);
        assert_eq!(world.apply(&w.finish()), Applied::Failed);
        // Unknown objects are ignored.
        let mut w = Writer::new();
        w.u32(opcode::PUBLIC_UPDATE_PROPERTY_BOOL)
            .u8(3)
            .u32(0x8000_0099)
            .u32(object::property_bool::LOCKED)
            .u32(1);
        assert_eq!(world.apply(&w.finish()), Applied::Ignored);
    }

    #[test]
    fn private_property_updates_reach_the_character_sheet() {
        let mut world = world_with_creature(0x8000_0030);
        // SpellComponentsRequired off: the server will not want comps.
        let mut w = Writer::new();
        w.u32(opcode::PRIVATE_UPDATE_PROPERTY_BOOL)
            .u8(1)
            .u32(object::property_bool::SPELL_COMPONENTS_REQUIRED)
            .u32(0);
        assert_eq!(world.apply(&w.finish()), Applied::Stats);
        assert_eq!(world.stats.spell_components_required(), Some(false));
        // CurrentAttacker names what hit us; 0 clears it.
        let mut w = Writer::new();
        w.u32(opcode::PRIVATE_UPDATE_INSTANCE_ID)
            .u8(1)
            .u32(object::property_iid::CURRENT_ATTACKER)
            .u32(0x8000_0030);
        assert_eq!(world.apply(&w.finish()), Applied::Stats);
        assert_eq!(world.stats.current_attacker(), Some(0x8000_0030));
        let mut w = Writer::new();
        w.u32(opcode::PRIVATE_UPDATE_INSTANCE_ID)
            .u8(2)
            .u32(object::property_iid::CURRENT_ATTACKER)
            .u32(0);
        assert_eq!(world.apply(&w.finish()), Applied::Stats);
        assert_eq!(world.stats.current_attacker(), None);
        // A float of our own is kept by id.
        let mut w = Writer::new();
        w.u32(opcode::PRIVATE_UPDATE_PROPERTY_FLOAT)
            .u8(1)
            .u32(0x2B)
            .f64(0.25);
        assert_eq!(world.apply(&w.finish()), Applied::Stats);
        assert_eq!(world.stats.float_prop(0x2B), Some(0.25));
        // Our motion table changed: the player object animates from it.
        let gen = world.generation;
        let mut w = Writer::new();
        w.u32(opcode::PRIVATE_UPDATE_PROPERTY_DATA_ID)
            .u8(1)
            .u32(object::property_did::MOTION_TABLE)
            .u32(0x0900_020E);
        assert_eq!(world.apply(&w.finish()), Applied::Stats);
        assert_eq!(world.player().unwrap().motion_table_id, 0x0900_020E);
        assert!(world
            .stats
            .dids
            .contains(&(object::property_did::MOTION_TABLE, 0x0900_020E)));
        assert!(world.generation > gen);
    }

    #[test]
    fn deaths_vectors_parents_pickups_and_effects() {
        let target = 0x8000_0030;
        let mut world = world_with_creature(target);
        // PlayerKilled: the victim's health is gone.
        let mut w = Writer::new();
        w.u32(opcode::PLAYER_KILLED)
            .string16("Drudge Skulker is killed by Me!")
            .u32(target)
            .u32(ME);
        assert_eq!(world.apply(&w.finish()), Applied::Health);
        assert_eq!(world.objects[&target].health, Some(0.0));
        // VectorUpdate: the velocity feeds prediction.
        let mut w = Writer::new();
        w.u32(opcode::VECTOR_UPDATE)
            .u32(target)
            .f32(1.0)
            .f32(2.0)
            .f32(0.5)
            .f32(0.0)
            .f32(0.0)
            .f32(0.0)
            .u16(1)
            .u16(7);
        assert_eq!(world.apply(&w.finish()), Applied::Moved);
        assert_eq!(world.objects[&target].velocity, Vec3::new(1.0, 2.0, 0.5));
        // ParentEvent: an arrow in the creature's hand is no longer in
        // the world.
        let arrow = 0x8000_0031;
        world.objects.insert(
            arrow,
            WorldObject {
                guid: arrow,
                name: "Arrow".into(),
                position: Some(object::Position::new_flat(0xA9B4_0019, Vec3::ZERO)),
                ..Default::default()
            },
        );
        let mut w = Writer::new();
        w.u32(opcode::PARENT_EVENT)
            .u32(target)
            .u32(arrow)
            .u32(1)
            .u32(0x4A)
            .u16(1)
            .u16(3);
        assert_eq!(world.apply(&w.finish()), Applied::Inventory);
        let a = &world.objects[&arrow];
        assert_eq!(
            (a.wielder, a.parent, a.position),
            (Some(target), Some(target), None)
        );
        // PickupEvent: something left the ground.
        let coin = 0x8000_0032;
        world.objects.insert(
            coin,
            WorldObject {
                guid: coin,
                name: "Pyreal".into(),
                position: Some(object::Position::new_flat(0xA9B4_0019, Vec3::ZERO)),
                ..Default::default()
            },
        );
        let mut w = Writer::new();
        w.u32(opcode::PICKUP_EVENT).u32(coin).u16(1).u16(2);
        assert_eq!(world.apply(&w.finish()), Applied::Inventory);
        assert_eq!(world.objects[&coin].position, None);
        // PlayEffect is reported with its script.
        let mut w = Writer::new();
        w.u32(opcode::PLAY_EFFECT).u32(target).u32(0x51).f32(1.0);
        assert_eq!(
            world.apply(&w.finish()),
            Applied::Effect {
                guid: target,
                script: 0x51
            }
        );
        // QueryItemManaResponse fills the item's mana.
        let mut w = Writer::new();
        w.u32(arrow).f32(0.75).u32(1);
        assert_eq!(
            world.apply(&game_event(event::QUERY_ITEM_MANA_RESPONSE, &w.finish())),
            Applied::Inventory
        );
        assert_eq!(world.objects[&arrow].mana, Some(0.75));
        // Short bodies fail rather than panic.
        assert_eq!(world.apply(&[0x4E, 0xF7, 0, 0, 1, 2]), Applied::Failed);
        assert_eq!(world.apply(&[0x9E, 0x01, 0, 0, 5, 0]), Applied::Failed);
    }

    #[test]
    fn usable_words() {
        // Handy Healing Kit: SourceContainedTargetRemoteOrSelf.
        assert!(usable::needs_target(0x22_0008));
        assert!(usable::on_self(0x22_0008));
        // Mana Stone: SourceContainedTargetSelfOrContained.
        assert!(usable::needs_target(0xA_0008));
        // A key: contained, target remote.
        assert!(usable::needs_target(0x20_0008));
        assert!(!usable::on_self(0x20_0008));
        // A book, a sign, a pyreal: no target.
        assert!(!usable::needs_target(8));
        assert!(!usable::needs_target(1));
        assert!(!usable::needs_target(0));
    }
}
