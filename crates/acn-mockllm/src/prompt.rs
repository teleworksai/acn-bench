//! Prompt bytes and tokens (MLM-10, MLM-11).

use serde_json::Value;

use crate::profile::{Profile, Segment};

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
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                let f = n.as_f64().unwrap_or(f64::NAN);
                let text = acn_trace::identity::float_text(f)
                    .map_err(|e| PromptError::Invalid(e.to_string()))?;
                out.push_str(&text);
            }
        }
        Value::String(s) => out.push_str(&serde_json::to_string(s).unwrap_or_default()),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(x, out)?;
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
                out.push_str(&serde_json::to_string(k).unwrap_or_default());
                out.push(':');
                canonical(&m[k], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn has_cache_control(v: &Value) -> bool {
    match v {
        Value::Object(m) => m.contains_key("cache_control") || m.values().any(has_cache_control),
        Value::Array(a) => a.iter().any(has_cache_control),
        _ => false,
    }
}

/// A request's prompt as the mock sees it.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub bytes: Vec<u8>,
    /// For each element, the byte offset its `\n` ends at and whether it carries a
    /// `cache_control` breakpoint.
    pub elements: Vec<(usize, bool)>,
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
/// followed by one `\n`.
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
    let is_system = |m: &&Value| m.get("role").and_then(Value::as_str) == Some("system");
    let mut out = String::new();
    let mut elements = Vec::new();
    for segment in &profile.prefix_order {
        let items: Vec<&Value> = match segment {
            Segment::Tools => tools.iter().collect(),
            Segment::System => messages.iter().filter(is_system).collect(),
            Segment::Messages => messages.iter().filter(|m| !is_system(m)).collect(),
        };
        for item in items {
            canonical(item, &mut out)?;
            out.push('\n');
            elements.push((out.len(), has_cache_control(item)));
        }
    }
    Ok(Prompt {
        bytes: out.into_bytes(),
        elements,
    })
}
