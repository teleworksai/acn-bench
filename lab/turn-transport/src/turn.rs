//! The turn-native transport, and the QUIC ablations that share its wire format: one
//! QUIC bidirectional stream carries one agent turn.
//!
//! Client to server, then FIN:
//!
//! ```text
//! OPEN   = 0x01 | flags u8 | session u64 | turn u32 | deadline_ms u32
//!               | base_len u64 | base_hash [32] | delta_len u32 | delta
//! RESUME = 0x02 | session u64 | turn u32 | offset u64
//! ```
//!
//! `flags` bit 0 is *resumable*. Set (the `turn` arms): the turn, not the stream or the
//! connection, owns the generation, which keeps running through a gap, and a `RESUME` on
//! any connection re-attaches to it. Unset (the `quic` ablations): the server aborts the
//! generation when it sees the stream fail, which on a black-holed path it only does at
//! the idle timeout.
//!
//! Server to client: one status byte (`OK`, `NEED_FULL`, `UNKNOWN_TURN`, `DEADLINE`,
//! `BAD_OFFSET`), then the turn's output as the raw stream, then FIN. There is no
//! per-token framing: the byte offset into the output is the resume cursor. A deadline
//! that expires mid-output resets the stream with `ERR_DEADLINE`.
//!
//! `base_len`/`base_hash` name the context the client believes the server holds (blake3
//! over canonical records); `delta` is what follows it. A server that holds something
//! else answers `NEED_FULL` and the client re-opens with `base_len = 0`.
//!
//! `deadline_ms` is what is left of the turn's budget when the request is sent, so a
//! retry does not extend it. It is relative because the clocks are not synchronised:
//! the server's copy runs late by the request's transit time. It bounds generation,
//! every write, and the lifetime of the resumable state. The client keeps a backstop
//! timer `CLIENT_GRACE` later, so that on a working path the server's verdict arrives
//! first. (QUIC's idle timeout is the other timer in play: see `IDLE_TIMEOUT`.)
//!
//! Not addressed at all: authentication. Session and turn numbers are guessable, and
//! whoever names them can resume the output or reset the session.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use quinn::{Connection, Endpoint, ReadError, ReadExactError, RecvStream, SendStream, VarInt};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio::sync::watch;
use tokio::task::AbortHandle;
use tokio::time::Instant;

use crate::workload::{ASSISTANT, SYSTEM, TurnStats, USER, Workload, record};

const OPEN: u8 = 1;
const RESUME: u8 = 2;
const FLAG_RESUMABLE: u8 = 1;
/// Bytes of an `OPEN` after the kind byte and before the delta; of a `RESUME` likewise.
const OPEN_HEADER: usize = 61;
const RESUME_HEADER: usize = 20;
const MAX_DELTA: usize = 16 << 20;

const ST_OK: u8 = 0;
const ST_NEED_FULL: u8 = 1;
const ST_UNKNOWN_TURN: u8 = 2;
const ST_DEADLINE: u8 = 3;
const ST_BAD_OFFSET: u8 = 4;

const ERR_DEADLINE: VarInt = VarInt::from_u32(3);

/// A gap longer than this kills the connection under every arm, so `quic-migrate`
/// degenerates into a restart and a blackout becomes a reconnect. `main` refuses gaps
/// that come close.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(10);
const CLIENT_GRACE: Duration = Duration::from_millis(250);
const RECONNECT_PAUSE: Duration = Duration::from_millis(50);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Copy, Debug)]
pub struct Tuning {
    /// quinn's path MTU discovery; its probes are bytes on the wire.
    pub mtud: bool,
    /// Client keep-alive. quinn's default is none, and it is not neutral: see the lab note.
    pub keep_alive: Option<Duration>,
}

fn transport(tuning: Tuning, client: bool) -> Result<Arc<quinn::TransportConfig>> {
    let mut t = quinn::TransportConfig::default();
    t.max_idle_timeout(Some(IDLE_TIMEOUT.try_into()?));
    if client {
        t.keep_alive_interval(tuning.keep_alive);
    }
    if !tuning.mtud {
        t.mtu_discovery_config(None);
    }
    Ok(Arc::new(t))
}

struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let (head, rest) = self.0.split_at_checked(N).context("short header")?;
        self.0 = rest;
        Ok(head.try_into()?)
    }
}

// ---------------------------------------------------------------- server

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Running,
    Done,
    Expired,
}

struct Out {
    bytes: Vec<u8>,
    phase: Phase,
}

struct Turn {
    out: watch::Sender<Out>,
    deadline: Instant,
    generator: Mutex<Option<AbortHandle>>,
}

impl Turn {
    fn abort(&self) {
        if let Some(generator) = lock(&self.generator).take() {
            generator.abort();
        }
    }
}

/// What the server holds of a session: enough to check a client's `base`. A real
/// server would hold the KV cache this stands for.
struct Session {
    len: u64,
    hash: blake3::Hasher,
}

type TurnKey = (u64, u32);

/// Lock order: `turns`, then `sessions`.
struct State {
    workload: Arc<Workload>,
    sessions: Mutex<HashMap<u64, Session>>,
    turns: Mutex<HashMap<TurnKey, Arc<Turn>>>,
    /// Output length of every turn the deadline cut short.
    expired: Mutex<Vec<usize>>,
}

pub struct TurnServer {
    pub addr: SocketAddr,
    pub cert: CertificateDer<'static>,
    endpoint: Endpoint,
    state: Arc<State>,
}

impl Drop for TurnServer {
    fn drop(&mut self) {
        self.endpoint.close(VarInt::from_u32(0), b"done");
        for turn in lock(&self.state.turns).values() {
            turn.abort();
        }
    }
}

impl TurnServer {
    pub fn start(workload: Arc<Workload>, tuning: Tuning) -> Result<Self> {
        let key = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()])?;
        let cert = CertificateDer::from(key.cert.der().to_vec());
        let secret = PrivatePkcs8KeyDer::from(key.signing_key.serialize_der());
        let mut config = quinn::ServerConfig::with_single_cert(vec![cert.clone()], secret.into())?;
        config.transport_config(transport(tuning, false)?);
        let endpoint = Endpoint::server(config, "127.0.0.1:0".parse()?)?;
        let state = Arc::new(State {
            workload,
            sessions: Mutex::default(),
            turns: Mutex::default(),
            expired: Mutex::default(),
        });
        let (accepting, shared) = (endpoint.clone(), state.clone());
        tokio::spawn(async move {
            while let Some(incoming) = accepting.accept().await {
                let state = shared.clone();
                tokio::spawn(async move {
                    let Ok(conn) = incoming.await else { return };
                    while let Ok((send, recv)) = conn.accept_bi().await {
                        let state = state.clone();
                        tokio::spawn(async move {
                            // A failed stream is a lost path; a resumable turn lives on.
                            let _ = serve_stream(state, send, recv).await;
                        });
                    }
                });
            }
        });
        Ok(Self {
            addr: endpoint.local_addr()?,
            cert,
            endpoint,
            state,
        })
    }

    /// Forget a session, as a server that re-homed or evicted it would.
    pub fn evict(&self, session: u64) {
        lock(&self.state.sessions).remove(&session);
    }

    pub fn live_turns(&self) -> usize {
        lock(&self.state.turns).len()
    }

    /// Output length of every turn the deadline cut short.
    pub fn expired(&self) -> Vec<usize> {
        lock(&self.state.expired).clone()
    }
}

async fn reply(send: &mut SendStream, status: u8) -> Result<()> {
    send.write_all(&[status]).await?;
    send.finish()?;
    Ok(())
}

async fn serve_stream(state: Arc<State>, mut send: SendStream, mut recv: RecvStream) -> Result<()> {
    let mut kind = [0u8; 1];
    recv.read_exact(&mut kind).await?;
    let (key, turn, offset, resumable) = match kind[0] {
        OPEN => {
            let mut header = [0u8; OPEN_HEADER];
            recv.read_exact(&mut header).await?;
            let mut h = Cursor(&header);
            let resumable = h.take::<1>()?[0] & FLAG_RESUMABLE != 0;
            let session = u64::from_le_bytes(h.take()?);
            let idx = u32::from_le_bytes(h.take()?);
            let budget = Duration::from_millis(u32::from_le_bytes(h.take()?).into());
            let base_len = u64::from_le_bytes(h.take()?);
            let base_hash: [u8; 32] = h.take()?;
            let delta_len = u32::from_le_bytes(h.take()?) as usize;
            if delta_len > MAX_DELTA {
                bail!("delta of {delta_len} bytes refused");
            }
            let mut delta = vec![0u8; delta_len];
            recv.read_exact(&mut delta).await?;

            let key = (session, idx);
            let turn = Arc::new(Turn {
                out: watch::Sender::new(Out {
                    bytes: Vec::new(),
                    phase: Phase::Running,
                }),
                deadline: Instant::now() + budget,
                generator: Mutex::new(None),
            });
            let accepted = {
                // One critical section: a generator that is finishing either appended
                // to the session before this, or finds itself replaced and does not.
                let mut turns = lock(&state.turns);
                let accepted = accept_delta(&state, session, base_len, &base_hash, &delta);
                if accepted {
                    // Opening a turn acknowledges the session's earlier ones, and
                    // replaces a previous attempt at this one.
                    turns.retain(|(s, t), old| {
                        let stale = *s == session && *t <= idx;
                        if stale {
                            old.abort();
                        }
                        !stale
                    });
                    turns.insert(key, turn.clone());
                }
                accepted
            };
            if !accepted {
                return reply(&mut send, ST_NEED_FULL).await;
            }
            let generator = tokio::spawn(generate(state.clone(), key, turn.clone()));
            *lock(&turn.generator) = Some(generator.abort_handle());
            (key, turn, 0, resumable)
        }
        RESUME => {
            let mut header = [0u8; RESUME_HEADER];
            recv.read_exact(&mut header).await?;
            let mut h = Cursor(&header);
            let key = (u64::from_le_bytes(h.take()?), u32::from_le_bytes(h.take()?));
            let offset = u64::from_le_bytes(h.take()?) as usize;
            let Some(turn) = lock(&state.turns).get(&key).cloned() else {
                return reply(&mut send, ST_UNKNOWN_TURN).await;
            };
            // A client cannot hold bytes that were never generated.
            if offset > turn.out.borrow().bytes.len() {
                return reply(&mut send, ST_BAD_OFFSET).await;
            }
            (key, turn, offset, true)
        }
        other => bail!("unknown request kind {other}"),
    };

    // The deadline bounds the writes too: a write blocked on flow control would
    // otherwise never see the turn expire.
    let sent =
        match tokio::time::timeout_at(turn.deadline, stream_out(&mut send, &turn, offset)).await {
            Ok(sent) => sent,
            Err(_) => {
                let _ = send.reset(ERR_DEADLINE);
                Ok(())
            }
        };
    if sent.is_err() && !resumable {
        // Status quo: the generation dies with its stream, and so does its state.
        turn.abort();
        let mut turns = lock(&state.turns);
        if turns.get(&key).is_some_and(|held| Arc::ptr_eq(held, &turn)) {
            turns.remove(&key);
        }
    }
    sent
}

/// Checks the client's base against what is held, and only then changes anything.
fn accept_delta(
    state: &State,
    session: u64,
    base_len: u64,
    base_hash: &[u8; 32],
    delta: &[u8],
) -> bool {
    let mut sessions = lock(&state.sessions);
    let empty = blake3::Hasher::new();
    let (held_len, held_hash) = match sessions.get(&session) {
        _ if base_len == 0 => (0, &empty),
        Some(held) => (held.len, &held.hash),
        None => return false,
    };
    if held_len != base_len || held_hash.finalize().as_bytes() != base_hash {
        return false;
    }
    let mut hash = held_hash.clone();
    hash.update(delta);
    let len = base_len + delta.len() as u64;
    sessions.insert(session, Session { len, hash });
    true
}

async fn generate(state: Arc<State>, key: TurnKey, turn: Arc<Turn>) {
    let workload = state.workload.clone();
    let tokens = workload.tokens(key.1);
    let run = async {
        let first = Instant::now() + workload.prefill;
        for (i, token) in tokens.iter().enumerate() {
            tokio::time::sleep_until(first + workload.token_interval * i as u32).await;
            turn.out
                .send_modify(|out| out.bytes.extend_from_slice(token.as_bytes()));
        }
    };
    let current = |turns: &HashMap<TurnKey, Arc<Turn>>| {
        turns.get(&key).is_some_and(|held| Arc::ptr_eq(held, &turn))
    };
    if tokio::time::timeout_at(turn.deadline, run).await.is_err() {
        turn.out.send_modify(|out| out.phase = Phase::Expired);
        lock(&state.expired).push(turn.out.borrow().bytes.len());
        let mut turns = lock(&state.turns);
        if current(&turns) {
            turns.remove(&key);
        }
        return;
    }
    {
        let turns = lock(&state.turns);
        if !current(&turns) {
            return; // replaced while finishing: the session is no longer this turn's
        }
        if let Some(held) = lock(&state.sessions).get_mut(&key.0) {
            let assistant = record(ASSISTANT, &turn.out.borrow().bytes);
            held.hash.update(&assistant);
            held.len += assistant.len() as u64;
        }
    }
    turn.out.send_modify(|out| out.phase = Phase::Done);
    // The deadline is also how long a finished turn stays resumable.
    tokio::time::sleep_until(turn.deadline).await;
    let mut turns = lock(&state.turns);
    if current(&turns) {
        turns.remove(&key);
    }
}

async fn stream_out(send: &mut SendStream, turn: &Turn, mut offset: usize) -> Result<()> {
    let mut out = turn.out.subscribe();
    let mut status_sent = false;
    loop {
        let (chunk, phase) = {
            let out = out.borrow_and_update();
            (
                out.bytes.get(offset..).unwrap_or_default().to_vec(),
                out.phase,
            )
        };
        if phase == Phase::Expired {
            if status_sent {
                send.reset(ERR_DEADLINE)?;
                return Ok(());
            }
            // A reset would discard the status byte with everything else unsent.
            return reply(send, ST_DEADLINE).await;
        }
        if !status_sent {
            send.write_all(&[ST_OK]).await?;
            status_sent = true;
        }
        send.write_all(&chunk).await?;
        offset += chunk.len();
        if phase == Phase::Done {
            send.finish()?;
            return Ok(());
        }
        out.changed().await?;
    }
}

// ---------------------------------------------------------------- client

/// What the client does when the link tells it the path is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// New connection, open the turn again from the start (no turn state kept).
    Restart,
    /// Keep the connection, rebind the socket, let QUIC migrate the path.
    Migrate,
    /// New connection, `RESUME` the turn at the byte offset already received.
    Resume,
}

#[derive(Clone, Copy, Debug)]
pub struct ClientCfg {
    /// Send the prefix as a delta against what the server holds.
    pub delta: bool,
    pub recovery: Recovery,
    pub deadline: Duration,
    /// With `Recovery::Resume`: once output is flowing, this much silence makes the
    /// client `RESUME` on a fresh stream of the same connection. New stream data is
    /// sent at once, where the stalled stream waits for QUIC's probe timeout. Never
    /// less than three smoothed round trips, or a reply could not arrive in time.
    pub silence: Option<Duration>,
    pub tuning: Tuning,
}

/// What a turn has so far, across however many streams it took.
struct Progress {
    start: Instant,
    output: Vec<u8>,
    first_byte: Option<Duration>,
    recoveries: u32,
}

/// What to send next. Built into bytes only once a connection exists, so the budget
/// an `OPEN` declares does not include the handshake.
#[derive(Clone, Copy)]
enum Request {
    Open { full: bool },
    Resume { offset: usize },
}

enum Attempt {
    Done,
    Lost,
    Silent,
    NeedFull,
    /// The server no longer knows the turn, or refuses the offset: start it again.
    StartOver,
}

pub struct TurnClient {
    cfg: ClientCfg,
    relay: SocketAddr,
    tls: quinn::ClientConfig,
    link_up: watch::Receiver<bool>,
    conn: Option<(Endpoint, Connection)>,
    pub session: u64,
    /// The canonical context so far, and the hash of the part the server should hold.
    context: Vec<u8>,
    held: blake3::Hasher,
    held_len: usize,
}

impl TurnClient {
    pub fn new(
        cfg: ClientCfg,
        relay: SocketAddr,
        cert: CertificateDer<'static>,
        link_up: watch::Receiver<bool>,
        session: u64,
        system: &str,
    ) -> Result<Self> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert)?;
        let mut tls = quinn::ClientConfig::with_root_certificates(Arc::new(roots))?;
        tls.transport_config(transport(cfg.tuning, true)?);
        Ok(Self {
            cfg,
            relay,
            tls,
            link_up,
            conn: None,
            session,
            context: record(SYSTEM, system.as_bytes()),
            held: blake3::Hasher::new(),
            held_len: 0,
        })
    }

    async fn connection(&mut self) -> Result<Connection> {
        if let Some((_, conn)) = &self.conn {
            return Ok(conn.clone());
        }
        self.link_up.wait_for(|up| *up).await?;
        let mut endpoint = Endpoint::client("127.0.0.1:0".parse()?)?;
        endpoint.set_default_client_config(self.tls.clone());
        let conn = endpoint.connect(self.relay, "localhost")?.await?;
        self.conn = Some((endpoint, conn.clone()));
        Ok(conn)
    }

    /// An error is expected while the link says it is down. One while it is up is
    /// either a blackout killing the connection or a bug, and must not pass silently
    /// as "path lost".
    fn lost(&self, doing: &str, error: &dyn std::fmt::Display) -> Attempt {
        if *self.link_up.borrow() {
            eprintln!("turn client: {doing} failed with the link up: {error}");
        }
        Attempt::Lost
    }

    fn open_request(&self, idx: u32, full: bool, deadline: Instant) -> Vec<u8> {
        let base_len = if full { 0 } else { self.held_len };
        let base_hash = if full {
            blake3::Hasher::new().finalize()
        } else {
            self.held.finalize()
        };
        let delta = &self.context[base_len..];
        let resumable = self.cfg.recovery == Recovery::Resume;
        let budget = deadline.saturating_duration_since(Instant::now());
        let mut req = vec![OPEN, if resumable { FLAG_RESUMABLE } else { 0 }];
        req.extend_from_slice(&self.session.to_le_bytes());
        req.extend_from_slice(&idx.to_le_bytes());
        req.extend_from_slice(&(budget.as_millis() as u32).to_le_bytes());
        req.extend_from_slice(&(base_len as u64).to_le_bytes());
        req.extend_from_slice(base_hash.as_bytes());
        req.extend_from_slice(&(delta.len() as u32).to_le_bytes());
        req.extend_from_slice(delta);
        req
    }

    fn resume_request(&self, idx: u32, offset: usize) -> Vec<u8> {
        let mut req = vec![RESUME];
        req.extend_from_slice(&self.session.to_le_bytes());
        req.extend_from_slice(&idx.to_le_bytes());
        req.extend_from_slice(&(offset as u64).to_le_bytes());
        req
    }

    pub async fn turn(&mut self, idx: u32, user: &str) -> Result<TurnStats> {
        let deadline = Instant::now() + self.cfg.deadline;
        self.context.extend(record(USER, user.as_bytes()));
        let mut p = Progress {
            start: Instant::now(),
            output: Vec::new(),
            first_byte: None,
            recoveries: 0,
        };
        let (mut discarded, mut fallbacks) = (0, 0);
        let open = Request::Open {
            full: !self.cfg.delta,
        };
        let mut request = open;

        loop {
            let backstop = deadline + CLIENT_GRACE;
            let outcome =
                tokio::time::timeout_at(backstop, self.attempt(idx, request, deadline, &mut p))
                    .await
                    .map_err(|_| anyhow!("turn {idx}: deadline exceeded at the client"))??;
            match outcome {
                Attempt::Done => break,
                Attempt::NeedFull | Attempt::StartOver => {
                    fallbacks += 1;
                    discarded += std::mem::take(&mut p.output).len();
                    request = Request::Open { full: true };
                }
                Attempt::Silent => {
                    p.recoveries += 1;
                    request = Request::Resume {
                        offset: p.output.len(),
                    };
                }
                Attempt::Lost => {
                    p.recoveries += 1;
                    if let Some((endpoint, conn)) = self.conn.take() {
                        conn.close(VarInt::from_u32(0), b"path lost");
                        drop(endpoint);
                    }
                    if self.cfg.recovery == Recovery::Resume {
                        request = Request::Resume {
                            offset: p.output.len(),
                        };
                    } else {
                        discarded += std::mem::take(&mut p.output).len();
                        request = open;
                    }
                }
            }
        }

        let total = p.start.elapsed();
        self.held.update(&self.context[self.held_len..]);
        let assistant = record(ASSISTANT, &p.output);
        self.held.update(&assistant);
        self.context.extend(assistant);
        self.held_len = self.context.len();
        Ok(TurnStats {
            ttft: p.first_byte.unwrap_or(total),
            total,
            recoveries: p.recoveries,
            fallbacks,
            discarded,
            output: String::from_utf8(p.output).context("turn output is not utf-8")?,
        })
    }

    /// One request on one stream. `p` survives a lost attempt.
    async fn attempt(
        &mut self,
        idx: u32,
        request: Request,
        deadline: Instant,
        p: &mut Progress,
    ) -> Result<Attempt> {
        let conn = match self.connection().await {
            Ok(conn) => conn,
            Err(e) => {
                // No signal says when a failed connect may succeed; do not spin.
                tokio::time::sleep(RECONNECT_PAUSE).await;
                return Ok(self.lost("connect", &e));
            }
        };
        let bytes = match request {
            Request::Open { full } => self.open_request(idx, full, deadline),
            Request::Resume { offset } => self.resume_request(idx, offset),
        };
        let mut link_up = self.link_up.clone();
        // Armed by the guards below: for a `RESUME`, and once output has started.
        let silence = self.cfg.silence.map(|limit| limit.max(conn.rtt() * 3));
        let quiet = || async move {
            match silence {
                Some(limit) => tokio::time::sleep(limit).await,
                None => std::future::pending().await,
            }
        };
        let exchange = async {
            let (mut send, mut recv) = conn.open_bi().await?;
            send.write_all(&bytes).await?;
            send.finish()?;
            let mut status = [0u8; 1];
            match recv.read_exact(&mut status).await {
                Ok(()) => {}
                Err(ReadExactError::ReadError(ReadError::Reset(code))) if code == ERR_DEADLINE => {
                    status[0] = ST_DEADLINE;
                }
                Err(e) => return Err(e.into()),
            }
            anyhow::Ok((recv, status[0]))
        };
        let (mut recv, status) = tokio::select! {
            r = exchange => match r {
                Ok(ok) => ok,
                Err(e) => return Ok(self.lost("request", &e)),
            },
            _ = async { link_up.wait_for(|up| !*up).await.map(drop) } => return Ok(Attempt::Lost),
            // Only a `RESUME` is small enough for silence to mean trouble.
            _ = quiet(), if matches!(request, Request::Resume { .. }) => return Ok(Attempt::Silent),
        };
        match status {
            ST_OK => {}
            ST_NEED_FULL => return Ok(Attempt::NeedFull),
            ST_UNKNOWN_TURN | ST_BAD_OFFSET => return Ok(Attempt::StartOver),
            ST_DEADLINE => bail!("server: deadline exceeded"),
            other => bail!("unknown status {other}"),
        }

        let mut buf = vec![0u8; 16 * 1024];
        loop {
            // `None` is the link going down; the watch guard must not outlive the select.
            let read = tokio::select! {
                read = recv.read(&mut buf) => Some(read),
                _ = async { link_up.wait_for(|up| !*up).await.map(drop) } => None,
                _ = quiet(), if p.first_byte.is_some() => return Ok(Attempt::Silent),
            };
            match read {
                Some(Ok(Some(n))) => {
                    p.first_byte.get_or_insert(p.start.elapsed());
                    p.output.extend_from_slice(&buf[..n]);
                }
                Some(Ok(None)) => return Ok(Attempt::Done),
                Some(Err(ReadError::Reset(code))) if code == ERR_DEADLINE => {
                    bail!("server: deadline exceeded")
                }
                Some(Err(e)) => return Ok(self.lost("read", &e)),
                None if self.cfg.recovery != Recovery::Migrate => return Ok(Attempt::Lost),
                None => {
                    // QUIC's own answer: same connection, same stream, new address.
                    p.recoveries += 1;
                    link_up.wait_for(|up| *up).await?;
                    if let Some((endpoint, _)) = &self.conn {
                        endpoint.rebind(std::net::UdpSocket::bind("127.0.0.1:0")?)?;
                    }
                }
            }
        }
    }
}
