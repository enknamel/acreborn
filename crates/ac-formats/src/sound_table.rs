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

/// Pick a candidate for `sound_type` from `table`, rolling `roll` (in
/// `0.0..1.0`) against each entry's probability in turn; the last entry is
/// the fallback when none hits.
pub fn pick_entry(table: &SoundTable, sound_type: u32, roll: f32) -> Option<&SoundEntry> {
    let data = table.get(sound_type)?;
    data.entries
        .iter()
        .find(|e| roll < e.probability)
        .or(data.entries.last())
}

/// The wave to play for `sound_type`, chosen by probability with a fresh
/// random roll. `None` when the table has no entry for the type.
pub fn sound_for(table: &SoundTable, sound_type: u32) -> Option<u32> {
    pick_entry(table, sound_type, random_unit()).map(|e| e.wave_id)
}

/// Uniform in `[0, 1)` without pulling in a RNG crate: a splitmix64 step
/// over a thread-local seed taken from the clock.
fn random_unit() -> f32 {
    use std::cell::Cell;
    use std::time::{SystemTime, UNIX_EPOCH};
    thread_local! {
        static STATE: Cell<u64> = Cell::new(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x9E37_79B9_7F4A_7C15),
        );
    }
    STATE.with(|s| {
        let x = s.get().wrapping_add(0x9E37_79B9_7F4A_7C15);
        s.set(x);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 24) as f32
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_unit_in_range() {
        for _ in 0..1000 {
            let r = random_unit();
            assert!((0.0..1.0).contains(&r), "{r}");
        }
    }
}
