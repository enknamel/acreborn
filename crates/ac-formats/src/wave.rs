//! Wave (0x0A): a sound clip. The file is the id, then the sizes of two
//! blobs: a `WAVEFORMATEX` (the `fmt ` chunk body of a RIFF WAV, without the
//! chunk header) and the sample data. Most clips are 16-bit PCM; a few are
//! MPEG Layer 3 (`format_tag` 0x55) with the 12-byte `MPEGLAYER3WAVEFORMAT`
//! extension after `cb_size`.
//!
//! [`Wave::write_riff`] wraps the two blobs back into a standard `.wav`.

use std::io::{self, Write};

use serde::Serialize;

use crate::{expect_id, Error, Reader, Result};

/// `WAVEFORMATEX` as stored in the file. `format_tag` values seen in the
/// portal archive are [`WaveFormat::PCM`] and [`WaveFormat::MP3`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WaveFormat {
    pub format_tag: u16,
    pub channels: u16,
    pub samples_per_sec: u32,
    pub avg_bytes_per_sec: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    /// Format-specific bytes after `cb_size` (empty for PCM).
    #[serde(skip)]
    pub extra: Vec<u8>,
}

impl WaveFormat {
    pub const PCM: u16 = 0x0001;
    pub const ADPCM: u16 = 0x0002;
    pub const IMA_ADPCM: u16 = 0x0011;
    pub const MP3: u16 = 0x0055;

    fn parse(header: &[u8]) -> Result<Self> {
        let mut r = Reader::new(header);
        let format_tag = r.u16()?;
        let channels = r.u16()?;
        let samples_per_sec = r.u32()?;
        let avg_bytes_per_sec = r.u32()?;
        let block_align = r.u16()?;
        let bits_per_sample = r.u16()?;
        // WAVEFORMAT proper is 16 bytes; WAVEFORMATEX adds cb_size and that
        // many extension bytes. Tolerate a header that stops at either point.
        let extra = if r.remaining() >= 2 {
            let cb = r.u16()? as usize;
            if cb > r.remaining() {
                return Err(Error::Invalid {
                    what: "wave header",
                    detail: format!("cb_size {cb} exceeds {} remaining bytes", r.remaining()),
                });
            }
            r.bytes(cb)?.to_vec()
        } else {
            Vec::new()
        };
        r.finish()?;
        Ok(WaveFormat {
            format_tag,
            channels,
            samples_per_sec,
            avg_bytes_per_sec,
            block_align,
            bits_per_sample,
            extra,
        })
    }

    /// The `fmt ` chunk body: 16 bytes for plain PCM, otherwise the full
    /// `WAVEFORMATEX` with `cb_size` and the extension.
    fn to_fmt_chunk(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(18 + self.extra.len());
        b.extend_from_slice(&self.format_tag.to_le_bytes());
        b.extend_from_slice(&self.channels.to_le_bytes());
        b.extend_from_slice(&self.samples_per_sec.to_le_bytes());
        b.extend_from_slice(&self.avg_bytes_per_sec.to_le_bytes());
        b.extend_from_slice(&self.block_align.to_le_bytes());
        b.extend_from_slice(&self.bits_per_sample.to_le_bytes());
        if self.format_tag != Self::PCM || !self.extra.is_empty() {
            b.extend_from_slice(&(self.extra.len() as u16).to_le_bytes());
            b.extend_from_slice(&self.extra);
        }
        b
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Wave {
    pub id: u32,
    pub format: WaveFormat,
    /// Sample data exactly as stored (PCM frames or an MP3 stream).
    #[serde(skip)]
    pub data: Vec<u8>,
    pub data_len: usize,
}

impl Wave {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);
        expect_id(&mut r, id)?;
        let header_len = r.u32()? as usize;
        let data_len = r.u32()? as usize;
        let format = WaveFormat::parse(r.bytes(header_len)?)?;
        let data = r.bytes(data_len)?.to_vec();
        r.finish()?;
        Ok(Wave {
            id,
            format,
            data,
            data_len,
        })
    }

    pub fn is_pcm(&self) -> bool {
        self.format.format_tag == WaveFormat::PCM
    }

    pub fn is_mp3(&self) -> bool {
        self.format.format_tag == WaveFormat::MP3
    }

    /// Duration implied by the header's byte rate, for any format.
    pub fn duration_secs(&self) -> f32 {
        if self.format.avg_bytes_per_sec == 0 {
            return 0.0;
        }
        self.data.len() as f32 / self.format.avg_bytes_per_sec as f32
    }

    /// Write a standard RIFF/WAVE file: `fmt ` chunk from the stored header,
    /// `data` chunk with the stored bytes (padded to an even length).
    pub fn write_riff(&self, w: &mut impl Write) -> io::Result<()> {
        let fmt = self.format.to_fmt_chunk();
        let fmt_padded = fmt.len() + (fmt.len() & 1);
        let data_padded = self.data.len() + (self.data.len() & 1);
        let riff_len = 4 + (8 + fmt_padded) + (8 + data_padded);
        w.write_all(b"RIFF")?;
        w.write_all(&(riff_len as u32).to_le_bytes())?;
        w.write_all(b"WAVE")?;
        w.write_all(b"fmt ")?;
        w.write_all(&(fmt.len() as u32).to_le_bytes())?;
        w.write_all(&fmt)?;
        if fmt.len() & 1 == 1 {
            w.write_all(&[0])?;
        }
        w.write_all(b"data")?;
        w.write_all(&(self.data.len() as u32).to_le_bytes())?;
        w.write_all(&self.data)?;
        if self.data.len() & 1 == 1 {
            w.write_all(&[0])?;
        }
        Ok(())
    }

    pub fn to_riff(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(44 + self.data.len() + self.format.extra.len());
        self.write_riff(&mut v).expect("Vec write cannot fail");
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm_file(samples: &[i16]) -> Vec<u8> {
        let mut data = Vec::new();
        for s in samples {
            data.extend_from_slice(&s.to_le_bytes());
        }
        let mut header = Vec::new();
        header.extend_from_slice(&1u16.to_le_bytes()); // PCM
        header.extend_from_slice(&1u16.to_le_bytes()); // mono
        header.extend_from_slice(&22050u32.to_le_bytes());
        header.extend_from_slice(&44100u32.to_le_bytes());
        header.extend_from_slice(&2u16.to_le_bytes());
        header.extend_from_slice(&16u16.to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes()); // cb_size
        let mut f = Vec::new();
        f.extend_from_slice(&0x0A00_0001u32.to_le_bytes());
        f.extend_from_slice(&(header.len() as u32).to_le_bytes());
        f.extend_from_slice(&(data.len() as u32).to_le_bytes());
        f.extend_from_slice(&header);
        f.extend_from_slice(&data);
        f
    }

    #[test]
    fn parse_and_riff_roundtrip() {
        let samples = [0i16, 1000, -1000, 32767, -32768];
        let w = Wave::parse(0x0A00_0001, &pcm_file(&samples)).unwrap();
        assert_eq!(w.format.format_tag, WaveFormat::PCM);
        assert_eq!(w.format.samples_per_sec, 22050);
        assert_eq!(w.format.bits_per_sample, 16);
        assert!(w.format.extra.is_empty());
        assert_eq!(w.data.len(), 10);

        let riff = w.to_riff();
        // 12 (RIFF header) + 8 + 16 (fmt) + 8 + 10 (data)
        assert_eq!(riff.len(), 54);
        assert_eq!(&riff[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(riff[4..8].try_into().unwrap()), 46);
        assert_eq!(&riff[8..12], b"WAVE");
        assert_eq!(&riff[12..16], b"fmt ");
        assert_eq!(u32::from_le_bytes(riff[16..20].try_into().unwrap()), 16);
        assert_eq!(&riff[20..22], &1u16.to_le_bytes());
        assert_eq!(&riff[36..40], b"data");
        assert_eq!(u32::from_le_bytes(riff[40..44].try_into().unwrap()), 10);
        assert_eq!(&riff[44..], &w.data[..]);
    }

    #[test]
    fn odd_data_is_padded() {
        let mut f = pcm_file(&[1, 2]);
        // Shrink the data blob to 3 bytes.
        let n = f.len();
        f.truncate(n - 1);
        f[8..12].copy_from_slice(&3u32.to_le_bytes());
        let w = Wave::parse(0x0A00_0001, &f).unwrap();
        let riff = w.to_riff();
        assert_eq!(riff.len(), 12 + 24 + 8 + 4);
        assert_eq!(u32::from_le_bytes(riff[40..44].try_into().unwrap()), 3);
        assert_eq!(*riff.last().unwrap(), 0);
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut f = pcm_file(&[1]);
        f.push(0);
        assert!(matches!(
            Wave::parse(0x0A00_0001, &f),
            Err(Error::Trailing { .. })
        ));
    }
}
