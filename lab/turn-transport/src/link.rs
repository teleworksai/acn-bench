//! The impaired link: a loopback relay that every arm dials instead of the server
//! (TCP for `sse`, UDP for the QUIC arms).
//!
//! It delays (one-way latency plus serialisation at the direction's rate, unbounded
//! queue), counts every byte offered to it, and opens a gap on request. A gap applies
//! where traffic enters the link and again where it leaves, so nothing queued crosses it:
//!
//! - `Blackout`: the path survives but carries nothing. UDP datagrams are dropped.
//!   TCP cannot be dropped from user space, so its bytes are held and serialised from
//!   the end of the gap; that omits the kernel's RTO backoff and flatters the baseline.
//! - `Break`: as a blackout, and every flow that existed before the gap is dead
//!   afterwards (handover with an address change, NAT rebinding). TCP connections are
//!   closed; UDP flows from the old client address are black-holed. Clients are told
//!   link-down and link-up, as an OS reporting an interface change would.
//!
//! The relay terminates TCP. The baseline's congestion control and loss recovery
//! therefore see loopback, not this link (no slow start, no retransmission), while
//! QUIC's see the link. A new TCP connection is charged one modelled round trip for its
//! handshake, since the relay's own accept completes at loopback speed.
//!
//! Counters are payload bytes at the relay's ingress: UDP payload (so all of QUIC,
//! including handshake and ACKs) and TCP payload (so none of TCP/IP's headers, SYNs or
//! ACKs). That asymmetry also favours the baseline. Loss is only recorded for UDP: TCP
//! bytes are held, not dropped, and what a `Break` strands in a closed connection is
//! not counted.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct LinkCfg {
    pub one_way: Duration,
    pub up_bps: u64,
    pub down_bps: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum GapKind {
    None,
    Blackout,
    Break,
}

/// Bytes and relay reads offered to the link, per direction. `*_lost` is the part
/// that was dropped, which only happens to UDP. For TCP `*_pkts` counts relay reads,
/// not segments.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct Snapshot {
    pub up_bytes: u64,
    pub up_pkts: u64,
    pub up_lost: u64,
    pub down_bytes: u64,
    pub down_pkts: u64,
    pub down_lost: u64,
}

impl Snapshot {
    pub fn since(&self, earlier: &Snapshot) -> Snapshot {
        Snapshot {
            up_bytes: self.up_bytes - earlier.up_bytes,
            up_pkts: self.up_pkts - earlier.up_pkts,
            up_lost: self.up_lost - earlier.up_lost,
            down_bytes: self.down_bytes - earlier.down_bytes,
            down_pkts: self.down_pkts - earlier.down_pkts,
            down_lost: self.down_lost - earlier.down_lost,
        }
    }
}

#[derive(Clone, Copy)]
enum Dir {
    Up,
    Down,
}

#[derive(Default)]
struct DirCount {
    bytes: AtomicU64,
    pkts: AtomicU64,
    lost: AtomicU64,
}

struct Shaper {
    bps: u64,
    one_way: Duration,
    free_at: Mutex<Instant>,
}

impl Shaper {
    fn new(bps: u64, one_way: Duration) -> Self {
        Self {
            bps,
            one_way,
            free_at: Mutex::new(Instant::now()),
        }
    }

    /// When a unit of `len` bytes entering now leaves the far end of the link.
    /// `held_until` is the end of a gap the unit has to wait out before it is sent.
    fn schedule(&self, len: usize, held_until: Option<Instant>) -> Instant {
        let mut free_at = self.free_at.lock().unwrap_or_else(|e| e.into_inner());
        let start = (*free_at).max(held_until.unwrap_or_else(Instant::now));
        *free_at = start + Duration::from_nanos(len as u64 * 8 * 1_000_000_000 / self.bps);
        *free_at + self.one_way
    }
}

struct Shared {
    up: Shaper,
    down: Shaper,
    up_count: DirCount,
    down_count: DirCount,
    gap_until: Mutex<Option<Instant>>,
    /// Bumped by a `Break`: flows opened under an older epoch are dead.
    epoch: AtomicU64,
    link_up: watch::Sender<bool>,
    shutdown: watch::Sender<bool>,
}

impl Shared {
    fn shaper(&self, dir: Dir) -> &Shaper {
        match dir {
            Dir::Up => &self.up,
            Dir::Down => &self.down,
        }
    }

    fn count(&self, dir: Dir, len: usize) {
        let c = match dir {
            Dir::Up => &self.up_count,
            Dir::Down => &self.down_count,
        };
        c.bytes.fetch_add(len as u64, Relaxed);
        c.pkts.fetch_add(1, Relaxed);
    }

    fn lose(&self, dir: Dir, len: usize) {
        let c = match dir {
            Dir::Up => &self.up_count,
            Dir::Down => &self.down_count,
        };
        c.lost.fetch_add(len as u64, Relaxed);
    }

    fn gap_end(&self) -> Option<Instant> {
        let until = *self.gap_until.lock().unwrap_or_else(|e| e.into_inner());
        until.filter(|t| Instant::now() < *t)
    }
}

pub struct Link {
    /// What the client dials instead of the server.
    pub addr: SocketAddr,
    shared: Arc<Shared>,
}

impl Drop for Link {
    fn drop(&mut self) {
        self.shared.shutdown.send_replace(true);
    }
}

impl Link {
    pub async fn udp(server: SocketAddr, cfg: LinkCfg) -> Result<Self> {
        let shared = shared(cfg);
        let front = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
        let addr = front.local_addr()?;
        tokio::spawn(stop_on_shutdown(
            shared.clone(),
            udp_front(shared.clone(), front, server),
        ));
        Ok(Self { addr, shared })
    }

    pub async fn tcp(server: SocketAddr, cfg: LinkCfg) -> Result<Self> {
        let shared = shared(cfg);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        tokio::spawn(stop_on_shutdown(
            shared.clone(),
            tcp_accept(shared.clone(), listener, server),
        ));
        Ok(Self { addr, shared })
    }

    /// `true` while the client's OS would report the link as usable. Only a
    /// `Break` toggles it; a blackout is invisible to the endpoints.
    pub fn state(&self) -> watch::Receiver<bool> {
        self.shared.link_up.subscribe()
    }

    pub fn snapshot(&self) -> Snapshot {
        let (u, d) = (&self.shared.up_count, &self.shared.down_count);
        Snapshot {
            up_bytes: u.bytes.load(Relaxed),
            up_pkts: u.pkts.load(Relaxed),
            up_lost: u.lost.load(Relaxed),
            down_bytes: d.bytes.load(Relaxed),
            down_pkts: d.pkts.load(Relaxed),
            down_lost: d.lost.load(Relaxed),
        }
    }

    /// Open a gap now and return when it has closed.
    pub async fn gap(&self, kind: GapKind, duration: Duration) {
        if kind == GapKind::None {
            return;
        }
        let until = Instant::now() + duration;
        let set = |v| {
            *self
                .shared
                .gap_until
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = v
        };
        set(Some(until));
        if kind == GapKind::Break {
            self.shared.epoch.fetch_add(1, Relaxed);
            self.shared.link_up.send_replace(false);
        }
        tokio::time::sleep_until(until).await;
        set(None);
        if kind == GapKind::Break {
            self.shared.link_up.send_replace(true);
        }
    }
}

fn shared(cfg: LinkCfg) -> Arc<Shared> {
    Arc::new(Shared {
        up: Shaper::new(cfg.up_bps, cfg.one_way),
        down: Shaper::new(cfg.down_bps, cfg.one_way),
        up_count: DirCount::default(),
        down_count: DirCount::default(),
        gap_until: Mutex::new(None),
        epoch: AtomicU64::new(0),
        link_up: watch::Sender::new(true),
        shutdown: watch::Sender::new(false),
    })
}

async fn stop_on_shutdown(shared: Arc<Shared>, task: impl Future<Output = Result<()>>) {
    let mut shutdown = shared.shutdown.subscribe();
    tokio::select! {
        _ = shutdown.wait_for(|stop| *stop) => {}
        _ = task => {}
    }
}

// ---------------------------------------------------------------- UDP

struct Datagram {
    at: Instant,
    dir: Dir,
    /// The epoch its flow was opened under.
    opened: u64,
    payload: Vec<u8>,
    via: Arc<UdpSocket>,
    /// `None` when `via` is connected (the server-facing sockets).
    to: Option<SocketAddr>,
}

async fn udp_deliver(
    shared: Arc<Shared>,
    mut queue: mpsc::UnboundedReceiver<Datagram>,
) -> Result<()> {
    while let Some(d) = queue.recv().await {
        tokio::time::sleep_until(d.at).await;
        // Queued before the gap or the break, leaving during or after it: lost.
        if shared.gap_end().is_some() || shared.epoch.load(Relaxed) != d.opened {
            shared.lose(d.dir, d.payload.len());
            continue;
        }
        // A send error here is a closed peer socket; to the sender it is loss.
        let _ = match d.to {
            Some(to) => d.via.send_to(&d.payload, to).await,
            None => d.via.send(&d.payload).await,
        };
    }
    Ok(())
}

/// Client-facing socket. Each client address is a flow with its own server-facing
/// socket, so a client that rebinds reaches the server from a new address.
async fn udp_front(shared: Arc<Shared>, front: Arc<UdpSocket>, server: SocketAddr) -> Result<()> {
    let (up_tx, up_rx) = mpsc::unbounded_channel();
    let (down_tx, down_rx) = mpsc::unbounded_channel();
    tokio::spawn(stop_on_shutdown(
        shared.clone(),
        udp_deliver(shared.clone(), up_rx),
    ));
    tokio::spawn(stop_on_shutdown(
        shared.clone(),
        udp_deliver(shared.clone(), down_rx),
    ));

    let mut flows: HashMap<SocketAddr, (u64, Arc<UdpSocket>)> = HashMap::new();
    let mut buf = vec![0u8; 65_535];
    loop {
        let (n, client) = front.recv_from(&mut buf).await?;
        shared.count(Dir::Up, n);
        let epoch = shared.epoch.load(Relaxed);
        if shared.gap_end().is_some() {
            shared.lose(Dir::Up, n);
            continue;
        }
        let back = match flows.get(&client) {
            Some((opened, back)) if *opened == epoch => back.clone(),
            Some(_) => {
                shared.lose(Dir::Up, n);
                continue;
            }
            None => {
                let back = Arc::new(UdpSocket::bind("127.0.0.1:0").await?);
                back.connect(server).await?;
                flows.insert(client, (epoch, back.clone()));
                tokio::spawn(stop_on_shutdown(
                    shared.clone(),
                    udp_back(
                        shared.clone(),
                        back.clone(),
                        front.clone(),
                        client,
                        epoch,
                        down_tx.clone(),
                    ),
                ));
                back
            }
        };
        let _ = up_tx.send(Datagram {
            at: shared.shaper(Dir::Up).schedule(n, None),
            dir: Dir::Up,
            opened: epoch,
            payload: buf[..n].to_vec(),
            via: back,
            to: None,
        });
    }
}

async fn udp_back(
    shared: Arc<Shared>,
    back: Arc<UdpSocket>,
    front: Arc<UdpSocket>,
    client: SocketAddr,
    opened: u64,
    down_tx: mpsc::UnboundedSender<Datagram>,
) -> Result<()> {
    let mut buf = vec![0u8; 65_535];
    loop {
        let n = back.recv(&mut buf).await?;
        shared.count(Dir::Down, n);
        if shared.gap_end().is_some() || shared.epoch.load(Relaxed) != opened {
            shared.lose(Dir::Down, n);
            continue;
        }
        let _ = down_tx.send(Datagram {
            at: shared.shaper(Dir::Down).schedule(n, None),
            dir: Dir::Down,
            opened,
            payload: buf[..n].to_vec(),
            via: front.clone(),
            to: Some(client),
        });
    }
}

// ---------------------------------------------------------------- TCP

async fn tcp_accept(shared: Arc<Shared>, listener: TcpListener, server: SocketAddr) -> Result<()> {
    loop {
        let (client, _) = listener.accept().await?;
        if !*shared.link_up.borrow() {
            continue; // dropped: the link is down, nothing connects
        }
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut link_up = shared.link_up.subscribe();
            tokio::select! {
                // A `Break` drops both sockets, which closes them.
                _ = link_up.wait_for(|up| !*up) => {}
                _ = stop_on_shutdown(shared.clone(), tcp_conn(shared.clone(), client, server)) => {}
            }
        });
    }
}

async fn tcp_conn(shared: Arc<Shared>, client: TcpStream, server: SocketAddr) -> Result<()> {
    let upstream = TcpStream::connect(server).await?;
    // The relay's accept completed at loopback speed; charge the handshake's round
    // trip here. (Slow start is not charged: the relay terminates TCP, so the
    // baseline's congestion control and loss recovery see loopback, not this link.)
    tokio::time::sleep(shared.up.one_way + shared.down.one_way).await;
    client.set_nodelay(true)?;
    upstream.set_nodelay(true)?;
    let (client_rd, client_wr) = client.into_split();
    let (server_rd, server_wr) = upstream.into_split();
    tokio::try_join!(
        tcp_pump(shared.clone(), Dir::Up, client_rd, server_wr),
        tcp_pump(shared.clone(), Dir::Down, server_rd, client_wr),
    )?;
    Ok(())
}

async fn tcp_pump(
    shared: Arc<Shared>,
    dir: Dir,
    mut from: OwnedReadHalf,
    mut to: OwnedWriteHalf,
) -> Result<()> {
    // `None` marks end of stream, delivered in order like the data before it.
    let (tx, mut rx) = mpsc::unbounded_channel::<(Instant, Option<Vec<u8>>)>();
    let writer = async move {
        while let Some((at, chunk)) = rx.recv().await {
            tokio::time::sleep_until(at).await;
            match chunk {
                Some(bytes) => to.write_all(&bytes).await?,
                None => to.shutdown().await?,
            }
        }
        anyhow::Ok(())
    };
    let reader = async move {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = from.read(&mut buf).await?;
            if n > 0 {
                shared.count(dir, n);
            }
            // Held through a gap, then serialised at the link's rate like anything else.
            let at = shared.shaper(dir).schedule(n, shared.gap_end());
            let _ = tx.send((at, (n > 0).then(|| buf[..n].to_vec())));
            if n == 0 {
                return anyhow::Ok(());
            }
        }
    };
    tokio::try_join!(reader, writer)?;
    Ok(())
}
