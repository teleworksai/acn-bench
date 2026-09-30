//! Provider normalisation at ingest (TRC-21): a provider's own response fields
//! become `acn.*` values through the mapping recorded in the frozen inventory,
//! never through code that knows a provider. The rules applied here are the ones
//! stated in the provider section of `acn_attributes.toml`; this file adds none.
//! The input is the complete response object: for a streamed call, what the stream
//! assembles to. The field each value came from is
//! returned with it, so a per-provider verdict is auditable.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::schema::{AbsentMeans, Inventory, Provider};

/// The normalised stop reason (`acn.call.stop_reason`, TRC-12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    ContentFilter,
    ClientAbort,
    TransportError,
    Other,
}

impl StopReason {
    /// The attribute value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EndTurn => "end_turn",
            Self::ToolUse => "tool_use",
            Self::MaxTokens => "max_tokens",
            Self::StopSequence => "stop_sequence",
            Self::ContentFilter => "content_filter",
            Self::ClientAbort => "client_abort",
            Self::TransportError => "transport_error",
            Self::Other => "other",
        }
    }

    fn from_value(s: &str) -> Self {
        match s {
            "end_turn" => Self::EndTurn,
            "tool_use" => Self::ToolUse,
            "max_tokens" => Self::MaxTokens,
            "stop_sequence" => Self::StopSequence,
            "content_filter" => Self::ContentFilter,
            "client_abort" => Self::ClientAbort,
            "transport_error" => Self::TransportError,
            _ => Self::Other,
        }
    }
}

/// What a provider response says, in `acn.*` terms. A count is `None` when the
/// provider returned no usage: absent, never zero (SPEC 010 §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Normalised {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub stop_reason: Option<StopReason>,
    pub stop_reason_raw: Option<String>,
    /// For each value above, the provider field or fields it was read from.
    pub sources: BTreeMap<String, String>,
}

/// Why a response could not be normalised.
#[derive(Debug, thiserror::Error)]
pub enum NormaliseError {
    #[error("no provider mapping for `{0}` in acn_attributes.toml (TRC-21)")]
    UnknownProvider(String),
    #[error("provider field `{path}` is {found}, not a non-negative integer")]
    NotACount { path: String, found: String },
    #[error(
        "provider field `{path}` is {found}, which does not fit the signed 64-bit column it is stored in"
    )]
    TooLarge { path: String, found: u64 },
    #[error("the input token total does not fit a signed 64-bit column")]
    TotalOverflow,
    #[error(
        "the response has a `{usage}` object without `{base}`: this is not the wire format the `{provider}` mapping describes (TRC-21)"
    )]
    UsageWithoutBase {
        provider: String,
        usage: String,
        base: String,
    },
    #[error("cache-read count {cache_read} exceeds the input token total {total}")]
    CacheExceedsTotal { cache_read: u64, total: u64 },
}

/// Follow a dotted path; a numeric segment indexes an array.
fn lookup<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut at = root;
    for seg in path.split('.') {
        at = match at {
            Value::Object(map) => map.get(seg)?,
            Value::Array(items) => items.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(at)
}

/// A count at `path`: absent or null is `None`; anything but a non-negative integer
/// that fits an `Int64` column is an error.
fn count(root: &Value, path: &str) -> Result<Option<u64>, NormaliseError> {
    match lookup(root, path) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let n = v.as_u64().ok_or_else(|| NormaliseError::NotACount {
                path: path.to_owned(),
                found: v.to_string(),
            })?;
            if i64::try_from(n).is_err() {
                return Err(NormaliseError::TooLarge {
                    path: path.to_owned(),
                    found: n,
                });
            }
            Ok(Some(n))
        }
    }
}

fn sources_of(p: &Provider) -> BTreeMap<String, String> {
    let mut sources = BTreeMap::new();
    sources.insert("input_tokens".to_owned(), p.input_tokens.join(" + "));
    sources.insert("output_tokens".to_owned(), p.output_tokens.clone());
    sources.insert("cache_read_tokens".to_owned(), p.cache_read.clone());
    sources.insert(
        "cache_write_tokens".to_owned(),
        p.cache_write
            .clone()
            .unwrap_or_else(|| "none: the provider has no cache-write count".to_owned()),
    );
    sources.insert("stop_reason".to_owned(), p.stop_reason_field.clone());
    sources
}

fn apply(p: &Provider, raw: &Value) -> Result<Normalised, NormaliseError> {
    // Without the provider's own prompt count there is no usage. A usage object that
    // lacks it is a different wire format, and silence would hide that.
    let has_usage = count(raw, &p.input_tokens_base)?.is_some();
    if !has_usage
        && let Some((parent, _)) = p.input_tokens_base.rsplit_once('.')
        && lookup(raw, parent).is_some_and(Value::is_object)
    {
        return Err(NormaliseError::UsageWithoutBase {
            provider: p.name.clone(),
            usage: parent.to_owned(),
            base: p.input_tokens_base.clone(),
        });
    }
    let mut total: u64 = 0;
    if has_usage {
        for path in &p.input_tokens {
            total = total
                .checked_add(count(raw, path)?.unwrap_or(0))
                .filter(|t| i64::try_from(*t).is_ok())
                .ok_or(NormaliseError::TotalOverflow)?;
        }
    }
    // A missing cache-read field is zero only for a provider that always reports it;
    // otherwise it is unknown, and a producer never invents a count (TRC-12).
    let cache_read = match (has_usage, count(raw, &p.cache_read)?, p.cache_read_absent) {
        (false, _, _) | (true, None, AbsentMeans::Absent) => None,
        (true, Some(n), _) => Some(n),
        (true, None, AbsentMeans::Zero) => Some(0),
    };
    if let Some(n) = cache_read
        && n > total
    {
        return Err(NormaliseError::CacheExceedsTotal {
            cache_read: n,
            total,
        });
    }
    let cache_write = match (&p.cache_write, has_usage) {
        (_, false) => None,
        (Some(path), true) => Some(count(raw, path)?.unwrap_or(0)),
        (None, true) => Some(0),
    };
    let raw_stop = lookup(raw, &p.stop_reason_field)
        .and_then(Value::as_str)
        .map(str::to_owned);
    let stop_reason = raw_stop.as_deref().map(|v| {
        p.stop_reason
            .get(v)
            .map_or(StopReason::Other, |n| StopReason::from_value(n))
    });
    Ok(Normalised {
        input_tokens: has_usage.then_some(total),
        output_tokens: if has_usage {
            count(raw, &p.output_tokens)?
        } else {
            None
        },
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        stop_reason,
        stop_reason_raw: raw_stop,
        sources: sources_of(p),
    })
}

/// Normalise one complete provider response.
pub fn response(
    inv: &Inventory,
    provider: &str,
    raw: &Value,
) -> Result<Normalised, NormaliseError> {
    let p = inv
        .provider(provider)
        .ok_or_else(|| NormaliseError::UnknownProvider(provider.to_owned()))?;
    apply(p, raw)
}
