//! Prompt bytes and tokens (MLM-10, MLM-11).

use serde_json::Value;

use crate::profile::{CacheModel, Profile, Segment};

/// A request whose prompt cannot be made canonical.
#[derive(Debug, thiserror::Error)]
pub enum PromptError {
    #[error("{0}")]
    Invalid(String),
}

/// The canonical JSON of a value (MLM-10): keys sorted bytewise, no insignificant
/// whitespace, numbers in the text form of CON-27(c), every `cache_control` member
/// removed.
pub fn canonical(v: &Value, out: &mut String) -> Result<(), PromptError> {
    canonical_marked(v, out, &mut Vec::new())
}

/// [`canonical`], also recording in `marks` the byte offset at which each object
/// carrying a `cache_control` member ends. Objects end in the order they are
/// written, so the offsets ascend.
fn canonical_marked(
    v: &Value,
    out: &mut String,
    marks: &mut Vec<usize>,
) -> Result<(), PromptError> {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                let f = n.as_f64().ok_or_else(|| {
                    PromptError::Invalid(format!("the number {n} has no float value"))
                })?;
                let text = acn_trace::identity::float_text(f)
                    .map_err(|e| PromptError::Invalid(e.to_string()))?;
                out.push_str(&text);
            }
        }
        Value::String(s) => out.push_str(&quote(s)?),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_marked(x, out, marks)?;
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().filter(|k| *k != "cache_control").collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&quote(k)?);
                out.push(':');
                canonical_marked(&m[k], out, marks)?;
            }
            out.push('}');
            if m.contains_key("cache_control") {
                marks.push(out.len());
            }
        }
    }
    Ok(())
}

fn quote(s: &str) -> Result<String, PromptError> {
    serde_json::to_string(s).map_err(|e| PromptError::Invalid(e.to_string()))
}

/// The roles MLM-1 accepts.
const ROLES: &[&str] = &["system", "user", "assistant", "tool"];

/// Refuse a message or tool whose shape MLM-1 does not admit (MLM-1: a request
/// the mock cannot parse gets a 400).
fn check_shapes(messages: &[Value], tools: &[Value]) -> Result<(), PromptError> {
    for (i, m) in messages.iter().enumerate() {
        let role = m.get("role").and_then(Value::as_str);
        if !role.is_some_and(|r| ROLES.contains(&r)) {
            return Err(PromptError::Invalid(format!(
                "messages[{i}]: `role` must be one of system, user, assistant, tool"
            )));
        }
        match m.get("content") {
            None | Some(Value::Null | Value::String(_)) => {}
            Some(Value::Array(parts)) if parts.iter().all(Value::is_object) => {}
            Some(_) => {
                return Err(PromptError::Invalid(format!(
                    "messages[{i}]: `content` must be a string or an array of parts"
                )));
            }
        }
    }
    for (i, t) in tools.iter().enumerate() {
        if t.pointer("/function/name")
            .and_then(Value::as_str)
            .is_none()
        {
            return Err(PromptError::Invalid(format!(
                "tools[{i}]: `function.name` is missing or not a string"
            )));
        }
    }
    Ok(())
}

/// A request's prompt as the mock sees it.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub bytes: Vec<u8>,
    /// The byte offset each `cache_control` breakpoint's prefix ends at, in prompt
    /// order (MLM-21): the end of the marked content part, or, for a marked message
    /// or tool definition, the end of its element including the `\n`.
    pub breakpoints: Vec<usize>,
}

impl Prompt {
    /// The number of tokens: ⌈bytes / 4⌉ (MLM-11).
    #[must_use]
    pub fn tokens(&self) -> u64 {
        tokens_of(self.bytes.len())
    }
}

/// ⌈b / 4⌉ (MLM-11).
#[must_use]
pub fn tokens_of(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(4)
}

/// The tokens of a byte string: 4-byte units, the last one shorter (MLM-11).
#[must_use]
pub fn tokenize(bytes: &[u8]) -> Vec<&[u8]> {
    bytes.chunks(4).collect()
}

/// The prompt bytes of a request body (MLM-10): tools, system messages and the
/// other messages, in the profile's `prefix_order`, each element's canonical JSON
/// followed by one `\n`. Under `explicit_breakpoints`, more than
/// `max_breakpoints` breakpoints is an error here, before any draw (MLM-21).
pub fn prompt(body: &Value, profile: &Profile) -> Result<Prompt, PromptError> {
    let empty = Vec::new();
    let tools = match body.get("tools") {
        None | Some(Value::Null) => &empty,
        Some(Value::Array(a)) => a,
        Some(_) => return Err(PromptError::Invalid("`tools` is not an array".into())),
    };
    let Some(Value::Array(messages)) = body.get("messages") else {
        return Err(PromptError::Invalid(
            "`messages` is missing or not an array".into(),
        ));
    };
    check_shapes(messages, tools)?;
    let is_system = |m: &&Value| m.get("role").and_then(Value::as_str) == Some("system");
    let mut out = String::new();
    let mut breakpoints = Vec::new();
    for segment in &profile.prefix_order {
        let items: Vec<&Value> = match segment {
            Segment::Tools => tools.iter().collect(),
            Segment::System => messages.iter().filter(is_system).collect(),
            Segment::Messages => messages.iter().filter(|m| !is_system(m)).collect(),
        };
        for item in items {
            let mut marks = Vec::new();
            canonical_marked(item, &mut out, &mut marks)?;
            // A mark on the element itself ends at its `}`: count the `\n` too.
            let end = out.len();
            out.push('\n');
            breakpoints.extend(marks.into_iter().map(|m| if m == end { m + 1 } else { m }));
        }
    }
    if profile.cache_model == CacheModel::ExplicitBreakpoints
        && breakpoints.len() as u64 > profile.max_breakpoints
    {
        return Err(PromptError::Invalid(format!(
            "{} cache_control breakpoints; at most {} are allowed",
            breakpoints.len(),
            profile.max_breakpoints
        )));
    }
    Ok(Prompt {
        bytes: out.into_bytes(),
        breakpoints,
    })
}
