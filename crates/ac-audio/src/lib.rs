//! Sound playback for decoded [`Wave`] clips, on top of `kira`.
//!
//! The API is deliberately small and position-independent for now:
//! [`Audio::new`] opens the default output device (fallible, so callers
//! without one keep running silent), [`Audio::play`] plays a clip once at a
//! volume, and [`sound_for`] / [`pick_entry`] resolve a sound type through a
//! [`SoundTable`] the way the client does: roll against each candidate's
//! `probability` in order.
//!
//! Clips are converted to kira frames on every `play`; they are short
//! (most are well under a second) so caching is left to the caller.

use std::io::Cursor;
use std::sync::{Arc, Mutex};

use ac_formats::wave::{Wave, WaveFormat};
use kira::sound::static_sound::{StaticSoundData, StaticSoundSettings};
use kira::sound::FromFileError;
use kira::{AudioManager, AudioManagerSettings, Decibels, DefaultBackend, Frame};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no audio device: {0}")]
    Device(String),
    #[error("wave {id:#010x}: unsupported format tag {tag:#x} ({bits}-bit, {channels} ch)")]
    Unsupported {
        id: u32,
        tag: u16,
        bits: u16,
        channels: u16,
    },
    #[error("wave {id:#010x}: mp3 decode failed: {source}")]
    Mp3 { id: u32, source: FromFileError },
    #[error("play failed: {0}")]
    Play(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Handle to the output device and mixer. Cheap to clone; all clones share
/// one device.
#[derive(Clone)]
pub struct Audio {
    manager: Arc<Mutex<AudioManager<DefaultBackend>>>,
}

impl Audio {
    /// Open the default output device. Fails (rather than panics) when there
    /// is none, so headless runs can skip audio.
    pub fn new() -> Result<Self> {
        let manager = AudioManager::<DefaultBackend>::new(AudioManagerSettings::default())
            .map_err(|e| Error::Device(e.to_string()))?;
        Ok(Audio {
            manager: Arc::new(Mutex::new(manager)),
        })
    }

    /// Play `wave` once. `volume` is linear amplitude, 1.0 = as authored;
    /// 0 or less plays nothing.
    pub fn play(&self, wave: &Wave, volume: f32) -> Result<()> {
        if volume <= 0.0 {
            return Ok(());
        }
        let sound = decode(wave)?.with_settings(StaticSoundSettings::new().volume(db(volume)));
        let mut m = self.manager.lock().unwrap_or_else(|e| e.into_inner());
        m.play(sound).map_err(|e| Error::Play(e.to_string()))?;
        Ok(())
    }
}

/// Linear amplitude to decibels, clamped to kira's silence floor.
fn db(volume: f32) -> Decibels {
    if volume <= 0.0 {
        Decibels::SILENCE
    } else {
        Decibels(20.0 * volume.log10())
    }
}

/// Convert a clip to kira's in-memory form: PCM is unpacked directly, the
/// odd MP3 clip goes through kira's decoder.
pub fn decode(wave: &Wave) -> Result<StaticSoundData> {
    let f = &wave.format;
    match f.format_tag {
        WaveFormat::PCM => {
            let frames = pcm_frames(wave)?;
            Ok(StaticSoundData {
                sample_rate: f.samples_per_sec,
                frames: frames.into(),
                settings: StaticSoundSettings::default(),
                slice: None,
            })
        }
        WaveFormat::MP3 => {
            StaticSoundData::from_cursor(Cursor::new(wave.data.clone())).map_err(|source| {
                Error::Mp3 {
                    id: wave.id,
                    source,
                }
            })
        }
        tag => Err(Error::Unsupported {
            id: wave.id,
            tag,
            bits: f.bits_per_sample,
            channels: f.channels,
        }),
    }
}

/// Unpack 8-bit unsigned or 16-bit signed PCM, mono or stereo, to frames.
fn pcm_frames(wave: &Wave) -> Result<Vec<Frame>> {
    let f = &wave.format;
    let unsupported = || Error::Unsupported {
        id: wave.id,
        tag: f.format_tag,
        bits: f.bits_per_sample,
        channels: f.channels,
    };
    let channels = match f.channels {
        1 | 2 => f.channels as usize,
        _ => return Err(unsupported()),
    };
    let samples: Vec<f32> = match f.bits_per_sample {
        8 => wave
            .data
            .iter()
            .map(|&b| (b as f32 - 128.0) / 128.0)
            .collect(),
        16 => wave
            .data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
            .collect(),
        _ => return Err(unsupported()),
    };
    Ok(samples
        .chunks_exact(channels)
        .map(|c| {
            if channels == 1 {
                Frame::from_mono(c[0])
            } else {
                Frame::new(c[0], c[1])
            }
        })
        .collect())
}

pub use ac_formats::sound_table::{pick_entry, sound_for};

#[cfg(test)]
mod tests {
    use super::*;
    use ac_formats::sound_table::SoundData;

    fn table(entries: &[(u32, f32)]) -> ac_formats::sound_table::SoundTable {
        SoundTable {
            id: 0x2000_0001,
            unknown: 0,
            sound_hash: vec![],
            sounds: vec![(
                3,
                SoundData {
                    entries: entries
                        .iter()
                        .map(|&(wave_id, probability)| SoundEntry {
                            wave_id,
                            priority: 1.0,
                            probability,
                            volume: 1.0,
                        })
                        .collect(),
                    unknown: 0,
                },
            )],
        }
    }

    #[test]
    fn picks_by_probability() {
        let t = table(&[(0x0A00_0010, 0.3), (0x0A00_0011, 0.6), (0x0A00_0012, 1.0)]);
        assert_eq!(pick_entry(&t, 3, 0.1).unwrap().wave_id, 0x0A00_0010);
        assert_eq!(pick_entry(&t, 3, 0.3).unwrap().wave_id, 0x0A00_0011);
        assert_eq!(pick_entry(&t, 3, 0.95).unwrap().wave_id, 0x0A00_0012);
        assert!(pick_entry(&t, 4, 0.5).is_none());
        assert!(sound_for(&t, 4).is_none());
        let w = sound_for(&t, 3).unwrap();
        assert!((0x0A00_0010..=0x0A00_0012).contains(&w));
    }

    #[test]
    fn falls_back_to_last_entry() {
        let t = table(&[(0x0A00_0010, 0.5), (0x0A00_0011, 0.5)]);
        assert_eq!(pick_entry(&t, 3, 0.99).unwrap().wave_id, 0x0A00_0011);
        assert!(pick_entry(&table(&[]), 3, 0.0).is_none());
    }

    #[test]
    fn random_unit_in_range() {
        for _ in 0..1000 {
            let r = random_unit();
            assert!((0.0..1.0).contains(&r), "{r}");
        }
    }

    fn pcm_wave(bits: u16, channels: u16, data: Vec<u8>) -> Wave {
        let data_len = data.len();
        Wave {
            id: 0x0A00_0001,
            format: WaveFormat {
                format_tag: WaveFormat::PCM,
                channels,
                samples_per_sec: 22050,
                avg_bytes_per_sec: 22050 * channels as u32 * bits as u32 / 8,
                block_align: channels * bits / 8,
                bits_per_sample: bits,
                extra: vec![],
            },
            data,
            data_len,
        }
    }

    #[test]
    fn decodes_pcm_variants() {
        let w = pcm_wave(16, 1, vec![0, 0, 0xFF, 0x7F, 0, 0x80]);
        let s = decode(&w).unwrap();
        assert_eq!(s.sample_rate, 22050);
        assert_eq!(s.frames.len(), 3);
        assert_eq!(s.frames[0], Frame::from_mono(0.0));
        assert!((s.frames[1].left - 1.0).abs() < 1e-4);
        assert_eq!(s.frames[2].left, -1.0);

        let w = pcm_wave(8, 2, vec![128, 128, 0, 255]);
        let s = decode(&w).unwrap();
        assert_eq!(s.frames.len(), 2);
        assert_eq!(s.frames[0], Frame::new(0.0, 0.0));
        assert_eq!(s.frames[1].left, -1.0);
        assert!((s.frames[1].right - 127.0 / 128.0).abs() < 1e-6);

        let mut w = pcm_wave(16, 1, vec![0, 0]);
        w.format.bits_per_sample = 24;
        assert!(matches!(
            decode(&w),
            Err(Error::Unsupported { bits: 24, .. })
        ));
    }

    #[test]
    fn volume_to_decibels() {
        assert_eq!(db(1.0).0, 0.0);
        assert!((db(0.5).0 + 6.02).abs() < 0.01);
        assert_eq!(db(0.0), Decibels::SILENCE);
    }

    /// `Audio::new` must not panic without a device; either outcome is fine
    /// on a test machine.
    #[test]
    fn new_is_fallible_not_fatal() {
        match Audio::new() {
            Ok(a) => {
                let w = pcm_wave(16, 1, vec![0; 64]);
                a.play(&w, 0.0).unwrap();
                a.play(&w, 0.5).unwrap();
            }
            Err(e) => eprintln!("no audio device: {e}"),
        }
    }
}
