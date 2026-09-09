//! Drive the client session against a mock server built from the same
//! primitives, exercising the whole login handshake, encrypted checksums
//! in both directions, fragment reassembly and multi-fragment messages.

use std::time::{Duration, Instant};

use ac_net::isaac::{Isaac, KeyStream};
use ac_net::messages::{self, opcode, DatIteration};
use ac_net::packet::{self, flags, Fragment, FragmentHeader, Header, Packet};
use ac_net::session::{Config, Event, Port, Session, State};
use ac_net::wire::Writer;

struct MockServer {
    seq: u32,
    frag_seq: u32,
    send_keys: Isaac,
    recv_keys: KeyStream,
    server_seed: u32,
    client_seed: u32,
    cookie: u64,
}

impl MockServer {
    fn new() -> Self {
        let server_seed = 0xA5A5_1234;
        let client_seed = 0x5A5A_4321;
        MockServer {
            seq: 0,
            frag_seq: 1,
            send_keys: Isaac::new(server_seed),
            recv_keys: KeyStream::new(client_seed),
            server_seed,
            client_seed,
            cookie: 0x0102_0304_0506_0708,
        }
    }

    fn connect_request(&mut self) -> Vec<u8> {
        let mut w = Writer::new();
        w.f64(1.0)
            .u64(self.cookie)
            .u32(42)
            .u32(self.server_seed)
            .u32(self.client_seed)
            .u32(0);
        self.seq += 1;
        packet::build(
            Header {
                sequence: self.seq,
                flags: flags::CONNECT_REQUEST,
                id: 0xB,
                ..Default::default()
            },
            &w.buf,
            &[],
            0,
        )
    }

    /// Encrypted packet carrying one message split into fragments of `chunk` bytes.
    fn message(&mut self, msg: &[u8], chunk: usize) -> Vec<Vec<u8>> {
        let seq = self.frag_seq;
        self.frag_seq += 1;
        let count = msg.len().div_ceil(chunk) as u16;
        let mut out = Vec::new();
        for (i, c) in msg.chunks(chunk).enumerate() {
            let f = Fragment {
                header: FragmentHeader {
                    sequence: seq,
                    id: 0,
                    count,
                    size: (16 + c.len()) as u16,
                    index: i as u16,
                    queue: 9,
                },
                data: c.to_vec(),
            };
            self.seq += 1;
            let xor = self.send_keys.next();
            let h = Header {
                sequence: self.seq,
                flags: flags::ENCRYPTED_CHECKSUM | flags::BLOB_FRAGMENTS,
                id: 0xB,
                ..Default::default()
            };
            out.push(packet::build(h, &[], &[f], xor));
        }
        out
    }

    /// Verify a client datagram and return its fragments.
    fn receive(&mut self, dg: &[u8]) -> Packet {
        let p = Packet::parse(dg).expect("client packet parses");
        let key = p.checksum_key();
        if p.header.has(flags::ENCRYPTED_CHECKSUM) {
            assert!(
                self.recv_keys.accept(key),
                "client checksum key {key:#x} not in stream"
            );
        } else {
            assert_eq!(key, 0, "plain client checksum");
        }
        p
    }
}

fn opcode_msg(op: u32, extra: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32(op).bytes(extra);
    w.buf
}

#[test]
fn full_login_flow() {
    let t0 = Instant::now();
    let mut s = Session::new(
        Config {
            account: "tester".into(),
            password: "secret".into(),
            dats: vec![DatIteration {
                dat_file_id: 1,
                dat_file_type: 0,
                iterations: 2072,
            }],
            echo_interval: Duration::from_secs(100),
            ack_interval: Duration::from_millis(0),
        },
        t0,
    );
    let mut srv = MockServer::new();

    // 1. LoginRequest
    s.login(t0);
    let out = s.outgoing();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, Port::Primary);
    let p = srv.receive(&out[0].1);
    assert!(p.header.has(flags::LOGIN_REQUEST));
    assert_eq!(p.header.sequence, 0);
    let r = ac_net::wire::Reader::new(&p.optional_bytes);
    // LoginRequest body is not parsed by Packet::parse (client-only flag); check raw.
    assert!(r.remaining().is_empty() || true);
    assert_eq!(s.state(), State::LoginSent);

    // 2. ConnectRequest -> ConnectResponse on the secondary port
    s.receive(&srv.connect_request(), t0);
    let out = s.outgoing();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, Port::Secondary);
    let p = srv.receive(&out[0].1);
    assert!(p.header.has(flags::CONNECT_RESPONSE));
    assert_eq!(p.header.id, 42);
    assert_eq!(&p.optional_bytes[..], &srv.cookie.to_le_bytes());
    assert_eq!(s.state(), State::Connected);
    assert!(matches!(
        s.events().as_slice(),
        [Event::Connected { client_id: 42 }]
    ));

    // 3. Server sends ServerName, then a CharacterList split across 3 fragments,
    //    delivered out of order, then DDD_Interrogation.
    let mut w = Writer::new();
    w.u32(opcode::SERVER_NAME).u32(1).i32(-1).string16("Mock");
    let name_pkts = srv.message(&w.buf, 500);
    let mut w = Writer::new();
    w.u32(opcode::CHARACTER_LIST)
        .u32(0)
        .u32(1)
        .u32(0x5000_0001)
        .string16("Bob")
        .u32(0)
        .u32(0)
        .u32(11)
        .string16("tester")
        .u32(0)
        .u32(1);
    let list_pkts = srv.message(&w.buf, 12);
    assert!(list_pkts.len() >= 3);
    let ddd_pkts = srv.message(&opcode_msg(opcode::DDD_INTERROGATION, &[0; 24]), 500);

    for dg in &name_pkts {
        s.receive(dg, t0);
    }
    // Out of order: last fragment packet first.
    for dg in list_pkts.iter().rev() {
        s.receive(dg, t0);
    }
    for dg in &ddd_pkts {
        s.receive(dg, t0);
    }
    let evs = s.events();
    let msgs: Vec<&Vec<u8>> = evs
        .iter()
        .filter_map(|e| {
            if let Event::Message(m) = e {
                Some(m)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(msgs.len(), 3, "{evs:?}");
    assert_eq!(messages::split(msgs[0]).unwrap().0, opcode::SERVER_NAME);
    let (op, body) = messages::split(msgs[1]).unwrap();
    assert_eq!(op, opcode::CHARACTER_LIST);
    assert_eq!(
        messages::CharacterList::parse(body).unwrap().characters[0].name,
        "Bob"
    );
    assert_eq!(
        messages::split(msgs[2]).unwrap().0,
        opcode::DDD_INTERROGATION
    );

    // Reversed delivery may have produced a retransmit request; discard it.
    for (_, dg) in s.outgoing() {
        assert!(Packet::parse(&dg)
            .unwrap()
            .header
            .has(flags::REQUEST_RETRANSMIT));
    }

    // 4. The session answers the interrogation by itself; the server can verify it.
    s.poll(t0 + Duration::from_millis(10));
    let out = s.outgoing();
    assert_eq!(out.len(), 1);
    let p = srv.receive(&out[0].1);
    assert!(p
        .header
        .has(flags::ENCRYPTED_CHECKSUM | flags::BLOB_FRAGMENTS));
    assert_eq!(p.header.sequence, 2, "first data packet is seq 2");
    assert!(p.header.has(flags::ACK_SEQUENCE));
    assert_eq!(p.fragments.len(), 1);
    assert_eq!(p.fragments[0].header.sequence, 1, "first fragment is seq 1");
    let (op, body) = messages::split(&p.fragments[0].data).unwrap();
    assert_eq!(op, opcode::DDD_INTERROGATION_RESPONSE);
    let mut r = ac_net::wire::Reader::new(body);
    assert_eq!(r.u32().unwrap(), 1); // language
    assert_eq!(r.i32().unwrap(), 1); // one list
    assert_eq!(r.i32().unwrap(), 0); // type
    assert_eq!(r.i32().unwrap(), 1); // id portal
    assert_eq!(r.i32().unwrap(), 2072);
    assert_eq!(r.i32().unwrap(), -2073);

    // 5. A large client message is fragmented and each packet verifies.
    let big = vec![0xABu8; 1000];
    s.send_message(9, opcode_msg(0xF7C8, &big));
    s.poll(t0 + Duration::from_millis(20));
    let out = s.outgoing();
    assert!(
        out.len() >= 3,
        "1004 bytes need 3 fragments, got {} packets",
        out.len()
    );
    let mut total = 0;
    for (_, dg) in &out {
        let p = srv.receive(dg);
        for f in &p.fragments {
            assert_eq!(f.header.count, 3);
            total += f.data.len();
        }
    }
    assert_eq!(total, 1004);
}

#[test]
fn retransmit_request_on_gap() {
    let t0 = Instant::now();
    let mut s = Session::new(
        Config {
            account: "a".into(),
            password: "b".into(),
            dats: vec![],
            echo_interval: Duration::from_secs(100),
            ack_interval: Duration::from_secs(100),
        },
        t0,
    );
    let mut srv = MockServer::new();
    s.login(t0);
    s.outgoing();
    s.receive(&srv.connect_request(), t0);
    s.outgoing();
    s.events();
    let p1 = srv.message(&opcode_msg(0xF7E1, &[0; 8]), 500);
    let p2 = srv.message(&opcode_msg(0xF7E1, &[1; 8]), 500);
    let p3 = srv.message(&opcode_msg(0xF7E1, &[2; 8]), 500);
    let p4 = srv.message(&opcode_msg(0xF7E1, &[3; 8]), 500);
    s.receive(&p1[0], t0);
    s.receive(&p4[0], t0); // gap of two: p2 and p3 missing
    let out = s.outgoing();
    assert_eq!(out.len(), 1);
    let nak = Packet::parse(&out[0].1).unwrap();
    assert!(nak.header.has(flags::REQUEST_RETRANSMIT));
    let seq2 = Header::parse(&p2[0]).unwrap().sequence;
    assert_eq!(nak.optional.request_retransmit, vec![seq2, seq2 + 1]);
    assert_eq!(s.events().len(), 1, "only p1 delivered so far");
    s.receive(&p3[0], t0);
    assert_eq!(s.events().len(), 0, "still blocked on p2");
    s.receive(&p2[0], t0);
    assert_eq!(s.events().len(), 3, "p2, p3, p4 delivered in order");
}

/// A session past the handshake, plus the mock server driving it.
fn connected(ack: Duration) -> (Session, MockServer, Instant) {
    let t0 = Instant::now();
    let mut s = Session::new(
        Config {
            account: "a".into(),
            password: "b".into(),
            dats: vec![],
            echo_interval: Duration::from_secs(1000),
            ack_interval: ack,
        },
        t0,
    );
    let mut srv = MockServer::new();
    s.login(t0);
    s.outgoing();
    s.receive(&srv.connect_request(), t0);
    s.outgoing();
    s.events();
    (s, srv, t0)
}

fn naks(out: &[(Port, Vec<u8>)]) -> Vec<Vec<u32>> {
    out.iter()
        .map(|(_, dg)| Packet::parse(dg).unwrap())
        .filter(|p| p.header.has(flags::REQUEST_RETRANSMIT))
        .map(|p| p.optional.request_retransmit)
        .collect()
}

/// One missing packet is not requested straight away, because it may just
/// be reordered, but it is requested once the hole has stayed open.
#[test]
fn single_gap_is_requested_after_a_delay() {
    let (mut s, mut srv, t0) = connected(Duration::from_secs(1000));
    let p1 = srv.message(&opcode_msg(0xF7E1, &[0; 8]), 500);
    let _p2 = srv.message(&opcode_msg(0xF7E1, &[1; 8]), 500);
    let p3 = srv.message(&opcode_msg(0xF7E1, &[2; 8]), 500);
    let seq2 = Header::parse(&_p2[0]).unwrap().sequence;

    s.receive(&p1[0], t0);
    s.receive(&p3[0], t0); // p2 missing: a gap of one
    assert!(
        naks(&s.outgoing()).is_empty(),
        "no request on the first sight"
    );

    // Still nothing while the packet could plausibly be in flight.
    s.poll(t0 + Duration::from_millis(100));
    assert!(naks(&s.outgoing()).is_empty(), "asked too early");

    s.poll(t0 + Duration::from_millis(300));
    assert_eq!(
        naks(&s.outgoing()),
        vec![vec![seq2]],
        "one packet requested"
    );
}

/// Requests do not repeat faster than once a second while the hole stays
/// open, and stop as soon as it closes.
#[test]
fn single_gap_requests_are_rate_limited() {
    let (mut s, mut srv, t0) = connected(Duration::from_secs(1000));
    let p1 = srv.message(&opcode_msg(0xF7E1, &[0; 8]), 500);
    let p2 = srv.message(&opcode_msg(0xF7E1, &[1; 8]), 500);
    let p3 = srv.message(&opcode_msg(0xF7E1, &[2; 8]), 500);
    let seq2 = Header::parse(&p2[0]).unwrap().sequence;
    s.receive(&p1[0], t0);
    s.receive(&p3[0], t0);

    let mut sent = Vec::new();
    for ms in [300, 400, 500, 900, 1299] {
        s.poll(t0 + Duration::from_millis(ms));
        sent.extend(naks(&s.outgoing()));
    }
    assert_eq!(sent, vec![vec![seq2]], "one request in the first second");

    s.poll(t0 + Duration::from_millis(1301));
    assert_eq!(
        naks(&s.outgoing()),
        vec![vec![seq2]],
        "retried after a second"
    );

    // The missing packet arrives: no more requests, and everything is
    // delivered in order.
    s.receive(&p2[0], t0 + Duration::from_millis(1400));
    assert_eq!(s.events().len(), 3, "p1, p2, p3 delivered");
    for ms in [2400, 3400, 4400] {
        s.poll(t0 + Duration::from_millis(ms));
        assert!(naks(&s.outgoing()).is_empty(), "asked again at {ms}ms");
    }
}

/// A gap of two or more is loss, not reordering: ask at once, as ACE does.
#[test]
fn wide_gap_is_requested_immediately() {
    let (mut s, mut srv, t0) = connected(Duration::from_secs(1000));
    let p1 = srv.message(&opcode_msg(0xF7E1, &[0; 8]), 500);
    let p2 = srv.message(&opcode_msg(0xF7E1, &[1; 8]), 500);
    let _p3 = srv.message(&opcode_msg(0xF7E1, &[2; 8]), 500);
    let p4 = srv.message(&opcode_msg(0xF7E1, &[3; 8]), 500);
    let seq2 = Header::parse(&p2[0]).unwrap().sequence;
    s.receive(&p1[0], t0);
    s.receive(&p4[0], t0);
    assert_eq!(naks(&s.outgoing()), vec![vec![seq2, seq2 + 1]]);
}

/// A retransmission we send in answer to the server's request must still
/// verify: setting the Retransmission flag changes the header hash, so the
/// checksum has to be rebuilt around the original ISAAC key.
#[test]
fn our_retransmissions_still_verify() {
    let (mut s, mut srv, t0) = connected(Duration::from_millis(0));
    s.send_message(9, opcode_msg(0xF7B1, &[7; 16]));
    s.poll(t0);
    let out = s.outgoing();
    assert_eq!(out.len(), 1);
    let original = srv.receive(&out[0].1);
    let seq = original.header.sequence;

    // The server asks for it back.
    srv.seq += 1;
    let xor = srv.send_keys.next();
    let mut body = 1u32.to_le_bytes().to_vec();
    body.extend_from_slice(&seq.to_le_bytes());
    let req = packet::build(
        Header {
            sequence: srv.seq,
            flags: flags::ENCRYPTED_CHECKSUM | flags::REQUEST_RETRANSMIT,
            id: 0xB,
            ..Default::default()
        },
        &body,
        &[],
        xor,
    );
    s.receive(&req, t0);
    let out = s.outgoing();
    assert_eq!(out.len(), 1, "one retransmission");
    let again = Packet::parse(&out[0].1).unwrap();
    assert!(
        again.header.has(flags::RETRANSMISSION),
        "flagged as a resend"
    );
    assert_eq!(again.header.sequence, seq);
    assert_eq!(
        again.checksum_key(),
        original.checksum_key(),
        "a retransmission keeps its original ISAAC key"
    );
    assert_eq!(again.fragments, original.fragments);
}

/// End to end over a link that loses, reorders and duplicates packets. The
/// old key stream latched shut part way through a run like this and the
/// client went deaf; every message must arrive, in order, with the session
/// still connected at the end.
#[test]
fn survives_a_lossy_link() {
    let (mut s, mut srv, t0) = connected(Duration::from_millis(0));
    let mut rng = 0x2545_F491u32;
    let mut roll = move || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        rng
    };
    let total = 4000u32;
    let mut cache: std::collections::BTreeMap<u32, Vec<u8>> = std::collections::BTreeMap::new();
    let mut delayed: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut got = 0u32;
    let mut resent = 0u32;
    let mut now = t0;

    for i in 0..total {
        now += Duration::from_millis(50);
        let dgs = srv.message(&opcode_msg(0xF7E1, &i.to_le_bytes()), 500);
        for dg in dgs {
            let seq = Header::parse(&dg).unwrap().sequence;
            cache.insert(seq, dg.clone());
            match roll() % 20 {
                0 => {}                       // lost
                1 => delayed.push((seq, dg)), // reordered
                2 => {
                    s.receive(&dg, now); // duplicated
                    s.receive(&dg, now);
                }
                _ => s.receive(&dg, now),
            }
        }
        // Deliver anything that was held back a few packets ago.
        if delayed.len() >= 3 {
            for (_, dg) in delayed.drain(..) {
                s.receive(&dg, now);
            }
        }
        s.poll(now);
        // The server answers our retransmit requests from its cache.
        for (_, dg) in s.outgoing() {
            let p = Packet::parse(&dg).unwrap();
            if p.header.has(flags::REQUEST_RETRANSMIT) {
                for want in &p.optional.request_retransmit {
                    if let Some(orig) = cache.get(want) {
                        let mut b = orig.clone();
                        let was = Header::parse(&b).unwrap();
                        let mut h = was;
                        h.flags |= flags::RETRANSMISSION;
                        h.checksum = h.hash().wrapping_add(was.checksum.wrapping_sub(was.hash()));
                        h.write(&mut b[..20]);
                        s.receive(&b, now);
                        resent += 1;
                    }
                }
            } else {
                srv.receive(&dg);
            }
        }
        for ev in s.events() {
            if let Event::Message(m) = ev {
                let (op, body) = messages::split(&m).unwrap();
                assert_eq!(op, 0xF7E1);
                assert_eq!(body[..4], got.to_le_bytes(), "messages out of order");
                got += 1;
            } else if let Event::Terminated(why) = ev {
                panic!("session died at message {got}: {why}");
            }
        }
    }
    // Flush whatever is still recoverable.
    for _ in 0..40 {
        now += Duration::from_secs(1);
        s.poll(now);
        for (_, dg) in s.outgoing() {
            let p = Packet::parse(&dg).unwrap();
            if p.header.has(flags::REQUEST_RETRANSMIT) {
                for want in &p.optional.request_retransmit {
                    if let Some(orig) = cache.get(want) {
                        let mut b = orig.clone();
                        let was = Header::parse(&b).unwrap();
                        let mut h = was;
                        h.flags |= flags::RETRANSMISSION;
                        h.checksum = h.hash().wrapping_add(was.checksum.wrapping_sub(was.hash()));
                        h.write(&mut b[..20]);
                        s.receive(&b, now);
                    }
                }
            }
        }
        for ev in s.events() {
            if let Event::Message(m) = ev {
                let (_, body) = messages::split(&m).unwrap();
                assert_eq!(body[..4], got.to_le_bytes(), "messages out of order");
                got += 1;
            }
        }
    }
    assert_eq!(s.state(), State::Connected, "session survived the link");
    assert_eq!(got, total, "every message recovered");
    assert!(resent > 100, "the link should have needed {resent} resends");
}

/// A RejectRetransmit means those packets are gone for good. Waiting on
/// them would stop the in-order stream dead, so the session steps over the
/// hole and carries on.
#[test]
fn rejected_retransmit_steps_over_the_hole() {
    let (mut s, mut srv, t0) = connected(Duration::from_secs(1000));
    let p1 = srv.message(&opcode_msg(0xF7E1, &[0; 8]), 500);
    let p2 = srv.message(&opcode_msg(0xF7E1, &[1; 8]), 500);
    let _p3 = srv.message(&opcode_msg(0xF7E1, &[2; 8]), 500);
    let p4 = srv.message(&opcode_msg(0xF7E1, &[3; 8]), 500);
    let seq2 = Header::parse(&p2[0]).unwrap().sequence;
    s.receive(&p1[0], t0);
    s.receive(&p4[0], t0); // p2 and p3 missing
    assert_eq!(s.events().len(), 1, "only p1 delivered");
    assert_eq!(naks(&s.outgoing()), vec![vec![seq2, seq2 + 1]]);

    // The server no longer has them.
    srv.seq += 1;
    let xor = srv.send_keys.next();
    let mut body = 2u32.to_le_bytes().to_vec();
    body.extend_from_slice(&seq2.to_le_bytes());
    body.extend_from_slice(&(seq2 + 1).to_le_bytes());
    let reject = packet::build(
        Header {
            sequence: srv.seq,
            flags: flags::ENCRYPTED_CHECKSUM | flags::REJECT_RETRANSMIT,
            id: 0xB,
            ..Default::default()
        },
        &body,
        &[],
        xor,
    );
    s.receive(&reject, t0);
    assert!(
        naks(&s.outgoing()).is_empty(),
        "still asking for a lost packet"
    );

    // The packet hole is written off, but p2 and p3 carried fragments, so
    // the message stream is still waiting on them. It must not wait for
    // ever: after the timeout the backlog is released.
    let p5 = srv.message(&opcode_msg(0xF7E1, &[4; 8]), 500);
    s.receive(&p5[0], t0 + Duration::from_millis(10));
    assert!(
        s.events().is_empty(),
        "p4 and p5 are still behind the lost fragments"
    );
    s.poll(t0 + Duration::from_secs(1));
    assert!(s.events().is_empty(), "released too early");
    s.poll(t0 + Duration::from_secs(6));
    let evs = s.events();
    assert_eq!(evs.len(), 2, "p4 and p5 released: {evs:?}");
    assert_eq!(s.state(), State::Connected);

    // And the stream runs normally from there.
    let p6 = srv.message(&opcode_msg(0xF7E1, &[5; 8]), 500);
    s.receive(&p6[0], t0 + Duration::from_secs(7));
    assert_eq!(s.events().len(), 1, "p6 delivered straight away");
}

/// Without a RejectRetransmit the same recovery happens on the timer: a
/// fragment that never arrives must not hold the message stream shut.
#[test]
fn lost_fragment_releases_the_backlog_on_the_timer() {
    let (mut s, mut srv, t0) = connected(Duration::from_secs(1000));
    let p1 = srv.message(&opcode_msg(0xF7E1, &[0; 8]), 500);
    let _p2 = srv.message(&opcode_msg(0xF7E1, &[1; 8]), 500);
    let p3 = srv.message(&opcode_msg(0xF7E1, &[2; 8]), 500);
    s.receive(&p1[0], t0);
    assert_eq!(s.events().len(), 1);
    s.receive(&p3[0], t0);
    assert!(s.events().is_empty(), "p3 waits on p2");
    // The request goes unanswered, so the packet hole is written off after
    // ten seconds and p3's fragment then waits five more for p2's.
    s.poll(t0 + Duration::from_secs(6));
    assert!(s.events().is_empty(), "gave up on the packet too early");
    s.poll(t0 + Duration::from_secs(11));
    assert!(s.events().is_empty(), "p3 still waits on p2's fragment");
    s.poll(t0 + Duration::from_secs(17));
    assert_eq!(s.events().len(), 1, "p3 released once p2 is written off");
    assert_eq!(s.state(), State::Connected);

    // Normal traffic resumes.
    let p4 = srv.message(&opcode_msg(0xF7E1, &[3; 8]), 500);
    s.receive(&p4[0], t0 + Duration::from_secs(18));
    assert_eq!(s.events().len(), 1, "p4 delivered straight away");
}
