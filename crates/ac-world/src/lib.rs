//! World state built from server messages. `World::apply` consumes decoded
//! messages (`ObjectCreate`, `PlayerCreate`, `UpdatePosition`,
//! `DeleteObject`) and keeps a table of objects with their model ids and
//! positions.

pub mod object;

use std::collections::HashMap;

use ac_net::messages::{self, opcode};
use glam::{Mat4, Quat, Vec3};

pub use object::{ObjectCreate, Position};

/// One object the server has placed in the world.
#[derive(Debug, Clone)]
pub struct WorldObject {
    pub guid: u32,
    pub name: String,
    pub weenie_class_id: u32,
    /// Setup (0x02) id, or 0 when unknown.
    pub setup_id: u32,
    pub motion_table_id: u32,
    pub scale: f32,
    /// Absent for objects carried by another object (inventory, wielded).
    pub position: Option<Position>,
    /// Parent object when carried/wielded.
    pub parent: Option<u32>,
    pub no_draw: bool,
    pub is_player: bool,
    pub palette_id: u32,
    pub sub_palettes: Vec<(u32, u8, u8)>,
    pub texture_changes: Vec<(u8, u32, u32)>,
    pub anim_part_changes: Vec<(u8, u32)>,
}

impl WorldObject {
    /// World-space transform (landblock origin + local frame), if placed.
    pub fn transform(&self) -> Option<Mat4> {
        let p = self.position?;
        let origin = landblock_origin(p.cell);
        Some(Mat4::from_scale_rotation_translation(
            Vec3::splat(self.scale),
            p.rotation,
            origin + p.local,
        ))
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

#[derive(Debug, Default)]
pub struct World {
    pub objects: HashMap<u32, WorldObject>,
    pub player_guid: Option<u32>,
    /// Bumped whenever the set of drawable objects or a position changes.
    pub generation: u64,
}

/// What `apply` did with a message, for logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    Created,
    Moved,
    Deleted,
    PlayerSet,
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

    pub fn apply(&mut self, msg: &[u8]) -> Applied {
        let Some((op, body)) = messages::split(msg) else {
            return Applied::Ignored;
        };
        match op {
            opcode::OBJECT_CREATE => match ObjectCreate::parse(body) {
                Ok(oc) => {
                    let is_player = self.player_guid == Some(oc.guid);
                    let obj = WorldObject {
                        guid: oc.guid,
                        name: oc.name,
                        weenie_class_id: oc.weenie_class_id,
                        setup_id: oc.setup_id,
                        motion_table_id: oc.motion_table_id,
                        scale: if oc.scale > 0.0 { oc.scale } else { 1.0 },
                        position: oc.position,
                        parent: oc.parent,
                        no_draw: oc.no_draw,
                        is_player,
                        palette_id: oc.palette_id,
                        sub_palettes: oc.sub_palettes,
                        texture_changes: oc.texture_changes,
                        anim_part_changes: oc.anim_part_changes,
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
                            // Our own echoes come back; only accept a server
                            // correction that moves us more than a few metres.
                            let far = match o.position {
                                Some(cur) => {
                                    let a = landblock_origin(cur.cell) + cur.local;
                                    let b = landblock_origin(up.position.cell) + up.position.local;
                                    (a - b).length() > 4.0
                                }
                                None => true,
                            };
                            if !far {
                                return Applied::Ignored;
                            }
                            tracing::info!(
                                "server moved the player to {:#010x} {:?}",
                                up.position.cell,
                                up.position.local
                            );
                        }
                        o.position = Some(up.position);
                        self.generation += 1;
                        Applied::Moved
                    } else {
                        Applied::Ignored
                    }
                }
                Err(e) => {
                    tracing::warn!("UpdatePosition: {e}");
                    Applied::Failed
                }
            },
            opcode::OBJECT_DELETE => {
                if body.len() >= 4 {
                    let guid = u32::from_le_bytes(body[..4].try_into().unwrap());
                    if self.objects.remove(&guid).is_some() {
                        self.generation += 1;
                        return Applied::Deleted;
                    }
                }
                Applied::Ignored
            }
            _ => Applied::Ignored,
        }
    }

    /// Objects that have a world position and a model.
    pub fn drawable(&self) -> impl Iterator<Item = &WorldObject> {
        self.objects
            .values()
            .filter(|o| o.position.is_some() && o.setup_id != 0 && !o.no_draw && o.parent.is_none())
    }
}

/// Convenience for callers building a camera.
pub fn quat_forward(q: Quat) -> Vec3 {
    q * Vec3::Y
}
