//! A local cross-process message bus.
//!
//! Plugins in one process share a blackboard; processes started separately
//! (by `aclauncher`, or several `acbot`/`acviewer` by hand) cannot see each
//! other. This crate links them: one process hosts a tiny hub
//! ([`BusServer`]) on loopback TCP and every process, the hub's included,
//! talks to it through a [`BusClient`]. The hub fans every post out to
//! every *other* connection (no echo), keeps the current shared values, and
//! answers a join with a snapshot of them.
//!
//! # Protocol
//!
//! One JSON object per line, in both directions:
//!
//! ```text
//! {"kind":"hello","name":"alice"}                         client -> hub, first line
//! {"kind":"state","values":{"leader":1}}                  hub -> client, answers hello
//! {"kind":"post","from":"alice","topic":"party.target","value":{"guid":5}}
//! {"kind":"set","key":"leader","value":1}
//! ```
//!
//! `post` and `set` flow both ways: a client sends them, the hub forwards
//! them to the other clients (`set` also updates the hub's map). The
//! default address is `127.0.0.1:9500`; the `ACREBORN_BUS` environment
//! variable overrides it ([`default_addr`]).
//!
//! # Hosting
//!
//! [`BusClient::connect_or_host`] connects if a hub is listening and
//! otherwise starts one in-process, so the first process up becomes the
//! hub and the rest join it. Every client reconnects with backoff when its
//! connection drops; a client made by `connect_or_host` also tries to bind
//! the address on every failed attempt and re-hosts if it wins, seeding
//! the new hub with the values it last saw. Two clients may race for the
//! port: one wins the bind, the other's bind fails with "address in use"
//! and its next attempt connects to the winner. Posts sent in the gap are
//! lost; `set`s made while disconnected are kept (latest per key) and sent
//! after the rejoin.
//!
//! # Threads
//!
//! The server accepts on a background thread and runs a reader plus a
//! writer thread per connection. The client runs one link thread that owns
//! the socket, reconnects, and forwards frames to a channel the
//! single-threaded caller drains with [`BusClient::poll`]. Nothing blocks
//! the caller.

use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
pub use serde_json::{self, Value};

/// The address used when neither the caller nor `ACREBORN_BUS` names one.
pub const DEFAULT_ADDR: &str = "127.0.0.1:9500";
/// Environment variable overriding [`DEFAULT_ADDR`].
pub const ADDR_ENV: &str = "ACREBORN_BUS";

/// The address to use: `ACREBORN_BUS` if set, else [`DEFAULT_ADDR`].
pub fn default_addr() -> String {
    std::env::var(ADDR_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ADDR.to_string())
}

/// Resolve a `--bus [ADDR]` style argument: `None` or empty means
/// [`default_addr`]; a bare port means that port on loopback.
pub fn resolve_addr(arg: Option<&str>) -> String {
    match arg.map(str::trim) {
        None | Some("") => default_addr(),
        Some(s) if s.chars().all(|c| c.is_ascii_digit()) => format!("127.0.0.1:{s}"),
        Some(s) => s.to_string(),
    }
}

/// One line on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Frame {
    Hello {
        name: String,
    },
    State {
        values: BTreeMap<String, Value>,
    },
    Post {
        from: String,
        topic: String,
        value: Value,
    },
    Set {
        key: String,
        value: Value,
    },
}

impl Frame {
    /// The frame as one line (newline included).
    pub fn to_line(&self) -> String {
        let mut s = serde_json::to_string(self).expect("frame serializes");
        s.push('\n');
        s
    }

    pub fn parse(line: &str) -> Result<Frame, serde_json::Error> {
        serde_json::from_str(line)
    }
}

/// What a client sees, in arrival order.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// The hub answered our hello (`hosting` when this process runs the
    /// hub); its `State` follows.
    Connected { hosting: bool },
    /// The link went down; the client is reconnecting.
    Disconnected,
    /// The hub's values on join.
    State { values: BTreeMap<String, Value> },
    /// Another process posted.
    Post {
        from: String,
        topic: String,
        value: Value,
    },
    /// A shared value changed (another process set it, or this one did
    /// while disconnected and the set was replayed on rejoin).
    Set { key: String, value: Value },
}

// ---------------------------------------------------------------- server

struct Conn {
    id: u64,
    tx: Sender<String>,
    stream: TcpStream,
}

#[derive(Default)]
struct ServerState {
    values: BTreeMap<String, Value>,
    conns: Vec<Conn>,
}

struct ServerInner {
    state: Mutex<ServerState>,
    stop: AtomicBool,
    next_id: AtomicU64,
}

impl ServerInner {
    /// Send `line` to every connection but `except`; drop the ones that
    /// went away.
    fn broadcast(&self, line: &str, except: u64) {
        let mut st = self.state.lock().unwrap();
        st.conns
            .retain(|c| c.id == except || c.tx.send(line.to_string()).is_ok());
    }

    fn remove(&self, id: u64) {
        let mut st = self.state.lock().unwrap();
        st.conns.retain(|c| c.id != id);
    }

    fn handle(&self, id: u64, my_tx: &Sender<String>, name: &mut Option<String>, frame: Frame) {
        match frame {
            Frame::Hello { name: n } => {
                *name = Some(n);
                let values = self.state.lock().unwrap().values.clone();
                let _ = my_tx.send(Frame::State { values }.to_line());
            }
            Frame::Post { .. } => self.broadcast(&frame.to_line(), id),
            Frame::Set { ref key, ref value } => {
                self.state
                    .lock()
                    .unwrap()
                    .values
                    .insert(key.clone(), value.clone());
                self.broadcast(&frame.to_line(), id);
            }
            Frame::State { .. } => {} // clients never send state
        }
    }
}

/// The hub: accepts connections, fans posts out, keeps the values.
pub struct BusServer {
    inner: Arc<ServerInner>,
    addr: SocketAddr,
    accept: Option<thread::JoinHandle<()>>,
}

impl BusServer {
    /// Bind and start accepting. `127.0.0.1:0` picks a free port
    /// ([`local_addr`](Self::local_addr) tells which).
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<BusServer> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let inner = Arc::new(ServerInner {
            state: Mutex::new(ServerState::default()),
            stop: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
        });
        let accept = {
            let inner = inner.clone();
            thread::Builder::new()
                .name("ac-bus-accept".into())
                .spawn(move || accept_loop(listener, inner))?
        };
        tracing::info!(%addr, "bus hub listening");
        Ok(BusServer {
            inner,
            addr,
            accept: Some(accept),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Snapshot of the shared values.
    pub fn values(&self) -> BTreeMap<String, Value> {
        self.inner.state.lock().unwrap().values.clone()
    }

    /// Insert values (without telling connected clients); used to carry
    /// state over when a client re-hosts.
    pub fn seed(&self, values: BTreeMap<String, Value>) {
        self.inner.state.lock().unwrap().values.extend(values);
    }

    pub fn connections(&self) -> usize {
        self.inner.state.lock().unwrap().conns.len()
    }
}

impl Drop for BusServer {
    fn drop(&mut self) {
        // Stop listening first, so nobody can join between the closing of
        // the existing connections and the end of the accept loop.
        self.inner.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.accept.take() {
            let _ = h.join();
        }
        let conns: Vec<Conn> = std::mem::take(&mut self.inner.state.lock().unwrap().conns);
        for c in conns {
            let _ = c.stream.shutdown(Shutdown::Both);
        }
    }
}

fn accept_loop(listener: TcpListener, inner: Arc<ServerInner>) {
    while !inner.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if let Err(e) = spawn_conn(stream, &inner) {
                    tracing::warn!(%peer, "bus hub: connection setup failed: {e}");
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                tracing::warn!("bus hub: accept failed: {e}");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn spawn_conn(stream: TcpStream, inner: &Arc<ServerInner>) -> io::Result<()> {
    // BSD sockets inherit the listener's non-blocking mode; the reader
    // wants to block.
    stream.set_nonblocking(false)?;
    let _ = stream.set_nodelay(true);
    let id = inner.next_id.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = mpsc::channel::<String>();
    let mut writer = stream.try_clone()?;
    let keep = stream.try_clone()?;
    inner.state.lock().unwrap().conns.push(Conn {
        id,
        tx: tx.clone(),
        stream: keep,
    });
    thread::Builder::new()
        .name(format!("ac-bus-w{id}"))
        .spawn(move || {
            for line in rx {
                if writer.write_all(line.as_bytes()).is_err() {
                    let _ = writer.shutdown(Shutdown::Both);
                    break;
                }
            }
        })?;
    let inner = inner.clone();
    thread::Builder::new()
        .name(format!("ac-bus-r{id}"))
        .spawn(move || {
            let mut name = None;
            let reader = BufReader::new(stream);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                match Frame::parse(&line) {
                    Ok(frame) => inner.handle(id, &tx, &mut name, frame),
                    Err(e) => tracing::warn!(id, "bus hub: bad line {line:?}: {e}"),
                }
            }
            inner.remove(id);
            tracing::debug!(
                id,
                name = name.as_deref().unwrap_or("?"),
                "bus hub: client left"
            );
        })?;
    Ok(())
}

// ---------------------------------------------------------------- client

enum LinkEvent {
    Send(Frame),
    Received(u64, Frame),
    ReaderClosed(u64),
    Shutdown,
}

/// Flags the link thread keeps up to date for the caller.
struct Status {
    connected: AtomicBool,
    hosting: AtomicBool,
}

/// A process's end of the bus. Cheap to poll from a frame loop; the
/// socket lives on a background thread.
pub struct BusClient {
    name: String,
    addr: String,
    link_tx: Sender<LinkEvent>,
    rx: Receiver<Incoming>,
    status: Arc<Status>,
}

/// Where the link thread is between reconnects.
struct Link {
    addr: String,
    name: String,
    host_if_absent: bool,
    server: Option<BusServer>,
    status: Arc<Status>,
    out: Sender<Incoming>,
    link_tx: Sender<LinkEvent>,
    /// The socket, when up, and its generation (stale reader events from
    /// an earlier socket are ignored).
    stream: Option<(u64, TcpStream)>,
    gen: u64,
    /// Everything we know the shared values to be; seeds a re-hosted hub.
    values: BTreeMap<String, Value>,
    /// Sets made while down, latest per key, sent after the next join.
    pending: BTreeMap<String, Value>,
    /// Posts made between the socket coming up and the hub's `state`.
    pending_posts: Vec<Frame>,
    /// Between `hello` and the hub's `state` pending sets wait.
    joined: bool,
    backoff: Duration,
    next_attempt: Instant,
}

const BACKOFF_MIN: Duration = Duration::from_millis(100);
const BACKOFF_MAX: Duration = Duration::from_secs(2);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

impl BusClient {
    /// Connect to a hub at `addr` (`HOST:PORT`); fails if none listens.
    pub fn connect(addr: &str, name: &str) -> io::Result<BusClient> {
        let stream = try_connect(addr)?;
        Ok(Self::start(addr, name, stream, None, false))
    }

    /// Connect to the hub at `addr`, or become it when nobody listens.
    pub fn connect_or_host(addr: &str, name: &str) -> io::Result<BusClient> {
        match try_connect(addr) {
            Ok(stream) => Ok(Self::start(addr, name, stream, None, true)),
            Err(e) if refused(&e) => match BusServer::bind(addr) {
                Ok(server) => {
                    let stream = try_connect(&server.local_addr().to_string())?;
                    Ok(Self::start(addr, name, stream, Some(server), true))
                }
                // Lost the race to another process: it is the hub now.
                Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                    let stream = try_connect(addr)?;
                    Ok(Self::start(addr, name, stream, None, true))
                }
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        }
    }

    fn start(
        addr: &str,
        name: &str,
        stream: TcpStream,
        server: Option<BusServer>,
        host_if_absent: bool,
    ) -> BusClient {
        let (link_tx, link_rx) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        let status = Arc::new(Status {
            connected: AtomicBool::new(false),
            hosting: AtomicBool::new(server.is_some()),
        });
        let mut link = Link {
            addr: addr.to_string(),
            name: name.to_string(),
            host_if_absent,
            server,
            status: status.clone(),
            out,
            link_tx: link_tx.clone(),
            stream: None,
            gen: 0,
            values: BTreeMap::new(),
            pending: BTreeMap::new(),
            pending_posts: Vec::new(),
            joined: false,
            backoff: BACKOFF_MIN,
            next_attempt: Instant::now(),
        };
        link.adopt(stream);
        thread::Builder::new()
            .name("ac-bus-link".into())
            .spawn(move || link.run(link_rx))
            .expect("spawn bus link thread");
        BusClient {
            name: name.to_string(),
            addr: addr.to_string(),
            link_tx,
            rx,
            status,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    pub fn is_connected(&self) -> bool {
        self.status.connected.load(Ordering::SeqCst)
    }

    /// Whether this process runs the hub right now.
    pub fn is_hosting(&self) -> bool {
        self.status.hosting.load(Ordering::SeqCst)
    }

    /// Everything that arrived since the last poll. Never blocks.
    pub fn poll(&self) -> Vec<Incoming> {
        let mut v = Vec::new();
        while let Ok(i) = self.rx.try_recv() {
            v.push(i);
        }
        v
    }

    /// Post as this client's name.
    pub fn post(&self, topic: impl Into<String>, value: impl Into<Value>) {
        let from = self.name.clone();
        self.post_as(from, topic, value);
    }

    /// Post with an explicit `from` tag.
    pub fn post_as(
        &self,
        from: impl Into<String>,
        topic: impl Into<String>,
        value: impl Into<Value>,
    ) {
        let _ = self.link_tx.send(LinkEvent::Send(Frame::Post {
            from: from.into(),
            topic: topic.into(),
            value: value.into(),
        }));
    }

    /// Set a shared value on the hub and every other process.
    pub fn set(&self, key: impl Into<String>, value: impl Into<Value>) {
        let _ = self.link_tx.send(LinkEvent::Send(Frame::Set {
            key: key.into(),
            value: value.into(),
        }));
    }
}

impl std::fmt::Debug for BusClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BusClient")
            .field("name", &self.name)
            .field("addr", &self.addr)
            .field("connected", &self.is_connected())
            .field("hosting", &self.is_hosting())
            .finish()
    }
}

impl Drop for BusClient {
    fn drop(&mut self) {
        let _ = self.link_tx.send(LinkEvent::Shutdown);
    }
}

fn refused(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::TimedOut
            | io::ErrorKind::NotConnected
    )
}

fn try_connect(addr: &str) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::InvalidInput, "no address");
    for sa in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&sa, CONNECT_TIMEOUT) {
            Ok(s) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

impl Link {
    /// Take a fresh socket: start its reader, say hello.
    fn adopt(&mut self, stream: TcpStream) {
        self.gen += 1;
        let gen = self.gen;
        let tx = self.link_tx.clone();
        let reader = match stream.try_clone() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("bus: cannot clone socket: {e}");
                return;
            }
        };
        let spawned = thread::Builder::new()
            .name(format!("ac-bus-reader{gen}"))
            .spawn(move || {
                let reader = BufReader::new(reader);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    match Frame::parse(&line) {
                        Ok(f) => {
                            if tx.send(LinkEvent::Received(gen, f)).is_err() {
                                break;
                            }
                        }
                        Err(e) => tracing::warn!("bus: bad line {line:?}: {e}"),
                    }
                }
                let _ = tx.send(LinkEvent::ReaderClosed(gen));
            });
        if spawned.is_err() {
            return;
        }
        self.stream = Some((gen, stream));
        self.joined = false;
        self.backoff = BACKOFF_MIN;
        self.status
            .hosting
            .store(self.server.is_some(), Ordering::SeqCst);
        let hello = Frame::Hello {
            name: self.name.clone(),
        };
        self.write(&hello);
    }

    fn write(&mut self, frame: &Frame) {
        let Some((_, stream)) = self.stream.as_mut() else {
            return;
        };
        if let Err(e) = stream.write_all(frame.to_line().as_bytes()) {
            tracing::debug!("bus: write failed: {e}");
            self.drop_link();
        }
    }

    fn drop_link(&mut self) {
        if let Some((_, s)) = self.stream.take() {
            let _ = s.shutdown(Shutdown::Both);
            if self.joined {
                self.status.connected.store(false, Ordering::SeqCst);
                let _ = self.out.send(Incoming::Disconnected);
            }
        }
        self.joined = false;
        self.pending_posts.clear();
        self.next_attempt = Instant::now();
    }

    fn run(mut self, rx: Receiver<LinkEvent>) {
        loop {
            let wait = if self.stream.is_some() {
                Duration::from_secs(60)
            } else {
                self.next_attempt.saturating_duration_since(Instant::now())
            };
            match rx.recv_timeout(wait) {
                Ok(LinkEvent::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(LinkEvent::Send(frame)) => self.send(frame),
                Ok(LinkEvent::Received(gen, frame)) => {
                    if self.stream.as_ref().is_some_and(|(g, _)| *g == gen) {
                        self.received(frame);
                    }
                }
                Ok(LinkEvent::ReaderClosed(gen)) => {
                    if self.stream.as_ref().is_some_and(|(g, _)| *g == gen) {
                        tracing::info!(addr = %self.addr, "bus: hub went away, reconnecting");
                        self.drop_link();
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if self.stream.is_none() && Instant::now() >= self.next_attempt {
                self.reconnect();
            }
        }
        // Closing the hub (if we run it) drops every other client's link.
        self.drop_link();
        self.server = None;
        self.status.hosting.store(false, Ordering::SeqCst);
    }

    fn send(&mut self, frame: Frame) {
        if let Frame::Set { key, value } = &frame {
            self.values.insert(key.clone(), value.clone());
            if self.stream.is_none() || !self.joined {
                self.pending.insert(key.clone(), value.clone());
                return;
            }
        }
        if self.joined {
            self.write(&frame);
        } else if self.stream.is_some() {
            self.pending_posts.push(frame);
        }
        // Posts while down are dropped: they are frame-scoped news.
    }

    fn received(&mut self, frame: Frame) {
        match frame {
            Frame::State { values } => {
                let hosting = self.server.is_some();
                self.status.connected.store(true, Ordering::SeqCst);
                let _ = self.out.send(Incoming::Connected { hosting });
                self.values
                    .extend(values.iter().map(|(k, v)| (k.clone(), v.clone())));
                let _ = self.out.send(Incoming::State { values });
                self.joined = true;
                // Sets made while down: tell the hub, and replay locally
                // so the caller's copy agrees with the state it just got.
                let pending = std::mem::take(&mut self.pending);
                for (key, value) in pending {
                    self.values.insert(key.clone(), value.clone());
                    self.write(&Frame::Set {
                        key: key.clone(),
                        value: value.clone(),
                    });
                    let _ = self.out.send(Incoming::Set { key, value });
                }
                for frame in std::mem::take(&mut self.pending_posts) {
                    self.write(&frame);
                }
            }
            Frame::Post { from, topic, value } => {
                let _ = self.out.send(Incoming::Post { from, topic, value });
            }
            Frame::Set { key, value } => {
                self.values.insert(key.clone(), value.clone());
                let _ = self.out.send(Incoming::Set { key, value });
            }
            Frame::Hello { .. } => {}
        }
    }

    fn reconnect(&mut self) {
        // A hub we ran is gone with its link; make a new one if wanted.
        self.server = None;
        self.status.hosting.store(false, Ordering::SeqCst);
        match try_connect(&self.addr) {
            Ok(s) => {
                self.adopt(s);
                return;
            }
            Err(e) if refused(&e) && self.host_if_absent => match BusServer::bind(&self.addr) {
                Ok(server) => {
                    server.seed(self.values.clone());
                    tracing::info!(addr = %self.addr, "bus: no hub answered, hosting");
                    match try_connect(&server.local_addr().to_string()) {
                        Ok(s) => {
                            self.server = Some(server);
                            self.adopt(s);
                            return;
                        }
                        Err(e) => tracing::warn!("bus: cannot reach own hub: {e}"),
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                    // Someone else just won the port; connect next round.
                    self.next_attempt = Instant::now() + BACKOFF_MIN;
                    return;
                }
                Err(e) => tracing::warn!(addr = %self.addr, "bus: cannot host: {e}"),
            },
            Err(e) => tracing::debug!(addr = %self.addr, "bus: connect failed: {e}"),
        }
        self.next_attempt = Instant::now() + self.backoff;
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Poll `c` until `pred` accepts an item (or the deadline passes),
    /// returning everything drained so far.
    fn wait_for(c: &BusClient, pred: impl Fn(&Incoming) -> bool) -> Vec<Incoming> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while Instant::now() < deadline {
            got.extend(c.poll());
            if got.iter().any(&pred) {
                return got;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting; got {got:?}");
    }

    fn wait_until(what: &str, f: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if f() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    /// A loopback port nobody listens on right now.
    fn free_port_addr() -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().to_string()
    }

    #[test]
    fn frames_round_trip() {
        let f = Frame::Post {
            from: "alice".into(),
            topic: "party.target".into(),
            value: json!({"guid": 5}),
        };
        let line = f.to_line();
        assert!(line.ends_with('\n'));
        assert_eq!(
            line.trim(),
            r#"{"kind":"post","from":"alice","topic":"party.target","value":{"guid":5}}"#
        );
        assert_eq!(Frame::parse(&line).unwrap(), f);
        assert_eq!(
            Frame::parse(r#"{"kind":"hello","name":"bob"}"#).unwrap(),
            Frame::Hello { name: "bob".into() }
        );
        assert_eq!(
            Frame::parse(r#"{"kind":"set","key":"leader","value":1}"#).unwrap(),
            Frame::Set {
                key: "leader".into(),
                value: json!(1)
            }
        );
        assert!(Frame::parse(r#"{"kind":"nope"}"#).is_err());
    }

    #[test]
    fn addresses_resolve() {
        assert_eq!(resolve_addr(Some("9600")), "127.0.0.1:9600");
        assert_eq!(resolve_addr(Some("10.0.0.2:1")), "10.0.0.2:1");
        assert!(resolve_addr(None).contains(':'));
        assert_eq!(resolve_addr(Some("")), resolve_addr(None));
    }

    #[test]
    fn two_clients_exchange_posts_and_values() {
        let server = BusServer::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().to_string();
        let a = BusClient::connect(&addr, "a").unwrap();
        let b = BusClient::connect(&addr, "b").unwrap();
        assert!(!a.is_hosting());
        let got = wait_for(&a, |i| matches!(i, Incoming::State { .. }));
        assert_eq!(got[0], Incoming::Connected { hosting: false });
        wait_for(&b, |i| matches!(i, Incoming::State { .. }));

        a.post("party.target", json!({"guid": 7}));
        let got = wait_for(&b, |i| matches!(i, Incoming::Post { .. }));
        assert!(got.contains(&Incoming::Post {
            from: "a".into(),
            topic: "party.target".into(),
            value: json!({"guid": 7}),
        }));

        a.set("leader", "a");
        let got = wait_for(&b, |i| matches!(i, Incoming::Set { .. }));
        assert!(got.contains(&Incoming::Set {
            key: "leader".into(),
            value: json!("a"),
        }));
        // No echo: a hears nothing of its own.
        thread::sleep(Duration::from_millis(50));
        assert!(a.poll().is_empty());
        assert_eq!(server.values().get("leader"), Some(&json!("a")));

        // A late joiner gets the values with its state.
        let c = BusClient::connect(&addr, "c").unwrap();
        let got = wait_for(&c, |i| matches!(i, Incoming::State { .. }));
        assert!(got.iter().any(|i| matches!(
            i,
            Incoming::State { values } if values.get("leader") == Some(&json!("a"))
        )));
        wait_until("three connections", || server.connections() == 3);
        drop(b);
        wait_until("two connections", || server.connections() == 2);
    }

    #[test]
    fn clients_reconnect_after_the_hub_drops() {
        let server = BusServer::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().to_string();
        let a = BusClient::connect(&addr, "a").unwrap();
        let b = BusClient::connect(&addr, "b").unwrap();
        wait_for(&a, |i| matches!(i, Incoming::State { .. }));
        wait_for(&b, |i| matches!(i, Incoming::State { .. }));
        a.set("leader", "a");
        wait_for(&b, |i| matches!(i, Incoming::Set { .. }));

        drop(server);
        wait_for(&a, |i| *i == Incoming::Disconnected);
        wait_for(&b, |i| *i == Incoming::Disconnected);
        assert!(!a.is_connected());
        // Sets while down are kept; posts are not.
        a.set("leader", "b");
        a.post("lost", 1);

        let server = BusServer::bind(&addr).unwrap();
        let got = wait_for(&a, |i| matches!(i, Incoming::Set { .. }));
        assert!(got.contains(&Incoming::Connected { hosting: false }));
        // The replayed set follows the (empty) state of the new hub.
        let state_at = got
            .iter()
            .position(|i| matches!(i, Incoming::State { .. }))
            .unwrap();
        let set_at = got
            .iter()
            .position(|i| matches!(i, Incoming::Set { .. }))
            .unwrap();
        assert!(state_at < set_at);
        // `b` learns the replayed value either as a set, or from the
        // hub's state when it rejoins after `a` did.
        let learnt = |i: &Incoming| match i {
            Incoming::Set { key, value } => key == "leader" && *value == json!("b"),
            Incoming::State { values } => values.get("leader") == Some(&json!("b")),
            _ => false,
        };
        let got = wait_for(&b, learnt);
        assert!(!got.iter().any(|i| matches!(i, Incoming::Post { .. })));
        wait_until("hub has leader", || {
            server.values().get("leader") == Some(&json!("b"))
        });
        a.post("after", 2);
        wait_for(
            &b,
            |i| matches!(i, Incoming::Post { topic, .. } if topic == "after"),
        );
    }

    #[test]
    fn first_client_hosts_and_a_survivor_rehosts() {
        let addr = free_port_addr();
        let a = BusClient::connect_or_host(&addr, "a").unwrap();
        assert!(a.is_hosting());
        let got = wait_for(&a, |i| matches!(i, Incoming::State { .. }));
        assert_eq!(got[0], Incoming::Connected { hosting: true });
        a.set("leader", "a");

        let b = BusClient::connect_or_host(&addr, "b").unwrap();
        assert!(!b.is_hosting());
        let got = wait_for(&b, |i| matches!(i, Incoming::State { .. }));
        assert!(got.iter().any(|i| matches!(
            i,
            Incoming::State { values } if values.get("leader") == Some(&json!("a"))
        )));
        b.post("hi", "b");
        wait_for(
            &a,
            |i| matches!(i, Incoming::Post { from, .. } if from == "b"),
        );

        // The hub leaves; b re-hosts and carries the values over.
        drop(a);
        let mut got = wait_for(&b, |i| *i == Incoming::Disconnected);
        if !got.iter().any(|i| matches!(i, Incoming::State { .. })) {
            got.extend(wait_for(&b, |i| matches!(i, Incoming::State { .. })));
        }
        assert!(
            got.contains(&Incoming::Connected { hosting: true }),
            "got {got:?}"
        );
        assert!(b.is_hosting());
        assert!(got.iter().any(|i| matches!(
            i,
            Incoming::State { values } if values.get("leader") == Some(&json!("a"))
        )));

        let c = BusClient::connect_or_host(&addr, "c").unwrap();
        assert!(!c.is_hosting());
        wait_for(&c, |i| matches!(i, Incoming::State { .. }));
        c.post("hi", "c");
        wait_for(
            &b,
            |i| matches!(i, Incoming::Post { from, .. } if from == "c"),
        );
    }
}
