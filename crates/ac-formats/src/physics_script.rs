//! PhysicsScript (0x33): a timed list of animation hooks (create/stop
//! particle emitters, sounds, transparency fades, ...) played on an object
//! when a script fires, for instance a spell's cast or a portal's swirl.

use serde::Serialize;

use crate::animation::Hook;
use crate::{expect_id, Reader, Result};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScriptHook {
    /// Seconds after the script starts.
    pub start_time: f64,
    pub hook: Hook,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PhysicsScript {
    pub id: u32,
    /// In start-time order as stored.
    pub hooks: Vec<ScriptHook>,
}

impl PhysicsScript {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let hooks = r.list(|r| {
            Ok(ScriptHook {
                start_time: r.f64()?,
                hook: Hook::parse(r)?,
            })
        })?;
        r.finish()?;
        Ok(PhysicsScript { id, hooks })
    }

    /// Seconds from the first hook to the last.
    pub fn duration(&self) -> f64 {
        self.hooks.iter().map(|h| h.start_time).fold(0.0, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::HookData;

    #[test]
    fn create_particle_hook_round_trips() {
        let mut b = Vec::new();
        b.extend_from_slice(&0x3300_0010u32.to_le_bytes());
        b.extend_from_slice(&2u32.to_le_bytes());
        // t = 0: CreateParticle (type 13, dir Both)
        b.extend_from_slice(&0.0f64.to_le_bytes());
        b.extend_from_slice(&13u32.to_le_bytes());
        b.extend_from_slice(&0i32.to_le_bytes());
        b.extend_from_slice(&0x3200_0042u32.to_le_bytes());
        b.extend_from_slice(&(-1i32).to_le_bytes());
        for v in [0.0f32, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&7u32.to_le_bytes());
        // t = 2.5: StopParticle (type 15)
        b.extend_from_slice(&2.5f64.to_le_bytes());
        b.extend_from_slice(&15u32.to_le_bytes());
        b.extend_from_slice(&0i32.to_le_bytes());
        b.extend_from_slice(&7u32.to_le_bytes());
        let s = PhysicsScript::parse(0x3300_0010, &b).unwrap();
        assert_eq!(s.hooks.len(), 2);
        assert!(matches!(
            s.hooks[0].hook.data,
            HookData::CreateParticle {
                emitter_info_id: 0x3200_0042,
                part_index: 0xFFFF_FFFF,
                emitter_id: 7,
                ..
            }
        ));
        assert_eq!(s.hooks[1].start_time, 2.5);
        assert!(matches!(
            s.hooks[1].hook.data,
            HookData::StopParticle { emitter_id: 7 }
        ));
        assert_eq!(s.duration(), 2.5);
    }
}
