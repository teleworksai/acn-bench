//! The live proxy (SPEC 020 §5, EMU-40 to EMU-47): an HTTP/1.1 proxy on the
//! loopback interface that carries a replicate's calls to its endpoint across
//! a scenario's one path, applying the same link models as the sim network to
//! the same messages (a request body, a response body, each SSE event).
//!
//! A request is read whole, offered to the uplink, and forwarded upstream at its
//! delivery time by a task of its own, so a client that gives up does not stop
//! it (EMU-33, EMU-45). A response is offered to the downlink as it is read
//! (whole, or event by event) and handed to the client no earlier than each
//! message's delivery time (EMU-43). A drop becomes what the client sees in
//! `sim`: silence, or an aborted connection (EMU-44).

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode, Uri};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt as _, Empty, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::client::conn::http1::SendRequest;
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;

use crate::clock::Clock;
use crate::link::{Direction, Fate, Link, LinkModel as _};

/// The header that tags an attempt (EMU-47).
pub const ATTEMPT_HEADER: &str = "x-acn-attempt";

/// Why the proxy could not start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}: {message}")]
pub struct ProxyError {
    /// `endpoint` (an endpoint the proxy cannot reach without `real-api`, or
    /// not a URL), `bind` (no loopback port) or `link` (a link out of range).
    pub reason: &'static str,
    pub message: String,
}

fn fail<T>(reason: &'static str, message: impl Into<String>) -> Result<T, ProxyError> {
    Err(ProxyError {
        reason,
        message: message.into(),
    })
}

/// One message as the proxy carried it (EMU-47): times on the link's scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub direction: Direction,
    pub bytes: u64,
    pub fate: Fate,
    /// When the proxy handed it to the client's connection; `None` if dropped
    /// or not yet handed over. For a stream event this is when it enters the
    /// response body's queue, which a client that reads slowly drains later.
    pub received_ns: Option<i64>,
}

/// The endpoint a proxy forwards to.
#[derive(Debug, Clone)]
struct Endpoint {
    /// `host:port` to connect to.
    addr: String,
    /// The `Host` header to send.
    host: HeaderValue,
    /// The base path, without a trailing `/`.
    base: String,
}

impl Endpoint {
    fn parse(endpoint: &str) -> Result<Self, ProxyError> {
        let uri: Uri = endpoint.parse().map_err(|e| ProxyError {
            reason: "endpoint",
            message: format!("`{endpoint}`: {e}"),
        })?;
        match uri.scheme_str() {
            Some("http") => {}
            Some("https") => {
                return fail(
                    "endpoint",
                    "an https endpoint needs TLS upstream, built only with `real-api` (SPEC 020 EMU-45)",
                );
            }
            _ => return fail("endpoint", format!("`{endpoint}` is not an http URL")),
        }
        let Some(auth) = uri.authority() else {
            return fail("endpoint", format!("`{endpoint}` names no host"));
        };
        if auth.as_str().contains('@') || uri.query().is_some() {
            return fail(
                "endpoint",
                "an endpoint carries no user information and no query",
            );
        }
        let port = auth.port_u16().unwrap_or(80);
        let host = HeaderValue::from_str(auth.as_str()).map_err(|e| ProxyError {
            reason: "endpoint",
            message: e.to_string(),
        })?;
        Ok(Self {
            addr: format!("{}:{port}", auth.host()),
            host,
            base: uri.path().trim_end_matches('/').to_owned(),
        })
    }
}

/// The two links of the path.
struct Links {
    up: Link,
    down: Link,
}

/// What the proxy recorded (EMU-47).
#[derive(Default)]
struct Log {
    /// Every record, in the order its fate was decided, with its attempt.
    records: Vec<(Option<u64>, Record)>,
}

/// State shared by every task of a proxy.
struct Shared {
    clock: Arc<dyn Clock>,
    origin_ns: i64,
    endpoint: Endpoint,
    links: Mutex<Links>,
    log: Mutex<Log>,
    stop: watch::Receiver<bool>,
    /// Every task the proxy spawned, aborted and joined at shutdown (EMU-40).
    tasks: Mutex<JoinSet<()>>,
    /// The first internal fault (a link refusing a message, a poisoned lock):
    /// the run, not the network, is broken, and the harness says so.
    fault: Mutex<Option<String>>,
}

impl Shared {
    /// Link time now (EMU-42).
    fn now(&self) -> i64 {
        self.clock.now_ns().saturating_sub(self.origin_ns)
    }

    /// Spawn a task the proxy owns.
    fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(&self, f: F) {
        if let Ok(mut tasks) = self.tasks.lock() {
            while tasks.try_join_next().is_some() {}
            tasks.spawn(f);
        }
    }

    /// Note an internal fault: the first one is kept, and logged without
    /// headers or bodies (HAR-22).
    fn fault(&self, what: String) {
        tracing::error!(fault = %what, "live proxy");
        if let Ok(mut f) = self.fault.lock()
            && f.is_none()
        {
            *f = Some(what);
        }
    }

    /// Wait until link time `t`.
    async fn sleep_until(&self, t: i64) {
        self.clock
            .sleep_until(self.origin_ns.saturating_add(t))
            .await;
    }

    /// Offer a message of `bytes` read now to the link in `direction`, and
    /// record its fate under `attempt` (EMU-42, EMU-47). The clock is read and
    /// the message offered under one lock, so a link's send times never
    /// decrease.
    fn offer(
        &self,
        direction: Direction,
        attempt: Option<u64>,
        bytes: u64,
    ) -> Option<(Fate, usize)> {
        let Ok(mut links) = self.links.lock() else {
            self.fault("the links' lock is poisoned".into());
            return None;
        };
        let send = self.now();
        let link = match direction {
            Direction::Up => &mut links.up,
            Direction::Down => &mut links.down,
        };
        let fate = match link.transmit(send, bytes) {
            Ok(f) => f,
            Err(e) => {
                self.fault(format!("the {direction} link refused a message: {e}"));
                return None;
            }
        };
        let Ok(mut log) = self.log.lock() else {
            self.fault("the log's lock is poisoned".into());
            return None;
        };
        log.records.push((
            attempt,
            Record {
                direction,
                bytes,
                fate,
                received_ns: None,
            },
        ));
        Some((fate, log.records.len() - 1))
    }

    /// Note that record `i` was handed to the client now.
    fn handed(&self, i: usize) {
        let now = self.now();
        if let Ok(mut log) = self.log.lock()
            && let Some((_, r)) = log.records.get_mut(i)
        {
            r.received_ns = Some(now);
        }
    }

    /// Wait until the proxy is told to stop.
    async fn stopped(&self) {
        let mut stop = self.stop.clone();
        while !*stop.borrow() {
            if stop.changed().await.is_err() {
                return;
            }
        }
    }
}

type Body = BoxBody<Bytes, std::io::Error>;

/// What a forward hands the client's connection: a response head, or an
/// abort; nothing at all keeps the connection silent (EMU-44).
enum Head {
    Response(Response<Body>),
    Abort,
}

/// A running proxy (EMU-40).
pub struct Proxy {
    addr: SocketAddr,
    shared: Arc<Shared>,
    stop: watch::Sender<bool>,
    accept: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl std::fmt::Debug for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proxy").field("addr", &self.addr).finish()
    }
}

impl Proxy {
    /// Start a proxy on `127.0.0.1:0` that forwards to `endpoint` across `up`
    /// and `down`, already built from the replicate seed (EMU-40), with time 0
    /// of the links at `origin_ns` on `clock`.
    pub async fn start(
        up: Link,
        down: Link,
        endpoint: &str,
        clock: Arc<dyn Clock>,
        origin_ns: i64,
    ) -> Result<Self, ProxyError> {
        if up.spec().direction != Direction::Up || down.spec().direction != Direction::Down {
            return fail("link", "the proxy takes an up link and a down link");
        }
        let endpoint = Endpoint::parse(endpoint)?;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| ProxyError {
                reason: "bind",
                message: e.to_string(),
            })?;
        let addr = listener.local_addr().map_err(|e| ProxyError {
            reason: "bind",
            message: e.to_string(),
        })?;
        let (stop, stop_rx) = watch::channel(false);
        let shared = Arc::new(Shared {
            clock,
            origin_ns,
            endpoint,
            links: Mutex::new(Links { up, down }),
            log: Mutex::new(Log::default()),
            stop: stop_rx,
            tasks: Mutex::new(JoinSet::new()),
            fault: Mutex::new(None),
        });
        let accept = tokio::spawn(accept_loop(listener, Arc::clone(&shared)));
        Ok(Self {
            addr,
            shared,
            stop,
            accept: Mutex::new(Some(accept)),
        })
    }

    /// The loopback address to send requests to.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Attempt `attempt`'s records so far, in decision order (EMU-47).
    #[must_use]
    pub fn records(&self, attempt: u64) -> Vec<Record> {
        self.shared.log.lock().map_or_else(
            |_| Vec::new(),
            |log| {
                log.records
                    .iter()
                    .filter(|(a, _)| *a == Some(attempt))
                    .map(|(_, r)| *r)
                    .collect()
            },
        )
    }

    /// Every fate the proxy decided, in order (EMU-37, EMU-47).
    #[must_use]
    pub fn fates(&self) -> Vec<(Direction, Fate)> {
        self.shared.log.lock().map_or_else(
            |_| Vec::new(),
            |log| {
                log.records
                    .iter()
                    .map(|(_, r)| (r.direction, r.fate))
                    .collect()
            },
        )
    }

    /// Close every connection and abort every forward in flight (EMU-40).
    pub async fn shutdown(&self) {
        let _ = self.stop.send(true);
        let accept = self.accept.lock().ok().and_then(|mut a| a.take());
        if let Some(a) = accept {
            let _ = a.await;
        }
        let mut tasks = self
            .shared
            .tasks
            .lock()
            .map(|mut t| std::mem::take(&mut *t))
            .unwrap_or_default();
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }

    /// The first internal fault, if any: a link that refused a message, a
    /// poisoned lock. A replicate whose proxy faulted is not a measurement.
    #[must_use]
    pub fn fault(&self) -> Option<String> {
        self.shared.fault.lock().ok().and_then(|f| f.clone())
    }
}

impl Drop for Proxy {
    /// A proxy dropped without `shutdown` still stops listening and lets its
    /// tasks end (EMU-40).
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

async fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    loop {
        let conn = tokio::select! {
            () = shared.stopped() => return,
            c = listener.accept() => c,
        };
        let Ok((stream, _)) = conn else { continue };
        let owner = Arc::clone(&shared);
        let shared = Arc::clone(&shared);
        owner.spawn(async move {
            // EMU-45: the upstream connections of this downstream connection.
            let pool: Pool = Arc::new(tokio::sync::Mutex::new(Vec::new()));
            let svc_shared = Arc::clone(&shared);
            let service = hyper::service::service_fn(move |req| {
                serve(req, Arc::clone(&svc_shared), Arc::clone(&pool))
            });
            let conn = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service);
            tokio::select! {
                () = shared.stopped() => {}
                _ = conn => {}
            }
        });
    }
}

type Pool = Arc<tokio::sync::Mutex<Vec<SendRequest<Full<Bytes>>>>>;

/// Hop-by-hop headers (RFC 9110 §7.6.1), and those a `Connection` header names.
fn hop_by_hop(headers: &HeaderMap) -> Vec<HeaderName> {
    let mut out: Vec<HeaderName> = [
        "connection",
        "keep-alive",
        "proxy-connection",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "proxy-authenticate",
        "proxy-authorization",
    ]
    .iter()
    .filter_map(|n| HeaderName::from_bytes(n.as_bytes()).ok())
    .collect();
    for v in headers.get_all(http::header::CONNECTION) {
        if let Ok(v) = v.to_str() {
            out.extend(
                v.split(',')
                    .filter_map(|n| HeaderName::from_bytes(n.trim().as_bytes()).ok()),
            );
        }
    }
    out
}

fn status(code: StatusCode) -> Response<Body> {
    let mut r = Response::new(Empty::new().map_err(|e: Infallible| match e {}).boxed());
    *r.status_mut() = code;
    r
}

/// One request on a client's connection (EMU-41 to EMU-44).
async fn serve(
    req: Request<Incoming>,
    shared: Arc<Shared>,
    pool: Pool,
) -> Result<Response<Body>, std::io::Error> {
    // EMU-41: what cannot be framed is refused, and offered to no link.
    if req.method() == http::Method::CONNECT || req.headers().contains_key(http::header::UPGRADE) {
        return Ok(status(StatusCode::NOT_IMPLEMENTED));
    }
    let attempt = req
        .headers()
        .get(ATTEMPT_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let (parts, body) = req.into_parts();
    // A request whose client leaves before its body is read takes no index.
    let body = body
        .collect()
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?
        .to_bytes();
    let len = u64::try_from(body.len()).unwrap_or(u64::MAX);
    let Some((fate, up_record)) = shared.offer(Direction::Up, attempt, len) else {
        return Err(std::io::Error::other("the uplink refused the request"));
    };
    let (head_tx, head_rx) = oneshot::channel();
    match fate.outcome {
        // EMU-44: a lost request is never forwarded; the connection is silent.
        Err(_) => {
            shared.stopped().await;
            Err(std::io::Error::other("the proxy stopped"))
        }
        Ok(deliver) => {
            // EMU-45: the forward runs apart from this connection's task.
            shared.spawn(forward(
                Arc::clone(&shared),
                pool,
                parts,
                body,
                attempt,
                (deliver, up_record),
                head_tx,
            ));
            tokio::select! {
                () = shared.stopped() => Err(std::io::Error::other("the proxy stopped")),
                h = head_rx => match h {
                    Ok(Head::Response(r)) => Ok(r),
                    Ok(Head::Abort) => Err(std::io::Error::other("aborted")),
                    // The forward ended without a word: stay silent.
                    Err(_) => {
                        shared.stopped().await;
                        Err(std::io::Error::other("the proxy stopped"))
                    }
                },
            }
        }
    }
}

/// An idle upstream connection of this downstream connection, or a new one.
async fn upstream(shared: &Shared, pool: &Pool) -> Option<SendRequest<Full<Bytes>>> {
    {
        let mut idle = pool.lock().await;
        while let Some(mut s) = idle.pop() {
            if s.ready().await.is_ok() {
                return Some(s);
            }
        }
    }
    let stream = TcpStream::connect(&shared.endpoint.addr).await.ok()?;
    let (send, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .ok()?;
    shared.spawn(async move {
        let _ = conn.await;
    });
    Some(send)
}

/// Forward a delivered request at its delivery time, and carry its response
/// back (EMU-43 to EMU-45).
async fn forward(
    shared: Arc<Shared>,
    pool: Pool,
    parts: http::request::Parts,
    body: Bytes,
    attempt: Option<u64>,
    deliver: (i64, usize),
    head_tx: oneshot::Sender<Head>,
) {
    tokio::select! {
        () = shared.stopped() => {}
        () = carry(&shared, &pool, parts, body, attempt, deliver, head_tx) => {}
    }
}

#[allow(clippy::too_many_lines)]
async fn carry(
    shared: &Arc<Shared>,
    pool: &Pool,
    parts: http::request::Parts,
    body: Bytes,
    attempt: Option<u64>,
    (deliver, up_record): (i64, usize),
    head_tx: oneshot::Sender<Head>,
) {
    shared.sleep_until(deliver).await;
    // The request is received when the proxy forwards it (EMU-43, EMU-47).
    shared.handed(up_record);
    // EMU-46: the request as sent, save hop-by-hop headers, the attempt
    // header, `Host` and the base path.
    let path = parts
        .uri
        .path_and_query()
        .map_or("/", http::uri::PathAndQuery::as_str);
    let Ok(uri) = format!("{}{path}", shared.endpoint.base).parse::<Uri>() else {
        let _ = head_tx.send(Head::Abort);
        return;
    };
    let mut req = Request::new(Full::new(body));
    *req.method_mut() = parts.method.clone();
    *req.uri_mut() = uri;
    let drop_names = hop_by_hop(&parts.headers);
    for (k, v) in &parts.headers {
        if drop_names.contains(k) || k.as_str() == ATTEMPT_HEADER || k == http::header::HOST {
            continue;
        }
        req.headers_mut().append(k.clone(), v.clone());
    }
    req.headers_mut()
        .insert(http::header::HOST, shared.endpoint.host.clone());
    let Some(mut send) = upstream(shared, pool).await else {
        // EMU-45: an upstream that cannot be reached closes the client's
        // connection at once.
        let _ = head_tx.send(Head::Abort);
        return;
    };
    let Ok(resp) = send.send_request(req).await else {
        let _ = head_tx.send(Head::Abort);
        return;
    };
    let (rparts, rbody) = resp.into_parts();
    let event_stream = rparts.status == StatusCode::OK
        && rparts
            .headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
    let mut head = Response::new(Empty::new().map_err(|e: Infallible| match e {}).boxed());
    *head.status_mut() = rparts.status;
    // A body the proxy re-frames gets its own length; a response without a
    // body (HEAD, 204, 304) keeps the upstream's, as sent (EMU-46).
    let bodiless = parts.method == http::Method::HEAD
        || rparts.status == StatusCode::NO_CONTENT
        || rparts.status == StatusCode::NOT_MODIFIED;
    let drop_names = hop_by_hop(&rparts.headers);
    for (k, v) in &rparts.headers {
        if drop_names.contains(k) || (k == http::header::CONTENT_LENGTH && !bodiless) {
            continue;
        }
        head.headers_mut().append(k.clone(), v.clone());
    }
    if event_stream {
        stream_back(shared, pool, send, rbody, head, attempt, head_tx).await;
    } else {
        // A whole body: one message (EMU-41).
        let Ok(bytes) = rbody
            .collect()
            .await
            .map(http_body_util::Collected::to_bytes)
        else {
            let _ = head_tx.send(Head::Abort);
            return;
        };
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        let offered = shared.offer(Direction::Down, attempt, len);
        pool.lock().await.push(send);
        let Some((fate, i)) = offered else {
            let _ = head_tx.send(Head::Abort);
            return;
        };
        match fate.outcome {
            // EMU-44: a lost body leaves the connection silent.
            Err(_) => shared.stopped().await,
            Ok(at) => {
                shared.sleep_until(at).await;
                *head.body_mut() = Full::new(bytes).map_err(|e: Infallible| match e {}).boxed();
                if head_tx.send(Head::Response(head)).is_ok() {
                    shared.handed(i);
                }
            }
        }
    }
}

/// Where the end of an SSE event is in `buf`: just past the first empty line,
/// lines ending in CRLF, LF or CR alike (EMU-41). A CR at the end of `buf`
/// waits for the next byte, which may be its LF.
fn event_end(buf: &[u8]) -> Option<usize> {
    let mut i = 0;
    let mut line_start = 0;
    while i < buf.len() {
        let eol = match buf[i] {
            b'\n' => 1,
            b'\r' => match buf.get(i + 1) {
                Some(b'\n') => 2,
                Some(_) => 1,
                None => return None,
            },
            _ => {
                i += 1;
                continue;
            }
        };
        if i == line_start {
            return Some(i + eol);
        }
        i += eol;
        line_start = i;
    }
    None
}

/// Whether an event is only comment lines (`: …`): it belongs to the next.
fn only_comments(event: &[u8]) -> bool {
    event
        .split(|b| *b == b'\n' || *b == b'\r')
        .filter(|l| !l.is_empty())
        .all(|l| l.first() == Some(&b':'))
}

/// What the reader hands the writer: an event, or the upstream's failure.
enum Read {
    Event(Event),
    /// The upstream's body broke, or a link refused an event: the client's
    /// response is aborted after the events before it (EMU-45).
    Failed,
}

/// One SSE event read from upstream, with its fate.
struct Event {
    bytes: Bytes,
    fate: Fate,
    record: usize,
}

/// Carry an event stream back, event by event (EMU-41, EMU-43, EMU-44).
async fn stream_back(
    shared: &Arc<Shared>,
    pool: &Pool,
    send: SendRequest<Full<Bytes>>,
    mut rbody: Incoming,
    mut head: Response<Body>,
    attempt: Option<u64>,
    head_tx: oneshot::Sender<Head>,
) {
    let (events_tx, mut events_rx) = mpsc::unbounded_channel::<Read>();
    // The reader offers each event to the downlink as soon as it is read whole
    // (EMU-42), whatever the writer is waiting for.
    let reader = {
        let shared = Arc::clone(shared);
        let pool = Arc::clone(pool);
        async move {
            let mut buf: Vec<u8> = Vec::new();
            let mut pending: Vec<u8> = Vec::new();
            let emit = |bytes: Vec<u8>| -> bool {
                let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                let Some((fate, record)) = shared.offer(Direction::Down, attempt, len) else {
                    let _ = events_tx.send(Read::Failed);
                    return false;
                };
                events_tx
                    .send(Read::Event(Event {
                        bytes: Bytes::from(bytes),
                        fate,
                        record,
                    }))
                    .is_ok()
            };
            while let Some(frame) = rbody.frame().await {
                let Ok(frame) = frame else {
                    let _ = events_tx.send(Read::Failed);
                    return;
                };
                let Ok(data) = frame.into_data() else {
                    continue;
                };
                buf.extend_from_slice(&data);
                while let Some(end) = event_end(&buf) {
                    let event: Vec<u8> = buf.drain(..end).collect();
                    pending.extend_from_slice(&event);
                    if only_comments(&event) {
                        continue;
                    }
                    if !emit(std::mem::take(&mut pending)) {
                        return;
                    }
                }
            }
            pending.extend_from_slice(&buf);
            if !pending.is_empty() {
                emit(pending);
            }
            pool.lock().await.push(send);
        }
    };
    let writer = async {
        let (body_tx, body_rx) = mpsc::unbounded_channel::<Result<Frame<Bytes>, std::io::Error>>();
        let body = StreamBody::new(futures_util::stream::unfold(body_rx, |mut rx| async {
            rx.recv().await.map(|item| (item, rx))
        }));
        *head.body_mut() = body.boxed();
        let mut head = Some((head, head_tx));
        let mut lost = false;
        // Abort the client's response: before the head, by closing the
        // connection unanswered; after it, by breaking the body (EMU-44).
        let abort = |head: &mut Option<(Response<Body>, oneshot::Sender<Head>)>| match head.take() {
            Some((_, tx)) => {
                let _ = tx.send(Head::Abort);
            }
            None => {
                let _ = body_tx.send(Err(std::io::Error::other("cut")));
            }
        };
        while let Some(read) = events_rx.recv().await {
            let e = match read {
                Read::Event(e) => e,
                // EMU-45: a failed upstream breaks the client's response
                // after the events before it.
                Read::Failed => {
                    abort(&mut head);
                    return;
                }
            };
            match e.fate.outcome {
                Err(_) => lost = true,
                Ok(at) => {
                    shared.sleep_until(at).await;
                    if lost {
                        // EMU-44: a lost event cuts the response at the next
                        // delivered one, head written or not.
                        abort(&mut head);
                        return;
                    }
                    if let Some((h, tx)) = head.take() {
                        // A client already gone still has its stream carried
                        // (EMU-45); only the hand-over fails.
                        let _ = tx.send(Head::Response(h));
                    }
                    if body_tx.send(Ok(Frame::data(e.bytes))).is_ok() {
                        shared.handed(e.record);
                    }
                }
            }
        }
        if lost {
            // Only the last events were lost: silence until the proxy stops.
            shared.stopped().await;
            drop(body_tx);
            return;
        }
        if let Some((h, tx)) = head.take() {
            // A stream with no event: the head alone.
            let _ = tx.send(Head::Response(h));
        }
        drop(body_tx);
    };
    tokio::join!(reader, writer);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cites: EMU-41
    #[test]
    fn events_end_at_the_first_blank_line_whatever_the_line_endings() {
        assert_eq!(event_end(b"data: a\n\ndata: b"), Some(9));
        assert_eq!(event_end(b"data: a\r\n\r\nx"), Some(11));
        assert_eq!(event_end(b"data: a\r\rx"), Some(9));
        // LF then CRLF: one blank line, ending after its LF.
        assert_eq!(event_end(b"data: a\n\r\ndata: b\n\n"), Some(10));
        // CR then CRLF: the CRLF is the blank line.
        assert_eq!(event_end(b"data: a\r\r\ndata: b\n\n"), Some(10));
        // A CR at the end waits for the next byte.
        assert_eq!(event_end(b"data: a\r"), None);
        assert_eq!(event_end(b"data: a\r\r"), None);
        assert_eq!(event_end(b"data: a\n"), None);
        assert_eq!(event_end(b""), None);
        assert!(only_comments(b": ping\n\n"));
        assert!(!only_comments(b": ping\ndata: x\n\n"));
    }
}
