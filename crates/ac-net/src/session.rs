//! Client-side session state machine (sans-IO).
//!
//! Feed it datagrams with [`Session::receive`] and time with
//! [`Session::poll`]; drain [`Session::outgoing`] to the sockets and
//! [`Session::events`] to the game. Login flow:
//!
//! ```text
//! C2S :9000  LoginRequest (seq 0, plain)
//! S2C        ConnectRequest (cookie, client id, seeds)
//! C2S :9001  ConnectResponse (cookie)            -> Connected
//! S2C        ServerName, CharacterList, DDD_Interrogation
//! C2S        DDD_InterrogationResponse
//! S2C        DDD_EndDDD
//! C2S        CharacterEnterWorldRequest -> S2C ServerReady -> C2S CharacterEnterWorld
//! ```

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use crate::isaac::{Isaac, KeyStream};
use crate::messages::{self, queue, DatIteration};
use crate::packet::{
    self, flags, Fragment, FragmentHeader, Header, Packet, MAX_FRAGMENT_DATA, MAX_PAYLOAD,
};

/// How long a hole in the incoming sequence is left alone before we ask
/// for the missing packet. A packet that was merely reordered normally
/// turns up well inside this, so the delay costs nothing and saves a
/// pointless request.
const NAK_DELAY: Duration = Duration::from_millis(250);
/// Floor on the gap between retransmit requests, the same rate ACE uses.
const NAK_INTERVAL: Duration = Duration::from_secs(1);
/// ACE reads at most this many sequence numbers from a request ((464-4)/4).
const MAX_NAK_SEQS: usize = 115;
/// Packets held back while waiting for a hole to fill. Past this the hole
/// is never going to close, so drop the newest rather than grow forever.
const MAX_OUT_OF_ORDER: usize = 512;
/// Checksum mismatches are warned about at most this often, with a running
/// count, so a burst is visible in the log without a warn per packet.
const MISMATCH_WARN_INTERVAL: Duration = Duration::from_secs(5);
/// Warn once when the server has gone quiet for this long.
const STALL_WARN: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    LoginSent,
    Connected,
    Terminated,
}

/// Which server port a datagram goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Port {
    /// The login/C2S port (9000).
    Primary,
    /// The +1 port used for ConnectResponse.
    Secondary,
}

#[derive(Debug)]
pub enum Event {
    /// Handshake done; server-assigned client id.
    Connected { client_id: u16 },
    /// A complete game message (opcode + body).
    Message(Vec<u8>),
    /// The server closed the session (NetError etc.).
    Terminated(String),
}

pub struct Config {
    pub account: String,
    pub password: String,
    /// DAT iterations reported in the DDD response.
    pub dats: Vec<DatIteration>,
    /// Send an EchoRequest at this interval once connected.
    pub echo_interval: Duration,
    pub ack_interval: Duration,
}

struct Partial {
    count: u16,
    parts: BTreeMap<u16, Vec<u8>>,
}

pub struct Session {
    cfg: Config,
    state: State,
    started: Instant,
    client_id: u16,
    cookie: u64,
    send_keys: Option<Isaac>,
    recv_keys: Option<KeyStream>,
    /// Next outgoing packet sequence.
    seq: u32,
    /// Next outgoing fragment sequence.
    frag_seq: u32,
    /// Next outgoing GameAction sequence.
    action_seq: u32,
    /// Highest in-order server packet sequence processed.
    last_recv: u32,
    out_of_order: BTreeMap<u32, Packet>,
    partials: HashMap<u32, Partial>,
    next_frag: u32,
    early_frags: BTreeMap<u32, Vec<u8>>,
    /// Sent packets kept for retransmission requests.
    sent: BTreeMap<u32, Vec<u8>>,
    outgoing: Vec<(Port, Vec<u8>)>,
    events: Vec<Event>,
    pending_msgs: Vec<(u16, Vec<u8>)>,
    /// The server's clock: the last time it sent, and when that arrived.
    /// The world's time of day is read off this (see
    /// `Session::server_time`).
    server_time: Option<(f64, Instant)>,
    last_echo: Instant,
    last_ack: Instant,
    ack_dirty: bool,
    echo_pending: Option<f32>,
    last_nak: Option<Instant>,
    /// When the current hole in the incoming sequence opened. A single
    /// missing packet is requested once it has been missing for
    /// `NAK_DELAY`; a wider gap is requested straight away.
    gap_since: Option<Instant>,
    /// Encrypted packets we could not verify, and when we last said so.
    mismatches: u64,
    mismatch_warned: Option<Instant>,
    /// When the server last sent us anything, for stall diagnostics.
    last_traffic: Instant,
    stall_warned: bool,
    /// ConnectResponse retry schedule until the server sends data. ACE only
    /// accepts the response after it finishes password verification, which
    /// can be tens of milliseconds after it sent the ConnectRequest.
    connect_retry_at: Option<Instant>,
    connect_tries: u32,
    got_data: bool,
}

impl Session {
    /// The server's clock now: the last time it told us, carried forward
    /// by how long ago that was. `None` before the first sync. The world's
    /// time of day comes off this.
    pub fn server_time(&self) -> Option<f64> {
        let (t, at) = self.server_time?;
        Some(t + at.elapsed().as_secs_f64())
    }

    pub fn new(cfg: Config, now: Instant) -> Self {
        Session {
            cfg,
            state: State::Idle,
            started: now,
            client_id: 0,
            cookie: 0,
            send_keys: None,
            recv_keys: None,
            seq: 0,
            frag_seq: 1,
            action_seq: 1,
            last_recv: 0,
            out_of_order: BTreeMap::new(),
            partials: HashMap::new(),
            next_frag: 1,
            early_frags: BTreeMap::new(),
            sent: BTreeMap::new(),
            outgoing: Vec::new(),
            events: Vec::new(),
            pending_msgs: Vec::new(),
            server_time: None,
            last_echo: now,
            last_ack: now,
            ack_dirty: false,
            echo_pending: None,
            last_nak: None,
            gap_since: None,
            mismatches: 0,
            mismatch_warned: None,
            last_traffic: now,
            stall_warned: false,
            connect_retry_at: None,
            connect_tries: 0,
            got_data: false,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn client_id(&self) -> u16 {
        self.client_id
    }

    pub fn outgoing(&mut self) -> Vec<(Port, Vec<u8>)> {
        std::mem::take(&mut self.outgoing)
    }

    pub fn events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn elapsed_secs(&self, now: Instant) -> f32 {
        (now - self.started).as_secs_f32()
    }

    fn time_field(&self, now: Instant) -> u16 {
        (now - self.started).as_millis() as u16
    }

    /// Start the handshake.
    pub fn login(&mut self, now: Instant) {
        let body = messages::login_request(
            &self.cfg.account,
            &self.cfg.password,
            self.elapsed_secs(now) as u32,
        );
        let h = Header {
            sequence: 0,
            flags: flags::LOGIN_REQUEST,
            id: 0,
            time: self.time_field(now),
            iteration: 0,
            ..Default::default()
        };
        tracing::debug!("-> LoginRequest for {}", self.cfg.account);
        self.outgoing
            .push((Port::Primary, packet::build(h, &body, &[], 0)));
        self.state = State::LoginSent;
    }

    /// Send a clean disconnect so the server drops the session immediately
    /// instead of waiting for the timeout. Safe to call once, on exit.
    pub fn disconnect(&mut self, now: Instant) {
        if self.state != State::Connected {
            return;
        }
        let xor = self.send_keys.as_mut().map(|k| k.next()).unwrap_or(0);
        let h = Header {
            sequence: self.seq,
            flags: flags::ENCRYPTED_CHECKSUM | flags::DISCONNECT,
            id: self.client_id,
            time: self.time_field(now),
            iteration: 0,
            ..Default::default()
        };
        self.seq += 1;
        self.outgoing
            .push((Port::Primary, packet::build(h, &[], &[], xor)));
        self.state = State::Terminated;
    }

    /// Queue a game message for sending on the next `poll`.
    pub fn send_message(&mut self, queue: u16, msg: Vec<u8>) {
        self.pending_msgs.push((queue, msg));
    }

    /// Queue a GameAction (0xF7B1) with the next action sequence.
    pub fn send_action(&mut self, action: u32, body: &[u8]) {
        let seq = self.action_seq;
        self.action_seq += 1;
        self.send_message(queue::WEENIE, messages::game_action(seq, action, body));
    }

    /// Process one datagram from the server.
    pub fn receive(&mut self, datagram: &[u8], now: Instant) {
        let p = match Packet::parse(datagram) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("bad packet: {e}");
                return;
            }
        };
        // Checksum.
        let key = p.checksum_key();
        if p.header.has(flags::ENCRYPTED_CHECKSUM) {
            let ok = match &mut self.recv_keys {
                Some(ks) => ks.accept(key),
                None => {
                    tracing::warn!("encrypted packet before handshake");
                    return;
                }
            };
            if !ok {
                self.note_mismatch(now, p.header.sequence, key);
                return;
            }
        } else if key != 0 {
            tracing::warn!("seq {}: plain checksum mismatch", p.header.sequence);
            return;
        }
        // Verified: the server is alive.
        self.last_traffic = now;
        self.stall_warned = false;

        // Handshake and control fields that don't need ordering.
        if let Some(cr) = p.optional.connect_request {
            if self.state == State::LoginSent {
                self.client_id = cr.client_id as u16;
                self.cookie = cr.cookie;
                self.send_keys = Some(Isaac::new(cr.client_seed));
                self.recv_keys = Some(KeyStream::new(cr.server_seed));
                // ACE initialises its received-sequence counter to 1 and sends
                // the first encrypted data packet as sequence 2 (sequence 1 is
                // never used). Match that so the first data packet is in order.
                self.last_recv = 1;
                let h = Header {
                    sequence: 1,
                    flags: flags::CONNECT_RESPONSE,
                    id: self.client_id,
                    time: self.time_field(now),
                    iteration: 0,
                    ..Default::default()
                };
                self.seq = 2;
                self.outgoing.push((
                    Port::Secondary,
                    packet::build(h, &cr.cookie.to_le_bytes(), &[], 0),
                ));
                self.state = State::Connected;
                self.connect_retry_at = Some(now + Duration::from_millis(100));
                self.connect_tries = 1;
                self.last_echo = now;
                self.last_ack = now;
                self.events.push(Event::Connected {
                    client_id: self.client_id,
                });
            }
            return;
        }
        if let Some((code, table)) = p.optional.net_error.or(p.optional.net_error_disconnect) {
            let closing = p.optional.net_error_disconnect.is_some();
            let tail = if closing {
                " and closed the connection"
            } else {
                ""
            };
            self.terminate(format!(
                "server reported a net error{tail} (code {code:#x}, table {table:#x})"
            ));
            return;
        }
        if p.header.has(flags::DISCONNECT) {
            self.terminate("server sent a disconnect".into());
            return;
        }
        for seq in &p.optional.request_retransmit {
            match self.sent.get(seq) {
                Some(bytes) => {
                    let mut b = bytes.clone();
                    let was = Header::parse(&b).unwrap();
                    let mut h = was;
                    h.flags |= flags::RETRANSMISSION;
                    // A retransmission keeps its original ISAAC key: that is
                    // the key the server recorded for this sequence. But the
                    // checksum covers the header, so setting the flag
                    // invalidates it. `checksum - header hash` is the
                    // key-carrying half, which is unchanged; re-add it to the
                    // hash of the new header.
                    h.checksum = h.hash().wrapping_add(was.checksum.wrapping_sub(was.hash()));
                    h.write(&mut b[..packet::HEADER_SIZE]);
                    self.outgoing.push((Port::Primary, b));
                }
                None => tracing::warn!("server asked for uncached seq {seq}"),
            }
        }
        if let Some(t) = p.optional.time_sync {
            self.server_time = Some((t, Instant::now()));
        }
        if let Some(ack) = p.optional.ack_sequence {
            self.sent.retain(|s, _| *s > ack);
        }
        if let Some(t) = p.optional.echo_request {
            self.echo_pending = Some(t);
        }

        // Ordering of data packets.
        let seq = p.header.sequence;
        // A pure AckSequence packet reuses the sender's current sequence
        // number (ACE sends it without incrementing), so it carries no
        // ordering information; everything else consumes a sequence and is
        // ordered like data, even when it has no fragments.
        if p.header.flags & !flags::ENCRYPTED_CHECKSUM == flags::ACK_SEQUENCE {
            return;
        }
        if seq <= self.last_recv {
            tracing::debug!("duplicate seq {seq}");
            return;
        }
        if seq != self.last_recv + 1 {
            self.out_of_order.insert(seq, p);
            // A hole is never going to close once this many packets have
            // piled up behind it; keep the ones nearest the hole.
            while self.out_of_order.len() > MAX_OUT_OF_ORDER {
                let k = *self.out_of_order.keys().next_back().unwrap();
                self.out_of_order.remove(&k);
            }
            if self.gap_since.is_none() {
                self.gap_since = Some(now);
            }
            // Like ACE, ask straight away once at least two packets are
            // missing: that is loss, not reordering. A single missing
            // packet is left to `poll`, which asks after `NAK_DELAY` so a
            // reordered packet has a chance to arrive on its own.
            if seq >= self.last_recv + 3 {
                self.request_retransmit(now);
            }
            return;
        }
        self.handle_ordered(p);
        while let Some(next) = self.out_of_order.remove(&(self.last_recv + 1)) {
            self.handle_ordered(next);
        }
        if self.out_of_order.is_empty() {
            self.gap_since = None;
        }
    }

    /// Ask the server for the packets missing from the front of the stream.
    /// Rate limited to `NAK_INTERVAL`, so repeated calls while a hole stays
    /// open cost one request a second.
    fn request_retransmit(&mut self, now: Instant) {
        let Some(&top) = self.out_of_order.keys().next_back() else {
            return;
        };
        if self.last_nak.is_some_and(|t| now - t < NAK_INTERVAL) {
            return;
        }
        let want: Vec<u32> = (self.last_recv + 1..top)
            .filter(|s| !self.out_of_order.contains_key(s))
            .take(MAX_NAK_SEQS)
            .collect();
        if want.is_empty() {
            return;
        }
        self.last_nak = Some(now);
        tracing::debug!("-> RequestRetransmit {want:?}");
        let mut body = (want.len() as u32).to_le_bytes().to_vec();
        for s in &want {
            body.extend_from_slice(&s.to_le_bytes());
        }
        // The server only honours a request with a cleartext checksum, and
        // a request does not consume an outgoing sequence number.
        let h = Header {
            sequence: self.seq,
            flags: flags::REQUEST_RETRANSMIT,
            id: self.client_id,
            time: self.time_field(now),
            ..Default::default()
        };
        self.outgoing
            .push((Port::Primary, packet::build(h, &body, &[], 0)));
    }

    /// Count a packet whose checksum did not verify. On a lossy link the
    /// odd one is normal; a flood means the key stream has lost the server,
    /// so report bursts with a running count rather than one warn each.
    fn note_mismatch(&mut self, now: Instant, seq: u32, key: u32) {
        self.mismatches += 1;
        if self
            .mismatch_warned
            .is_some_and(|t| now - t < MISMATCH_WARN_INTERVAL)
        {
            tracing::debug!("seq {seq}: encrypted checksum mismatch (key {key:#x})");
            return;
        }
        self.mismatch_warned = Some(now);
        let unclaimed = self.recv_keys.as_ref().map_or(0, |k| k.window_len());
        let total = self.mismatches;
        tracing::warn!(
            "seq {seq}: encrypted checksum mismatch (key {key:#x}); \
             {total} so far, {unclaimed} keys awaiting a packet"
        );
    }

    /// End the session, logging why even if the caller drops the event.
    fn terminate(&mut self, why: String) {
        if self.state == State::Terminated {
            return;
        }
        tracing::warn!("session terminated: {why}");
        self.state = State::Terminated;
        self.events.push(Event::Terminated(why));
    }

    fn handle_ordered(&mut self, p: Packet) {
        self.got_data = true;
        self.connect_retry_at = None;
        self.last_recv = p.header.sequence;
        self.ack_dirty = true;
        for f in p.fragments {
            self.handle_fragment(f);
        }
    }

    fn handle_fragment(&mut self, f: Fragment) {
        let seq = f.header.sequence;
        let complete = if f.header.count <= 1 {
            Some(f.data)
        } else {
            let e = self.partials.entry(seq).or_insert_with(|| Partial {
                count: f.header.count,
                parts: BTreeMap::new(),
            });
            e.parts.entry(f.header.index).or_insert(f.data);
            if e.parts.len() as u16 == e.count {
                let p = self.partials.remove(&seq).unwrap();
                Some(p.parts.into_values().flatten().collect())
            } else {
                None
            }
        };
        let Some(msg) = complete else { return };
        if seq == self.next_frag || self.next_frag == 1 && seq == 0 {
            self.deliver(msg);
            self.next_frag = seq + 1;
            while let Some(m) = self.early_frags.remove(&self.next_frag) {
                self.deliver(m);
                self.next_frag += 1;
            }
        } else if seq > self.next_frag {
            self.early_frags.insert(seq, msg);
        } else {
            tracing::debug!("stale fragment {seq}");
        }
    }

    fn deliver(&mut self, msg: Vec<u8>) {
        // Login-phase messages we answer ourselves.
        if let Some((op, body)) = messages::split(&msg) {
            match op {
                messages::opcode::DDD_INTERROGATION => {
                    let resp = messages::ddd_interrogation_response(&self.cfg.dats);
                    self.send_message(queue::DATABASE, resp);
                }
                messages::opcode::ACCOUNT_BOOT => {
                    self.terminate("the server booted this account".into());
                }
                messages::opcode::CHARACTER_ERROR => {
                    let code = body
                        .get(..4)
                        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
                    let why = match code {
                        Some(c) => format!("{} (character error {c:#x})", character_error(c)),
                        None => "character error with no code".into(),
                    };
                    self.terminate(why);
                }
                _ => {}
            }
        }
        self.events.push(Event::Message(msg));
    }

    /// Time-driven work: flush queued messages, acks, echoes.
    pub fn poll(&mut self, now: Instant) {
        if self.state != State::Connected {
            return;
        }
        // A hole that has stayed open past `NAK_DELAY` is real loss, even
        // if only one packet is missing: ask for it. Without this a single
        // dropped packet stalls the in-order stream until the server
        // volunteers a resend, which it may never do.
        if self
            .gap_since
            .is_some_and(|t| now.saturating_duration_since(t) >= NAK_DELAY)
        {
            self.request_retransmit(now);
        }
        // The server talks constantly once we are in; silence means the
        // link or the session is gone.
        if !self.stall_warned && now.saturating_duration_since(self.last_traffic) >= STALL_WARN {
            self.stall_warned = true;
            tracing::warn!(
                "no packet from the server for {:.0?}; last in-order sequence {}, \
                 {} unverified packets so far",
                now - self.last_traffic,
                self.last_recv,
                self.mismatches
            );
        }
        if let Some(t) = self.connect_retry_at {
            if !self.got_data && now >= t {
                if self.connect_tries >= 20 {
                    self.terminate("server never accepted our ConnectResponse".into());
                    return;
                }
                let h = Header {
                    sequence: 1,
                    flags: flags::CONNECT_RESPONSE,
                    id: self.client_id,
                    time: self.time_field(now),
                    iteration: 0,
                    ..Default::default()
                };
                self.outgoing.push((
                    Port::Secondary,
                    packet::build(h, &self.cookie.to_le_bytes(), &[], 0),
                ));
                self.connect_tries += 1;
                self.connect_retry_at = Some(now + Duration::from_millis(250));
            }
        }
        // Build fragments from pending messages.
        let msgs = std::mem::take(&mut self.pending_msgs);
        let mut frags: Vec<Fragment> = Vec::new();
        for (q, m) in msgs {
            let seq = self.frag_seq;
            self.frag_seq += 1;
            let count = m.len().div_ceil(MAX_FRAGMENT_DATA).max(1) as u16;
            for (i, chunk) in m.chunks(MAX_FRAGMENT_DATA).enumerate() {
                frags.push(Fragment {
                    header: FragmentHeader {
                        sequence: seq,
                        id: 0x8000_0000 | seq,
                        count,
                        size: (packet::FRAGMENT_HEADER_SIZE + chunk.len()) as u16,
                        index: i as u16,
                        queue: q,
                    },
                    data: chunk.to_vec(),
                });
            }
        }
        let mut optional = Vec::new();
        let mut hflags = flags::ENCRYPTED_CHECKSUM;
        if self.ack_dirty && now - self.last_ack >= self.cfg.ack_interval {
            hflags |= flags::ACK_SEQUENCE;
            optional.extend_from_slice(&self.last_recv.to_le_bytes());
            self.ack_dirty = false;
            self.last_ack = now;
        }
        if let Some(t) = self.echo_pending.take() {
            hflags |= flags::ECHO_RESPONSE;
            optional.extend_from_slice(&t.to_le_bytes());
            optional.extend_from_slice(&0f32.to_le_bytes());
        } else if now - self.last_echo >= self.cfg.echo_interval {
            hflags |= flags::ECHO_REQUEST;
            optional.extend_from_slice(&self.elapsed_secs(now).to_le_bytes());
            self.last_echo = now;
        }
        if frags.is_empty() && optional.is_empty() {
            return;
        }
        // Pack fragments into datagrams.
        let mut batch: Vec<Fragment> = Vec::new();
        let mut used = optional.len();
        let mut first = true;
        let flush = |this: &mut Session,
                     batch: &mut Vec<Fragment>,
                     optional: &[u8],
                     hflags: u32,
                     now: Instant| {
            let mut fl = hflags;
            if !batch.is_empty() {
                fl |= flags::BLOB_FRAGMENTS;
            }
            let xor = this.send_keys.as_mut().map(|k| k.next()).unwrap_or(0);
            let h = Header {
                sequence: this.seq,
                flags: fl,
                id: this.client_id,
                time: this.time_field(now),
                iteration: 0,
                ..Default::default()
            };
            let dg = packet::build(h, optional, batch, xor);
            this.sent.insert(this.seq, dg.clone());
            this.seq += 1;
            this.outgoing.push((Port::Primary, dg));
            batch.clear();
        };
        for f in frags {
            let len = packet::FRAGMENT_HEADER_SIZE + f.data.len();
            if used + len > MAX_PAYLOAD && !batch.is_empty() {
                let opt = if first { optional.clone() } else { Vec::new() };
                let fl = if first {
                    hflags
                } else {
                    flags::ENCRYPTED_CHECKSUM
                };
                flush(self, &mut batch, &opt, fl, now);
                first = false;
                used = 0;
            }
            used += len;
            batch.push(f);
        }
        let opt = if first { optional.clone() } else { Vec::new() };
        let fl = if first {
            hflags
        } else {
            flags::ENCRYPTED_CHECKSUM
        };
        flush(self, &mut batch, &opt, fl, now);
        // Bound the retransmit cache.
        while self.sent.len() > 512 {
            let k = *self.sent.keys().next().unwrap();
            self.sent.remove(&k);
        }
    }
}

/// Human phrase for a CharacterError code, from the client's `CharError`
/// list (names as documented in ACE's `CharacterError` enum).
fn character_error(code: u32) -> &'static str {
    match code {
        0x01 => "that account is already logged on",
        0x03 => "the server could not read the account",
        0x04 | 0x08 => "the server disconnected",
        0x05 => "the server could not log the character off",
        0x06 => "the server could not delete the character",
        0x09 => "the account name was not valid",
        0x0A => "that account does not exist",
        0x0B => "the server refused to start the game",
        0x0C => "stress-test account",
        0x0D => "that character is already in the world",
        0x0E => "the account behind that character is missing",
        0x0F => "that character belongs to another account",
        0x10 => "that character is still in the world on the server",
        0x11 => "that character is too old for this server",
        0x12 => "that character is corrupt",
        0x13 => "the start server is down",
        0x14 => "the server could not place the character in the world",
        0x15 => "the logon server is full",
        0x17 => "that character is locked",
        0x18 => "the subscription has expired",
        _ => "the server refused the character",
    }
}
