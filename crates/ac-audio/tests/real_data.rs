//! Decode real clips from the portal archive into kira frames and resolve
//! sound types through the human sound table. Skipped unless `AC_DATA_DIR`
//! is set. No audio device is needed.

use std::path::PathBuf;

use ac_dat::{DatArchive, FileKind};
use ac_formats::sound_table::SoundTable;
use ac_formats::wave::Wave;

fn portal() -> Option<DatArchive> {
    let dir = PathBuf::from(std::env::var_os("AC_DATA_DIR")?);
    Some(DatArchive::open(dir.join("client_portal.dat")).unwrap())
}

/// Human sound table (weenie DID 0x20000001).
const HUMAN: u32 = 0x2000_0001;

#[test]
fn human_sounds_decode() {
    let Some(dat) = portal() else {
        eprintln!("AC_DATA_DIR unset; skipping");
        return;
    };
    let table = SoundTable::parse(HUMAN, &dat.read(HUMAN).unwrap()).unwrap();
    // Attack1, Wound1, Death1.
    for sound_type in [3u32, 0x0C, 0x0F] {
        let wave_id = ac_audio::sound_for(&table, sound_type)
            .unwrap_or_else(|| panic!("no wave for sound type {sound_type}"));
        let wave = Wave::parse(wave_id, &dat.read(wave_id).unwrap()).unwrap();
        let sound = ac_audio::decode(&wave).unwrap();
        assert_eq!(sound.sample_rate, wave.format.samples_per_sec);
        assert!(sound.frames.len() > 100, "{wave_id:08X}");
        let expected = wave.data.len()
            / (wave.format.channels as usize * wave.format.bits_per_sample as usize / 8);
        assert_eq!(sound.frames.len(), expected, "{wave_id:08X}");
    }
}

/// Every clip in the archive converts; the one MP3 goes through kira's
/// decoder and must come out with roughly the duration its header claims.
#[test]
fn every_wave_decodes() {
    let Some(dat) = portal() else {
        return;
    };
    let mut n = 0;
    let mut mp3 = 0;
    for e in dat.entries().filter(|e| dat.kind(e.id) == FileKind::Wave) {
        let wave = Wave::parse(e.id, &dat.read(e.id).unwrap()).unwrap();
        let sound = ac_audio::decode(&wave).unwrap_or_else(|err| panic!("{:08X}: {err}", e.id));
        let secs = sound.frames.len() as f32 / sound.sample_rate as f32;
        if wave.is_mp3() {
            mp3 += 1;
            let claimed = wave.duration_secs();
            assert!(
                (secs - claimed).abs() < 0.5,
                "{:08X}: {secs}s decoded vs {claimed}s claimed",
                e.id
            );
        }
        n += 1;
    }
    eprintln!("{n} waves decoded ({mp3} mp3)");
    assert!(n > 0);
}
