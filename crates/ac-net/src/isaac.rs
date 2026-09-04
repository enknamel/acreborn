//! The client's ISAAC variant used to XOR packet checksums. It is not
//! standard ISAAC: the key schedule mixes the golden ratio only, and the
//! seed goes straight into `a`, `b` and `c` before the first scramble.
//! Ported from ACE's `ACE.Common.Cryptography.ISAAC`, which matches the
//! client; values are consumed from index 255 downwards.

pub struct Isaac {
    offset: usize,
    a: u32,
    b: u32,
    c: u32,
    mm: [u32; 256],
    rsl: [u32; 256],
}

impl Isaac {
    pub fn new(seed: u32) -> Self {
        let mut s = Isaac {
            offset: 255,
            a: 0,
            b: 0,
            c: 0,
            mm: [0; 256],
            rsl: [0; 256],
        };
        let mut x = [0x9E37_79B9u32; 8];
        for _ in 0..4 {
            shuffle(&mut x);
        }
        for pass in 0..2 {
            for j in (0..256).step_by(8) {
                for k in 0..8 {
                    x[k] = x[k].wrapping_add(if pass == 0 { s.rsl[j + k] } else { s.mm[j + k] });
                }
                shuffle(&mut x);
                s.mm[j..j + 8].copy_from_slice(&x);
            }
        }
        s.a = seed;
        s.b = seed;
        s.c = seed;
        s.scramble();
        s
    }

    pub fn next(&mut self) -> u32 {
        let v = self.rsl[self.offset];
        if self.offset > 0 {
            self.offset -= 1;
        } else {
            self.scramble();
            self.offset = 255;
        }
        v
    }

    fn scramble(&mut self) {
        self.c = self.c.wrapping_add(1);
        self.b = self.b.wrapping_add(self.c);
        for i in 0..256 {
            let x = self.mm[i];
            self.a = match i & 3 {
                0 => self.a ^ (self.a << 13),
                1 => self.a ^ (self.a >> 6),
                2 => self.a ^ (self.a << 2),
                _ => self.a ^ (self.a >> 16),
            };
            self.a = self.a.wrapping_add(self.mm[(i + 128) & 0xFF]);
            let y = self.mm[(x >> 2) as usize & 0xFF]
                .wrapping_add(self.a)
                .wrapping_add(self.b);
            self.mm[i] = y;
            self.b = self.mm[(y >> 10) as usize & 0xFF].wrapping_add(x);
            self.rsl[i] = self.b;
        }
    }
}

fn shuffle(x: &mut [u32; 8]) {
    x[0] ^= x[1] << 11;
    x[3] = x[3].wrapping_add(x[0]);
    x[1] = x[1].wrapping_add(x[2]);
    x[1] ^= x[2] >> 2;
    x[4] = x[4].wrapping_add(x[1]);
    x[2] = x[2].wrapping_add(x[3]);
    x[2] ^= x[3] << 8;
    x[5] = x[5].wrapping_add(x[2]);
    x[3] = x[3].wrapping_add(x[4]);
    x[3] ^= x[4] >> 16;
    x[6] = x[6].wrapping_add(x[3]);
    x[4] = x[4].wrapping_add(x[5]);
    x[4] ^= x[5] << 10;
    x[7] = x[7].wrapping_add(x[4]);
    x[5] = x[5].wrapping_add(x[6]);
    x[5] ^= x[6] >> 4;
    x[0] = x[0].wrapping_add(x[5]);
    x[6] = x[6].wrapping_add(x[7]);
    x[6] ^= x[7] << 8;
    x[1] = x[1].wrapping_add(x[6]);
    x[7] = x[7].wrapping_add(x[0]);
    x[7] ^= x[0] >> 9;
    x[2] = x[2].wrapping_add(x[7]);
    x[0] = x[0].wrapping_add(x[1]);
}

/// Receiving side: the peer's key stream with a bounded look-ahead so a
/// dropped packet does not desynchronise us (ACE's `CryptoSystem`).
pub struct KeyStream {
    isaac: Isaac,
    current: u32,
    /// Keys we skipped past while searching; may still arrive (reordered
    /// or retransmitted packets keep their original XOR).
    pending: Vec<u32>,
}

impl KeyStream {
    pub const MAX_EFFORT: usize = 256;

    pub fn new(seed: u32) -> Self {
        let mut isaac = Isaac::new(seed);
        let current = isaac.next();
        KeyStream {
            isaac,
            current,
            pending: Vec::new(),
        }
    }

    /// True if `key` is the current key or one within the look-ahead
    /// window (or a previously skipped key). Consumes it on success.
    pub fn accept(&mut self, key: u32) -> bool {
        if self.current == key {
            self.current = self.isaac.next();
            return true;
        }
        if let Some(i) = self.pending.iter().position(|&k| k == key) {
            self.pending.swap_remove(i);
            return true;
        }
        let budget = Self::MAX_EFFORT.saturating_sub(self.pending.len());
        for _ in 0..budget {
            self.pending.push(self.current);
            self.current = self.isaac.next();
            if self.current == key {
                self.current = self.isaac.next();
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_seed_sensitive() {
        let mut a = Isaac::new(0x1234_5678);
        let mut b = Isaac::new(0x1234_5678);
        let mut c = Isaac::new(0x1234_5679);
        let va: Vec<u32> = (0..600).map(|_| a.next()).collect();
        let vb: Vec<u32> = (0..600).map(|_| b.next()).collect();
        let vc: Vec<u32> = (0..600).map(|_| c.next()).collect();
        assert_eq!(va, vb);
        assert_ne!(va, vc);
        // Crosses a scramble boundary without repeating.
        assert_ne!(va[0], va[256]);
    }

    #[test]
    fn keystream_tolerates_skips() {
        let mut peer = Isaac::new(42);
        let mut ks = KeyStream::new(42);
        let k0 = peer.next();
        let k1 = peer.next();
        let k2 = peer.next();
        assert!(ks.accept(k0));
        assert!(ks.accept(k2)); // skipped k1
        assert!(ks.accept(k1)); // late arrival still accepted
        assert!(!ks.accept(0xDEAD_BEEF));
    }
}

#[cfg(test)]
mod golden {
    use super::Isaac;

    /// First 300 keys per seed as produced by ACE's `ACE.Common.Cryptography.ISAAC`
    /// (`reference/tools/AceDump isaac <seed> 300`).
    #[test]
    fn matches_ace_vectors() {
        for (seed, text) in [
            (
                0x0000_0000u32,
                include_str!("../../../tests/golden/net/isaac_00000000.txt"),
            ),
            (
                0x1234_5678,
                include_str!("../../../tests/golden/net/isaac_12345678.txt"),
            ),
            (
                0xDEAD_BEEF,
                include_str!("../../../tests/golden/net/isaac_deadbeef.txt"),
            ),
        ] {
            let mut isaac = Isaac::new(seed);
            for (i, line) in text.lines().enumerate() {
                let want = u32::from_str_radix(line.trim(), 16).unwrap();
                assert_eq!(isaac.next(), want, "seed {seed:#x} index {i}");
            }
        }
    }
}
