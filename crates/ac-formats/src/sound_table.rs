//! SoundTable (0x20): which Wave (0x0A) clips an object plays for each
//! sound type (the `Sound` enum the server sends in `SoundEvent` messages:
//! attacks, footsteps, wounds, death, ...). Each type maps to a weighted
//! list of candidates; the client rolls against `probability` to pick one.

use serde::Serialize;

use crate::{expect_id, Reader, Result};

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SoundEntry {
    /// Wave (0x0A) id.
    pub wave_id: u32,
    pub priority: f32,
    /// Relative weight among the entries for one sound type.
    pub probability: f32,
    pub volume: f32,
}

impl SoundEntry {
    fn parse(r: &mut Reader) -> Result<Self> {
        Ok(SoundEntry {
            wave_id: r.u32()?,
            priority: r.f32()?,
            probability: r.f32()?,
            volume: r.f32()?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SoundData {
    pub entries: Vec<SoundEntry>,
    pub unknown: u32,
}

impl SoundData {
    fn parse(r: &mut Reader) -> Result<Self> {
        Ok(SoundData {
            entries: r.list(SoundEntry::parse)?,
            unknown: r.u32()?,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SoundTable {
    pub id: u32,
    pub unknown: u32,
    /// Always a single all-zero entry in the shipped archives.
    pub sound_hash: Vec<SoundEntry>,
    /// sound type -> candidate clips.
    pub sounds: Vec<(u32, SoundData)>,
}

impl SoundTable {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let unknown = r.u32()?;
        let sound_hash = r.list(SoundEntry::parse)?;
        let sounds = r.packed_hash_table(|r| r.u32(), SoundData::parse)?;
        r.finish()?;
        Ok(SoundTable {
            id,
            unknown,
            sound_hash,
            sounds,
        })
    }

    /// Candidate clips for a sound type.
    pub fn get(&self, sound_type: u32) -> Option<&SoundData> {
        self.sounds
            .iter()
            .find(|(k, _)| *k == sound_type)
            .map(|(_, d)| d)
    }

    pub fn sound_types(&self) -> impl Iterator<Item = u32> + '_ {
        self.sounds.iter().map(|(k, _)| *k)
    }
}
