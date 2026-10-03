//! Backends and responses (HAR-20..24): which dialect a backend speaks, one
//! attempt's raw exchange, and its assembly into the provider's own non-streamed
//! response shape, which the frozen mapping of TRC-21 then normalises.

use serde_json::{Value, json};

use crate::HarnessError;
use crate::context::{Dialect, ToolCall};

/// The `acn.backend` value of the mock (CON-26).
pub const MOCK: &str = "mockllm";

/// The mock's identity header (MLM-4).
pub const MOCK_HEADER: &str = "x-acn-mockllm";

/// The prefix of the mock's `system_fingerprint` (MLM-4).
pub const MOCK_FINGERPRINT: &str = "acn-mockllm:";

/// A backend the harness can call (HAR-20).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Mockllm,
    Openai,
    Vllm,
    Sglang,
    Anthropic,
}

impl Backend {
    /// Parse an `acn.backend` value.
    pub fn parse(s: &str) -> Result<Self, HarnessError> {
        Ok(match s {
            MOCK => Self::Mockllm,
            "openai" => Self::Openai,
            "vllm" => Self::Vllm,
            "sglang" => Self::Sglang,
            "anthropic" => Self::Anthropic,
            _ => {
                return Err(HarnessError::Config(format!(
                    "unknown backend `{s}`; one of mockllm, openai, vllm, sglang, anthropic"
                )));
            }
        })
    }

    /// The `acn.backend` value, which is also the provider name of TRC-21.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mockllm => MOCK,
            Self::Openai => "openai",
            Self::Vllm => "vllm",
            Self::Sglang => "sglang",
            Self::Anthropic => "anthropic",
        }
    }

    #[must_use]
    pub fn dialect(self) -> Dialect {
        match self {
            Self::Anthropic => Dialect::Messages,
            _ => Dialect::ChatCompletions,
        }
    }

    /// Whether requests carry `cache_control` breakpoints (HAR-16).
    #[must_use]
    pub fn marks_breakpoints(self) -> bool {
        matches!(self, Self::Mockllm | Self::Anthropic)
    }

    /// The request path under the endpoint.
    #[must_use]
    pub fn path(self) -> &'static str {
        match self.dialect() {
            Dialect::ChatCompletions => "/v1/chat/completions",
            Dialect::Messages => "/v1/messages",
        }
    }
}

/// How an attempt failed before a complete response arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// No response, or a broken one (HAR-24).
    Transport(String),
    /// `opt.request_timeout_ms` passed (HAR-24: `client_abort`).
    Timeout,
}

/// One attempt at one call, as the transport saw it.
#[derive(Debug, Clone, Default)]
pub struct Exchange {
    /// When the request was handed to the transport.
    pub start_ns: i64,
    /// When the attempt ended: the last byte, or the failure.
    pub end_ns: i64,
    pub status: u16,
    /// Header names in lowercase.
    pub headers: Vec<(String, String)>,
    /// A streamed response: each event's `data` payload and its arrival time.
    pub events: Vec<(i64, String)>,
    /// A non-streamed response, or an error body.
    pub body: Vec<u8>,
    /// Response body bytes received.
    pub bytes_down: u64,
    pub failure: Option<Failure>,
}

impl Exchange {
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// Whether the response says it came from the mock (MLM-4).
    #[must_use]
    pub fn from_mock(&self) -> bool {
        if self.header(MOCK_HEADER).is_some() {
            return true;
        }
        let fp = |v: &Value| {
            v.get("system_fingerprint")
                .and_then(Value::as_str)
                .is_some_and(|f| f.starts_with(MOCK_FINGERPRINT))
        };
        serde_json::from_slice::<Value>(&self.body).is_ok_and(|v| fp(&v))
            || self
                .events
                .iter()
                .any(|(_, d)| serde_json::from_str::<Value>(d).is_ok_and(|v| fp(&v)))
    }
}

/// A complete response, assembled.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    /// The provider's non-streamed response shape, for TRC-21.
    pub raw: Value,
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    /// Arrival times of the chunks that carried content or a tool call; for a
    /// non-streamed response, the response's own time.
    pub token_times: Vec<i64>,
}

/// The most content blocks or tool calls one response may carry: a stream's
/// `index` is the provider's, and an absurd one must not allocate without bound.
pub const MAX_INDEX: usize = 1024;

/// Why an exchange is not a complete response.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AssembleError {
    /// The stream ended early or reported an error: a transport error, retried
    /// (HAR-24, MLM-41).
    #[error("cut: {0}")]
    Cut(String),
    /// Not a response of the dialect at all: not retried (ADR-17).
    #[error("malformed: {0}")]
    Malformed(String),
}

fn malformed<T>(m: impl Into<String>) -> Result<T, AssembleError> {
    Err(AssembleError::Malformed(m.into()))
}

/// The end of the first event in `buf`: its length and the delimiter's. Events
/// end with a blank line, `\n\n` or `\r\n\r\n`.
fn event_end(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = buf.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2));
    let crlf = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if b.0 < a.0 { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// Parse server-sent events from a byte buffer: each complete event's `data`
/// payload. Returns the events and the number of bytes consumed.
#[must_use]
pub fn sse_events(buf: &[u8]) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut used = 0;
    while let Some((end, delim)) = event_end(&buf[used..]) {
        let event = &buf[used..used + end];
        used += end + delim;
        let text = String::from_utf8_lossy(event);
        let data: Vec<&str> = text
            .lines()
            .map(|l| l.trim_end_matches('\r'))
            .filter_map(|l| l.strip_prefix("data:"))
            .map(|d| d.strip_prefix(' ').unwrap_or(d))
            .collect();
        if !data.is_empty() {
            out.push(data.join("\n"));
        }
    }
    (out, used)
}

/// Assemble a complete response, or say why the exchange is not one.
pub fn assemble(dialect: Dialect, ex: &Exchange, streamed: bool) -> Result<Reply, AssembleError> {
    if !streamed {
        let raw: Value = serde_json::from_slice(&ex.body)
            .map_err(|e| AssembleError::Malformed(format!("the response is not JSON: {e}")))?;
        let (text, tool_calls) = match dialect {
            Dialect::ChatCompletions => message_of_chat(&raw["choices"][0]["message"])?,
            Dialect::Messages => message_of_blocks(&raw["content"])?,
        };
        return Ok(Reply {
            raw,
            text,
            tool_calls,
            token_times: vec![ex.end_ns],
        });
    }
    match dialect {
        Dialect::ChatCompletions => assemble_chat_stream(ex),
        Dialect::Messages => assemble_messages_stream(ex),
    }
}

/// A tool call needs a non-empty id and name; only missing arguments default.
fn tool_call(
    id: Option<&str>,
    name: Option<&str>,
    arguments: String,
) -> Result<ToolCall, AssembleError> {
    match (id.filter(|s| !s.is_empty()), name.filter(|s| !s.is_empty())) {
        (Some(id), Some(name)) => Ok(ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments,
        }),
        _ => malformed("a tool call has no id or no name"),
    }
}

fn message_of_chat(m: &Value) -> Result<(Option<String>, Vec<ToolCall>), AssembleError> {
    if !m.is_object() {
        return malformed("the response has no `choices[0].message`");
    }
    let text = m.get("content").and_then(Value::as_str).map(str::to_owned);
    let mut calls = Vec::new();
    for c in m
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        calls.push(tool_call(
            c["id"].as_str(),
            c["function"]["name"].as_str(),
            c["function"]["arguments"]
                .as_str()
                .unwrap_or("{}")
                .to_owned(),
        )?);
    }
    Ok((text, calls))
}

fn message_of_blocks(content: &Value) -> Result<(Option<String>, Vec<ToolCall>), AssembleError> {
    let Some(blocks) = content.as_array() else {
        return malformed("the response has no `content` array");
    };
    let mut text: Option<String> = None;
    let mut calls = Vec::new();
    for b in blocks {
        match b["type"].as_str() {
            Some("text") => text
                .get_or_insert_default()
                .push_str(b["text"].as_str().unwrap_or_default()),
            Some("tool_use") => calls.push(tool_call(
                b["id"].as_str(),
                b["name"].as_str(),
                b.get("input").map_or_else(|| "{}".into(), Value::to_string),
            )?),
            _ => {}
        }
    }
    Ok((text, calls))
}

/// A provider's block or call index, bounded (MAX_INDEX).
fn index_of(v: &Value) -> Result<usize, AssembleError> {
    let i = v.as_u64().unwrap_or(0);
    match usize::try_from(i) {
        Ok(i) if i < MAX_INDEX => Ok(i),
        _ => malformed(format!("index {i} exceeds {MAX_INDEX}")),
    }
}

/// Chat Completions chunks, accumulated per TRC-21: deltas per choice, `usage`
/// from the chunk that carries it. Complete only with a `finish_reason` and
/// `[DONE]` (MLM-41: a cut stream has neither).
fn assemble_chat_stream(ex: &Exchange) -> Result<Reply, AssembleError> {
    let mut text: Option<String> = None;
    let mut calls: Vec<(String, String, String)> = Vec::new();
    let mut finish = Value::Null;
    let mut usage = Value::Null;
    let mut fingerprint = Value::Null;
    let mut done = false;
    let mut times = Vec::new();
    for (at, data) in &ex.events {
        if data.trim() == "[DONE]" {
            done = true;
            continue;
        }
        let v: Value = serde_json::from_str(data)
            .map_err(|e| AssembleError::Malformed(format!("a stream chunk is not JSON: {e}")))?;
        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
            usage = u.clone();
        }
        if let Some(f) = v.get("system_fingerprint").filter(|f| !f.is_null()) {
            fingerprint = f.clone();
        }
        let Some(choice) = v["choices"].get(0) else {
            continue;
        };
        let delta = &choice["delta"];
        let mut carried = false;
        if let Some(t) = delta.get("content").and_then(Value::as_str) {
            text.get_or_insert_default().push_str(t);
            carried |= !t.is_empty();
        }
        for tc in delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let i = index_of(&tc["index"])?;
            while calls.len() <= i {
                calls.push(Default::default());
            }
            if let Some(id) = tc["id"].as_str() {
                calls[i].0.push_str(id);
            }
            if let Some(n) = tc["function"]["name"].as_str() {
                calls[i].1.push_str(n);
            }
            if let Some(a) = tc["function"]["arguments"].as_str() {
                calls[i].2.push_str(a);
            }
            carried = true;
        }
        if carried {
            times.push(*at);
        }
        if !choice["finish_reason"].is_null() {
            finish = choice["finish_reason"].clone();
        }
    }
    if finish.is_null() || !done {
        return Err(AssembleError::Cut(
            "the stream ended before its final chunk".into(),
        ));
    }
    let tool_calls: Vec<ToolCall> = calls
        .into_iter()
        .map(|(id, name, arguments)| {
            let arguments = if arguments.is_empty() {
                "{}".into()
            } else {
                arguments
            };
            tool_call(Some(&id), Some(&name), arguments)
        })
        .collect::<Result<_, _>>()?;
    let mut message = json!({ "role": "assistant", "content": text });
    if !tool_calls.is_empty() {
        message["tool_calls"] = tool_calls
            .iter()
            .map(|c| json!({ "id": c.id, "type": "function", "function": { "name": c.name, "arguments": c.arguments } }))
            .collect();
    }
    Ok(Reply {
        raw: json!({
            "choices": [{ "index": 0, "message": message, "finish_reason": finish }],
            "usage": usage,
            "system_fingerprint": fingerprint,
        }),
        text,
        tool_calls,
        token_times: times,
    })
}

/// Anthropic Messages events assembled into a `message` object: content blocks
/// from their deltas, `usage` merged from `message_start` and `message_delta`.
/// Complete only with `message_stop`.
fn assemble_messages_stream(ex: &Exchange) -> Result<Reply, AssembleError> {
    let mut blocks: Vec<Value> = Vec::new();
    let mut partial_json: Vec<String> = Vec::new();
    let mut usage = serde_json::Map::new();
    let mut stop_reason = Value::Null;
    let mut stopped = false;
    let mut times = Vec::new();
    for (at, data) in &ex.events {
        let v: Value = serde_json::from_str(data)
            .map_err(|e| AssembleError::Malformed(format!("a stream event is not JSON: {e}")))?;
        let index = index_of(&v["index"])?;
        match v["type"].as_str() {
            Some("message_start") => {
                if let Some(u) = v["message"]["usage"].as_object() {
                    usage.extend(u.clone());
                }
            }
            Some("content_block_start") => {
                while blocks.len() <= index {
                    blocks.push(Value::Null);
                    partial_json.push(String::new());
                }
                blocks[index] = v["content_block"].clone();
            }
            Some("content_block_delta") => {
                if index >= blocks.len() {
                    return malformed("a delta for a block that never started");
                }
                let d = &v["delta"];
                match d["type"].as_str() {
                    Some("text_delta") => {
                        let t = d["text"].as_str().unwrap_or_default();
                        let cur = blocks[index]["text"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned();
                        blocks[index]["text"] = json!(cur + t);
                    }
                    Some("input_json_delta") => {
                        partial_json[index]
                            .push_str(d["partial_json"].as_str().unwrap_or_default());
                    }
                    _ => {}
                }
                times.push(*at);
            }
            Some("message_delta") => {
                if !v["delta"]["stop_reason"].is_null() {
                    stop_reason = v["delta"]["stop_reason"].clone();
                }
                if let Some(u) = v["usage"].as_object() {
                    usage.extend(u.clone());
                }
            }
            Some("message_stop") => stopped = true,
            // The provider's own words stay out of the trace (HAR-34): the type only.
            Some("error") => {
                return Err(AssembleError::Cut(format!(
                    "the stream reported an error of type `{}`",
                    v["error"]["type"].as_str().unwrap_or("unknown")
                )));
            }
            _ => {}
        }
    }
    if !stopped {
        return Err(AssembleError::Cut(
            "the stream ended before message_stop".into(),
        ));
    }
    for (b, p) in blocks.iter_mut().zip(&partial_json) {
        if b["type"] == "tool_use" {
            b["input"] = if p.is_empty() {
                json!({})
            } else {
                serde_json::from_str(p)
                    .map_err(|e| AssembleError::Malformed(format!("tool input is not JSON: {e}")))?
            };
        }
    }
    let content = Value::Array(blocks);
    let (text, tool_calls) = message_of_blocks(&content)?;
    Ok(Reply {
        raw: json!({ "type": "message", "role": "assistant", "content": content,
            "stop_reason": stop_reason, "usage": Value::Object(usage) }),
        text,
        tool_calls,
        token_times: times,
    })
}
