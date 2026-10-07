//! What the harness sends (HAR-21): a dialect-neutral context — system prompt,
//! tool definitions, messages — and its encoding as an OpenAI Chat Completions or
//! an Anthropic Messages request, with the breakpoints of HAR-16. Bodies are
//! `serde_json` objects, whose keys are sorted, so a body's bytes are a function
//! of the context, the knob map and the workload alone.

use serde_json::{Map, Value, json};

use crate::knobs::Placement;

/// The wire format of a backend (SPEC 040 §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    ChatCompletions,
    Messages,
}

/// A tool call the model asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// The arguments as a JSON string.
    pub arguments: String,
}

/// One message of a context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    User {
        text: String,
    },
    Assistant {
        text: Option<String>,
        tool_calls: Vec<ToolCall>,
    },
    ToolResult {
        call_id: String,
        /// The tool that produced it.
        tool: String,
        /// The session-wide ordinal of a main-lineage result (HAR-13); `None` for a
        /// sub-agent's.
        ordinal: Option<u64>,
        content: String,
    },
}

impl Msg {
    /// The dialect-neutral JSON of TRC-12's canonical byte string.
    fn canonical(&self) -> Value {
        match self {
            Self::User { text } => json!({ "role": "user", "content": text }),
            Self::Assistant { text, tool_calls } => json!({
                "role": "assistant", "content": text,
                "tool_calls": tool_calls.iter().map(|c| json!({ "id": c.id, "name": c.name, "arguments": c.arguments })).collect::<Vec<_>>(),
            }),
            Self::ToolResult {
                call_id, content, ..
            } => json!({ "role": "tool", "tool_call_id": call_id, "content": content }),
        }
    }
}

/// A tool as presented to the model.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// What a backend's requests look like: its dialect, and whether they carry
/// breakpoints (HAR-16) and an allowed-tools restriction (HAR-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoding {
    pub dialect: Dialect,
    pub marks_breakpoints: bool,
    pub restricts_tools: bool,
}

impl Encoding {
    /// `dialect` with neither breakpoints nor a restriction.
    #[must_use]
    pub fn plain(dialect: Dialect) -> Self {
        Self {
            dialect,
            marks_breakpoints: false,
            restricts_tools: false,
        }
    }
}

/// What a call allows the model to call (HAR-4, HAR-14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolChoice {
    /// No tool call: a compaction call, or a forked child with no tools.
    Forbid,
    /// Only these tools, by name, in this order: a forked child's own.
    Allowed(Vec<String>),
    /// Exactly this tool: the generator's choice of a drawn class (SPEC 050
    /// GEN-11), encoded as that requirement writes it.
    Only(String),
}

/// Everything one call sends, before encoding.
#[derive(Debug, Clone, PartialEq)]
pub struct Context {
    pub system: String,
    pub tools: Vec<ToolDef>,
    pub messages: Vec<Msg>,
    /// `None` leaves the choice to the model.
    pub tool_choice: Option<ToolChoice>,
}

/// The sampling parameters of one call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sampling {
    pub max_tokens: u64,
    pub temperature: f64,
    pub stream: bool,
}

fn ephemeral() -> Value {
    json!({ "type": "ephemeral" })
}

impl Context {
    /// TRC-12's canonical byte string: the system prompt, the tool definitions and
    /// the messages, in that order, each serialised by `serde_json`. The
    /// `bytes_scaled` counting method and the `window_full` estimate read it.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Value::String(self.system.clone()).to_string();
        for t in &self.tools {
            out.push_str(
                &json!({ "name": t.name, "description": t.description, "parameters": t.parameters })
                    .to_string(),
            );
        }
        for m in &self.messages {
            out.push_str(&m.canonical().to_string());
        }
        out.into_bytes()
    }

    /// The request body in `enc`'s dialect, marking breakpoints by `placement`
    /// where the backend takes them (HAR-16). A tool choice is written only when
    /// the context has tools: `Forbid` always, `Allowed` only where the backend
    /// takes an allowed-tools restriction (HAR-4, HAR-14).
    #[must_use]
    pub fn encode(
        &self,
        enc: Encoding,
        model: &str,
        sampling: Sampling,
        placement: Placement,
    ) -> Value {
        let Encoding {
            dialect,
            marks_breakpoints: mark,
            restricts_tools: subset,
        } = enc;
        let placement = if mark { placement } else { Placement::None };
        let mut body = match dialect {
            Dialect::ChatCompletions => self.chat_completions(model, sampling, placement),
            Dialect::Messages => self.messages_api(model, sampling, placement),
        };
        if let (Some(choice), false) = (&self.tool_choice, self.tools.is_empty())
            && let Some(map) = body.as_object_mut()
        {
            let value = match (choice, dialect) {
                (ToolChoice::Forbid, Dialect::ChatCompletions) => Some(json!("none")),
                (ToolChoice::Forbid, Dialect::Messages) => Some(json!({ "type": "none" })),
                (ToolChoice::Allowed(names), Dialect::ChatCompletions) if subset => {
                    let tools: Vec<Value> = names
                        .iter()
                        .map(|n| json!({ "type": "function", "function": { "name": n } }))
                        .collect();
                    Some(json!({ "type": "allowed_tools",
                        "allowed_tools": { "mode": "auto", "tools": tools } }))
                }
                // No subset restriction on this backend: the instruction line
                // carries it (HAR-14).
                (ToolChoice::Allowed(_), _) => None,
                (ToolChoice::Only(name), Dialect::ChatCompletions) => Some(json!({
                    "type": "allowed_tools",
                    "allowed_tools": {
                        "tools": [{ "type": "function", "function": { "name": name } }]
                    }
                })),
                (ToolChoice::Only(name), Dialect::Messages) => {
                    Some(json!({ "type": "tool", "name": name }))
                }
            };
            if let Some(v) = value {
                map.insert("tool_choice".into(), v);
            }
        }
        body
    }

    fn chat_completions(&self, model: &str, s: Sampling, placement: Placement) -> Value {
        // The system prompt is always one text part, so that marking it changes
        // nothing but the `cache_control` member (MLM-10 strips it; ADR-17).
        let mut system = json!({ "type": "text", "text": self.system });
        if placement != Placement::None {
            system["cache_control"] = ephemeral();
        }
        let mut messages = Vec::with_capacity(self.messages.len() + 1);
        messages.push(json!({ "role": "system", "content": [system] }));
        for m in &self.messages {
            messages.push(match m {
                Msg::User { text } => json!({ "role": "user", "content": text }),
                Msg::Assistant { text, tool_calls } => {
                    // `null` content only beside tool calls: providers refuse an
                    // assistant message with neither (ADR-17).
                    let content = match text {
                        Some(t) => json!(t),
                        None if tool_calls.is_empty() => json!(""),
                        None => Value::Null,
                    };
                    let mut a = json!({ "role": "assistant", "content": content });
                    if !tool_calls.is_empty() {
                        a["tool_calls"] = tool_calls
                            .iter()
                            .map(|c| {
                                json!({ "id": c.id, "type": "function",
                                    "function": { "name": c.name, "arguments": c.arguments } })
                            })
                            .collect();
                    }
                    a
                }
                Msg::ToolResult {
                    call_id, content, ..
                } => json!({ "role": "tool", "tool_call_id": call_id, "content": content }),
            });
        }
        // A string content is the message's one content block; the breakpoint is
        // carried on the message, so the bytes stay those of the unmarked request
        // and the next call's prefix still matches (MLM-21; ADR-17).
        if placement == Placement::RollingTail
            && let Some(last) = messages.last_mut()
            && last["role"] != "system"
        {
            last["cache_control"] = ephemeral();
        }
        let mut tools: Vec<Value> = self
            .tools
            .iter()
            .map(|t| {
                json!({ "type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": t.parameters } })
            })
            .collect();
        if placement == Placement::SystemAndTools
            && let Some(last) = tools.last_mut()
        {
            last["cache_control"] = ephemeral();
        }
        let mut body = Map::new();
        body.insert("model".into(), json!(model));
        body.insert("messages".into(), Value::Array(messages));
        if !tools.is_empty() {
            body.insert("tools".into(), Value::Array(tools));
        }
        body.insert("max_completion_tokens".into(), json!(s.max_tokens));
        body.insert("temperature".into(), json!(s.temperature));
        body.insert("stream".into(), json!(s.stream));
        if s.stream {
            body.insert("stream_options".into(), json!({ "include_usage": true }));
        }
        Value::Object(body)
    }

    fn messages_api(&self, model: &str, s: Sampling, placement: Placement) -> Value {
        // Consecutive messages of one role are merged: the Messages API alternates.
        let mut out: Vec<(String, Vec<Value>)> = Vec::new();
        for m in &self.messages {
            let (role, blocks) = match m {
                Msg::User { text } => ("user", vec![json!({ "type": "text", "text": text })]),
                Msg::Assistant { text, tool_calls } => {
                    let mut b = Vec::new();
                    if let Some(t) = text.as_deref().filter(|t| !t.is_empty()) {
                        b.push(json!({ "type": "text", "text": t }));
                    }
                    for c in tool_calls {
                        let input: Value =
                            serde_json::from_str(&c.arguments).unwrap_or_else(|_| json!({}));
                        b.push(json!({ "type": "tool_use", "id": c.id, "name": c.name, "input": input }));
                    }
                    // An empty reply sends nothing: the Messages API refuses
                    // empty text blocks, and the user turns around it merge.
                    if b.is_empty() {
                        continue;
                    }
                    ("assistant", b)
                }
                Msg::ToolResult {
                    call_id, content, ..
                } => (
                    "user",
                    vec![
                        json!({ "type": "tool_result", "tool_use_id": call_id, "content": content }),
                    ],
                ),
            };
            match out.last_mut() {
                Some((r, b)) if r == role => b.extend(blocks),
                _ => out.push((role.to_owned(), blocks)),
            }
        }
        if placement == Placement::RollingTail
            && let Some((_, blocks)) = out.last_mut()
            && let Some(last) = blocks.last_mut()
        {
            last["cache_control"] = ephemeral();
        }
        let messages: Vec<Value> = out
            .into_iter()
            .map(|(role, content)| json!({ "role": role, "content": content }))
            .collect();
        let mut system = json!({ "type": "text", "text": self.system });
        if placement != Placement::None {
            system["cache_control"] = ephemeral();
        }
        let mut tools: Vec<Value> = self
            .tools
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.parameters }))
            .collect();
        if placement == Placement::SystemAndTools
            && let Some(last) = tools.last_mut()
        {
            last["cache_control"] = ephemeral();
        }
        let mut body = Map::new();
        body.insert("model".into(), json!(model));
        body.insert("system".into(), json!([system]));
        body.insert("messages".into(), Value::Array(messages));
        if !tools.is_empty() {
            body.insert("tools".into(), Value::Array(tools));
        }
        body.insert("max_tokens".into(), json!(s.max_tokens));
        body.insert("temperature".into(), json!(s.temperature));
        body.insert("stream".into(), json!(s.stream));
        Value::Object(body)
    }
}

/// The length of the common prefix of two byte strings.
#[must_use]
pub fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}
