//! The mock as a pure function of its state (MLM-5): a request, its tenant and its
//! arrival time in; the response body, the stream chunks with their emission
//! times, and the timing out. No socket, no clock read. The HTTP server applies
//! exactly this.
//!
//! Random draws come from the `mockllm` sub-stream (MLM-6), in a fixed order per
//! request (ADR-16): three fault draws, always made; then, for a 429, the
//! retry-after; for a text answer, its length and then one word per token; for a
//! cut stream, the cut point; then one jitter per output token.

use std::collections::BTreeMap;

use rand_chacha::ChaCha20Rng;
use rand_core::Rng as _;
use serde_json::{Value, json};

use crate::cache::{Accounting, Cache};
use crate::profile::{PPM, Profile, Profiles};
use crate::prompt::{self, Prompt};

/// The words answers are made of: 4 bytes each, so a word is a token (MLM-40).
pub const WORDS: &[&str] = &[
    "lore", "mock", "data", "text", "word", "acn ", "loop", "link", "turn", "call", "tool", "node",
    "path", "time", "byte", "span",
];

/// The timing of one response (MLM-30, MLM-31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timing {
    pub queue_ns: i64,
    pub prefill_ns: i64,
    pub decode_ns: i64,
}

impl Timing {
    /// The `x-acn-mock-timing` header value.
    #[must_use]
    pub fn header(&self) -> String {
        format!(
            "queue_ns={} prefill_ns={} decode_ns={}",
            self.queue_ns, self.prefill_ns, self.decode_ns
        )
    }
}

/// One server-sent event and the time it is emitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub at_ns: i64,
    /// The event's data line content (a JSON object, or `[DONE]`).
    pub data: String,
}

/// What the mock answers to one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// HTTP status.
    pub status: u16,
    /// Headers, in order.
    pub headers: Vec<(String, String)>,
    /// The non-streamed body (or the error body).
    pub body: Vec<u8>,
    /// The stream events of a streamed 200; a cut stream stops early, without a
    /// final chunk, usage or `[DONE]` (MLM-41).
    pub chunks: Vec<Chunk>,
    /// When the non-streamed body is sent: the last token's time (MLM-30).
    pub respond_at_ns: i64,
    /// Each output token's emission time.
    pub token_times: Vec<i64>,
    pub timing: Timing,
    pub accounting: Accounting,
    pub stream: bool,
}

/// The mock: its profiles, a cache and a slot pool per profile, and its random
/// stream. A profile is a model, so profiles share neither cache entries nor
/// slots (ADR-16).
#[derive(Debug, Clone)]
pub struct Mock {
    profiles: Profiles,
    caches: BTreeMap<String, Cache>,
    /// Per profile, when each slot becomes free (MLM-30).
    slots: BTreeMap<String, Vec<i64>>,
    rng: ChaCha20Rng,
}

/// The `x-acn-mockllm` header value for `profile` (MLM-4); `-` when no profile
/// applies.
#[must_use]
pub fn marker(profile: &str) -> String {
    format!(
        "acn-mockllm/{} profile={profile}",
        env!("CARGO_PKG_VERSION")
    )
}

/// The OpenAI-shaped error body (MLM-1).
#[must_use]
pub fn error_body(kind: &str, message: &str) -> Vec<u8> {
    json!({ "error": { "message": message, "type": kind, "code": Value::Null } })
        .to_string()
        .into_bytes()
}

/// A request's token limit: `max_completion_tokens` if set, else `max_tokens`; a
/// `null` is unset, and any other non-count value is refused (MLM-1).
fn token_limit(req: &Value) -> Result<Option<u64>, String> {
    for key in ["max_completion_tokens", "max_tokens"] {
        match req.get(key) {
            None | Some(Value::Null) => {}
            Some(v) => {
                return v
                    .as_u64()
                    .map(Some)
                    .ok_or_else(|| format!("`{key}` must be a non-negative integer"));
            }
        }
    }
    Ok(None)
}

/// A uniform draw over `0..n` (n > 0), exact: rejection sampling on 64 bits.
fn below(rng: &mut ChaCha20Rng, n: u64) -> u64 {
    let zone = u64::MAX - (u64::MAX % n);
    loop {
        let x = rng.next_u64();
        if x < zone {
            return x % n;
        }
    }
}

impl Mock {
    /// A mock over the embedded profiles whose draws come from the `mockllm`
    /// sub-stream under `stream_seed` (the replicate seed of CON-30(a)).
    pub fn new(stream_seed: u64) -> Result<Self, crate::MockError> {
        Self::with_profiles(crate::profile::embedded()?, stream_seed)
    }

    /// A mock over the given profiles, which are checked as
    /// [`Profiles::parse`] checks them.
    pub fn with_profiles(profiles: Profiles, stream_seed: u64) -> Result<Self, crate::MockError> {
        profiles.check()?;
        Ok(Self {
            profiles,
            caches: BTreeMap::new(),
            slots: BTreeMap::new(),
            rng: acn_trace::identity::substream_rng(stream_seed, "mockllm")?,
        })
    }

    /// The profiles this mock serves.
    #[must_use]
    pub fn profiles(&self) -> &Profiles {
        &self.profiles
    }

    /// The caches' sizes over every profile (prefix entries, blocks).
    #[must_use]
    pub fn cache_sizes(&self) -> (usize, usize) {
        self.caches.values().fold((0, 0), |(p, b), c| {
            let (cp, cb) = c.sizes();
            (p + cp, b + cb)
        })
    }

    fn base_headers(profile: &str) -> Vec<(String, String)> {
        vec![("x-acn-mockllm".to_owned(), marker(profile))]
    }

    /// An error, answered at arrival: no slot, no cache change, zero timing
    /// (MLM-31 puts the timing header on every response).
    fn fail(status: u16, kind: &str, message: &str, profile: &str, arrival: i64) -> Outcome {
        let mut headers = Self::base_headers(profile);
        headers.push(("x-acn-mock-timing".into(), Timing::default().header()));
        headers.push(("content-type".into(), "application/json".into()));
        Outcome {
            status,
            headers,
            body: error_body(kind, message),
            chunks: Vec::new(),
            respond_at_ns: arrival,
            token_times: Vec::new(),
            timing: Timing::default(),
            accounting: Accounting::default(),
            stream: false,
        }
    }

    /// Handle one request (MLM-1..41). Requests must be handled in arrival order;
    /// [`Mock::handle_batch`] orders concurrent ones (MLM-7).
    pub fn handle(&mut self, body: &[u8], tenant: &str, arrival_ns: i64) -> Outcome {
        let req: Value = match serde_json::from_slice(body) {
            Ok(v @ Value::Object(_)) => v,
            _ => {
                return Self::fail(
                    400,
                    "invalid_request_error",
                    "the body is not a JSON object",
                    "-",
                    arrival_ns,
                );
            }
        };
        let name = req
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let Some(profile) = self.profiles.get(&name).cloned() else {
            return Self::fail(
                400,
                "invalid_request_error",
                &format!("unknown model `{name}`; GET /v1/models lists the profiles"),
                "-",
                arrival_ns,
            );
        };
        let prompt = match prompt::prompt(&req, &profile) {
            Ok(p) => p,
            Err(e) => {
                return Self::fail(
                    400,
                    "invalid_request_error",
                    &e.to_string(),
                    &name,
                    arrival_ns,
                );
            }
        };
        let stream = req.get("stream").and_then(Value::as_bool).unwrap_or(false);
        let include_usage = req
            .pointer("/stream_options/include_usage")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let limit = match token_limit(&req) {
            Ok(l) => l,
            Err(e) => return Self::fail(400, "invalid_request_error", &e, &name, arrival_ns),
        };

        // Faults: three draws, always, so a rate never shifts the other draws.
        let draws = [
            below(&mut self.rng, PPM),
            below(&mut self.rng, PPM),
            below(&mut self.rng, PPM),
        ];
        if draws[0] < profile.fault_429_ppm {
            let retry = 1 + below(&mut self.rng, profile.retry_after_s_max);
            let mut o = Self::fail(
                429,
                "rate_limit_error",
                "injected rate limit",
                &name,
                arrival_ns,
            );
            o.headers.push(("retry-after".into(), retry.to_string()));
            return o;
        }
        if draws[1] < profile.fault_500_ppm {
            return Self::fail(
                500,
                "server_error",
                "injected server error",
                &name,
                arrival_ns,
            );
        }
        let cut = stream && draws[2] < profile.fault_cut_ppm;

        // The cache: a cut stream changes nothing (MLM-41).
        let cache = self.caches.entry(name.clone()).or_default();
        let accounting = if cut {
            cache.clone().account(&profile, tenant, &prompt, arrival_ns)
        } else {
            cache.account(&profile, tenant, &prompt, arrival_ns)
        };

        let reply = self.reply(&req, &prompt, &profile, limit);
        let n_tokens = reply.tokens();

        // Timing (MLM-30).
        // The slot that frees earliest, ties to the lowest index (ADR-16).
        let slots = self.slots.entry(name.clone()).or_default();
        if profile.slots > 0 {
            slots.resize(
                usize::try_from(profile.slots).unwrap_or(usize::MAX),
                i64::MIN,
            );
        }
        let slot = slots
            .iter()
            .copied()
            .enumerate()
            .min_by_key(|(i, t)| (*t, *i));
        let start = slot.map_or(arrival_ns, |(_, free)| arrival_ns.max(free));
        let queue_ns = start - arrival_ns;
        let new_tokens = prompt.tokens() - accounting.cached_tokens;
        let to_i64 = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
        let prefill_ns = profile
            .prefill_base_ns
            .saturating_add(
                profile
                    .prefill_ns_per_new_token
                    .saturating_mul(to_i64(new_tokens)),
            )
            .saturating_add(
                profile
                    .prefill_ns_per_cached_token
                    .saturating_mul(to_i64(accounting.cached_tokens)),
            );
        let first = start.saturating_add(prefill_ns);
        let cut_after = if cut {
            Some(1 + below(&mut self.rng, n_tokens.max(1)))
        } else {
            None
        };
        let mut token_times = Vec::with_capacity(n_tokens as usize);
        let mut prev = first;
        for k in 0..n_tokens {
            let span = 2 * profile.itl_jitter_ns.unsigned_abs() + 1;
            let jitter = to_i64(below(&mut self.rng, span)) - profile.itl_jitter_ns;
            let nominal = first
                .saturating_add(profile.itl_ns.saturating_mul(to_i64(k)))
                .saturating_add(if k == 0 { 0 } else { jitter });
            let t = nominal.max(prev);
            token_times.push(t);
            prev = t;
        }
        let last = token_times.last().copied().unwrap_or(first);
        if let Some((i, _)) = slot
            && let Some(busy) = self.slots.get_mut(&name).and_then(|s| s.get_mut(i))
        {
            *busy = last;
        }
        let timing = Timing {
            queue_ns,
            prefill_ns,
            decode_ns: last - first,
        };

        let id = {
            let mut h = blake3::Hasher::new();
            h.update(tenant.as_bytes());
            h.update(&arrival_ns.to_le_bytes());
            h.update(&prompt.bytes);
            format!("chatcmpl-{}", &h.finalize().to_hex()[..24])
        };
        let created = arrival_ns.div_euclid(1_000_000_000);
        let fingerprint = format!("acn-mockllm:{name}");
        let usage = json!({
            "prompt_tokens": prompt.tokens(),
            "completion_tokens": n_tokens,
            "total_tokens": prompt.tokens() + n_tokens,
            "prompt_tokens_details": {
                "cached_tokens": accounting.cached_tokens,
                "cache_write_tokens": accounting.cache_write_tokens,
            },
        });
        let message = match &reply {
            Reply::Text { words, .. } => json!({ "role": "assistant", "content": words.concat() }),
            Reply::Tool(call) => {
                json!({ "role": "assistant", "content": Value::Null, "tool_calls": [call] })
            }
        };
        let finish = reply.finish();
        let body = json!({
            "id": id, "object": "chat.completion", "created": created, "model": name,
            "system_fingerprint": fingerprint,
            "choices": [{ "index": 0, "message": message, "finish_reason": finish }],
            "usage": usage,
        });
        let chunk = |delta: Value, finish: Value| {
            json!({
                "id": id, "object": "chat.completion.chunk", "created": created, "model": name,
                "system_fingerprint": fingerprint,
                "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
            })
            .to_string()
        };
        let mut chunks = Vec::new();
        match &reply {
            // An empty answer still says who speaks, so the stream assembles to
            // the plain response's `""` (MLM-3).
            Reply::Text { words, .. } if words.is_empty() && cut_after.is_none() => {
                chunks.push(Chunk {
                    at_ns: last,
                    data: chunk(json!({ "role": "assistant", "content": "" }), Value::Null),
                });
            }
            Reply::Text { words, .. } => {
                for (k, w) in words.iter().enumerate() {
                    let delta = if k == 0 {
                        json!({ "role": "assistant", "content": w })
                    } else {
                        json!({ "content": w })
                    };
                    chunks.push(Chunk {
                        at_ns: token_times[k],
                        data: chunk(delta, Value::Null),
                    });
                }
            }
            Reply::Tool(call) => {
                let mut c = call.clone();
                c["index"] = json!(0);
                chunks.push(Chunk {
                    at_ns: token_times[0],
                    data: chunk(
                        json!({ "role": "assistant", "tool_calls": [c] }),
                        Value::Null,
                    ),
                });
            }
        }
        if let Some(after) = cut_after {
            // A cut stream: some tokens, no final chunk, no usage, no [DONE].
            chunks.truncate(usize::try_from(after).unwrap_or(usize::MAX));
        } else {
            chunks.push(Chunk {
                at_ns: last,
                data: chunk(json!({}), json!(finish)),
            });
            if include_usage {
                chunks.push(Chunk {
                    at_ns: last,
                    data: json!({
                        "id": id, "object": "chat.completion.chunk", "created": created, "model": name,
                        "system_fingerprint": fingerprint, "choices": [], "usage": usage,
                    })
                    .to_string(),
                });
            }
            chunks.push(Chunk {
                at_ns: last,
                data: "[DONE]".into(),
            });
        }

        let mut headers = Self::base_headers(&name);
        headers.push(("x-acn-mock-timing".into(), timing.header()));
        headers.push((
            "content-type".into(),
            if stream {
                "text/event-stream"
            } else {
                "application/json"
            }
            .into(),
        ));
        Outcome {
            status: 200,
            headers,
            body: body.to_string().into_bytes(),
            chunks: if stream { chunks } else { Vec::new() },
            respond_at_ns: last,
            token_times,
            timing,
            accounting,
            stream,
        }
    }

    /// The BLAKE3 of a request's prompt bytes, or `None` for a request with no
    /// prompt (one that gets a 400).
    fn prompt_hash(&self, body: &[u8]) -> Option<[u8; 32]> {
        let req: Value = serde_json::from_slice(body).ok()?;
        let profile = self.profiles.get(req.get("model")?.as_str()?)?;
        let p = prompt::prompt(&req, profile).ok()?;
        Some(*blake3::hash(&p.bytes).as_bytes())
    }

    /// Handle requests that arrive together (MLM-7): they are processed in order of
    /// arrival, tenant, then the BLAKE3 of their prompt bytes, whatever order they
    /// are given in. Requests with no prompt go first among their equals, by the
    /// BLAKE3 of their body (ADR-16). The outcomes are returned in the order given.
    pub fn handle_batch(&mut self, requests: &[(Vec<u8>, String, i64)]) -> Vec<Outcome> {
        let mut order: Vec<usize> = (0..requests.len()).collect();
        order.sort_by_cached_key(|&i| {
            let (body, tenant, at) = &requests[i];
            (
                *at,
                tenant.clone(),
                self.prompt_hash(body),
                *blake3::hash(body).as_bytes(),
            )
        });
        let mut out: Vec<Option<Outcome>> = vec![None; requests.len()];
        for i in order {
            let (body, tenant, at) = &requests[i];
            out[i] = Some(self.handle(body, tenant, *at));
        }
        out.into_iter().flatten().collect()
    }

    /// The reply policy (MLM-40).
    fn reply(
        &mut self,
        req: &Value,
        prompt: &Prompt,
        profile: &Profile,
        limit: Option<u64>,
    ) -> Reply {
        let empty = Vec::new();
        let messages = req
            .get("messages")
            .and_then(Value::as_array)
            .unwrap_or(&empty);
        let tools = req.get("tools").and_then(Value::as_array).unwrap_or(&empty);
        let since_user = messages
            .iter()
            .rev()
            .take_while(|m| m.get("role").and_then(Value::as_str) != Some("user"))
            .filter(|m| m.get("role").and_then(Value::as_str) == Some("tool"))
            .count() as u64;
        if !tools.is_empty() && since_user < profile.tool_calls_per_turn {
            let tool = &tools[(since_user as usize) % tools.len()];
            // `prompt::prompt` refused a tool without a name.
            let name = tool
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let id = format!("call_{}", &blake3::hash(&prompt.bytes).to_hex()[..16]);
            return Reply::Tool(json!({
                "id": id, "type": "function",
                "function": { "name": name, "arguments": "{}" },
            }));
        }
        let span = profile.output_tokens_max - profile.output_tokens_min + 1;
        let drawn = profile.output_tokens_min + below(&mut self.rng, span);
        let n = limit.map_or(drawn, |l| drawn.min(l));
        let words = (0..n)
            .map(|_| WORDS[below(&mut self.rng, WORDS.len() as u64) as usize])
            .collect();
        Reply::Text {
            words,
            truncated: n < drawn,
        }
    }
}

enum Reply {
    Text {
        words: Vec<&'static str>,
        truncated: bool,
    },
    Tool(Value),
}

impl Reply {
    /// A tool call is one output token (ADR-16); an answer, one per word.
    fn tokens(&self) -> u64 {
        match self {
            Self::Text { words, .. } => words.len() as u64,
            Self::Tool(_) => 1,
        }
    }

    fn finish(&self) -> &'static str {
        match self {
            Self::Text {
                truncated: true, ..
            } => "length",
            Self::Text { .. } => "stop",
            Self::Tool(_) => "tool_calls",
        }
    }
}
