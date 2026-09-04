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
    last_echo: Instant,
    last_ack: Instant,
    ack_dirty: bool,
    echo_pending: Option<f32>,
    last_nak: Option<Instant>,
}

impl Session {
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
            last_echo: now,
            last_ack: now,
            ack_dirty: false,
            echo_pending: None,
            last_nak: None,
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
        self.outgoing
            .push((Port::Primary, packet::build(h, &body, &[], 0)));
        self.state = State::LoginSent;
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
            match &mut self.recv_keys {
                Some(ks) => {
                    if !ks.accept(key) {
                        tracing::warn!(
                            "seq {}: encrypted checksum mismatch (key {key:#x})",
                            p.header.sequence
                        );
                        return;
                    }
                }
                None => {
                    tracing::warn!("encrypted packet before handshake");
                    return;
                }
            }
        } else if key != 0 {
            tracing::warn!("seq {}: plain checksum mismatch", p.header.sequence);
            return;
        }

        // Handshake and control fields that don't need ordering.
        if let Some(cr) = p.optional.connect_request {
            if self.state == State::LoginSent {
                self.client_id = cr.client_id as u16;
                self.cookie = cr.cookie;
                self.send_keys = Some(Isaac::new(cr.client_seed));
                self.recv_keys = Some(KeyStream::new(cr.server_seed));
                self.last_recv = p.header.sequence;
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
                self.last_echo = now;
                self.last_ack = now;
                self.events.push(Event::Connected {
                    client_id: self.client_id,
                });
            }
            return;
        }
        if let Some((code, table)) = p.optional.net_error.or(p.optional.net_error_disconnect) {
            self.state = State::Terminated;
            self.events
                .push(Event::Terminated(format!("net error {code:#x}/{table:#x}")));
            return;
        }
        if p.header.has(flags::DISCONNECT) {
            self.state = State::Terminated;
            self.events.push(Event::Terminated("disconnect".into()));
            return;
        }
        for seq in &p.optional.request_retransmit {
            match self.sent.get(seq) {
                Some(bytes) => {
                    let mut b = bytes.clone();
                    let mut h = Header::parse(&b).unwrap();
                    h.flags |= flags::RETRANSMISSION;
                    // Retransmissions keep their original checksum/XOR.
                    h.checksum = Header::parse(&b).unwrap().checksum;
                    h.write(&mut b[..packet::HEADER_SIZE]);
                    self.outgoing.push((Port::Primary, b));
                }
                None => tracing::warn!("server asked for uncached seq {seq}"),
            }
        }
        if let Some(ack) = p.optional.ack_sequence {
            self.sent.retain(|s, _| *s > ack);
        }
        if let Some(t) = p.optional.echo_request {
            self.echo_pending = Some(t);
        }

        // Ordering of data packets.
        let seq = p.header.sequence;
        let control_only = p.fragments.is_empty();
        if control_only {
            // Acks/time syncs may repeat sequences; nothing to order.
            if seq > self.last_recv
                && p.header.flags
                    & !(flags::ENCRYPTED_CHECKSUM
                        | flags::ACK_SEQUENCE
                        | flags::TIME_SYNC
                        | flags::ECHO_RESPONSE
                        | flags::ECHO_REQUEST
                        | flags::FLOW)
                    == 0
            {
                // pure control packets still advance the sequence
                self.last_recv = seq;
                self.ack_dirty = true;
            }
            return;
        }
        if seq <= self.last_recv {
            tracing::debug!("duplicate seq {seq}");
            return;
        }
        if seq != self.last_recv + 1 {
            self.out_of_order.insert(seq, p);
            // Like ACE: only ask once the gap is at least two packets, and
            // at most once a second, so plain reordering fixes itself.
            let want: Vec<u32> = (self.last_recv + 1..seq)
                .filter(|s| !self.out_of_order.contains_key(s))
                .collect();
            let rate_ok = self
                .last_nak
                .is_none_or(|t| now - t >= Duration::from_secs(1));
            if !want.is_empty() && seq >= self.last_recv + 3 && rate_ok {
                self.last_nak = Some(now);
                let mut body = (want.len() as u32).to_le_bytes().to_vec();
                for s in &want {
                    body.extend_from_slice(&s.to_le_bytes());
                }
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
            return;
        }
        self.handle_ordered(p);
        while let Some(next) = self.out_of_order.remove(&(self.last_recv + 1)) {
            self.handle_ordered(next);
        }
    }

    fn handle_ordered(&mut self, p: Packet) {
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
        if let Some((op, _)) = messages::split(&msg) {
            match op {
                messages::opcode::DDD_INTERROGATION => {
                    let resp = messages::ddd_interrogation_response(&self.cfg.dats);
                    self.send_message(queue::DATABASE, resp);
                }
                messages::opcode::ACCOUNT_BOOT | messages::opcode::CHARACTER_ERROR => {
                    self.state = State::Terminated;
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
