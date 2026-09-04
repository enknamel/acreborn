//! Block-compressed texture decoding (DXT1/3/5, a.k.a. BC1/2/3) to RGBA8.

fn rgb565(c: u16) -> [u8; 3] {
    let r5 = (c >> 11) & 0x1F;
    let g6 = (c >> 5) & 0x3F;
    let b5 = c & 0x1F;
    [
        ((r5 << 3) | (r5 >> 2)) as u8,
        ((g6 << 2) | (g6 >> 4)) as u8,
        ((b5 << 3) | (b5 >> 2)) as u8,
    ]
}

fn lerp3(a: [u8; 3], b: [u8; 3], num: u32, den: u32) -> [u8; 3] {
    let mut o = [0u8; 3];
    for i in 0..3 {
        o[i] = ((a[i] as u32 * (den - num) + b[i] as u32 * num) / den) as u8;
    }
    o
}

/// Decode one 8-byte color block. `dxt1` enables the 1-bit alpha /
/// 3-color mode; DXT3/5 color blocks always use the 4-color mode.
fn color_block(block: &[u8], dxt1: bool, out: &mut [[u8; 4]; 16]) {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let p0 = rgb565(c0);
    let p1 = rgb565(c1);
    let (p2, p3, p3_alpha) = if !dxt1 || c0 > c1 {
        (lerp3(p0, p1, 1, 3), lerp3(p0, p1, 2, 3), 255u8)
    } else {
        (lerp3(p0, p1, 1, 2), [0, 0, 0], 0u8)
    };
    let palette = [
        [p0[0], p0[1], p0[2], 255],
        [p1[0], p1[1], p1[2], 255],
        [p2[0], p2[1], p2[2], 255],
        [p3[0], p3[1], p3[2], p3_alpha],
    ];
    let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    for (i, o) in out.iter_mut().enumerate() {
        *o = palette[((bits >> (2 * i)) & 3) as usize];
    }
}

fn alpha_block_dxt3(block: &[u8], out: &mut [[u8; 4]; 16]) {
    for i in 0..16 {
        let nib = (block[i / 2] >> ((i % 2) * 4)) & 0xF;
        out[i][3] = nib * 17;
    }
}

fn alpha_block_dxt5(block: &[u8], out: &mut [[u8; 4]; 16]) {
    let a0 = block[0] as u32;
    let a1 = block[1] as u32;
    let mut table = [0u8; 8];
    table[0] = a0 as u8;
    table[1] = a1 as u8;
    if a0 > a1 {
        for i in 1..7 {
            table[i + 1] = (((7 - i as u32) * a0 + i as u32 * a1) / 7) as u8;
        }
    } else {
        for i in 1..5 {
            table[i + 1] = (((5 - i as u32) * a0 + i as u32 * a1) / 5) as u8;
        }
        table[6] = 0;
        table[7] = 255;
    }
    let mut bits: u64 = 0;
    for i in 0..6 {
        bits |= (block[2 + i] as u64) << (8 * i);
    }
    for i in 0..16 {
        out[i][3] = table[((bits >> (3 * i)) & 7) as usize];
    }
}

/// Decode `data` (DXT1 if `block_size == 8`, DXT3/DXT5 if 16) into
/// `width * height * 4` RGBA bytes. Returns `None` if `data` is too short.
pub fn decode(data: &[u8], width: u32, height: u32, kind: DxtKind) -> Option<Vec<u8>> {
    let bw = width.div_ceil(4) as usize;
    let bh = height.div_ceil(4) as usize;
    let block_size = match kind {
        DxtKind::Dxt1 => 8,
        _ => 16,
    };
    if data.len() < bw * bh * block_size {
        return None;
    }
    let mut out = vec![0u8; (width * height * 4) as usize];
    let mut px = [[0u8; 4]; 16];
    for by in 0..bh {
        for bx in 0..bw {
            let block = &data[(by * bw + bx) * block_size..][..block_size];
            match kind {
                DxtKind::Dxt1 => color_block(block, true, &mut px),
                DxtKind::Dxt3 => {
                    color_block(&block[8..], false, &mut px);
                    alpha_block_dxt3(block, &mut px);
                }
                DxtKind::Dxt5 => {
                    color_block(&block[8..], false, &mut px);
                    alpha_block_dxt5(block, &mut px);
                }
            }
            for (i, p) in px.iter().enumerate() {
                let x = bx * 4 + i % 4;
                let y = by * 4 + i / 4;
                if x < width as usize && y < height as usize {
                    let o = (y * width as usize + x) * 4;
                    out[o..o + 4].copy_from_slice(p);
                }
            }
        }
    }
    Some(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DxtKind {
    Dxt1,
    Dxt3,
    Dxt5,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dxt1_solid_block() {
        // c0 = white (0xFFFF), c1 = black, all indices 0 -> white.
        let block = [0xFF, 0xFF, 0x00, 0x00, 0, 0, 0, 0];
        let px = decode(&block, 4, 4, DxtKind::Dxt1).unwrap();
        assert_eq!(&px[..4], &[255, 255, 255, 255]);
        assert_eq!(&px[60..64], &[255, 255, 255, 255]);
    }

    #[test]
    fn dxt5_alpha_table() {
        let mut block = [0u8; 16];
        block[0] = 255; // a0
        block[1] = 0; // a1
                      // indices all 1 -> a1 = 0 alpha
        for b in block[2..8].iter_mut() {
            *b = 0b0100_1001;
        }
        block[8] = 0xFF;
        block[9] = 0xFF;
        let px = decode(&block, 4, 4, DxtKind::Dxt5).unwrap();
        assert_eq!(px[3], 0);
    }
}
