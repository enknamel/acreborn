//! PhysicsScriptTable (0x34): which PhysicsScript (0x33) an object plays
//! for each script type (the `PlayScript` enum the server sends in
//! `PlayScriptType` messages, and the Setup's `default_script_table`).
//! Each type lists candidate scripts with a `modifier` threshold; the
//! client plays the first whose modifier the requested intensity reaches.

use serde::Serialize;

use crate::{expect_id, Reader, Result};

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ScriptAndMod {
    pub modifier: f32,
    /// PhysicsScript (0x33) id.
    pub script_id: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PhysicsScriptTable {
    pub id: u32,
    /// script type -> candidates.
    pub scripts: Vec<(u32, Vec<ScriptAndMod>)>,
}

impl PhysicsScriptTable {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let scripts = r.map(
            |r| r.u32(),
            |r| {
                r.list(|r| {
                    Ok(ScriptAndMod {
                        modifier: r.f32()?,
                        script_id: r.u32()?,
                    })
                })
            },
        )?;
        r.finish()?;
        Ok(PhysicsScriptTable { id, scripts })
    }

    /// The script for `script_type` at intensity `modifier`: the first
    /// candidate whose modifier is at most the requested one, else the
    /// last candidate.
    pub fn get(&self, script_type: u32, modifier: f32) -> Option<u32> {
        let (_, list) = self.scripts.iter().find(|(k, _)| *k == script_type)?;
        list.iter()
            .find(|s| s.modifier <= modifier)
            .or(list.last())
            .map(|s| s.script_id)
    }

    pub fn script_types(&self) -> impl Iterator<Item = u32> + '_ {
        self.scripts.iter().map(|(k, _)| *k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_by_modifier() {
        let mut b = Vec::new();
        b.extend_from_slice(&0x3400_0001u32.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&0x5Au32.to_le_bytes());
        b.extend_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&1.0f32.to_le_bytes());
        b.extend_from_slice(&0x3300_0001u32.to_le_bytes());
        b.extend_from_slice(&0.0f32.to_le_bytes());
        b.extend_from_slice(&0x3300_0002u32.to_le_bytes());
        let t = PhysicsScriptTable::parse(0x3400_0001, &b).unwrap();
        assert_eq!(t.script_types().collect::<Vec<_>>(), vec![0x5A]);
        assert_eq!(t.get(0x5A, 1.0), Some(0x3300_0001));
        assert_eq!(t.get(0x5A, 0.5), Some(0x3300_0002));
        assert_eq!(t.get(0x5B, 1.0), None);
    }
}
