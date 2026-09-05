//! Parsers for the object messages. Layouts follow ACE's
//! `WorldObject_Networking.SerializeCreateObject` and `PositionPack`.

use ac_net::wire::{Reader, Truncated};
use glam::{Quat, Vec3};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Truncated(#[from] Truncated),
    #[error("unsupported field: {0}")]
    Unsupported(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A cell id plus a frame local to that cell's landblock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub cell: u32,
    pub local: Vec3,
    pub rotation: Quat,
}

impl Position {
    /// `u32 cell, f32 x y z, f32 qw qx qy qz`.
    pub fn parse(r: &mut Reader) -> Result<Self> {
        let cell = r.u32()?;
        let local = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        let w = r.f32()?;
        let x = r.f32()?;
        let y = r.f32()?;
        let z = r.f32()?;
        Ok(Position {
            cell,
            local,
            rotation: Quat::from_xyzw(x, y, z, w).normalize(),
        })
    }

    pub fn landblock(&self) -> u32 {
        self.cell & 0xFFFF_0000
    }

    pub fn is_indoors(&self) -> bool {
        (self.cell & 0xFFFF) >= 0x100
    }
}

pub mod physics_flags {
    pub const CSETUP: u32 = 0x1;
    pub const MTABLE: u32 = 0x2;
    pub const VELOCITY: u32 = 0x4;
    pub const ACCELERATION: u32 = 0x8;
    pub const OMEGA: u32 = 0x10;
    pub const PARENT: u32 = 0x20;
    pub const CHILDREN: u32 = 0x40;
    pub const OBJ_SCALE: u32 = 0x80;
    pub const FRICTION: u32 = 0x100;
    pub const ELASTICITY: u32 = 0x200;
    pub const TIMESTAMPS: u32 = 0x400;
    pub const STABLE: u32 = 0x800;
    pub const PETABLE: u32 = 0x1000;
    pub const DEFAULT_SCRIPT: u32 = 0x2000;
    pub const DEFAULT_SCRIPT_INTENSITY: u32 = 0x4000;
    pub const POSITION: u32 = 0x8000;
    pub const MOVEMENT: u32 = 0x10000;
    pub const ANIMATION_FRAME: u32 = 0x20000;
    pub const TRANSLUCENCY: u32 = 0x40000;
}

/// `PhysicsState` bit that hides an object.
pub const PHYSICS_STATE_NO_DRAW: u32 = 0x20;
/// `PhysicsState` bit for hidden (admin-invisible) objects.
pub const PHYSICS_STATE_HIDDEN: u32 = 0x4000;

pub mod weenie_flags {
    pub const PLURAL_NAME: u32 = 0x1;
    pub const ITEMS_CAPACITY: u32 = 0x2;
    pub const CONTAINERS_CAPACITY: u32 = 0x4;
    pub const VALUE: u32 = 0x8;
    pub const USABLE: u32 = 0x10;
    pub const USE_RADIUS: u32 = 0x20;
    pub const MONARCH: u32 = 0x40;
    pub const UI_EFFECTS: u32 = 0x80;
    pub const AMMO_TYPE: u32 = 0x100;
    pub const COMBAT_USE: u32 = 0x200;
    pub const STRUCTURE: u32 = 0x400;
    pub const MAX_STRUCTURE: u32 = 0x800;
    pub const STACK_SIZE: u32 = 0x1000;
    pub const MAX_STACK_SIZE: u32 = 0x2000;
    pub const CONTAINER: u32 = 0x4000;
    pub const WIELDER: u32 = 0x8000;
    pub const VALID_LOCATIONS: u32 = 0x10000;
    pub const CURRENTLY_WIELDED_LOCATION: u32 = 0x20000;
    pub const PRIORITY: u32 = 0x40000;
    pub const TARGET_TYPE: u32 = 0x80000;
    pub const RADAR_BLIP_COLOR: u32 = 0x100000;
    pub const BURDEN: u32 = 0x200000;
    pub const SPELL: u32 = 0x400000;
    pub const RADAR_BEHAVIOR: u32 = 0x800000;
    pub const WORKMANSHIP: u32 = 0x1000000;
    pub const HOUSE_OWNER: u32 = 0x2000000;
    pub const HOUSE_RESTRICTIONS: u32 = 0x4000000;
    pub const PSCRIPT: u32 = 0x8000000;
    pub const HOOK_TYPE: u32 = 0x10000000;
    pub const HOOK_ITEM_TYPES: u32 = 0x20000000;
    pub const ICON_OVERLAY: u32 = 0x40000000;
    pub const MATERIAL_TYPE: u32 = 0x80000000;
    pub const INCLUDES_SECOND_HEADER: u32 = 0x0400_0000; // in ObjectDescriptionFlag
    pub const F2_ICON_UNDERLAY: u32 = 0x1;
    pub const F2_COOLDOWN: u32 = 0x2;
    pub const F2_COOLDOWN_DURATION: u32 = 0x4;
    pub const F2_PET_OWNER: u32 = 0x8;
}

/// Read a "packed dword of known type": the value is stored without the
/// type's high bits and re-added here.
fn packed_of_type(r: &mut Reader, ty: u32) -> Result<u32> {
    let v = r.packed_u32()?;
    Ok(if v != 0 && v & ty == 0 { v + ty } else { v })
}

#[derive(Debug, Clone)]
pub struct ObjectCreate {
    pub guid: u32,
    pub palette_id: u32,
    /// (sub palette id, offset/8, length/8)
    pub sub_palettes: Vec<(u32, u8, u8)>,
    pub texture_changes: Vec<(u8, u32, u32)>,
    pub anim_part_changes: Vec<(u8, u32)>,
    pub physics_flags: u32,
    pub physics_state: u32,
    pub placement: Option<u32>,
    pub position: Option<Position>,
    pub motion_table_id: u32,
    pub sound_table_id: u32,
    pub physics_table_id: u32,
    pub setup_id: u32,
    pub parent: Option<u32>,
    pub parent_location: u32,
    pub children: Vec<(u32, u32)>,
    pub scale: f32,
    pub translucency: f32,
    pub velocity: Vec3,
    pub name: String,
    pub weenie_class_id: u32,
    pub icon_id: u32,
    /// RenderSurface (0x06) drawn over the icon, or 0.
    pub icon_overlay: u32,
    /// RenderSurface (0x06) drawn under the icon, or 0.
    pub icon_underlay: u32,
    pub item_type: u32,
    pub object_desc_flags: u32,
    pub weenie_flags: u32,
    pub container: Option<u32>,
    pub wielder: Option<u32>,
    pub stack_size: u32,
    /// Base value in pyreals (vendor prices scale it).
    pub value: u32,
    pub spell_id: u32,
    /// EquipMask bits the item can be wielded in.
    pub valid_locations: u32,
    pub wielded_location: u32,
    pub no_draw: bool,
}

/// The ObjDesc block: palette, texture and part swaps that dress a model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObjDesc {
    pub palette_id: u32,
    pub sub_palettes: Vec<(u32, u8, u8)>,
    pub texture_changes: Vec<(u8, u32, u32)>,
    pub anim_part_changes: Vec<(u8, u32)>,
}

impl ObjDesc {
    pub fn parse(r: &mut Reader) -> Result<Self> {
        let _eleven = r.u8()?;
        let n_pal = r.u8()? as usize;
        let n_tex = r.u8()? as usize;
        let n_parts = r.u8()? as usize;
        let palette_id = if n_pal > 0 {
            packed_of_type(r, 0x0400_0000)?
        } else {
            0
        };
        let mut sub_palettes = Vec::with_capacity(n_pal);
        for _ in 0..n_pal {
            sub_palettes.push((packed_of_type(r, 0x0400_0000)?, r.u8()?, r.u8()?));
        }
        let mut texture_changes = Vec::with_capacity(n_tex);
        for _ in 0..n_tex {
            texture_changes.push((
                r.u8()?,
                packed_of_type(r, 0x0500_0000)?,
                packed_of_type(r, 0x0500_0000)?,
            ));
        }
        let mut anim_part_changes = Vec::with_capacity(n_parts);
        for _ in 0..n_parts {
            anim_part_changes.push((r.u8()?, packed_of_type(r, 0x0100_0000)?));
        }
        r.align4()?;
        Ok(ObjDesc {
            palette_id,
            sub_palettes,
            texture_changes,
            anim_part_changes,
        })
    }
}

/// ObjDescEvent (0xF625): an object's new look after (un)equipping.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjDescEvent {
    pub guid: u32,
    pub desc: ObjDesc,
    pub instance_seq: u16,
    pub visual_seq: u16,
}

impl ObjDescEvent {
    pub fn parse(body: &[u8]) -> Result<Self> {
        let mut r = Reader::new(body);
        let guid = r.u32()?;
        let desc = ObjDesc::parse(&mut r)?;
        Ok(ObjDescEvent {
            guid,
            desc,
            instance_seq: r.u16()?,
            visual_seq: r.u16()?,
        })
    }
}

/// The weenie header: the game-data half of an ObjectCreate, also sent on
/// its own for vendor stock (SerializeGameDataOnly).
#[derive(Debug, Clone, PartialEq)]
pub struct WeenieDesc {
    pub name: String,
    pub weenie_class_id: u32,
    pub icon_id: u32,
    pub item_type: u32,
    pub object_desc_flags: u32,
    pub weenie_flags: u32,
    pub value: u32,
    pub stack_size: u32,
    pub container: Option<u32>,
    pub wielder: Option<u32>,
    pub valid_locations: u32,
    pub wielded_location: u32,
    pub icon_overlay: u32,
    pub icon_underlay: u32,
    /// Spell a scroll teaches / an item casts, or 0.
    pub spell_id: u32,
}

impl WeenieDesc {
    pub fn parse(r: &mut Reader) -> Result<Self> {
        use weenie_flags::*;
        let weenie_flags = r.u32()?;
        let name = r.string16()?;
        let weenie_class_id = r.packed_u32()?;
        let icon_id = packed_of_type(r, 0x0600_0000)?;
        let item_type = r.u32()?;
        let object_desc_flags = r.u32()?;
        r.align4()?;
        let weenie_flags2 = if object_desc_flags & INCLUDES_SECOND_HEADER != 0 {
            r.u32()?
        } else {
            0
        };
        if weenie_flags & PLURAL_NAME != 0 {
            r.string16()?;
        }
        if weenie_flags & ITEMS_CAPACITY != 0 {
            r.u8()?;
        }
        if weenie_flags & CONTAINERS_CAPACITY != 0 {
            r.u8()?;
        }
        if weenie_flags & AMMO_TYPE != 0 {
            r.u16()?;
        }
        let value = if weenie_flags & VALUE != 0 {
            r.u32()?
        } else {
            0
        };
        if weenie_flags & USABLE != 0 {
            r.u32()?;
        }
        if weenie_flags & USE_RADIUS != 0 {
            r.f32()?;
        }
        if weenie_flags & TARGET_TYPE != 0 {
            r.u32()?;
        }
        if weenie_flags & UI_EFFECTS != 0 {
            r.u32()?;
        }
        if weenie_flags & COMBAT_USE != 0 {
            r.u8()?;
        }
        if weenie_flags & STRUCTURE != 0 {
            r.u16()?;
        }
        if weenie_flags & MAX_STRUCTURE != 0 {
            r.u16()?;
        }
        let stack_size = if weenie_flags & STACK_SIZE != 0 {
            r.u16()? as u32
        } else {
            1
        };
        if weenie_flags & MAX_STACK_SIZE != 0 {
            r.u16()?;
        }
        let container = if weenie_flags & CONTAINER != 0 {
            Some(r.u32()?).filter(|&c| c != 0)
        } else {
            None
        };
        let wielder = if weenie_flags & WIELDER != 0 {
            Some(r.u32()?).filter(|&c| c != 0)
        } else {
            None
        };
        let valid_locations = if weenie_flags & VALID_LOCATIONS != 0 {
            r.u32()?
        } else {
            0
        };
        let wielded_location = if weenie_flags & CURRENTLY_WIELDED_LOCATION != 0 {
            r.u32()?
        } else {
            0
        };
        if weenie_flags & PRIORITY != 0 {
            r.u32()?;
        }
        if weenie_flags & RADAR_BLIP_COLOR != 0 {
            r.u8()?;
        }
        if weenie_flags & RADAR_BEHAVIOR != 0 {
            r.u8()?;
        }
        if weenie_flags & PSCRIPT != 0 {
            r.u16()?;
        }
        if weenie_flags & WORKMANSHIP != 0 {
            r.f32()?;
        }
        if weenie_flags & BURDEN != 0 {
            r.u16()?;
        }
        let spell_id = if weenie_flags & SPELL != 0 {
            r.u16()? as u32
        } else {
            0
        };
        if weenie_flags & HOUSE_OWNER != 0 {
            r.u32()?;
        }
        if weenie_flags & HOUSE_RESTRICTIONS != 0 {
            return Err(Error::Unsupported("house restrictions"));
        }
        if weenie_flags & HOOK_ITEM_TYPES != 0 {
            r.u32()?;
        }
        if weenie_flags & MONARCH != 0 {
            r.u32()?;
        }
        if weenie_flags & HOOK_TYPE != 0 {
            r.u16()?;
        }
        let icon_overlay = if weenie_flags & ICON_OVERLAY != 0 {
            packed_of_type(r, 0x0600_0000)?
        } else {
            0
        };
        let icon_underlay = if weenie_flags2 & F2_ICON_UNDERLAY != 0 {
            packed_of_type(r, 0x0600_0000)?
        } else {
            0
        };
        if weenie_flags & MATERIAL_TYPE != 0 {
            r.u32()?;
        }
        if weenie_flags2 & F2_COOLDOWN != 0 {
            r.u32()?;
        }
        if weenie_flags2 & F2_COOLDOWN_DURATION != 0 {
            r.f64()?;
        }
        if weenie_flags2 & F2_PET_OWNER != 0 {
            r.u32()?;
        }

        Ok(WeenieDesc {
            name,
            weenie_class_id,
            icon_id,
            item_type,
            object_desc_flags,
            weenie_flags,
            value,
            stack_size,
            container,
            wielder,
            valid_locations,
            wielded_location,
            icon_overlay,
            icon_underlay,
            spell_id,
        })
    }
}

/// ApproachVendor (game event 0x0062): a vendor's terms and stock.
#[derive(Debug, Clone, PartialEq)]
pub struct VendorItem {
    /// Prototype guid to name in Buy; -1 stack means unlimited supply.
    pub guid: u32,
    pub stack: u32,
    pub desc: WeenieDesc,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApproachVendor {
    pub vendor: u32,
    pub item_types: u32,
    pub min_value: u32,
    pub max_value: u32,
    pub magical: bool,
    /// Price multipliers applied to an item's value: `buy_rate` is what the
    /// vendor pays when buying from us, `sell_rate` what it charges.
    pub buy_rate: f32,
    pub sell_rate: f32,
    pub alt_currency: u32,
    pub alt_amount: u32,
    pub alt_name: String,
    pub items: Vec<VendorItem>,
}

impl ApproachVendor {
    pub fn parse(body: &[u8]) -> Result<Self> {
        let mut r = Reader::new(body);
        let vendor = r.u32()?;
        let item_types = r.u32()?;
        let min_value = r.u32()?;
        let max_value = r.u32()?;
        let magical = r.u32()? != 0;
        let buy_rate = r.f32()?;
        let sell_rate = r.f32()?;
        let alt_currency = r.u32()?;
        let alt_amount = r.u32()?;
        let alt_name = r.string16()?;
        let n = r.u32()? as usize;
        let mut items = Vec::with_capacity(n.min(512));
        for _ in 0..n {
            let packed = r.u32()?;
            let guid = r.u32()?;
            let desc = WeenieDesc::parse(&mut r)?;
            items.push(VendorItem {
                guid,
                stack: packed & 0x00FF_FFFF,
                desc,
            });
        }
        Ok(ApproachVendor {
            vendor,
            item_types,
            min_value,
            max_value,
            magical,
            buy_rate,
            sell_rate,
            alt_currency,
            alt_amount,
            alt_name,
            items,
        })
    }
}

impl ObjectCreate {
    pub fn parse(body: &[u8]) -> Result<Self> {
        let mut r = Reader::new(body);
        let guid = r.u32()?;

        // Model data (ObjDesc).
        let ObjDesc {
            palette_id,
            sub_palettes,
            texture_changes,
            anim_part_changes,
        } = ObjDesc::parse(&mut r)?;

        // Physics data.
        use physics_flags::*;
        let physics_flags = r.u32()?;
        let physics_state = r.u32()?;
        let mut placement = None;
        if physics_flags & MOVEMENT != 0 {
            let len = r.u32()? as usize;
            if len > 0 {
                r.bytes(len)?;
                let _autonomous = r.u32()?;
            }
        } else if physics_flags & ANIMATION_FRAME != 0 {
            placement = Some(r.u32()?);
        }
        let position = if physics_flags & POSITION != 0 {
            Some(Position::parse(&mut r)?)
        } else {
            None
        };
        let motion_table_id = if physics_flags & MTABLE != 0 {
            r.u32()?
        } else {
            0
        };
        let sound_table_id = if physics_flags & STABLE != 0 {
            r.u32()?
        } else {
            0
        };
        let physics_table_id = if physics_flags & PETABLE != 0 {
            r.u32()?
        } else {
            0
        };
        let setup_id = if physics_flags & CSETUP != 0 {
            r.u32()?
        } else {
            0
        };
        let (mut parent, mut parent_location) = (None, 0);
        if physics_flags & PARENT != 0 {
            parent = Some(r.u32()?).filter(|&p| p != 0);
            parent_location = r.u32()?;
        }
        let mut children = Vec::new();
        if physics_flags & CHILDREN != 0 {
            let n = r.u32()?;
            for _ in 0..n {
                children.push((r.u32()?, r.u32()?));
            }
        }
        let scale = if physics_flags & OBJ_SCALE != 0 {
            r.f32()?
        } else {
            1.0
        };
        if physics_flags & FRICTION != 0 {
            r.f32()?;
        }
        if physics_flags & ELASTICITY != 0 {
            r.f32()?;
        }
        let translucency = if physics_flags & TRANSLUCENCY != 0 {
            r.f32()?
        } else {
            0.0
        };
        let mut velocity = Vec3::ZERO;
        if physics_flags & VELOCITY != 0 {
            velocity = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        }
        if physics_flags & ACCELERATION != 0 {
            r.bytes(12)?;
        }
        if physics_flags & OMEGA != 0 {
            r.bytes(12)?;
        }
        if physics_flags & DEFAULT_SCRIPT != 0 {
            r.u32()?;
        }
        if physics_flags & DEFAULT_SCRIPT_INTENSITY != 0 {
            r.f32()?;
        }
        r.bytes(9 * 2)?; // sequences
        r.align4()?;

        // Weenie data.
        let WeenieDesc {
            name,
            weenie_class_id,
            icon_id,
            item_type,
            object_desc_flags,
            weenie_flags,
            value,
            stack_size,
            container,
            wielder,
            valid_locations,
            wielded_location,
            icon_overlay,
            icon_underlay,
            spell_id,
        } = WeenieDesc::parse(&mut r)?;
        Ok(ObjectCreate {
            guid,
            palette_id,
            sub_palettes,
            texture_changes,
            anim_part_changes,
            physics_flags,
            physics_state,
            placement,
            position,
            motion_table_id,
            sound_table_id,
            physics_table_id,
            setup_id,
            parent: parent.or(wielder).or(container),
            parent_location,
            children,
            scale,
            translucency,
            velocity,
            name,
            weenie_class_id,
            icon_id,
            icon_overlay,
            icon_underlay,
            item_type,
            object_desc_flags,
            weenie_flags,
            container,
            wielder,
            stack_size,
            value,
            spell_id,
            valid_locations,
            wielded_location,
            no_draw: physics_state & (PHYSICS_STATE_NO_DRAW | PHYSICS_STATE_HIDDEN) != 0,
        })
    }
}

/// UpdatePosition (0xF748): guid + PositionPack.
#[derive(Debug, Clone)]
pub struct UpdatePosition {
    pub guid: u32,
    pub flags: u32,
    pub position: Position,
    pub velocity: Vec3,
    pub placement: Option<u32>,
}

impl UpdatePosition {
    pub fn parse(body: &[u8]) -> Result<Self> {
        let mut r = Reader::new(body);
        let guid = r.u32()?;
        let flags = r.u32()?;
        let cell = r.u32()?;
        let local = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        let w = if flags & 0x08 == 0 { r.f32()? } else { 0.0 };
        let x = if flags & 0x10 == 0 { r.f32()? } else { 0.0 };
        let y = if flags & 0x20 == 0 { r.f32()? } else { 0.0 };
        let z = if flags & 0x40 == 0 { r.f32()? } else { 0.0 };
        let velocity = if flags & 0x01 != 0 {
            Vec3::new(r.f32()?, r.f32()?, r.f32()?)
        } else {
            Vec3::ZERO
        };
        let placement = if flags & 0x02 != 0 {
            Some(r.u32()?)
        } else {
            None
        };
        // instance, position, teleport, force-position sequences (u16 each)
        r.bytes(8)?;
        Ok(UpdatePosition {
            guid,
            flags,
            position: Position {
                cell,
                local,
                rotation: Quat::from_xyzw(x, y, z, w).normalize(),
            },
            velocity,
            placement,
        })
    }
}

/// Motion commands relevant to remote animation.
pub mod motion_cmd {
    pub const READY: u32 = 0x4100_0003;
    pub const WALK_FORWARD: u32 = 0x4500_0005;
    pub const RUN_FORWARD: u32 = 0x4400_0007;
}

/// A movement target from a MoveTo* movement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MoveTarget {
    Position { cell: u32, local: Vec3 },
    Object(u32),
}

/// MovementEvent (0xF74C, "UpdateMotion"): the server's description of what
/// an object is doing: its stance and forward/sidestep/turn commands, or a
/// move-to / turn-to instruction.
#[derive(Debug, Clone, PartialEq)]
pub struct MovementEvent {
    pub guid: u32,
    pub instance_seq: u16,
    pub movement_seq: u16,
    pub autonomous: bool,
    pub movement_type: u8,
    pub motion_flags: u8,
    /// Stance (low 16 bits of the MotionStance id).
    pub style: u16,
    pub forward: u32,
    pub forward_speed: f32,
    pub sidestep: u32,
    pub sidestep_speed: f32,
    pub turn: u32,
    pub turn_speed: f32,
    /// One-shot motions (emotes, attacks): (command, sequence, speed).
    pub commands: Vec<(u16, u16, f32)>,
    pub target: Option<MoveTarget>,
    pub run_rate: f32,
    pub desired_heading: Option<f32>,
}

impl MovementEvent {
    pub fn parse(body: &[u8]) -> Result<Self> {
        let mut r = Reader::new(body);
        let guid = r.u32()?;
        let instance_seq = r.u16()?;
        let movement_seq = r.u16()?;
        let _server_control_seq = r.u16()?;
        let autonomous = r.u8()? != 0;
        r.align4()?;
        let movement_type = r.u8()?;
        let motion_flags = r.u8()?;
        let style = r.u16()?;
        let mut ev = MovementEvent {
            guid,
            instance_seq,
            movement_seq,
            autonomous,
            movement_type,
            motion_flags,
            style,
            forward: motion_cmd::READY,
            forward_speed: 1.0,
            sidestep: 0,
            sidestep_speed: 1.0,
            turn: 0,
            turn_speed: 1.0,
            commands: Vec::new(),
            target: None,
            run_rate: 1.0,
            desired_heading: None,
        };
        let move_params = |r: &mut Reader| -> Result<(u32, f32)> {
            let flags = r.u32()?;
            let _dist = r.f32()?;
            let _min = r.f32()?;
            let _fail = r.f32()?;
            let _speed = r.f32()?;
            let _threshold = r.f32()?;
            let heading = r.f32()?;
            Ok((flags, heading))
        };
        let turn_params = |r: &mut Reader| -> Result<f32> {
            let _flags = r.u32()?;
            let _speed = r.f32()?;
            Ok(r.f32()?)
        };
        match movement_type {
            0 => {
                // InterpretedMotionState
                let packed = r.u32()?;
                let flags = packed & 0x7F;
                let n_cmds = (packed >> 7) as usize;
                if flags & 0x1 != 0 {
                    ev.style = r.u16()?;
                }
                if flags & 0x2 != 0 {
                    ev.forward = r.u16()? as u32;
                }
                if flags & 0x8 != 0 {
                    ev.sidestep = r.u16()? as u32;
                }
                if flags & 0x20 != 0 {
                    ev.turn = r.u16()? as u32;
                }
                if flags & 0x4 != 0 {
                    ev.forward_speed = r.f32()?;
                }
                if flags & 0x10 != 0 {
                    ev.sidestep_speed = r.f32()?;
                }
                if flags & 0x40 != 0 {
                    ev.turn_speed = r.f32()?;
                }
                for _ in 0..n_cmds {
                    ev.commands.push((r.u16()?, r.u16()?, r.f32()?));
                }
                r.align4()?;
                if motion_flags & 0x1 != 0 {
                    let _sticky = r.u32()?;
                }
            }
            6 => {
                let target = r.u32()?;
                let _cell = r.u32()?;
                let _origin = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
                let (_, heading) = move_params(&mut r)?;
                ev.run_rate = r.f32()?;
                ev.target = Some(MoveTarget::Object(target));
                ev.desired_heading = Some(heading);
                ev.forward = motion_cmd::RUN_FORWARD;
            }
            7 => {
                let cell = r.u32()?;
                let local = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
                let (_, heading) = move_params(&mut r)?;
                ev.run_rate = r.f32()?;
                ev.target = Some(MoveTarget::Position { cell, local });
                ev.desired_heading = Some(heading);
                ev.forward = motion_cmd::RUN_FORWARD;
            }
            8 => {
                let target = r.u32()?;
                let _heading_of_target = r.f32()?;
                ev.desired_heading = Some(turn_params(&mut r)?);
                ev.target = Some(MoveTarget::Object(target));
            }
            9 => {
                ev.desired_heading = Some(turn_params(&mut r)?);
            }
            _ => {}
        }
        Ok(ev)
    }
}
