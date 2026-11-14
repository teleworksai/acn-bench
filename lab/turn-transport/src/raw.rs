//! The framing ablation: the same TCP path and the same whole-context request as the
//! `sse` baseline, without HTTP, JSON or SSE. A request is `len u32 le | records`, the
//! canonical records the QUIC arms send; the reply is the output as raw bytes, ended by
//! one NUL. With `coalesce`, the server writes at most once per interval, the first
//! token at once if `first_now` is set.
//!
//! No TLS, like the baseline. A connection that dies is retried from scratch: nothing
//! resumes, so the server generates the turn again. Read against `sse`, it shows what
//! the framing costs; read against `quic`, what the transport costs.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::workload::{ASSISTANT, Coalesce, SYSTEM, TurnStats, USER, Workload, record};

/// As in the baseline: a failure or two can land after link-up; more is not the link.
const MAX_FAILURES_WITH_LINK_UP: u32 = 5;
/// Ends a reply. The workload's text never holds one.
const END: u8 = 0;
/// A request larger than this is a framing error, not a conversation.
const MAX_REQUEST: usize = 64 << 20;

pub struct RawServer {
    pub addr: SocketAddr,
    accept: JoinHandle<()>,
}

impl Drop for RawServer {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl RawServer {
    pub async fn start(workload: Arc<Workload>, coalesce: Option<Coalesce>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let accept = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let workload = workload.clone();
                tokio::spawn(async move {
                    let _ = stream.set_nodelay(true);
                    // Ends at the first failed read or write: the generation dies
                    // with its connection.
                    let _ = serve(stream, &workload, coalesce).await;
                });
            }
        });
        Ok(Self { addr, accept })
    }
}

/// How many user records a context holds: the turn being asked for, counted from one.
fn user_records(mut context: &[u8]) -> Result<u32> {
    let mut users = 0;
    while !context.is_empty() {
        let (Some(&role), Some(len)) = (context.first(), context.get(1..5)) else {
            bail!("truncated record header");
        };
        let len = u32::from_le_bytes(len.try_into()?) as usize;
        let Some(rest) = context.get(5 + len..) else {
            bail!("truncated record body");
        };
        users += u32::from(role == USER);
        context = rest;
    }
    Ok(users)
}

async fn serve(
    mut stream: TcpStream,
    workload: &Workload,
    coalesce: Option<Coalesce>,
) -> Result<()> {
    loop {
        let len = stream.read_u32_le().await? as usize;
        if len > MAX_REQUEST {
            bail!("request of {len} bytes");
        }
        let mut context = vec![0; len];
        stream.read_exact(&mut context).await?;
        let Some(turn) = user_records(&context)?.checked_sub(1) else {
            bail!("a request with no user record");
        };
        let first = Instant::now() + workload.prefill;
        let mut held = Vec::new();
        let mut flush_at = None;
        for (i, token) in workload.tokens(turn).into_iter().enumerate() {
            let due = first + workload.token_interval * i as u32;
            // Tokens held back are written when their interval ends, even if the
            // next token is not due yet.
            if let Some(at) = flush_at
                && at <= due
            {
                tokio::time::sleep_until(at).await;
                stream.write_all(&held).await?;
                held.clear();
                flush_at = None;
            }
            tokio::time::sleep_until(due).await;
            match coalesce {
                None => stream.write_all(token.as_bytes()).await?,
                Some(c) if c.first_now && i == 0 => stream.write_all(token.as_bytes()).await?,
                Some(c) => {
                    held.extend_from_slice(token.as_bytes());
                    flush_at.get_or_insert(due + c.interval);
                }
            }
        }
        if let Some(at) = flush_at {
            tokio::time::sleep_until(at).await;
        }
        held.push(END);
        stream.write_all(&held).await?;
    }
}

// ---------------------------------------------------------------- client

pub struct RawClient {
    relay: SocketAddr,
    link_up: watch::Receiver<bool>,
    stream: Option<TcpStream>,
    /// The conversation so far, as canonical records.
    context: Vec<u8>,
}

impl RawClient {
    pub fn new(relay: SocketAddr, link_up: watch::Receiver<bool>, system: &str) -> Self {
        Self {
            relay,
            link_up,
            stream: None,
            context: record(SYSTEM, system.as_bytes()),
        }
    }

    pub async fn turn(&mut self, user: &str) -> Result<TurnStats> {
        let start = Instant::now();
        self.context.extend(record(USER, user.as_bytes()));
        let mut stats = TurnStats::default();
        let mut first_byte = None;
        let mut failures_with_link_up = 0;
        let output = loop {
            let mut output = Vec::new();
            match self.attempt(start, &mut first_byte, &mut output).await {
                Ok(()) => break output,
                Err(e) => {
                    if *self.link_up.borrow() {
                        eprintln!("raw client: attempt failed with the link up: {e:#}");
                        failures_with_link_up += 1;
                        if failures_with_link_up > MAX_FAILURES_WITH_LINK_UP {
                            return Err(e.context("raw client: giving up"));
                        }
                    }
                    stats.recoveries += 1;
                    stats.discarded += output.len();
                    self.stream = None;
                }
            }
        };
        stats.total = start.elapsed();
        stats.ttft = first_byte.unwrap_or(stats.total);
        self.context.extend(record(ASSISTANT, &output));
        stats.output = String::from_utf8(output)?;
        Ok(stats)
    }

    async fn attempt(
        &mut self,
        start: Instant,
        first_byte: &mut Option<Duration>,
        output: &mut Vec<u8>,
    ) -> Result<()> {
        if self.stream.is_none() {
            self.link_up.wait_for(|up| *up).await?;
            let stream = TcpStream::connect(self.relay).await?;
            stream.set_nodelay(true)?;
            self.stream = Some(stream);
        }
        let Some(stream) = self.stream.as_mut() else {
            bail!("no connection");
        };
        // One write, header and context together, as the baseline sends one request.
        let mut request = Vec::with_capacity(4 + self.context.len());
        request.extend_from_slice(&(self.context.len() as u32).to_le_bytes());
        request.extend_from_slice(&self.context);
        stream.write_all(&request).await?;
        let mut buf = [0; 4096];
        loop {
            let n = stream.read(&mut buf).await?;
            if n == 0 {
                bail!("connection closed before the end of the reply");
            }
            let (data, ended) = match buf[..n].split_last() {
                Some((&END, data)) => (data, true),
                _ => (&buf[..n], false),
            };
            if data.contains(&END) {
                bail!("bytes after the end of the reply");
            }
            if !data.is_empty() {
                first_byte.get_or_insert(start.elapsed());
                output.extend_from_slice(data);
            }
            if ended {
                return Ok(());
            }
        }
    }
}
