//! The status-quo baseline: plain HTTP/1.1 (hyper), one keep-alive connection, every
//! request carries the whole conversation as JSON, the response is an SSE stream shaped
//! like a messages API (`message_start`, `content_block_delta`*, `message_stop`).
//!
//! No TLS, no request compression. A stream that dies is retried from scratch on a new
//! connection: the API has no resume, so the server generates the turn again.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use anyhow::{Result, bail};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::{Body, Frame, Incoming};
use hyper::client::conn::http1::SendRequest;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::workload::{TurnStats, Workload};

/// A break is reported just before the dead connection errors, so one or two failures
/// can land after link-up; more than this is not the link.
const MAX_FAILURES_WITH_LINK_UP: u32 = 5;

/// Stands in for a credential of realistic length. Not a key.
const FAKE_KEY: &str = "lab-not-a-key-0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";

#[derive(Serialize, Deserialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    system: &'a str,
    messages: &'a [Message],
}

#[derive(Deserialize)]
struct ReceivedRequest {
    model: String,
    messages: Vec<Message>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    MessageStart { message: MessageMeta },
    ContentBlockDelta { index: u32, delta: TextDelta },
    MessageStop,
}

#[derive(Serialize, Deserialize)]
struct MessageMeta {
    id: String,
    role: String,
    model: String,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TextDelta {
    TextDelta { text: String },
}

fn frame(name: &str, event: &Event) -> Result<Bytes> {
    Ok(Bytes::from(format!(
        "event: {name}\ndata: {}\n\n",
        serde_json::to_string(event)?
    )))
}

// ---------------------------------------------------------------- server

struct ChannelBody(mpsc::Receiver<Bytes>);

impl Body for ChannelBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        self.0
            .poll_recv(cx)
            .map(|chunk| chunk.map(|bytes| Ok(Frame::data(bytes))))
    }
}

pub struct SseServer {
    pub addr: SocketAddr,
    accept: JoinHandle<()>,
}

impl Drop for SseServer {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl SseServer {
    pub async fn start(workload: Arc<Workload>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let accept = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let workload = workload.clone();
                tokio::spawn(async move {
                    let _ = stream.set_nodelay(true);
                    let service = service_fn(move |req| respond(req, workload.clone()));
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Ok(Self { addr, accept })
    }
}

async fn respond(req: Request<Incoming>, workload: Arc<Workload>) -> Result<Response<ChannelBody>> {
    let body = req.into_body().collect().await?.to_bytes();
    let request: ReceivedRequest = serde_json::from_slice(&body)?;
    let turn = (request.messages.len() / 2) as u32;
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        // Ends at the first failed send: the generation dies with its stream.
        let _ = generate(&workload, turn, request.model, tx).await;
    });
    Ok(Response::builder()
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .header("request-id", format!("req_lab_{turn:020}"))
        .body(ChannelBody(rx))?)
}

async fn generate(
    workload: &Workload,
    turn: u32,
    model: String,
    tx: mpsc::Sender<Bytes>,
) -> Result<()> {
    let first = Instant::now() + workload.prefill;
    let start = Event::MessageStart {
        message: MessageMeta {
            id: format!("msg_lab_{turn:020}"),
            role: "assistant".to_owned(),
            model,
        },
    };
    tx.send(frame("message_start", &start)?).await?;
    for (i, token) in workload.tokens(turn).into_iter().enumerate() {
        tokio::time::sleep_until(first + workload.token_interval * i as u32).await;
        let delta = Event::ContentBlockDelta {
            index: 0,
            delta: TextDelta::TextDelta { text: token },
        };
        tx.send(frame("content_block_delta", &delta)?).await?;
    }
    tx.send(frame("message_stop", &Event::MessageStop)?).await?;
    Ok(())
}

// ---------------------------------------------------------------- client

pub struct SseClient {
    relay: SocketAddr,
    link_up: watch::Receiver<bool>,
    sender: Option<SendRequest<Full<Bytes>>>,
    system: String,
    messages: Vec<Message>,
}

impl SseClient {
    pub fn new(relay: SocketAddr, link_up: watch::Receiver<bool>, system: String) -> Self {
        Self {
            relay,
            link_up,
            sender: None,
            system,
            messages: Vec::new(),
        }
    }

    async fn sender(&mut self) -> Result<&mut SendRequest<Full<Bytes>>> {
        let usable = self.sender.as_ref().is_some_and(|s| !s.is_closed());
        if !usable {
            self.link_up.wait_for(|up| *up).await?;
            let stream = TcpStream::connect(self.relay).await?;
            stream.set_nodelay(true)?;
            let (sender, conn) =
                hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
            tokio::spawn(conn);
            self.sender = Some(sender);
        }
        self.sender
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("no connection"))
    }

    pub async fn turn(&mut self, user: &str) -> Result<TurnStats> {
        let start = Instant::now();
        self.messages.push(Message {
            role: "user".to_owned(),
            content: user.to_owned(),
        });
        let body = Bytes::from(serde_json::to_vec(&MessagesRequest {
            model: "lab-model-1",
            max_tokens: 4096,
            stream: true,
            system: &self.system,
            messages: &self.messages,
        })?);

        let mut stats = TurnStats::default();
        let mut first_byte = None;
        let mut failures_with_link_up = 0;
        let output = loop {
            let mut output = String::new();
            match self
                .attempt(body.clone(), start, &mut first_byte, &mut output)
                .await
            {
                Ok(()) => break output,
                Err(e) => {
                    // Expected while the link says it is down. With the link up it is
                    // a bug or a bad response, and retrying it forever would hide it.
                    if *self.link_up.borrow() {
                        eprintln!("sse client: attempt failed with the link up: {e:#}");
                        failures_with_link_up += 1;
                        if failures_with_link_up > MAX_FAILURES_WITH_LINK_UP {
                            return Err(e.context("sse client: giving up"));
                        }
                    }
                    stats.recoveries += 1;
                    stats.discarded += output.len();
                    self.sender = None;
                }
            }
        };

        stats.total = start.elapsed();
        stats.ttft = first_byte.unwrap_or(stats.total);
        self.messages.push(Message {
            role: "assistant".to_owned(),
            content: output.clone(),
        });
        stats.output = output;
        Ok(stats)
    }

    async fn attempt(
        &mut self,
        body: Bytes,
        start: Instant,
        first_byte: &mut Option<std::time::Duration>,
        output: &mut String,
    ) -> Result<()> {
        let request = Request::post("/v1/messages")
            .header("host", "api.lab.invalid")
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .header("x-api-key", FAKE_KEY)
            .header("api-version", "2023-06-01")
            .header("user-agent", "lab-turn-transport/0.0.0")
            .body(Full::new(body))?;
        let response = self.sender().await?.send_request(request).await?;
        if !response.status().is_success() {
            bail!("status {}", response.status());
        }
        let mut body = response.into_body();
        let mut pending = Vec::new();
        let mut stopped = false;
        // Read to the end of the body even after `message_stop`, so the connection
        // stays reusable for the next turn.
        while let Some(chunk) = body.frame().await {
            let Ok(data) = chunk?.into_data() else {
                continue;
            };
            pending.extend_from_slice(&data);
            while let Some(end) = pending.windows(2).position(|w| w == b"\n\n") {
                let raw: Vec<u8> = pending.drain(..end + 2).collect();
                let text = std::str::from_utf8(&raw)?;
                let Some(json) = text.lines().find_map(|l| l.strip_prefix("data: ")) else {
                    continue;
                };
                match serde_json::from_str(json)? {
                    Event::MessageStart { .. } => {}
                    Event::ContentBlockDelta {
                        delta: TextDelta::TextDelta { text },
                        ..
                    } => {
                        first_byte.get_or_insert(start.elapsed());
                        output.push_str(&text);
                    }
                    Event::MessageStop => stopped = true,
                }
            }
        }
        if !stopped {
            bail!("stream ended before message_stop");
        }
        Ok(())
    }
}
