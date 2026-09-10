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
                // Indexed on purpose: k walks three arrays at once and
                // the published algorithm is written this way. Turning
                // it into an iterator chain would read further from the
                // spec it is checked against, in code whose only job is
                // to agree with the server bit for bit.
                #[allow(clippy::needless_range_loop)]
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

    /// The next word of the keystream. Named for the cipher, not for
    /// `Iterator`: this is a keystream, not a sequence anyone should
    /// be able to `collect`.
    #[allow(clippy::should_implement_trait)]
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

/// Receiving side: a sliding window over the peer's key stream.
///
/// Every encrypted packet carries one key from the peer's ISAAC stream, in
/// send order. On a lossless link we consume them one at a time. On a real
/// link packets are lost, reordered and duplicated, so the window holds
/// keys we generated but nobody has claimed yet, and remembers the keys we
/// already used.
///
/// Two rules keep the window from wedging shut, which is what a plain port
/// of ACE's `CryptoSystem` does:
///
/// * the look ahead never runs more than [`KeyStream::MAX_EFFORT`] keys past
///   the newest key we accepted, so unrecognised packets cannot walk the
///   stream away from the peer;
/// * unclaimed keys are dropped once they fall [`BACKLOG`] behind, oldest
///   first, instead of filling a fixed budget and failing every packet from
///   then on.
pub struct KeyStream {
    isaac: Isaac,
    /// Generated but unused keys with their index in the stream, oldest
    /// first. The front is the next key a lossless peer will send.
    window: std::collections::VecDeque<(u64, u32)>,
    /// Index of the next key `isaac` will produce.
    next_index: u64,
    /// One past the index of the newest key we accepted.
    live: u64,
    /// Keys already used, oldest first: lets a duplicated or twice
    /// retransmitted packet verify without disturbing the window.
    recent: std::collections::VecDeque<u32>,
    stats: KeyStats,
}

/// Unclaimed keys are dropped once they are this far behind the newest key
/// we accepted. A retransmit older than this is given up on; the session
/// keeps running rather than failing every later packet.
const BACKLOG: usize = 256;
/// How many used keys we remember for duplicate detection.
const RECENT: usize = 256;

/// Counters for logging; see [`KeyStream::stats`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct KeyStats {
    /// Keys that fell out of the window unclaimed (packets lost for good).
    pub lost: u64,
    /// Packets whose key we had already used.
    pub duplicates: u64,
    /// Packets whose key was nowhere in the window.
    pub rejected: u64,
}

impl KeyStream {
    /// How far past the newest accepted key we will look ahead. ACE uses
    /// the same number for its own search.
    pub const MAX_EFFORT: usize = 256;

    pub fn new(seed: u32) -> Self {
        let mut ks = KeyStream {
            isaac: Isaac::new(seed),
            window: std::collections::VecDeque::new(),
            next_index: 0,
            live: 0,
            recent: std::collections::VecDeque::new(),
            stats: KeyStats::default(),
        };
        ks.fill();
        ks
    }

    pub fn stats(&self) -> KeyStats {
        self.stats
    }

    /// How many generated keys are still waiting to be claimed. One on a
    /// healthy connection; it grows with loss and shrinks again as the
    /// stale entries age out.
    pub fn window_len(&self) -> usize {
        self.window.len()
    }

    /// True if `key` is one this peer could have sent: the next key, one
    /// within the look-ahead window, a key we skipped past earlier (a
    /// retransmission keeps its original key), or one we have just used (a
    /// duplicate). Consumes it on success.
    pub fn accept(&mut self, key: u32) -> bool {
        // The lossless case: exactly the next key in the stream.
        if self.window.front().is_some_and(|&(_, k)| k == key) {
            let (idx, _) = self.window.pop_front().expect("front checked");
            self.consumed(idx, key);
            return true;
        }
        // A key we skipped over: a reordered or retransmitted packet.
        if let Some(i) = self.window.iter().position(|&(_, k)| k == key) {
            let (idx, _) = self.window.remove(i).expect("index from position");
            self.consumed(idx, key);
            return true;
        }
        // A packet we have already seen. The session drops it by sequence;
        // recognising it here stops a duplicate from costing a full search.
        if self.recent.contains(&key) {
            self.stats.duplicates += 1;
            return true;
        }
        // Look ahead for keys the peer has sent but we have not seen,
        // keeping everything we pass over: those packets may still arrive.
        while self.next_index < self.live + Self::MAX_EFFORT as u64 {
            let idx = self.next_index;
            let k = self.isaac.next();
            self.next_index += 1;
            if k == key {
                self.consumed(idx, key);
                return true;
            }
            self.window.push_back((idx, k));
        }
        self.stats.rejected += 1;
        self.trim();
        false
    }

    /// Book-keeping after `key` at stream index `idx` was accepted.
    fn consumed(&mut self, idx: u64, key: u32) {
        self.live = self.live.max(idx + 1);
        self.recent.push_back(key);
        while self.recent.len() > RECENT {
            self.recent.pop_front();
        }
        self.trim();
        self.fill();
    }

    /// Forget unclaimed keys that have fallen too far behind.
    fn trim(&mut self) {
        let cutoff = self.live.saturating_sub(BACKLOG as u64);
        while let Some(&(idx, _)) = self.window.front() {
            if idx >= cutoff {
                break;
            }
            self.window.pop_front();
            self.stats.lost += 1;
        }
    }

    /// Keep the next key materialised so the lossless path is one compare.
    fn fill(&mut self) {
        if self.window.is_empty() && self.next_index < self.live + Self::MAX_EFFORT as u64 {
            let idx = self.next_index;
            let k = self.isaac.next();
            self.next_index += 1;
            self.window.push_back((idx, k));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A peer that hands out keys in send order, like ACE's server ISAAC.
    fn peer(seed: u32, n: usize) -> Vec<u32> {
        let mut p = Isaac::new(seed);
        (0..n).map(|_| p.next()).collect()
    }

    #[test]
    fn deterministic_and_seed_sensitive() {
        let va = peer(0x1234_5678, 600);
        let vb = peer(0x1234_5678, 600);
        let vc = peer(0x1234_5679, 600);
        assert_eq!(va, vb);
        assert_ne!(va, vc);
        // Crosses a scramble boundary without repeating.
        assert_ne!(va[0], va[256]);
    }

    #[test]
    fn keystream_tolerates_skips() {
        let k = peer(42, 3);
        let mut ks = KeyStream::new(42);
        assert!(ks.accept(k[0]));
        assert!(ks.accept(k[2])); // skipped k1
        assert!(ks.accept(k[1])); // late arrival still accepted
        assert!(!ks.accept(0xDEAD_BEEF));
    }

    /// The zero-loss path must consume exactly one key per packet.
    #[test]
    fn lossless_stream_is_exact() {
        let k = peer(7, 4000);
        let mut ks = KeyStream::new(7);
        for (i, &key) in k.iter().enumerate() {
            assert!(ks.accept(key), "key {i} rejected");
        }
        assert_eq!(ks.window_len(), 1, "one key of look ahead, no backlog");
        assert_eq!(ks.stats(), KeyStats::default(), "nothing lost or skipped");
    }

    /// A run of unrecognised packets used to consume 256 keys each until the
    /// window latched shut and every later packet failed. It must recover.
    #[test]
    fn survives_a_burst_of_garbage() {
        let k = peer(11, 600);
        let mut ks = KeyStream::new(11);
        for &key in &k[..50] {
            assert!(ks.accept(key));
        }
        for i in 0..500u32 {
            assert!(!ks.accept(0x8000_0000 | i), "garbage {i} accepted");
        }
        for (i, &key) in k[50..].iter().enumerate() {
            assert!(ks.accept(key), "key {} rejected after garbage", i + 50);
        }
        assert_eq!(ks.stats().rejected, 500);
    }

    /// Interleaving garbage with real traffic must not degrade over time.
    #[test]
    fn garbage_interleaved_with_traffic() {
        let k = peer(13, 3000);
        let mut ks = KeyStream::new(13);
        for (i, &key) in k.iter().enumerate() {
            if i % 7 == 0 {
                assert!(!ks.accept(0xC000_0000 ^ i as u32));
            }
            assert!(ks.accept(key), "key {i} rejected");
        }
    }

    /// Steady loss over a long session: everything that arrives is accepted,
    /// and the window does not grow without bound.
    #[test]
    fn steady_loss_over_a_long_session() {
        let k = peer(17, 20_000);
        let mut ks = KeyStream::new(17);
        let mut delivered = 0;
        for (i, &key) in k.iter().enumerate() {
            if i % 10 == 3 {
                continue; // lost in the network
            }
            assert!(ks.accept(key), "key {i} rejected after {delivered} good");
            delivered += 1;
        }
        assert!(
            ks.window_len() <= BACKLOG + 1,
            "window grew to {}",
            ks.window_len()
        );
        assert!(ks.stats().lost > 0, "stale keys should age out");
    }

    /// Reordering inside a window, in the worst order: newest first.
    #[test]
    fn reordering_in_reverse() {
        let k = peer(19, 128);
        let mut ks = KeyStream::new(19);
        for &key in k.iter().rev() {
            assert!(ks.accept(key));
        }
        // The stream carries on from where it got to.
        let mut p = Isaac::new(19);
        for _ in 0..128 {
            p.next();
        }
        assert!(ks.accept(p.next()));
    }

    /// Duplicated packets (server resend racing our own request) verify and
    /// cost nothing: the window must not move.
    #[test]
    fn duplicates_are_recognised() {
        let k = peer(23, 40);
        let mut ks = KeyStream::new(23);
        for &key in &k[..20] {
            assert!(ks.accept(key));
        }
        let before = ks.window_len();
        for &key in &k[..20] {
            assert!(ks.accept(key), "duplicate rejected");
        }
        assert_eq!(ks.window_len(), before, "duplicates moved the window");
        assert_eq!(ks.stats().duplicates, 20);
        for &key in &k[20..] {
            assert!(ks.accept(key));
        }
    }

    /// A retransmission carries its original key and can arrive well after
    /// the packets that followed it.
    #[test]
    fn late_retransmission_within_the_backlog() {
        let k = peer(29, 400);
        let mut ks = KeyStream::new(29);
        assert!(ks.accept(k[0]));
        for &key in &k[2..200] {
            assert!(ks.accept(key));
        }
        assert!(ks.accept(k[1]), "retransmit of an early packet");
    }

    /// The look ahead is bounded: a key far past the live position is
    /// refused, and that refusal does not disturb the live window.
    #[test]
    fn look_ahead_is_bounded() {
        let k = peer(31, 2000);
        let mut ks = KeyStream::new(31);
        assert!(ks.accept(k[0]));
        assert!(
            !ks.accept(k[1 + KeyStream::MAX_EFFORT]),
            "beyond look ahead"
        );
        for (i, &key) in k[1..1 + KeyStream::MAX_EFFORT].iter().enumerate() {
            assert!(ks.accept(key), "key {} rejected", i + 1);
        }
        for (i, &key) in k[1 + KeyStream::MAX_EFFORT..].iter().enumerate() {
            assert!(
                ks.accept(key),
                "key {} rejected",
                i + 1 + KeyStream::MAX_EFFORT
            );
        }
    }

    /// Loss, reordering, duplication and garbage all at once for a long run.
    #[test]
    fn survives_a_hostile_link() {
        let k = peer(37, 30_000);
        let mut ks = KeyStream::new(37);
        let mut rng = 0x1234_5678u32;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            rng
        };
        let mut held: Vec<u32> = Vec::new();
        let mut accepted = 0;
        for (i, &key) in k.iter().enumerate() {
            match next() % 16 {
                // Lost for good.
                0 => continue,
                // Delayed: delivered a few packets later, out of order.
                1 => {
                    held.push(key);
                    continue;
                }
                // Garbage from a stale connection.
                2 => {
                    ks.accept(next());
                }
                // Duplicated.
                3 => {
                    assert!(ks.accept(key), "key {i} rejected");
                    assert!(ks.accept(key), "duplicate of {i} rejected");
                    accepted += 1;
                    continue;
                }
                _ => {}
            }
            assert!(ks.accept(key), "key {i} rejected");
            accepted += 1;
            if held.len() >= 4 {
                for h in held.drain(..) {
                    assert!(ks.accept(h), "delayed packet rejected");
                    accepted += 1;
                }
            }
        }
        assert!(accepted > 20_000, "only {accepted} packets got through");
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
