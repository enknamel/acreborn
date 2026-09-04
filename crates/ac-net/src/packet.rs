//! Packet and fragment framing.
//!
//! ```text
//! packet  = header(20) [optional header fields...] [fragment*]
//! header  = u32 seq, u32 flags, u32 checksum, u16 id, u16 time, u16 size, u16 iteration
//! fragment= u32 seq, u32 id, u16 count, u16 size(incl. 16), u16 index, u16 queue, data
//! ```
//! `checksum` = hash32(header with checksum=0xBADD70DD) + (hash32(optional
//! bytes) + Σ fragment hashes) [^ ISAAC key when EncryptedChecksum].

use crate::hash32::hash32;

pub const HEADER_SIZE: usize = 20;
pub const FRAGMENT_HEADER_SIZE: usize = 16;
/// Max bytes after the packet header in one datagram.
pub const MAX_PAYLOAD: usize = 464;
pub const MAX_FRAGMENT_DATA: usize = MAX_PAYLOAD - FRAGMENT_HEADER_SIZE;
pub const CHECKSUM_PLACEHOLDER: u32 = 0xBADD_70DD;

pub mod flags {
    pub const RETRANSMISSION: u32 = 0x0000_0001;
    pub const ENCRYPTED_CHECKSUM: u32 = 0x0000_0002;
    pub const BLOB_FRAGMENTS: u32 = 0x0000_0004;
    pub const SERVER_SWITCH: u32 = 0x0000_0100;
    pub const LOGON_SERVER_ADDR: u32 = 0x0000_0200;
    pub const EMPTY_HEADER1: u32 = 0x0000_0400;
    pub const REFERRAL: u32 = 0x0000_0800;
    pub const REQUEST_RETRANSMIT: u32 = 0x0000_1000;
    pub const REJECT_RETRANSMIT: u32 = 0x0000_2000;
    pub const ACK_SEQUENCE: u32 = 0x0000_4000;
    pub const DISCONNECT: u32 = 0x0000_8000;
    pub const LOGIN_REQUEST: u32 = 0x0001_0000;
    pub const WORLD_LOGIN_REQUEST: u32 = 0x0002_0000;
    pub const CONNECT_REQUEST: u32 = 0x0004_0000;
    pub const CONNECT_RESPONSE: u32 = 0x0008_0000;
    pub const NET_ERROR: u32 = 0x0010_0000;
    pub const NET_ERROR_DISCONNECT: u32 = 0x0020_0000;
    pub const CICMD_COMMAND: u32 = 0x0040_0000;
    pub const TIME_SYNC: u32 = 0x0100_0000;
    pub const ECHO_REQUEST: u32 = 0x0200_0000;
    pub const ECHO_RESPONSE: u32 = 0x0400_0000;
    pub const FLOW: u32 = 0x0800_0000;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Header {
    pub sequence: u32,
    pub flags: u32,
    pub checksum: u32,
    pub id: u16,
    pub time: u16,
    pub size: u16,
    pub iteration: u16,
}

impl Header {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < HEADER_SIZE {
            return None;
        }
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        Some(Header {
            sequence: u32_at(0),
            flags: u32_at(4),
            checksum: u32_at(8),
            id: u16_at(12),
            time: u16_at(14),
            size: u16_at(16),
            iteration: u16_at(18),
        })
    }

    pub fn write(&self, out: &mut [u8]) {
        out[0..4].copy_from_slice(&self.sequence.to_le_bytes());
        out[4..8].copy_from_slice(&self.flags.to_le_bytes());
        out[8..12].copy_from_slice(&self.checksum.to_le_bytes());
        out[12..14].copy_from_slice(&self.id.to_le_bytes());
        out[14..16].copy_from_slice(&self.time.to_le_bytes());
        out[16..18].copy_from_slice(&self.size.to_le_bytes());
        out[18..20].copy_from_slice(&self.iteration.to_le_bytes());
    }

    /// Hash of the header with the checksum field replaced by the placeholder.
    pub fn hash(&self) -> u32 {
        let mut tmp = *self;
        tmp.checksum = CHECKSUM_PLACEHOLDER;
        let mut b = [0u8; HEADER_SIZE];
        tmp.write(&mut b);
        hash32(&b)
    }

    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FragmentHeader {
    pub sequence: u32,
    pub id: u32,
    pub count: u16,
    /// Including the 16-byte header.
    pub size: u16,
    pub index: u16,
    pub queue: u16,
}

impl FragmentHeader {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < FRAGMENT_HEADER_SIZE {
            return None;
        }
        Some(FragmentHeader {
            sequence: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            id: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            count: u16::from_le_bytes([b[8], b[9]]),
            size: u16::from_le_bytes([b[10], b[11]]),
            index: u16::from_le_bytes([b[12], b[13]]),
            queue: u16::from_le_bytes([b[14], b[15]]),
        })
    }

    pub fn write(&self, out: &mut [u8]) {
        out[0..4].copy_from_slice(&self.sequence.to_le_bytes());
        out[4..8].copy_from_slice(&self.id.to_le_bytes());
        out[8..10].copy_from_slice(&self.count.to_le_bytes());
        out[10..12].copy_from_slice(&self.size.to_le_bytes());
        out[12..14].copy_from_slice(&self.index.to_le_bytes());
        out[14..16].copy_from_slice(&self.queue.to_le_bytes());
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    pub header: FragmentHeader,
    pub data: Vec<u8>,
}

impl Fragment {
    pub fn hash(&self) -> u32 {
        let mut b = [0u8; FRAGMENT_HEADER_SIZE];
        self.header.write(&mut b);
        hash32(&b).wrapping_add(hash32(&self.data))
    }
}

/// Optional header fields the server can send, in wire order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Optional {
    pub server_switch: Option<[u8; 8]>,
    pub referral: Option<Vec<u8>>,
    pub request_retransmit: Vec<u32>,
    pub reject_retransmit: Vec<u32>,
    pub ack_sequence: Option<u32>,
    pub connect_request: Option<ConnectRequest>,
    pub net_error: Option<(u32, u32)>,
    pub net_error_disconnect: Option<(u32, u32)>,
    pub cicmd: Option<[u8; 8]>,
    pub time_sync: Option<f64>,
    pub echo_request: Option<f32>,
    pub echo_response: Option<(f32, f32)>,
    pub flow: Option<(u32, u16)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConnectRequest {
    pub server_time: f64,
    pub cookie: u64,
    pub client_id: u32,
    /// Seed for the server-to-client key stream.
    pub server_seed: u32,
    /// Seed for the client-to-server key stream.
    pub client_seed: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    pub header: Header,
    pub optional: Optional,
    /// Raw bytes of the optional fields (hashed as one block).
    pub optional_bytes: Vec<u8>,
    pub fragments: Vec<Fragment>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("datagram shorter than header")]
    Short,
    #[error("header size {0} exceeds datagram")]
    BadSize(u16),
    #[error("truncated optional header field {0:#x}")]
    TruncatedOptional(u32),
    #[error("bad fragment at offset {0}")]
    BadFragment(usize),
}

struct Cur<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize, flag: u32) -> Result<&'a [u8], ParseError> {
        if self.pos + n > self.b.len() {
            return Err(ParseError::TruncatedOptional(flag));
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u32(&mut self, flag: u32) -> Result<u32, ParseError> {
        let s = self.take(4, flag)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u16(&mut self, flag: u32) -> Result<u16, ParseError> {
        let s = self.take(2, flag)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }
    fn u64(&mut self, flag: u32) -> Result<u64, ParseError> {
        let s = self.take(8, flag)?;
        Ok(u64::from_le_bytes(s.try_into().unwrap()))
    }
    fn f32(&mut self, flag: u32) -> Result<f32, ParseError> {
        Ok(f32::from_bits(self.u32(flag)?))
    }
    fn f64(&mut self, flag: u32) -> Result<f64, ParseError> {
        Ok(f64::from_bits(self.u64(flag)?))
    }
}

impl Packet {
    /// Parse a server-to-client datagram.
    pub fn parse(datagram: &[u8]) -> Result<Self, ParseError> {
        let header = Header::parse(datagram).ok_or(ParseError::Short)?;
        let body = &datagram[HEADER_SIZE..];
        if header.size as usize > body.len() {
            return Err(ParseError::BadSize(header.size));
        }
        let body = &body[..header.size as usize];
        let mut c = Cur { b: body, pos: 0 };
        let mut o = Optional::default();
        use flags::*;
        if header.has(SERVER_SWITCH) {
            o.server_switch = Some(c.take(8, SERVER_SWITCH)?.try_into().unwrap());
        }
        if header.has(REFERRAL) {
            o.referral = Some(c.take(32, REFERRAL)?.to_vec());
        }
        if header.has(REQUEST_RETRANSMIT) {
            let n = c.u32(REQUEST_RETRANSMIT)?;
            for _ in 0..n {
                o.request_retransmit.push(c.u32(REQUEST_RETRANSMIT)?);
            }
        }
        if header.has(REJECT_RETRANSMIT) {
            let n = c.u32(REJECT_RETRANSMIT)?;
            for _ in 0..n {
                o.reject_retransmit.push(c.u32(REJECT_RETRANSMIT)?);
            }
        }
        if header.has(ACK_SEQUENCE) {
            o.ack_sequence = Some(c.u32(ACK_SEQUENCE)?);
        }
        if header.has(LOGIN_REQUEST) {
            // Client-only: the whole remaining body is the login request.
            let n = body.len() - c.pos;
            c.take(n, LOGIN_REQUEST)?;
        }
        if header.has(WORLD_LOGIN_REQUEST) {
            c.take(8, WORLD_LOGIN_REQUEST)?;
        }
        if header.has(CONNECT_REQUEST) {
            let server_time = c.f64(CONNECT_REQUEST)?;
            let cookie = c.u64(CONNECT_REQUEST)?;
            let client_id = c.u32(CONNECT_REQUEST)?;
            let server_seed = c.u32(CONNECT_REQUEST)?;
            let client_seed = c.u32(CONNECT_REQUEST)?;
            let _pad = c.u32(CONNECT_REQUEST)?;
            o.connect_request = Some(ConnectRequest {
                server_time,
                cookie,
                client_id,
                server_seed,
                client_seed,
            });
        }
        if header.has(CONNECT_RESPONSE) {
            // Client-only: the connection cookie.
            c.take(8, CONNECT_RESPONSE)?;
        }
        if header.has(NET_ERROR) {
            o.net_error = Some((c.u32(NET_ERROR)?, c.u32(NET_ERROR)?));
        }
        if header.has(NET_ERROR_DISCONNECT) {
            o.net_error_disconnect =
                Some((c.u32(NET_ERROR_DISCONNECT)?, c.u32(NET_ERROR_DISCONNECT)?));
        }
        if header.has(CICMD_COMMAND) {
            o.cicmd = Some(c.take(8, CICMD_COMMAND)?.try_into().unwrap());
        }
        if header.has(TIME_SYNC) {
            o.time_sync = Some(c.f64(TIME_SYNC)?);
        }
        if header.has(ECHO_REQUEST) {
            o.echo_request = Some(c.f32(ECHO_REQUEST)?);
        }
        if header.has(ECHO_RESPONSE) {
            o.echo_response = Some((c.f32(ECHO_RESPONSE)?, c.f32(ECHO_RESPONSE)?));
        }
        if header.has(FLOW) {
            o.flow = Some((c.u32(FLOW)?, c.u16(FLOW)?));
        }
        let optional_bytes = body[..c.pos].to_vec();
        let mut fragments = Vec::new();
        if header.has(BLOB_FRAGMENTS) {
            while c.pos < body.len() {
                let fh =
                    FragmentHeader::parse(&body[c.pos..]).ok_or(ParseError::BadFragment(c.pos))?;
                let size = fh.size as usize;
                if size < FRAGMENT_HEADER_SIZE || c.pos + size > body.len() {
                    return Err(ParseError::BadFragment(c.pos));
                }
                let data = body[c.pos + FRAGMENT_HEADER_SIZE..c.pos + size].to_vec();
                fragments.push(Fragment { header: fh, data });
                c.pos += size;
            }
        }
        Ok(Packet {
            header,
            optional: o,
            optional_bytes,
            fragments,
        })
    }

    /// Hash of everything after the header (what the ISAAC key XORs).
    pub fn payload_hash(&self) -> u32 {
        let mut h = hash32(&self.optional_bytes);
        for f in &self.fragments {
            h = h.wrapping_add(f.hash());
        }
        h
    }

    /// For an encrypted packet, the ISAAC key the sender used; for a plain
    /// packet, 0 if the checksum is valid.
    pub fn checksum_key(&self) -> u32 {
        self.header.checksum.wrapping_sub(self.header.hash()) ^ self.payload_hash()
    }
}

/// Build an outgoing datagram. `optional` is written verbatim after the
/// header; `xor` is the ISAAC key when `flags` has EncryptedChecksum.
pub fn build(mut header: Header, optional: &[u8], fragments: &[Fragment], xor: u32) -> Vec<u8> {
    let mut out = vec![0u8; HEADER_SIZE];
    out.extend_from_slice(optional);
    let mut payload_hash = hash32(optional);
    for f in fragments {
        let mut fb = [0u8; FRAGMENT_HEADER_SIZE];
        f.header.write(&mut fb);
        out.extend_from_slice(&fb);
        out.extend_from_slice(&f.data);
        payload_hash = payload_hash.wrapping_add(f.hash());
    }
    header.size = (out.len() - HEADER_SIZE) as u16;
    header.checksum = header.hash().wrapping_add(payload_hash ^ xor);
    header.write(&mut out[..HEADER_SIZE]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_plain_packet() {
        let frag = Fragment {
            header: FragmentHeader {
                sequence: 1,
                id: 0x8000_0001,
                count: 1,
                size: (FRAGMENT_HEADER_SIZE + 8) as u16,
                index: 0,
                queue: 9,
            },
            data: vec![0xF7, 0xC8, 0, 0, 1, 2, 3, 4],
        };
        let h = Header {
            flags: flags::BLOB_FRAGMENTS | flags::ACK_SEQUENCE,
            sequence: 5,
            id: 7,
            ..Default::default()
        };
        let dg = build(h, &42u32.to_le_bytes(), &[frag.clone()], 0);
        let p = Packet::parse(&dg).unwrap();
        assert_eq!(p.header.sequence, 5);
        assert_eq!(p.optional.ack_sequence, Some(42));
        assert_eq!(p.fragments, vec![frag]);
        assert_eq!(p.checksum_key(), 0, "plain checksum must verify");
    }

    #[test]
    fn encrypted_key_recovers() {
        let h = Header {
            flags: flags::ENCRYPTED_CHECKSUM | flags::TIME_SYNC,
            sequence: 9,
            ..Default::default()
        };
        let dg = build(h, &1.5f64.to_le_bytes(), &[], 0xCAFE_BABE);
        let p = Packet::parse(&dg).unwrap();
        assert_eq!(p.optional.time_sync, Some(1.5));
        assert_eq!(p.checksum_key(), 0xCAFE_BABE);
    }

    #[test]
    fn connect_request_layout() {
        let mut body = Vec::new();
        body.extend_from_slice(&123.0f64.to_le_bytes());
        body.extend_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
        body.extend_from_slice(&0x2Au32.to_le_bytes());
        body.extend_from_slice(&0xAAAA_AAAAu32.to_le_bytes());
        body.extend_from_slice(&0xBBBB_BBBBu32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        let dg = build(
            Header {
                flags: flags::CONNECT_REQUEST,
                ..Default::default()
            },
            &body,
            &[],
            0,
        );
        let p = Packet::parse(&dg).unwrap();
        let cr = p.optional.connect_request.unwrap();
        assert_eq!(cr.client_id, 0x2A);
        assert_eq!(cr.server_seed, 0xAAAA_AAAA);
        assert_eq!(cr.client_seed, 0xBBBB_BBBB);
        assert_eq!(cr.cookie, 0x1122_3344_5566_7788);
    }
}
