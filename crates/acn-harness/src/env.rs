//! Where calls go and how time passes. An [`Env`] makes one attempt at a call and
//! waits on the run's clock (HAR-40). [`SimEnv`] is `sim`: the in-process mock on a
//! [`SimClock`], driven by a small deterministic scheduler — every lineage's
//! pending wait or call is registered with it, and calls due at the same instant
//! go to the mock together through `Mock::handle_batch`, so their order is MLM-7's
//! and never a scheduler's (HAR-41). [`LiveEnv`] is `live`: HTTP on the wall clock.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use acn_emu::clock::{Clock, SimClock, WallClock};
use acn_mockllm::{Mock, Outcome};

use crate::HarnessError;
use crate::wire::{Exchange, Failure, sse_events};

/// One attempt at a call, and the clock (HAR-24, HAR-33, HAR-40).
pub trait Env {
    /// Nanoseconds on the run's clock.
    fn now(&self) -> i64;
    /// Resolve once the run's clock reads at least `t_ns`.
    fn sleep_until(&self, t_ns: i64) -> impl Future<Output = ()>;
    /// POST `body` to `path` once; give up after `timeout_ns`.
    fn exchange(
        &self,
        path: &'static str,
        body: Vec<u8>,
        stream: bool,
        timeout_ns: i64,
    ) -> impl Future<Output = Exchange>;
}

// ---- sim -------------------------------------------------------------------

enum Op {
    Sleep(i64),
    Call(Vec<u8>),
}

enum Done {
    Woke,
    Called(Box<Outcome>),
}

struct SimState {
    clock: SimClock,
    mock: Mock,
    tenant: String,
    next: u64,
    ops: BTreeMap<u64, Op>,
    done: BTreeMap<u64, Done>,
}

/// The `sim` environment of one replicate: its mock, its tenant and its clock.
#[derive(Clone)]
pub struct SimEnv(Rc<RefCell<SimState>>);

impl std::fmt::Debug for SimEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SimEnv")
    }
}

/// A wait or call registered with the scheduler.
struct OpFuture {
    env: SimEnv,
    op: Option<Op>,
    id: Option<u64>,
}

impl Future for OpFuture {
    type Output = Done;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Done> {
        let this = &mut *self;
        let mut st = this.env.0.borrow_mut();
        match this.id {
            None => {
                let id = st.next;
                st.next += 1;
                if let Some(op) = this.op.take() {
                    st.ops.insert(id, op);
                }
                this.id = Some(id);
                Poll::Pending
            }
            Some(id) => st.done.remove(&id).map_or(Poll::Pending, Poll::Ready),
        }
    }
}

impl SimEnv {
    /// A replicate's environment: a fresh clock at 0, its own mock, its tenant
    /// (HAR-42: the isolation marker).
    #[must_use]
    pub fn new(mock: Mock, tenant: String) -> Self {
        Self(Rc::new(RefCell::new(SimState {
            clock: SimClock::new(),
            mock,
            tenant,
            next: 0,
            ops: BTreeMap::new(),
            done: BTreeMap::new(),
        })))
    }

    /// The mock's cache sizes, for tests.
    #[must_use]
    pub fn cache_sizes(&self) -> (usize, usize) {
        self.0.borrow().mock.cache_sizes()
    }

    /// Run `fut` to completion on virtual time. Each round wakes the waits that
    /// are due, then hands every call registered at this instant to the mock as
    /// one batch, and only when neither is left moves the clock to the next wait.
    pub fn drive<F: Future>(&self, fut: F) -> Result<F::Output, HarnessError> {
        let mut fut = std::pin::pin!(fut);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return Ok(v);
            }
            let mut st = self.0.borrow_mut();
            let now = st.clock.now_ns();
            let due: Vec<u64> = st
                .ops
                .iter()
                .filter(|(_, o)| matches!(o, Op::Sleep(t) if *t <= now))
                .map(|(id, _)| *id)
                .collect();
            if !due.is_empty() {
                for id in due {
                    st.ops.remove(&id);
                    st.done.insert(id, Done::Woke);
                }
                continue;
            }
            let calls: Vec<(u64, Vec<u8>)> = st
                .ops
                .iter()
                .filter_map(|(id, o)| match o {
                    Op::Call(b) => Some((*id, b.clone())),
                    Op::Sleep(_) => None,
                })
                .collect();
            if !calls.is_empty() {
                let tenant = st.tenant.clone();
                let requests: Vec<(Vec<u8>, String, i64)> = calls
                    .iter()
                    .map(|(_, b)| (b.clone(), tenant.clone(), now))
                    .collect();
                let outcomes = st.mock.handle_batch(&requests);
                for ((id, _), o) in calls.iter().zip(outcomes) {
                    st.ops.remove(id);
                    st.done.insert(*id, Done::Called(Box::new(o)));
                }
                continue;
            }
            let next = st
                .ops
                .values()
                .filter_map(|o| match o {
                    Op::Sleep(t) => Some(*t),
                    Op::Call(_) => None,
                })
                .min();
            match next {
                Some(t) => st.clock.advance_to(t),
                None => {
                    return Err(HarnessError::Internal(
                        "the sim scheduler has nothing to run and the run has not finished".into(),
                    ));
                }
            }
        }
    }

    fn op(&self, op: Op) -> OpFuture {
        OpFuture {
            env: self.clone(),
            op: Some(op),
            id: None,
        }
    }
}

fn sse_len(data: &str) -> u64 {
    u64::try_from("data: ".len() + data.len() + 2).unwrap_or(u64::MAX)
}

impl Env for SimEnv {
    fn now(&self) -> i64 {
        self.0.borrow().clock.now_ns()
    }

    async fn sleep_until(&self, t_ns: i64) {
        if t_ns > self.now() {
            self.op(Op::Sleep(t_ns)).await;
        }
    }

    async fn exchange(
        &self,
        _path: &'static str,
        body: Vec<u8>,
        stream: bool,
        timeout_ns: i64,
    ) -> Exchange {
        let start = self.now();
        let Done::Called(o) = self.op(Op::Call(body)).await else {
            return Exchange {
                start_ns: start,
                end_ns: start,
                failure: Some(Failure::Transport(
                    "internal: a call was woken as a wait".into(),
                )),
                ..Exchange::default()
            };
        };
        let o = *o;
        let streamed = stream && o.status == 200;
        let end = if o.status != 200 {
            start
        } else if streamed {
            o.chunks.last().map_or(start, |c| c.at_ns)
        } else {
            o.respond_at_ns
        };
        let deadline = start.saturating_add(timeout_ns);
        if end > deadline {
            self.sleep_until(deadline).await;
            return Exchange {
                start_ns: start,
                end_ns: deadline,
                status: o.status,
                failure: Some(Failure::Timeout),
                ..Exchange::default()
            };
        }
        self.sleep_until(end).await;
        let (events, body, bytes_down) = if streamed {
            let events: Vec<(i64, String)> =
                o.chunks.iter().map(|c| (c.at_ns, c.data.clone())).collect();
            let n = events.iter().map(|(_, d)| sse_len(d)).sum();
            (events, Vec::new(), n)
        } else {
            let n = u64::try_from(o.body.len()).unwrap_or(u64::MAX);
            (Vec::new(), o.body, n)
        };
        Exchange {
            start_ns: start,
            end_ns: end,
            status: o.status,
            headers: o.headers,
            events,
            body,
            bytes_down,
            failure: None,
        }
    }
}

// ---- live ------------------------------------------------------------------

/// The `live` environment: one endpoint over HTTP, on the wall clock.
#[derive(Clone)]
pub struct LiveEnv {
    client: reqwest::Client,
    clock: Arc<WallClock>,
    endpoint: String,
    headers: Vec<(String, String)>,
}

impl std::fmt::Debug for LiveEnv {
    /// Never the headers: they carry credentials (HAR-22).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveEnv")
            .field("endpoint", &self.endpoint)
            .field(
                "headers",
                &format_args!("<{} redacted>", self.headers.len()),
            )
            .finish_non_exhaustive()
    }
}

impl LiveEnv {
    /// An environment that sends `headers` (credentials, the tenant) with every
    /// request to `endpoint` (a base URL, without `/v1/...`).
    pub fn new(
        clock: Arc<WallClock>,
        endpoint: &str,
        headers: Vec<(String, String)>,
    ) -> Result<Self, HarnessError> {
        // No proxy from the environment: `HTTP_PROXY` and friends would change
        // what a run receives without changing its identity, and would receive
        // its credentials (CON-29, HAR-22, HAR-25).
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| HarnessError::Config(format!("http client: {e}")))?;
        Ok(Self {
            client,
            clock,
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            headers,
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self
            .client
            .request(method, format!("{}{path}", self.endpoint));
        for (k, v) in &self.headers {
            r = r.header(k, v);
        }
        r
    }

    /// HAR-23: whether `GET /v1/models` shows the mock's marker.
    pub async fn probe_is_mock(&self) -> Result<bool, HarnessError> {
        let resp = self
            .request(reqwest::Method::GET, "/v1/models")
            .send()
            .await
            .map_err(|e| HarnessError::Backend(format!("GET /v1/models: {}", cause(e))))?;
        Ok(resp.headers().contains_key(crate::wire::MOCK_HEADER))
    }

    async fn attempt(
        &self,
        path: &'static str,
        body: Vec<u8>,
        stream: bool,
        start: i64,
    ) -> Exchange {
        use futures_util::StreamExt as _;
        let mut ex = Exchange {
            start_ns: start,
            ..Exchange::default()
        };
        let resp = match self
            .request(reqwest::Method::POST, path)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                ex.end_ns = self.clock.now_ns();
                ex.failure = Some(Failure::Transport(cause(e)));
                return ex;
            }
        };
        ex.status = resp.status().as_u16();
        ex.headers = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|v| (k.as_str().to_owned(), v.to_owned()))
            })
            .collect();
        if stream && ex.status == 200 {
            let mut s = resp.bytes_stream();
            let mut buf = Vec::new();
            while let Some(chunk) = s.next().await {
                match chunk {
                    Ok(bytes) => {
                        let at = self.clock.now_ns();
                        ex.bytes_down += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                        buf.extend_from_slice(&bytes);
                        let (events, used) = sse_events(&buf);
                        buf.drain(..used);
                        ex.events.extend(events.into_iter().map(|d| (at, d)));
                    }
                    Err(e) => {
                        ex.failure = Some(Failure::Transport(cause(e)));
                        break;
                    }
                }
            }
        } else {
            match resp.bytes().await {
                Ok(b) => {
                    ex.bytes_down = u64::try_from(b.len()).unwrap_or(u64::MAX);
                    ex.body = b.to_vec();
                }
                Err(e) => ex.failure = Some(Failure::Transport(cause(e))),
            }
        }
        ex.end_ns = self.clock.now_ns();
        ex
    }
}

/// A transport error with its causes (connection refused, DNS, TLS), without
/// the URL: reqwest's own text stops at "error sending request".
fn cause(e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut text = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        text.push_str(": ");
        text.push_str(&s.to_string());
        source = s.source();
    }
    text
}

impl Env for LiveEnv {
    fn now(&self) -> i64 {
        self.clock.now_ns()
    }

    async fn sleep_until(&self, t_ns: i64) {
        self.clock.sleep_until(t_ns).await;
    }

    async fn exchange(
        &self,
        path: &'static str,
        body: Vec<u8>,
        stream: bool,
        timeout_ns: i64,
    ) -> Exchange {
        let start = self.now();
        let deadline = start.saturating_add(timeout_ns);
        tokio::select! {
            ex = self.attempt(path, body, stream, start) => ex,
            () = self.clock.sleep_until(deadline) => Exchange {
                start_ns: start,
                end_ns: self.now(),
                failure: Some(Failure::Timeout),
                ..Exchange::default()
            },
        }
    }
}

/// Run every future to completion, polling all of them each time, in order. The
/// sim scheduler's no-op waker never re-polls a single child, so a combinator that
/// waits for wake-ups would stall; this one does not, and it is deterministic.
pub async fn join_all<F: Future>(futs: Vec<F>) -> Vec<F::Output> {
    let mut futs: Vec<Pin<Box<F>>> = futs.into_iter().map(Box::pin).collect();
    let mut out: Vec<Option<F::Output>> = futs.iter().map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (f, slot) in futs.iter_mut().zip(out.iter_mut()) {
            if slot.is_none() {
                match f.as_mut().poll(cx) {
                    Poll::Ready(v) => *slot = Some(v),
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await;
    out.into_iter().flatten().collect()
}
