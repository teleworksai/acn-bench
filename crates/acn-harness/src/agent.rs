//! The agent loop (HAR-1..5) and what it records (HAR-30..34): sessions, turns,
//! calls with retries, simulated tools, compaction and fan-out, each emitted as
//! the span TRC-10..14 defines, with times read from the run's clock.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use acn_trace::normalise::{self, Normalised};
use acn_trace::schema::Inventory;
use opentelemetry::trace::{Span as _, SpanKind, TraceContextExt as _, Tracer as _};
use opentelemetry::{Context as Cx, KeyValue};
use opentelemetry_sdk::trace::SdkTracer;
use rand_chacha::ChaCha20Rng;
use rand_core::Rng as _;
use serde_json::Value;

use crate::HarnessError;
use crate::context::{
    Context, Dialect, Encoding, Msg, Sampling, ToolCall, ToolChoice, ToolDef, common_prefix,
};
use crate::env::{Env, join_all};
use crate::knobs::{Backfill, Compaction, Fanout, Knobs};
use crate::wire::{AssembleError, Backend, Exchange, Failure, LinkRecord, Reply, assemble};
use crate::workload::{Range, Tool, Turn, Workload};

/// The printable ASCII simulated results are made of (HAR-2).
pub const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789 ";

/// A uniform draw over `0..n` (n > 0), exact: rejection sampling on 64 bits.
pub fn below(rng: &mut ChaCha20Rng, n: u64) -> u64 {
    if n <= 1 {
        return 0;
    }
    let zone = u64::MAX - (u64::MAX % n);
    loop {
        let x = rng.next_u64();
        if x < zone {
            return x % n;
        }
    }
}

/// A uniform draw over an inclusive range.
pub fn draw(rng: &mut ChaCha20Rng, r: Range) -> u64 {
    r.min + below(rng, r.max - r.min + 1)
}

/// A uniformly random permutation of `0..n` (Fisher–Yates, from the top).
pub fn permutation(rng: &mut ChaCha20Rng, n: usize) -> Vec<usize> {
    let mut v: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        let j = usize::try_from(below(rng, i as u64 + 1)).unwrap_or(0);
        v.swap(i, j);
    }
    v
}

/// MLM-11's token: 4 bytes. The `window_full` estimate and the mock's counting
/// method use it.
pub const BYTES_PER_TOKEN: u64 = 4;

/// The `acn.call.error_class` of a response the dialect cannot read (ADR-17).
pub const MALFORMED: &str = "malformed_response";

/// TRC-12's scaling: `⌊shared_bytes × tokens / bytes⌋` in integer arithmetic.
#[must_use]
pub fn scaled(shared_bytes: u64, tokens: u64, bytes: u64) -> u64 {
    if bytes == 0 {
        return 0;
    }
    u64::try_from(u128::from(shared_bytes) * u128::from(tokens) / u128::from(bytes))
        .unwrap_or(tokens)
}

fn int(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn ms(ns: i64) -> f64 {
    // A duration in ms for an attribute: exact for any duration below 2^53 ns.
    #[allow(clippy::cast_precision_loss)]
    let v = ns as f64 / 1_000_000.0;
    v
}

/// A run-clock time as the `SystemTime` a span takes, for other modules.
pub(crate) fn at_ns(ns: i64) -> SystemTime {
    at(ns)
}

fn at(ns: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_nanos(u64::try_from(ns).unwrap_or(0))
}

/// How `acn.call.new_input_tokens` is counted (HAR-32).
#[derive(Debug, Clone)]
pub enum Counting {
    /// The mock's own prompt bytes and 4-byte tokens (MLM-10, MLM-11).
    Tokens(Box<acn_mockllm::profile::Profile>),
    /// TRC-12's canonical byte strings, scaled.
    BytesScaled,
}

impl Counting {
    #[must_use]
    pub fn method(&self) -> &'static str {
        match self {
            Self::Tokens(_) => "tokens",
            Self::BytesScaled => "bytes_scaled",
        }
    }
}

/// The run's options (HAR-24, HAR-25, TRC-12).
#[derive(Debug, Clone)]
pub struct Opts {
    pub endpoint: String,
    pub max_retries: u64,
    pub retry_base_ms: u64,
    pub request_timeout_ms: u64,
    pub stall_threshold_ms: f64,
}

impl Default for Opts {
    /// The defaults `acn_attributes.toml` records.
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            max_retries: 3,
            retry_base_ms: 500,
            request_timeout_ms: 600_000,
            stall_threshold_ms: 250.0,
        }
    }
}

/// What does not change within a run.
#[derive(Debug, Clone)]
pub struct Setup {
    pub workload: Workload,
    pub knobs: Knobs,
    pub backend: Backend,
    pub model: String,
    pub opts: Opts,
    pub inv: Inventory,
    pub counting: Counting,
    /// The run-level `acn.session` attributes (TRC-10), without `acn.replicate`.
    pub session_attrs: Vec<KeyValue>,
}

/// The replicate's random streams (HAR-40).
#[derive(Debug)]
pub struct Streams {
    pub tools: ChaCha20Rng,
    pub knobs: ChaCha20Rng,
    pub workload: ChaCha20Rng,
}

impl Streams {
    /// The sub-streams of replicate seed `s` (CON-30(b)).
    pub fn new(s: u64) -> Result<Self, HarnessError> {
        let rng = |n| acn_trace::identity::substream_rng(s, n);
        Ok(Self {
            tools: rng("harness.tools")?,
            knobs: rng("harness.knobs")?,
            workload: rng("harness.workload")?,
        })
    }

    /// A sub-agent's own streams, `harness.<stream>.<scope>`: its draws never
    /// depend on when its siblings' responses arrive (ADR-17).
    pub fn scoped(s: u64, scope: &str) -> Result<Self, HarnessError> {
        let rng = |n: &str| acn_trace::identity::substream_rng(s, &format!("harness.{n}.{scope}"));
        Ok(Self {
            tools: rng("tools")?,
            knobs: rng("knobs")?,
            workload: rng("workload")?,
        })
    }
}

/// What a replicate with a scenario needs to record its link spans (SPEC 020
/// EMU-36).
pub struct NetSpans<'a> {
    /// The `acn-emu` resource's tracer, sharing the replicate's id stream.
    pub tracer: &'a SdkTracer,
    /// The run's `acn.scenario` span, which every link span links to.
    pub scenario: opentelemetry::trace::SpanContext,
    /// The path's name in the scenario (`acn.link.id`).
    pub link_id: String,
    /// `acn.link.model` of the up and the down link.
    pub up_model: String,
    pub down_model: String,
}

/// One replicate in progress: everything its lineages share.
pub struct Replicate<'a, E: Env> {
    pub setup: &'a Setup,
    pub env: &'a E,
    pub tracer: &'a SdkTracer,
    /// With a scenario, how its link spans are recorded.
    pub net: Option<NetSpans<'a>>,
    pub marker: String,
    pub replicate: u32,
    /// The replicate seed, for sub-agents' scoped streams.
    pub seed: u64,
    pub streams: RefCell<Streams>,
}

/// Retry-after waits longer than this are cut to it (ADR-17).
pub const MAX_RETRY_AFTER_S: u64 = 300;

/// The tool choice that lets a call use only `tools` (HAR-14): none at all when
/// there are none.
fn choice_of(tools: &[String]) -> ToolChoice {
    if tools.is_empty() {
        ToolChoice::Forbid
    } else {
        ToolChoice::Allowed(tools.to_vec())
    }
}

/// A context lineage (TRC-12): the main chain or one sub-agent.
#[derive(Debug, Clone)]
struct Lineage {
    system_base: String,
    tools: Vec<String>,
    messages: Vec<Msg>,
    /// The start of the turn in progress, for the timestamp (HAR-11).
    turn_start: i64,
    /// The bytes the next call is compared with: the previous call's context
    /// followed by its response (TRC-12), and, for a sub-agent's first call, the
    /// spawning context.
    prev: Option<Vec<u8>>,
    /// The previous call's uncached input tokens, for `read_cost_threshold`.
    last_uncached: Option<u64>,
    /// The `acn.call.index` of the next call in this turn (TRC-12).
    call_index: i64,
    /// A sub-agent's first context under `fork_from_prefix`, sent as it is (HAR-14).
    first: Option<Context>,
    /// Under `fork_from_prefix`, the only tools the sub-agent may call: it
    /// presents `tools` (the parent's) and executes only these (HAR-14).
    allowed: Option<Vec<String>>,
    /// The task this lineage belongs to.
    task: usize,
    /// A sub-agent's own streams; the main lineage uses the replicate's.
    own: Option<std::rc::Rc<RefCell<Streams>>>,
}

/// One call's result.
struct Called {
    reply: Option<Reply>,
    index: i64,
    input_tokens: Option<u64>,
    /// The context sent and its compared bytes, for fan-out.
    context: Context,
    compared: Vec<u8>,
}

/// How a turn ended (TRC-11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    Success,
    Failure,
    Timeout,
    Aborted,
}

impl TurnOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Timeout => "timeout",
            Self::Aborted => "aborted",
        }
    }
}

impl<E: Env> Replicate<'_, E> {
    fn sampling(&self, max_tokens: u64) -> Sampling {
        let a = &self.setup.workload.agent;
        Sampling {
            max_tokens,
            temperature: a.temperature,
            stream: a.stream,
        }
    }

    /// The system prompt of HAR-11 and HAR-42.
    fn system(&self, base: &str, turn_start: i64) -> String {
        let mut s = format!("Session: {}\n", self.marker);
        if self.setup.knobs.timestamp_in_system_prompt {
            s.push_str(&format!("Current time: {}\n", turn_start / 1_000_000));
        }
        s.push_str(base);
        s
    }

    /// The streams a lineage draws from.
    fn streams_of<'b>(&'b self, lin: &'b Lineage) -> &'b RefCell<Streams> {
        lin.own.as_deref().unwrap_or(&self.streams)
    }

    fn tool_defs(&self, lin: &Lineage) -> Result<Vec<ToolDef>, HarnessError> {
        let names = &lin.tools;
        let order: Vec<usize> = if self.setup.knobs.tool_order_stable {
            (0..names.len()).collect()
        } else {
            permutation(&mut self.streams_of(lin).borrow_mut().knobs, names.len())
        };
        order
            .into_iter()
            .map(|i| {
                let t = self.tool(&names[i])?;
                Ok(ToolDef {
                    name: t.name.clone(),
                    description: format!("{} {}", self.marker, t.description),
                    parameters: t.parameters_json()?,
                })
            })
            .collect()
    }

    fn tool(&self, name: &str) -> Result<&Tool, HarnessError> {
        self.setup
            .workload
            .tool(name)
            .ok_or_else(|| HarnessError::Workload(format!("tool `{name}` is not defined")))
    }

    fn context(&self, lin: &Lineage) -> Result<Context, HarnessError> {
        Ok(Context {
            system: self.system(&lin.system_base, lin.turn_start),
            tools: self.tool_defs(lin)?,
            messages: lin.messages.clone(),
            tool_choice: lin.allowed.as_ref().map(|a| choice_of(a)),
        })
    }

    /// The bytes a context is compared by (HAR-32).
    fn compared(&self, ctx: &Context) -> Result<Vec<u8>, HarnessError> {
        match &self.setup.counting {
            Counting::Tokens(profile) => {
                // The mock's prompt bytes read only the tools and messages
                // (MLM-10): neither breakpoints nor a tool choice matter here.
                let json = ctx.encode(
                    Encoding::plain(Dialect::ChatCompletions),
                    &self.setup.model,
                    self.sampling(1),
                    crate::knobs::Placement::None,
                );
                acn_mockllm::prompt::prompt(&json, profile)
                    .map(|p| p.bytes)
                    .map_err(|e| HarnessError::Internal(format!("mock prompt bytes: {e}")))
            }
            Counting::BytesScaled => Ok(ctx.canonical_bytes()),
        }
    }

    /// Tokens of a common prefix of `a` and `b` (TRC-12): whole tokens, or the
    /// shared bytes scaled by `tokens / bytes` of the one request whose token
    /// count is known, `basis`.
    fn shared_tokens(&self, a: &[u8], b: &[u8], basis: (u64, usize)) -> u64 {
        let lcp = common_prefix(a, b) as u64;
        match self.setup.counting {
            Counting::Tokens(_) => lcp / BYTES_PER_TOKEN,
            Counting::BytesScaled => scaled(lcp, basis.0, basis.1 as u64),
        }
    }

    /// Make one call with its retries (HAR-24) and record its `chat` span. An
    /// attempt is abandoned at `opt.request_timeout_ms` or at the turn's
    /// deadline, whichever comes first.
    async fn call(
        &self,
        lin: &mut Lineage,
        ctx: Context,
        max_tokens: u64,
        deadline: Option<i64>,
        parent: &Cx,
    ) -> Result<Called, HarnessError> {
        let s = self.setup;
        let sampling = self.sampling(max_tokens);
        let body = serde_json::to_vec(&ctx.encode(
            s.backend.encoding(),
            &s.model,
            sampling,
            s.knobs.cache_breakpoint_placement,
        ))
        .map_err(|e| HarnessError::Internal(e.to_string()))?;
        let compared = self.compared(&ctx)?;
        let index = lin.call_index;
        lin.call_index += 1;
        let start = self.env.now();
        let request_timeout = int(s.opts.request_timeout_ms.saturating_mul(1_000_000));
        let (mut up, mut down, mut retries) = (0u64, 0u64, 0u64);
        let mut last: Exchange;
        // Every message of every attempt, for the link spans (EMU-36).
        let mut carried: Vec<LinkRecord> = Vec::new();
        let (mut reply, mut stop, mut error) = loop {
            let now = self.env.now();
            let (timeout_ns, by_deadline) = match deadline {
                Some(d) if d.saturating_sub(now) < request_timeout => {
                    (d.saturating_sub(now).max(0), true)
                }
                _ => (request_timeout, false),
            };
            last = self
                .env
                .exchange(s.backend.path(), body.clone(), sampling.stream, timeout_ns)
                .await;
            up += body.len() as u64;
            down += last.bytes_down;
            carried.extend(last.links.iter().copied());
            if s.backend != Backend::Mockllm && last.from_mock() {
                return Err(HarnessError::BackendMismatch(format!(
                    "a response to a `{}` run carries the mock's marker (CON-26, HAR-23)",
                    s.backend.as_str()
                )));
            }
            // `acn.call.error_class` is one of a few fixed classes; the detail
            // goes to the log (HAR-34: no provider text in the trace).
            let retryable: String = match &last.failure {
                Some(Failure::Timeout) => {
                    let class = if by_deadline { "deadline" } else { "timeout" };
                    break (None, Some("client_abort"), Some(class.to_owned()));
                }
                Some(Failure::Transport(e)) => {
                    tracing::warn!(call = index, "transport error: {e}");
                    "transport".into()
                }
                None if last.status == 200 => {
                    match assemble(s.backend.dialect(), &last, sampling.stream) {
                        Ok(r) => break (Some(r), None, None),
                        Err(AssembleError::Cut(e)) => {
                            tracing::warn!(call = index, "{e}");
                            "transport".into()
                        }
                        Err(AssembleError::Malformed(e)) => {
                            tracing::warn!(call = index, "malformed response: {e}");
                            break (None, Some("other"), Some(MALFORMED.into()));
                        }
                    }
                }
                None if last.status == 429 || last.status >= 500 => format!("http_{}", last.status),
                None => break (None, Some("other"), Some(format!("http_{}", last.status))),
            };
            if retries >= s.opts.max_retries {
                break (None, Some("transport_error"), Some(retryable));
            }
            let wait_ns = match last
                .header("retry-after")
                .and_then(|v| v.trim().parse::<u64>().ok())
            {
                Some(secs) => secs.min(MAX_RETRY_AFTER_S).saturating_mul(1_000_000_000),
                None => s
                    .opts
                    .retry_base_ms
                    .saturating_mul(1_000_000)
                    .saturating_mul(1u64 << retries.min(32)),
            };
            let now = self.env.now();
            self.env.sleep_until(now.saturating_add(int(wait_ns))).await;
            retries += 1;
        };
        let end = self.env.now().max(last.end_ns);

        // A response the frozen mapping cannot read is a malformed response
        // of this call, not the end of the run (ADR-17).
        let norm: Option<Normalised> = match &reply {
            Some(r) => match normalise::response(&s.inv, s.backend.as_str(), &r.raw) {
                Ok(n) => Some(n),
                Err(e) => {
                    tracing::warn!(call = index, "usage: {e}");
                    (reply, stop, error) = (None, Some("other"), Some(MALFORMED.into()));
                    None
                }
            },
            None => None,
        };
        let input_tokens = norm.as_ref().and_then(|n| n.input_tokens);
        let mut attrs = vec![
            KeyValue::new("gen_ai.operation.name", "chat"),
            KeyValue::new("gen_ai.provider.name", s.backend.as_str()),
            KeyValue::new("gen_ai.request.model", s.model.clone()),
            KeyValue::new("acn.call.index", index),
            KeyValue::new("acn.call.new_input_tokens_method", s.counting.method()),
            KeyValue::new("acn.call.wire_bytes_up", int(up)),
            KeyValue::new("acn.call.wire_bytes_down", int(down)),
            KeyValue::new("acn.call.streamed", sampling.stream),
            KeyValue::new("acn.call.retries", int(retries)),
        ];
        if let Some(r) = &reply {
            attrs.extend(raw_usage(s.backend.dialect(), &r.raw));
        }
        if let Some(n) = &norm {
            if let Some(v) = n.input_tokens {
                attrs.push(KeyValue::new("acn.call.input_tokens", int(v)));
                let shared = lin
                    .prev
                    .as_deref()
                    .map_or(0, |p| self.shared_tokens(p, &compared, (v, compared.len())));
                attrs.push(KeyValue::new(
                    "acn.call.new_input_tokens",
                    int(v.saturating_sub(shared.min(v))),
                ));
            }
            if let Some(v) = n.output_tokens {
                attrs.push(KeyValue::new("acn.call.output_tokens", int(v)));
            }
            if let Some(v) = n.cache_read_tokens {
                attrs.push(KeyValue::new("acn.cache.read_tokens", int(v)));
            }
            if let Some(v) = n.cache_write_tokens {
                attrs.push(KeyValue::new("acn.cache.write_tokens", int(v)));
            }
            if let Some(v) = n.stop_reason {
                attrs.push(KeyValue::new("acn.call.stop_reason", v.as_str()));
            }
            if let Some(v) = &n.stop_reason_raw {
                attrs.push(KeyValue::new("acn.call.stop_reason_raw", v.clone()));
            }
            lin.last_uncached = match (n.input_tokens, n.cache_read_tokens) {
                (Some(i), Some(c)) => Some(i.saturating_sub(c)),
                (Some(i), None) => Some(i),
                _ => None,
            };
        } else {
            lin.last_uncached = None;
        }
        if let Some(st) = stop {
            attrs.push(KeyValue::new("acn.call.stop_reason", st));
        }
        if let Some(e) = &error {
            attrs.push(KeyValue::new("acn.call.error_class", e.clone()));
        }
        let times = reply
            .as_ref()
            .map(|r| r.token_times.clone())
            .unwrap_or_default();
        if let Some(first) = times.first() {
            attrs.push(KeyValue::new("acn.call.ttft_ms", ms(first - start)));
        }
        let gaps: Vec<i64> = times.windows(2).map(|w| w[1] - w[0]).collect();
        if sampling.stream && !gaps.is_empty() {
            attrs.push(KeyValue::new(
                "acn.call.itl_p50_ms",
                ms(quantile(&gaps, 50)),
            ));
            attrs.push(KeyValue::new(
                "acn.call.itl_p99_ms",
                ms(quantile(&gaps, 99)),
            ));
        }
        let mut span = self
            .tracer
            .span_builder("chat")
            .with_kind(SpanKind::Client)
            .with_start_time(at(start))
            .with_attributes(attrs)
            .start_with_context(self.tracer, parent);
        if let (Some(first), Some(last_t)) = (times.first(), times.last()) {
            span.add_event_with_timestamp("acn.stream.first_token", at(*first), vec![]);
            if sampling.stream {
                let threshold = s.opts.stall_threshold_ms;
                for (k, w) in times.windows(2).enumerate() {
                    let gap = ms(w[1] - w[0]);
                    if gap > threshold {
                        span.add_event_with_timestamp(
                            "acn.stream.stall",
                            at(w[1]),
                            vec![
                                KeyValue::new("gap_ms", gap),
                                KeyValue::new("tokens_before", int(k as u64 + 1)),
                            ],
                        );
                    }
                }
            }
            span.add_event_with_timestamp("acn.stream.last_token", at(*last_t), vec![]);
        }
        let chat_cx = parent.with_span(span);
        if let Some(net) = &self.net {
            self.link_spans(net, &carried, &chat_cx);
        }
        chat_cx.span().end_with_timestamp(at(end));

        // The next call of this lineage is compared with this context followed by
        // the message its response became (TRC-12).
        let mut after = ctx.clone();
        if let Some(r) = &reply {
            after.messages.push(assistant(r));
        }
        lin.prev = Some(self.compared(&after)?);
        Ok(Called {
            reply,
            index,
            input_tokens,
            context: ctx,
            compared,
        })
    }

    /// One `acn.link` span per message an attempt carried (SPEC 020 EMU-36),
    /// under the call's `chat` span, from the `acn-emu` resource, each linked
    /// to the run's `acn.scenario` span.
    fn link_spans(&self, net: &NetSpans<'_>, carried: &[LinkRecord], chat: &Cx) {
        for r in carried {
            let f = r.fate;
            let dropped = f.outcome.is_err();
            let dequeue = r.received_ns.unwrap_or(f.send_ns);
            let applied = if dropped { 0 } else { f.hold_ns + f.delay_ns };
            let (direction, model) = match r.direction {
                acn_emu::link::Direction::Up => ("up", net.up_model.clone()),
                acn_emu::link::Direction::Down => ("down", net.down_model.clone()),
            };
            let mut span = net
                .tracer
                .span_builder("acn.link")
                .with_kind(SpanKind::Internal)
                .with_start_time(at(f.send_ns))
                .with_links(vec![opentelemetry::trace::Link::with_context(
                    net.scenario.clone(),
                )])
                .with_attributes(vec![
                    KeyValue::new("acn.link.id", net.link_id.clone()),
                    KeyValue::new("acn.link.direction", direction),
                    KeyValue::new("acn.link.model", model),
                    KeyValue::new("acn.link.bytes", int(r.bytes)),
                    KeyValue::new("acn.link.enqueue_ns", f.send_ns),
                    KeyValue::new("acn.link.dequeue_ns", dequeue),
                    KeyValue::new("acn.link.applied_delay_ms", ms(applied)),
                    KeyValue::new("acn.link.rate_limited_ms", ms(f.rate_wait_ns)),
                    KeyValue::new("acn.link.dropped", dropped),
                    KeyValue::new("acn.link.reordered", f.reordered),
                ])
                .start_with_context(net.tracer, chat);
            span.end_with_timestamp(at(dequeue));
        }
    }

    /// Run one simulated tool (HAR-2) and record its span.
    async fn run_tool(
        &self,
        call: &ToolCall,
        lin: &Lineage,
        requesting: i64,
        parent: &Cx,
    ) -> Result<String, HarnessError> {
        // A forked child executes only its own tools (HAR-14).
        let available = lin.allowed.as_ref().unwrap_or(&lin.tools);
        let start = self.env.now();
        let tool = self
            .setup
            .workload
            .tool(&call.name)
            .filter(|t| available.contains(&t.name) && !t.is_subagent());
        let (class, content) = match tool {
            Some(t) => {
                // Length, then duration, then the bytes: one fixed order (HAR-2).
                let (dur, content) = {
                    let mut st = self.streams_of(lin).borrow_mut();
                    let r = t.result_bytes.unwrap_or(Range { min: 0, max: 0 });
                    let d = t.duration_ns.unwrap_or(Range { min: 0, max: 0 });
                    let len = draw(&mut st.tools, r);
                    let dur = draw(&mut st.tools, d);
                    (dur, filler(&mut st.tools, len))
                };
                self.env.sleep_until(start.saturating_add(int(dur))).await;
                (t.class.clone(), content)
            }
            None => (
                "other".to_owned(),
                format!("error: tool '{}' is not available here", call.name),
            ),
        };
        let mut span = self
            .tracer
            .span_builder("execute_tool")
            .with_kind(SpanKind::Internal)
            .with_start_time(at(start))
            .with_attributes(vec![
                KeyValue::new("gen_ai.tool.name", call.name.clone()),
                KeyValue::new("gen_ai.tool.call.id", call.id.clone()),
                KeyValue::new("acn.tool.class", class),
                KeyValue::new("acn.tool.result_bytes", int(content.len() as u64)),
                KeyValue::new("acn.tool.placement", "local"),
                KeyValue::new("acn.tool.requesting_call", requesting),
            ])
            .start_with_context(self.tracer, parent);
        span.end_with_timestamp(at(self.env.now()));
        Ok(content)
    }

    /// One session: one task of the workload (HAR-1).
    pub async fn session(&self, task_index: usize, seed: i64) -> Result<(), HarnessError> {
        let s = self.setup;
        let task = &s.workload.tasks[task_index];
        let mut attrs = s.session_attrs.clone();
        attrs.push(KeyValue::new("acn.seed", seed));
        attrs.push(KeyValue::new("acn.replicate", i64::from(self.replicate)));
        let session = self
            .tracer
            .span_builder("acn.session")
            .with_kind(SpanKind::Internal)
            .with_start_time(at(self.env.now()))
            .with_attributes(attrs)
            .start(self.tracer);
        let session_cx = Cx::new().with_span(session);
        let mut lin = Lineage {
            system_base: s.workload.agent.system_prompt.clone(),
            tools: task.tools.clone(),
            messages: Vec::new(),
            turn_start: 0,
            prev: None,
            last_uncached: None,
            call_index: 0,
            first: None,
            allowed: None,
            task: task_index,
            own: None,
        };
        let mut ordinals: Vec<String> = Vec::new();
        for (k, turn) in task.turns.iter().enumerate() {
            if k > 0 {
                let think = draw(&mut self.streams.borrow_mut().workload, turn.think_time_ns);
                let now = self.env.now();
                self.env.sleep_until(now.saturating_add(int(think))).await;
            }
            self.turn(&mut lin, k, turn, &mut ordinals, &session_cx)
                .await?;
        }
        session_cx.span().end_with_timestamp(at(self.env.now()));
        Ok(())
    }

    /// One turn of the main lineage (HAR-1, HAR-3, HAR-4, HAR-13).
    async fn turn(
        &self,
        lin: &mut Lineage,
        k: usize,
        turn: &Turn,
        ordinals: &mut Vec<String>,
        session_cx: &Cx,
    ) -> Result<(), HarnessError> {
        let s = self.setup;
        let start = self.env.now();
        lin.turn_start = start;
        lin.call_index = 0;
        let turn_span = self
            .tracer
            .span_builder("acn.turn")
            .with_kind(SpanKind::Internal)
            .with_start_time(at(start))
            .start_with_context(self.tracer, session_cx);
        let cx = session_cx.with_span(turn_span);

        // HAR-13: earlier results that changed.
        let mut prefix = String::new();
        for &ord in &turn.updates {
            let Some(tool) = usize::try_from(ord).ok().and_then(|i| ordinals.get(i)) else {
                continue;
            };
            let range = self
                .tool(tool)?
                .result_bytes
                .unwrap_or(Range { min: 0, max: 0 });
            let content = {
                let mut st = self.streams.borrow_mut();
                let len = draw(&mut st.tools, range);
                filler(&mut st.tools, len)
            };
            match s.knobs.backfill_mode {
                Backfill::MidPrefix => {
                    for m in &mut lin.messages {
                        if let Msg::ToolResult {
                            ordinal: Some(o),
                            content: c,
                            ..
                        } = m
                            && *o == ord
                        {
                            c.clone_from(&content);
                        }
                    }
                }
                Backfill::TailRestate => {
                    prefix.push_str(&format!("Updated result {ord}:\n{content}\n"));
                }
            }
        }
        let turn_first = lin.messages.len();
        lin.messages.push(Msg::User {
            text: format!("{prefix}{}", turn.user),
        });

        let deadline = turn
            .deadline_ms
            .map(|d| start.saturating_add(int(d.saturating_mul(1_000_000))));
        let mut calls = 0u64;
        let mut compaction: Option<Compaction> = None;
        let mut called = BTreeSet::new();
        let mut useful: Option<i64> = None;
        let mut turn_first = turn_first;
        let outcome = loop {
            if calls >= s.workload.agent.max_calls_per_turn {
                break TurnOutcome::Aborted;
            }
            if deadline.is_some_and(|d| self.env.now() >= d) {
                break TurnOutcome::Timeout;
            }
            // HAR-4, HAR-15: compaction before the call, at most once per call.
            let ctx = self.context(lin)?;
            if self.should_compact(lin, &ctx) {
                let mut cctx = ctx.clone();
                cctx.messages.push(Msg::User {
                    text: s.workload.agent.summary_instruction.clone(),
                });
                // HAR-4: the same tools, so the same prefix; no tool call, so a
                // text summary.
                // `encode` drops it when the context has no tools.
                cctx.tool_choice = Some(ToolChoice::Forbid);
                let c = self
                    .call(
                        lin,
                        cctx,
                        s.workload.agent.summary_max_tokens,
                        deadline,
                        &cx,
                    )
                    .await?;
                let Some(r) = c.reply else {
                    break TurnOutcome::Aborted;
                };
                let summary = if r.tool_calls.is_empty() {
                    r.text.unwrap_or_default()
                } else {
                    String::new()
                };
                let current: Vec<Msg> = lin.messages[turn_first..].to_vec();
                lin.messages = vec![Msg::User {
                    text: format!("Summary of earlier conversation:\n{summary}"),
                }];
                lin.messages.extend(current);
                turn_first = 1;
                lin.last_uncached = None;
                compaction = Some(s.knobs.compaction_trigger);
            }
            let ctx = self.context(lin)?;
            let c = self
                .call(lin, ctx, s.workload.agent.max_tokens, deadline, &cx)
                .await?;
            calls += 1;
            // HAR-1: an answer that arrives after the deadline is a timeout.
            if deadline.is_some_and(|d| self.env.now() >= d) {
                if let Some(r) = &c.reply {
                    lin.messages.push(assistant(r));
                }
                break TurnOutcome::Timeout;
            }
            let Some(reply) = c.reply.clone() else {
                break TurnOutcome::Aborted;
            };
            lin.messages.push(assistant(&reply));
            if reply.tool_calls.is_empty() {
                useful = reply.token_times.first().map(|t| t - start);
                let ok = turn.expect_tools.iter().all(|t| called.contains(t));
                break if ok {
                    TurnOutcome::Success
                } else {
                    TurnOutcome::Failure
                };
            }
            for tc in &reply.tool_calls {
                // `turn` runs the main lineage only, which has no `allowed` set:
                // its tools are what it may call. A forked child runs in `child`,
                // where `run_tool` checks `allowed` (HAR-14).
                // The checker counts only tools the lineage has (HAR-3).
                if lin.tools.contains(&tc.name) {
                    called.insert(tc.name.clone());
                }
                let spawn = self
                    .setup
                    .workload
                    .tool(&tc.name)
                    .filter(|t| t.is_subagent() && lin.tools.contains(&t.name));
                let content = match spawn {
                    Some(t) => self.fan_out(t, tc, &c, k, lin, &cx).await?,
                    None => self.run_tool(tc, lin, c.index, &cx).await?,
                };
                let ordinal = ordinals.len() as u64;
                ordinals.push(tc.name.clone());
                lin.messages.push(Msg::ToolResult {
                    call_id: tc.id.clone(),
                    tool: tc.name.clone(),
                    ordinal: Some(ordinal),
                    content,
                });
            }
            if deadline.is_some_and(|d| self.env.now() >= d) {
                break TurnOutcome::Timeout;
            }
        };
        let span = cx.span();
        span.set_attribute(KeyValue::new("acn.turn.index", int(k as u64)));
        span.set_attribute(KeyValue::new("acn.turn.outcome", outcome.as_str()));
        span.set_attribute(KeyValue::new(
            "acn.turn.compaction",
            compaction.map_or("none", Compaction::as_str),
        ));
        if let Some(d) = turn.deadline_ms {
            #[allow(clippy::cast_precision_loss)]
            span.set_attribute(KeyValue::new("acn.turn.deadline_ms", d as f64));
        }
        if outcome == TurnOutcome::Success
            && let Some(u) = useful
        {
            span.set_attribute(KeyValue::new("acn.turn.first_useful_result_ms", ms(u)));
        }
        span.end_with_timestamp(at(self.env.now()));
        Ok(())
    }

    /// HAR-15.
    fn should_compact(&self, lin: &Lineage, ctx: &Context) -> bool {
        let a = &self.setup.workload.agent;
        match self.setup.knobs.compaction_trigger {
            Compaction::WindowFull => {
                (ctx.canonical_bytes().len() as u64).div_ceil(BYTES_PER_TOKEN)
                    >= a.compact_at_tokens
            }
            Compaction::ReadCostThreshold => lin
                .last_uncached
                .is_some_and(|u| u > a.read_cost_threshold_tokens),
        }
    }

    /// HAR-5, HAR-14: spawn a subagent tool's children and gather their answers.
    async fn fan_out(
        &self,
        tool: &Tool,
        tc: &ToolCall,
        spawning: &Called,
        turn_index: usize,
        parent: &Lineage,
        turn_cx: &Cx,
    ) -> Result<String, HarnessError> {
        let (Some(width), Some(child)) = (tool.width, &tool.child) else {
            return Err(HarnessError::Workload(format!(
                "tool `{}` has no width or child",
                tool.name
            )));
        };
        let start = self.env.now();
        let tool_span = self
            .tracer
            .span_builder("execute_tool")
            .with_kind(SpanKind::Internal)
            .with_start_time(at(start))
            .with_attributes(vec![
                KeyValue::new("gen_ai.tool.name", tool.name.clone()),
                KeyValue::new("gen_ai.tool.call.id", tc.id.clone()),
                KeyValue::new("acn.tool.class", "subagent"),
                KeyValue::new("acn.tool.placement", "local"),
                KeyValue::new("acn.tool.requesting_call", spawning.index),
            ])
            .start_with_context(self.tracer, turn_cx);
        let tool_cx = turn_cx.with_span(tool_span);
        let mut children = Vec::new();
        for i in 0..width {
            // Each child draws from its own streams (ADR-17).
            let scope = format!("{}.{turn_index}.{}.{i}", parent.task, spawning.index);
            let own = Some(std::rc::Rc::new(RefCell::new(Streams::scoped(
                self.seed, &scope,
            )?)));
            let instruction = format!("{}\n(child {} of {width})", child.instruction, i + 1);
            let mut lin = match self.setup.knobs.fanout_prompting {
                Fanout::ForkFromPrefix => {
                    // HAR-14: the parent's tools, but only the child's to call.
                    let line = if child.tools.is_empty() {
                        "Use no tools.".to_owned()
                    } else {
                        format!("Use only these tools: {}.", child.tools.join(", "))
                    };
                    let mut first = spawning.context.clone();
                    first.messages.push(Msg::User {
                        text: format!("{instruction}\n{line}"),
                    });
                    first.tool_choice = Some(choice_of(&child.tools));
                    Lineage {
                        system_base: parent.system_base.clone(),
                        tools: parent.tools.clone(),
                        messages: first.messages.clone(),
                        turn_start: parent.turn_start,
                        prev: None,
                        last_uncached: None,
                        call_index: 0,
                        first: Some(first),
                        allowed: Some(child.tools.clone()),
                        task: parent.task,
                        own,
                    }
                }
                Fanout::PerChild => Lineage {
                    system_base: child.system_prompt.clone(),
                    tools: child.tools.clone(),
                    messages: vec![Msg::User { text: instruction }],
                    turn_start: parent.turn_start,
                    prev: None,
                    last_uncached: None,
                    call_index: 0,
                    first: None,
                    allowed: None,
                    task: parent.task,
                    own,
                },
            };
            // TRC-12: a sub-agent's first call is compared with the spawning context.
            lin.prev = Some(spawning.compared.clone());
            let first_ctx = match lin.first.take() {
                Some(f) => f,
                None => self.context(&lin)?,
            };
            // Scaled by the spawning request's own tokens per byte; with no
            // count from it, whole MLM-11 tokens (ADR-17).
            let child_bytes = self.compared(&first_ctx)?;
            let shared = match spawning.input_tokens {
                Some(t) => self.shared_tokens(
                    &spawning.compared,
                    &child_bytes,
                    (t, spawning.compared.len()),
                ),
                None => common_prefix(&spawning.compared, &child_bytes) as u64 / BYTES_PER_TOKEN,
            };
            let span = self
                .tracer
                .span_builder("invoke_agent")
                .with_kind(SpanKind::Internal)
                .with_start_time(at(start))
                .with_attributes(vec![
                    KeyValue::new("gen_ai.operation.name", "invoke_agent"),
                    KeyValue::new("acn.fanout.parent_turn", int(turn_index as u64)),
                    KeyValue::new("acn.fanout.parent_call", spawning.index),
                    KeyValue::new("acn.fanout.depth", 1i64),
                    KeyValue::new("acn.fanout.width", int(width)),
                    KeyValue::new("acn.fanout.shared_prefix_tokens", int(shared)),
                ])
                .start_with_context(self.tracer, &tool_cx);
            children.push((lin, first_ctx, tool_cx.with_span(span)));
        }
        let answers = join_all(
            children
                .into_iter()
                .map(|(lin, first, cx)| self.child(lin, first, child.max_calls, cx))
                .collect(),
        )
        .await;
        let mut texts = Vec::with_capacity(answers.len());
        for a in answers {
            texts.push(a?);
        }
        let result = texts.join("\n");
        let span = tool_cx.span();
        span.set_attribute(KeyValue::new(
            "acn.tool.result_bytes",
            int(result.len() as u64),
        ));
        span.end_with_timestamp(at(self.env.now()));
        Ok(result)
    }

    /// One sub-agent: the loop of HAR-1 for one turn, with no compaction and no
    /// spawning (HAR-5). Its answer is its last response's text.
    async fn child(
        &self,
        mut lin: Lineage,
        first: Context,
        max_calls: u64,
        cx: Cx,
    ) -> Result<String, HarnessError> {
        let mut ctx = first;
        let mut answer = String::new();
        for _ in 0..max_calls {
            let c = self
                .call(
                    &mut lin,
                    ctx,
                    self.setup.workload.agent.max_tokens,
                    None,
                    &cx,
                )
                .await?;
            let Some(reply) = c.reply else {
                break;
            };
            lin.messages.push(assistant(&reply));
            if reply.tool_calls.is_empty() {
                answer = reply.text.unwrap_or_default();
                break;
            }
            for tc in &reply.tool_calls {
                let content = self.run_tool(tc, &lin, c.index, &cx).await?;
                lin.messages.push(Msg::ToolResult {
                    call_id: tc.id.clone(),
                    tool: tc.name.clone(),
                    ordinal: None,
                    content,
                });
            }
            ctx = self.context(&lin)?;
        }
        cx.span().end_with_timestamp(at(self.env.now()));
        Ok(answer)
    }
}

/// The message a response becomes.
fn assistant(r: &Reply) -> Msg {
    Msg::Assistant {
        text: r.text.clone(),
        tool_calls: r.tool_calls.clone(),
    }
}

/// `len` printable bytes from the tool stream (HAR-2).
fn filler(rng: &mut ChaCha20Rng, len: u64) -> String {
    (0..len)
        .map(|_| {
            let i = usize::try_from(below(rng, ALPHABET.len() as u64)).unwrap_or(0);
            char::from(ALPHABET[i])
        })
        .collect()
}

/// The nearest-rank `p`th percentile of `v` (non-empty).
fn quantile(v: &[i64], p: u64) -> i64 {
    let mut s = v.to_vec();
    s.sort_unstable();
    let n = s.len() as u64;
    let rank = (p * n).div_ceil(100).max(1);
    s[usize::try_from(rank - 1).unwrap_or(0).min(s.len() - 1)]
}

/// The provider's own usage fields as GenAI attributes (HAR-31).
fn raw_usage(dialect: Dialect, raw: &Value) -> Vec<KeyValue> {
    let u = &raw["usage"];
    let field = |p: &str| u.pointer(p).and_then(Value::as_i64);
    let (input, output, read, write) = match dialect {
        Dialect::ChatCompletions => (
            field("/prompt_tokens"),
            field("/completion_tokens"),
            field("/prompt_tokens_details/cached_tokens"),
            field("/prompt_tokens_details/cache_write_tokens"),
        ),
        Dialect::Messages => (
            field("/input_tokens"),
            field("/output_tokens"),
            field("/cache_read_input_tokens"),
            field("/cache_creation_input_tokens"),
        ),
    };
    let mut out = Vec::new();
    for (k, v) in [
        ("gen_ai.usage.input_tokens", input),
        ("gen_ai.usage.output_tokens", output),
        ("gen_ai.usage.cache_read.input_tokens", read),
        ("gen_ai.usage.cache_creation.input_tokens", write),
    ] {
        if let Some(v) = v {
            out.push(KeyValue::new(k, v));
        }
    }
    out
}
