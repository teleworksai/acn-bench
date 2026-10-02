//! The ingester (TRC-30): the derived views of SPEC 010 §7, computed from a run's
//! spans, events, links and resources and from nothing else (TRC-35). It is the
//! only code that reads convention attributes (`gen_ai.*`); everything downstream
//! reads views or promoted `acn.*` columns.
//!
//! Deterministic by construction: rows follow the stored order of their spans,
//! `_ms` attributes become nanoseconds by round-half-even wherever they are read, every
//! aggregate is an integer sum, and no reduction is parallel. The writer is checked
//! against `views.toml` (TRC-37): every declared column must be produced, with its
//! type and nullability, and nothing else. The readings the spec leaves open — the
//! critical path in particular — are recorded in ADR-14.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arrow_array::builder::{
    BooleanBuilder, FixedSizeBinaryBuilder, Float64Builder, Int64Builder, MapBuilder, StringBuilder,
};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Fields, Schema};

use crate::model::{AttrValue, SpanRow, Trace};
use crate::schema::{self, Inventory, View, Views};

/// A trace the views cannot be derived from.
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("{0}")]
    Invalid(String),
    #[error("arrow: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error(transparent)]
    Schema(#[from] schema::SchemaError),
}

type Result<T> = std::result::Result<T, IngestError>;

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(IngestError::Invalid(message.into()))
}

type SpanId = [u8; 8];

/// One cell of a view row, before it is typed against `views.toml`.
#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Utf8(Option<String>),
    Int(Option<i64>),
    Float(Option<f64>),
    Bool(Option<bool>),
    Id(Option<SpanId>),
    Map(Vec<(String, i64)>),
}

type Row = BTreeMap<&'static str, Cell>;

/// A `_ms` float as integer nanoseconds: round-half-even of `v × 1e6` (ADR-12).
/// A negative duration is refused: every `_ms` attribute is a time span or a delay.
pub fn ms_to_ns(v: f64) -> Result<i64> {
    if v < 0.0 {
        return invalid(format!("{v} ms is negative; a duration never is"));
    }
    let x = (v * 1e6).round_ties_even();
    // i64::MAX as f64 rounds up to 2^63, so the bound is exclusive.
    #[allow(clippy::cast_precision_loss)]
    let limit = i64::MAX as f64;
    if !x.is_finite() || x >= limit || x < -limit {
        return invalid(format!("{v} ms does not fit Int64 nanoseconds"));
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(x as i64)
}

/// The deepest span tree the ingester accepts. Real nesting is a handful of
/// levels (session, turn, sub-agents); the bound keeps a corrupt or crafted
/// bundle from exhausting the stack in the critical-path walk (ADR-14).
pub const MAX_DEPTH: usize = 256;

/// The work spans a critical path is made of (ADR-14).
fn is_work(name: &str) -> bool {
    matches!(name, "chat" | "execute_tool" | "invoke_agent")
}

/// The span each event may sit on (TRC-12, TRC-15).
fn event_owner(event: &str) -> Option<&'static str> {
    match event {
        "acn.stream.first_token" | "acn.stream.last_token" | "acn.stream.stall" => Some("chat"),
        "acn.scenario.step" | "acn.scenario.outage" => Some("acn.scenario"),
        _ => None,
    }
}

/// The tie order of work spans on the critical path: chats first.
fn work_rank(name: &str) -> u8 {
    match name {
        "chat" => 0,
        "execute_tool" => 1,
        _ => 2,
    }
}

fn count(n: usize) -> Result<i64> {
    i64::try_from(n).or_else(|_| invalid("a count does not fit Int64"))
}

fn sid(s: &SpanRow) -> String {
    s.span_id.iter().map(|b| format!("{b:02x}")).collect()
}

/// Indexed access to a trace's spans and their tree.
struct Index<'a> {
    by_id: BTreeMap<([u8; 16], SpanId), usize>,
    children: BTreeMap<usize, Vec<usize>>,
    events: BTreeMap<([u8; 16], SpanId), Vec<usize>>,
    trace: &'a Trace,
}

impl<'a> Index<'a> {
    /// Index a trace, refusing what would make a view silently wrong or hang:
    /// a duplicate id, a missing parent, a span that ends before it starts, a
    /// parent cycle, nesting deeper than [`MAX_DEPTH`], and an event that belongs
    /// to no span or to a span of the wrong kind.
    fn new(trace: &'a Trace) -> Result<Self> {
        let spans = &trace.spans;
        let mut by_id = BTreeMap::new();
        for (i, s) in spans.iter().enumerate() {
            if s.end_ns < s.start_ns {
                return invalid(format!("`{}` {} ends before it starts", s.name, sid(s)));
            }
            if by_id.insert((s.trace_id, s.span_id), i).is_some() {
                return invalid(format!("span id {} appears twice", sid(s)));
            }
        }
        let mut parent = vec![None; spans.len()];
        let mut children: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (i, s) in spans.iter().enumerate() {
            if let Some(p) = s.parent_span_id {
                let Some(&pi) = by_id.get(&(s.trace_id, p)) else {
                    return invalid(format!(
                        "`{}` {} names a parent the trace does not hold",
                        s.name,
                        sid(s)
                    ));
                };
                parent[i] = Some(pi);
                children.entry(pi).or_default().push(i);
            }
        }
        // Depth of every span, iteratively: a cycle or an over-deep tree is an
        // error, never a hang or a stack overflow.
        let mut depth: Vec<Option<usize>> = vec![None; spans.len()];
        for (start, first) in spans.iter().enumerate() {
            let mut path = Vec::new();
            let mut cur = Some(start);
            let base = loop {
                match cur {
                    None => break 0,
                    Some(n) if depth[n].is_some() => break depth[n].unwrap_or(0) + 1,
                    Some(n) => {
                        if path.len() > MAX_DEPTH || path.contains(&n) {
                            return invalid(format!(
                                "the parents of `{}` {} form a cycle or nest deeper than {MAX_DEPTH}",
                                first.name,
                                sid(first)
                            ));
                        }
                        path.push(n);
                        cur = parent[n];
                    }
                }
            };
            for (k, &n) in path.iter().rev().enumerate() {
                if base + k > MAX_DEPTH {
                    return invalid(format!(
                        "`{}` {} nests deeper than {MAX_DEPTH} spans",
                        spans[n].name,
                        sid(&spans[n])
                    ));
                }
                depth[n] = Some(base + k);
            }
        }
        let mut events: BTreeMap<_, Vec<usize>> = BTreeMap::new();
        for (i, e) in trace.events.iter().enumerate() {
            let Some(&owner) = by_id.get(&(e.trace_id, e.span_id)) else {
                return invalid(format!(
                    "event `{}` belongs to no span of the trace",
                    e.name
                ));
            };
            if let Some(kind) = event_owner(&e.name)
                && spans[owner].name != kind
            {
                return invalid(format!(
                    "event `{}` sits on `{}` {}, not on a `{kind}`",
                    e.name,
                    spans[owner].name,
                    sid(&spans[owner])
                ));
            }
            events.entry((e.trace_id, e.span_id)).or_default().push(i);
        }
        Ok(Self {
            by_id,
            children,
            events,
            trace,
        })
    }

    fn span(&self, i: usize) -> &'a SpanRow {
        &self.trace.spans[i]
    }

    fn parent(&self, i: usize) -> Option<usize> {
        let s = self.span(i);
        s.parent_span_id
            .and_then(|p| self.by_id.get(&(s.trace_id, p)).copied())
    }

    fn children(&self, i: usize) -> &[usize] {
        self.children.get(&i).map_or(&[], Vec::as_slice)
    }

    /// Every descendant of `i`, in stored order.
    fn descendants(&self, i: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![i];
        while let Some(n) = stack.pop() {
            for &c in self.children(n) {
                out.push(c);
                stack.push(c);
            }
        }
        out.sort_unstable();
        out
    }

    /// The nearest ancestor of `i` named `name`.
    fn ancestor(&self, i: usize, name: &str) -> Option<usize> {
        let mut cur = self.parent(i);
        while let Some(p) = cur {
            if self.span(p).name == name {
                return Some(p);
            }
            cur = self.parent(p);
        }
        None
    }

    /// The enclosing `invoke_agent` below the turn: the call's context lineage.
    fn lineage(&self, i: usize) -> Option<usize> {
        let mut cur = self.parent(i);
        while let Some(p) = cur {
            match self.span(p).name.as_str() {
                "invoke_agent" => return Some(p),
                "acn.turn" | "acn.session" => return None,
                _ => cur = self.parent(p),
            }
        }
        None
    }

    fn events_of(&self, i: usize, name: &str) -> Vec<&'a crate::model::EventRow> {
        let s = self.span(i);
        self.events
            .get(&(s.trace_id, s.span_id))
            .map(|v| {
                v.iter()
                    .map(|&e| &self.trace.events[e])
                    .filter(|e| e.name == name)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn named(&self, name: &str) -> Vec<usize> {
        (0..self.trace.spans.len())
            .filter(|&i| self.trace.spans[i].name == name)
            .collect()
    }
}

// ---- attribute access: a required attribute that is missing is an error ----

fn get<'a>(s: &'a SpanRow, key: &str) -> Option<&'a AttrValue> {
    s.attrs.get(key)
}

fn opt_str(s: &SpanRow, key: &str) -> Result<Option<String>> {
    match get(s, key) {
        None => Ok(None),
        Some(AttrValue::String(v)) => Ok(Some(v.clone())),
        Some(v) => invalid(format!("`{}`: `{key}` is {v:?}, not a string", s.name)),
    }
}

fn opt_int(s: &SpanRow, key: &str) -> Result<Option<i64>> {
    match get(s, key) {
        None => Ok(None),
        Some(AttrValue::Int(v)) => Ok(Some(*v)),
        Some(v) => invalid(format!("`{}`: `{key}` is {v:?}, not an int", s.name)),
    }
}

fn opt_bool(s: &SpanRow, key: &str) -> Result<Option<bool>> {
    match get(s, key) {
        None => Ok(None),
        Some(AttrValue::Bool(v)) => Ok(Some(*v)),
        Some(v) => invalid(format!("`{}`: `{key}` is {v:?}, not a bool", s.name)),
    }
}

fn opt_ms(s: &SpanRow, key: &str) -> Result<Option<i64>> {
    match get(s, key) {
        None => Ok(None),
        Some(AttrValue::Float(v)) => ms_to_ns(*v).map(Some),
        Some(v) => invalid(format!("`{}`: `{key}` is {v:?}, not a float", s.name)),
    }
}

fn need<T>(s: &SpanRow, key: &str, v: Option<T>) -> Result<T> {
    v.map_or_else(
        || {
            invalid(format!(
                "`{}` {:02x?} lacks the required `{key}`",
                s.name, s.span_id
            ))
        },
        Ok,
    )
}

fn req_str(s: &SpanRow, key: &str) -> Result<String> {
    need(s, key, opt_str(s, key)?)
}
fn req_int(s: &SpanRow, key: &str) -> Result<i64> {
    need(s, key, opt_int(s, key)?)
}
fn req_bool(s: &SpanRow, key: &str) -> Result<bool> {
    need(s, key, opt_bool(s, key)?)
}
fn req_ms(s: &SpanRow, key: &str) -> Result<i64> {
    need(s, key, opt_ms(s, key)?)
}

fn dur(s: &SpanRow) -> Result<i64> {
    s.end_ns
        .checked_sub(s.start_ns)
        .ok_or_else(|| IngestError::Invalid(format!("`{}`: duration overflows", s.name)))
}

fn add(a: i64, b: i64) -> Result<i64> {
    a.checked_add(b)
        .ok_or_else(|| IngestError::Invalid("a view sum overflows Int64".into()))
}

/// A sum that is null when any term is absent (ADR-12: partial sums never pass
/// for totals).
fn sum_all(values: impl IntoIterator<Item = Option<i64>>) -> Result<Option<i64>> {
    let mut total = 0i64;
    for v in values {
        match v {
            None => return Ok(None),
            Some(x) => total = add(total, x)?,
        }
    }
    Ok(Some(total))
}

/// The critical path below `container` (ADR-14): among its chat, execute_tool and
/// invoke_agent children, start from the one that ends last and step back to the
/// child that ends latest at or before the current one's start, ties broken by the
/// lowest span id. A child that has such children of its own (a sub-agent, or a
/// tool that spawned one) is replaced by its own critical path. The leaves are the
/// chats and tools whose time the turn waited on, in time order.
fn critical_path(ix: &Index<'_>, container: usize) -> Vec<usize> {
    let work: Vec<usize> = ix
        .children(container)
        .iter()
        .copied()
        .filter(|&c| is_work(&ix.span(c).name))
        .collect();
    let latest = |bound: Option<i64>, taken: &BTreeSet<usize>| {
        work.iter()
            .copied()
            .filter(|c| !taken.contains(c))
            .filter(|&c| bound.is_none_or(|b| ix.span(c).end_ns <= b))
            .max_by(|&a, &b| {
                // Latest end wins. On a tie, the span that started earlier (the
                // longer wait), then a chat over a tool over a sub-agent; the
                // span id, which is random, decides only between spans that are
                // identical in time and kind (ADR-14).
                let (sa, sb) = (ix.span(a), ix.span(b));
                sa.end_ns
                    .cmp(&sb.end_ns)
                    .then(sb.start_ns.cmp(&sa.start_ns))
                    .then(work_rank(&sb.name).cmp(&work_rank(&sa.name)))
                    .then(sb.span_id.cmp(&sa.span_id))
            })
    };
    // A span already on the path is never a candidate again: a zero-length span
    // ends at its own start and would otherwise be its own predecessor, cutting
    // off the spans before it.
    let mut chain = Vec::new();
    let mut taken = BTreeSet::new();
    let mut cur = latest(None, &taken);
    while let Some(c) = cur {
        chain.push(c);
        taken.insert(c);
        cur = latest(Some(ix.span(c).start_ns), &taken);
    }
    chain.reverse();
    let mut leaves = Vec::new();
    for c in chain {
        let nested = ix.children(c).iter().any(|&g| is_work(&ix.span(g).name));
        if nested {
            leaves.extend(critical_path(ix, c));
        } else if ix.span(c).name != "invoke_agent" {
            leaves.push(c);
        }
    }
    leaves
}

/// Applied delay plus rate-limited time of the `acn.link` spans that carry a call's
/// traffic (TRC-32's network wait, per call).
fn link_wait(ix: &Index<'_>, call: usize) -> Result<i64> {
    let mut total = 0i64;
    for &l in ix.children(call) {
        let s = ix.span(l);
        if s.name == "acn.link" {
            total = add(total, req_ms(s, "acn.link.applied_delay_ms")?)?;
            total = add(total, req_ms(s, "acn.link.rate_limited_ms")?)?;
        }
    }
    Ok(total)
}

/// The run id, which every session must agree on (TRC-35: from the spans alone).
fn run_id(ix: &Index<'_>) -> Result<Option<String>> {
    let mut ids = BTreeSet::new();
    for s in ix.named("acn.session") {
        ids.insert(req_str(ix.span(s), "acn.run_id")?);
    }
    if ids.len() > 1 {
        return invalid(format!("sessions disagree on acn.run_id: {ids:?}"));
    }
    Ok(ids.into_iter().next())
}

fn session_rows(ix: &Index<'_>, inv: &Inventory) -> Result<Vec<Row>> {
    let Some(outcomes) = inv
        .attribute("acn.turn.outcome")
        .map(|a| a.values.clone())
        .filter(|v| !v.is_empty())
    else {
        return invalid("the inventory declares no value set for acn.turn.outcome (TRC-31)");
    };
    let mut rows = Vec::new();
    for i in ix.named("acn.session") {
        let s = ix.span(i);
        let desc = ix.descendants(i);
        let chats: Vec<&SpanRow> = desc
            .iter()
            .map(|&d| ix.span(d))
            .filter(|d| d.name == "chat")
            .collect();
        let turns: Vec<&SpanRow> = ix
            .children(i)
            .iter()
            .map(|&c| ix.span(c))
            .filter(|c| c.name == "acn.turn")
            .collect();
        let mut counts: BTreeMap<String, i64> = outcomes.iter().map(|o| (o.clone(), 0)).collect();
        for t in &turns {
            let o = req_str(t, "acn.turn.outcome")?;
            let Some(n) = counts.get_mut(&o) else {
                return invalid(format!("turn outcome `{o}` is not in the closed set"));
            };
            *n += 1;
        }
        let mut up = 0;
        let mut down = 0;
        let mut with_usage = 0i64;
        for c in &chats {
            up = add(up, req_int(c, "acn.call.wire_bytes_up")?)?;
            down = add(down, req_int(c, "acn.call.wire_bytes_down")?)?;
            if get(c, "acn.call.input_tokens").is_some() {
                with_usage += 1;
            }
        }
        rows.push(Row::from([
            ("run_id", Cell::Utf8(Some(req_str(s, "acn.run_id")?))),
            ("session_id", Cell::Id(Some(s.span_id))),
            (
                "hypothesis_id",
                Cell::Utf8(Some(req_str(s, "acn.hypothesis.id")?)),
            ),
            (
                "hypothesis_status",
                Cell::Utf8(Some(req_str(s, "acn.hypothesis.status")?)),
            ),
            ("backend", Cell::Utf8(Some(req_str(s, "acn.backend")?))),
            ("mode", Cell::Utf8(Some(req_str(s, "acn.mode")?))),
            (
                "scenario_hash",
                Cell::Utf8(Some(req_str(s, "acn.scenario.hash")?)),
            ),
            (
                "workload_hash",
                Cell::Utf8(Some(req_str(s, "acn.workload.hash")?)),
            ),
            ("seed", Cell::Int(Some(req_int(s, "acn.seed")?))),
            ("replicate", Cell::Int(Some(req_int(s, "acn.replicate")?))),
            ("role", Cell::Utf8(Some(req_str(s, "acn.role")?))),
            ("turns", Cell::Int(Some(count(turns.len())?))),
            ("calls", Cell::Int(Some(count(chats.len())?))),
            ("duration_ns", Cell::Int(Some(dur(s)?))),
            (
                "input_tokens_total",
                Cell::Int(sum_all(
                    chats
                        .iter()
                        .map(|c| opt_int(c, "acn.call.input_tokens"))
                        .collect::<Result<Vec<_>>>()?,
                )?),
            ),
            (
                "cache_read_tokens_total",
                Cell::Int(sum_all(
                    chats
                        .iter()
                        .map(|c| opt_int(c, "acn.cache.read_tokens"))
                        .collect::<Result<Vec<_>>>()?,
                )?),
            ),
            ("calls_with_usage", Cell::Int(Some(with_usage))),
            ("wire_bytes_up_total", Cell::Int(Some(up))),
            ("wire_bytes_down_total", Cell::Int(Some(down))),
            ("outcome_counts", Cell::Map(counts.into_iter().collect())),
        ]));
    }
    Ok(rows)
}

fn turn_rows(ix: &Index<'_>) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    for sess in ix.named("acn.session") {
        let s = ix.span(sess);
        let run = req_str(s, "acn.run_id")?;
        let mut turns: Vec<(i64, usize)> = Vec::new();
        for &t in ix.children(sess) {
            if ix.span(t).name == "acn.turn" {
                turns.push((req_int(ix.span(t), "acn.turn.index")?, t));
            }
        }
        turns.sort_unstable();
        if turns.windows(2).any(|w| w[0].0 == w[1].0) {
            return invalid("two turns of a session share an acn.turn.index");
        }
        let mut prev_end: Option<i64> = None;
        for (index, t) in turns {
            let ts = ix.span(t);
            let desc = ix.descendants(t);
            let path = critical_path(ix, t);
            let mut chain = 0i64;
            let mut tool_wait = 0i64;
            let mut network = 0i64;
            let mut chat_time = 0i64;
            let mut chat_network = 0i64;
            let mut queue = Vec::new();
            for &leaf in &path {
                let l = ix.span(leaf);
                if l.name == "chat" {
                    if ix.parent(leaf) == Some(t) {
                        chain += 1;
                    }
                    chat_time = add(chat_time, dur(l)?)?;
                    let w = link_wait(ix, leaf)?;
                    chat_network = add(chat_network, w)?;
                    network = add(network, w)?;
                    queue.push(opt_ms(l, "acn.server.queue_ms")?);
                } else {
                    tool_wait = add(tool_wait, dur(l)?)?;
                    if req_str(l, "acn.tool.placement")? == "remote" {
                        network = add(network, link_wait(ix, leaf)?)?;
                    }
                }
            }
            let mut width = 0i64;
            let mut depth = 0i64;
            let mut stalls = 0i64;
            let mut retries = 0i64;
            for &d in &desc {
                let ds = ix.span(d);
                match ds.name.as_str() {
                    "invoke_agent" => {
                        width = width.max(req_int(ds, "acn.fanout.width")?);
                        depth = depth.max(req_int(ds, "acn.fanout.depth")?);
                    }
                    "chat" => {
                        retries = add(retries, req_int(ds, "acn.call.retries")?)?;
                        let n = ix.events_of(d, "acn.stream.stall").len();
                        stalls = add(stalls, count(n)?)?;
                    }
                    _ => {}
                }
            }
            let think = match prev_end {
                None => None,
                Some(e) => Some(
                    ts.start_ns
                        .checked_sub(e)
                        .ok_or_else(|| IngestError::Invalid("think time overflows".into()))?,
                ),
            };
            prev_end = Some(ts.end_ns);
            rows.push(Row::from([
                ("run_id", Cell::Utf8(Some(run.clone()))),
                ("session_id", Cell::Id(Some(s.span_id))),
                ("turn_id", Cell::Id(Some(ts.span_id))),
                ("turn_index", Cell::Int(Some(index))),
                ("think_time_before_ns", Cell::Int(think)),
                ("chain_length", Cell::Int(Some(chain))),
                ("fanout_width", Cell::Int(Some(width))),
                ("fanout_depth", Cell::Int(Some(depth))),
                ("duration_ns", Cell::Int(Some(dur(ts)?))),
                (
                    "first_useful_result_ns",
                    Cell::Int(opt_ms(ts, "acn.turn.first_useful_result_ms")?),
                ),
                (
                    "deadline_ns",
                    Cell::Int(opt_ms(ts, "acn.turn.deadline_ms")?),
                ),
                (
                    "outcome",
                    Cell::Utf8(Some(req_str(ts, "acn.turn.outcome")?)),
                ),
                ("network_wait_ns", Cell::Int(Some(network))),
                ("tool_wait_ns", Cell::Int(Some(tool_wait))),
                (
                    "model_wait_ns",
                    Cell::Int(Some(chat_time.checked_sub(chat_network).ok_or_else(
                        || IngestError::Invalid("model wait overflows".into()),
                    )?)),
                ),
                ("queue_wait_ns", Cell::Int(sum_all(queue)?)),
                ("stalls", Cell::Int(Some(stalls))),
                ("retries", Cell::Int(Some(retries))),
                (
                    "compaction",
                    Cell::Utf8(Some(req_str(ts, "acn.turn.compaction")?)),
                ),
            ]));
        }
    }
    Ok(rows)
}

/// Where a call or tool sits.
struct Place {
    run_id: String,
    session: SpanId,
    turn: usize,
    turn_index: i64,
    lineage: Option<usize>,
}

fn place(ix: &Index<'_>, i: usize) -> Result<Place> {
    let s = ix.span(i);
    let Some(turn) = ix.ancestor(i, "acn.turn") else {
        return invalid(format!("`{}` {} is in no turn", s.name, sid(s)));
    };
    let Some(sess) = ix.ancestor(turn, "acn.session") else {
        return invalid(format!(
            "the turn of `{}` {} is in no session",
            s.name,
            sid(s)
        ));
    };
    Ok(Place {
        run_id: req_str(ix.span(sess), "acn.run_id")?,
        session: ix.span(sess).span_id,
        turn,
        turn_index: req_int(ix.span(turn), "acn.turn.index")?,
        lineage: ix.lineage(i),
    })
}

/// TRC-12: within a turn and lineage, `acn.call.index` counts the chats from 0 in
/// start order (ties by span id), and every tool's `acn.tool.requesting_call`
/// names one of them. A view keyed on those numbers is wrong otherwise.
fn check_call_indices(ix: &Index<'_>) -> Result<()> {
    // (turn, lineage) -> (start, span id, call index) of each chat.
    type Calls = Vec<(i64, SpanId, i64)>;
    let mut chats: BTreeMap<(usize, Option<usize>), Calls> = BTreeMap::new();
    for c in ix.named("chat") {
        let p = place(ix, c)?;
        let s = ix.span(c);
        chats.entry((p.turn, p.lineage)).or_default().push((
            s.start_ns,
            s.span_id,
            req_int(s, "acn.call.index")?,
        ));
    }
    for v in chats.values_mut() {
        v.sort_unstable();
        for (expected, (_, id, index)) in v.iter().enumerate() {
            if *index != count(expected)? {
                return invalid(format!(
                    "chat {} has acn.call.index {index}, but it is call {expected} of its turn and lineage in start order (TRC-12)",
                    id.iter().map(|b| format!("{b:02x}")).collect::<String>()
                ));
            }
        }
    }
    for t in ix.named("execute_tool") {
        let p = place(ix, t)?;
        let s = ix.span(t);
        let req = req_int(s, "acn.tool.requesting_call")?;
        let n = chats.get(&(p.turn, p.lineage)).map_or(0, Vec::len);
        if req < 0 || req >= count(n)? {
            return invalid(format!(
                "tool {} names requesting call {req}, which its turn and lineage do not hold (TRC-13)",
                sid(s)
            ));
        }
    }
    Ok(())
}

fn call_rows(ix: &Index<'_>) -> Result<Vec<Row>> {
    // Tools by (turn span, lineage, requesting call), for the preceding-tool columns.
    let mut tools: BTreeMap<(usize, Option<usize>, i64), Vec<usize>> = BTreeMap::new();
    for t in ix.named("execute_tool") {
        let p = place(ix, t)?;
        let req = req_int(ix.span(t), "acn.tool.requesting_call")?;
        tools.entry((p.turn, p.lineage, req)).or_default().push(t);
    }
    let mut rows = Vec::new();
    for c in ix.named("chat") {
        let s = ix.span(c);
        let Place {
            run_id: run,
            session,
            turn,
            turn_index,
            lineage,
        } = place(ix, c)?;
        let index = req_int(s, "acn.call.index")?;
        let set: &[usize] = if index > 0 {
            tools
                .get(&(turn, lineage, index - 1))
                .map_or(&[], Vec::as_slice)
        } else {
            &[]
        };
        let (count, span_ns, class) =
            if set.is_empty() {
                (None, None, None)
            } else {
                let first = ix.span(set[0]);
                let start = set
                    .iter()
                    .fold(first.start_ns, |m, &t| m.min(ix.span(t).start_ns));
                let end = set
                    .iter()
                    .fold(first.end_ns, |m, &t| m.max(ix.span(t).end_ns));
                let mut longest = set[0];
                for &t in set {
                    let (a, b) = (ix.span(t), ix.span(longest));
                    let (da, db) = (dur(a)?, dur(b)?);
                    if da > db || (da == db && a.span_id < b.span_id) {
                        longest = t;
                    }
                }
                (
                    Some(count(set.len())?),
                    Some(end.checked_sub(start).ok_or_else(|| {
                        IngestError::Invalid("preceding-tool span overflows".into())
                    })?),
                    Some(req_str(ix.span(longest), "acn.tool.class")?),
                )
            };
        let input = opt_int(s, "acn.call.input_tokens")?;
        let cache_read = opt_int(s, "acn.cache.read_tokens")?;
        #[allow(clippy::cast_precision_loss)] // a ratio of token counts
        let ratio = match (cache_read, input) {
            (Some(r), Some(i)) if i != 0 => Some(r as f64 / i as f64),
            _ => None,
        };
        let streamed = req_bool(s, "acn.call.streamed")?;
        let ttft =
            if streamed {
                match ix.events_of(c, "acn.stream.first_token").first() {
                    Some(e) if e.time_ns < s.start_ns => {
                        return invalid("a first token before its call started");
                    }
                    Some(e) => Some(e.time_ns.checked_sub(s.start_ns).ok_or_else(|| {
                        IngestError::Invalid("time to first token overflows".into())
                    })?),
                    None => None,
                }
            } else if get(s, "acn.call.ttft_ms").is_some() {
                Some(dur(s)?)
            } else {
                None
            };
        rows.push(Row::from([
            ("run_id", Cell::Utf8(Some(run))),
            ("session_id", Cell::Id(Some(session))),
            ("turn_index", Cell::Int(Some(turn_index))),
            ("call_id", Cell::Id(Some(s.span_id))),
            ("call_index", Cell::Int(Some(index))),
            ("lineage_id", Cell::Id(lineage.map(|l| ix.span(l).span_id))),
            (
                "provider",
                Cell::Utf8(Some(req_str(s, "gen_ai.provider.name")?)),
            ),
            (
                "model",
                Cell::Utf8(Some(req_str(s, "gen_ai.request.model")?)),
            ),
            ("input_tokens", Cell::Int(input)),
            (
                "new_input_tokens",
                Cell::Int(opt_int(s, "acn.call.new_input_tokens")?),
            ),
            (
                "new_input_tokens_method",
                Cell::Utf8(Some(req_str(s, "acn.call.new_input_tokens_method")?)),
            ),
            (
                "output_tokens",
                Cell::Int(opt_int(s, "acn.call.output_tokens")?),
            ),
            (
                "stop_reason",
                Cell::Utf8(opt_str(s, "acn.call.stop_reason")?),
            ),
            ("duration_ns", Cell::Int(Some(dur(s)?))),
            ("preceding_tool_count", Cell::Int(count)),
            ("preceding_tool_ns", Cell::Int(span_ns)),
            ("preceding_tool_class", Cell::Utf8(class)),
            ("cache_read_tokens", Cell::Int(cache_read)),
            (
                "cache_write_tokens",
                Cell::Int(opt_int(s, "acn.cache.write_tokens")?),
            ),
            ("cached_token_ratio", Cell::Float(ratio)),
            ("ttft_ns", Cell::Int(ttft)),
            ("itl_p50_ns", Cell::Int(opt_ms(s, "acn.call.itl_p50_ms")?)),
            ("itl_p99_ns", Cell::Int(opt_ms(s, "acn.call.itl_p99_ms")?)),
            (
                "wire_bytes_up",
                Cell::Int(Some(req_int(s, "acn.call.wire_bytes_up")?)),
            ),
            (
                "wire_bytes_down",
                Cell::Int(Some(req_int(s, "acn.call.wire_bytes_down")?)),
            ),
            ("streamed", Cell::Bool(Some(streamed))),
            (
                "server_queue_ns",
                Cell::Int(opt_ms(s, "acn.server.queue_ms")?),
            ),
            (
                "server_prefill_ns",
                Cell::Int(opt_ms(s, "acn.server.prefill_ms")?),
            ),
            (
                "server_decode_ns",
                Cell::Int(opt_ms(s, "acn.server.decode_ms")?),
            ),
            ("retries", Cell::Int(Some(req_int(s, "acn.call.retries")?))),
            (
                "error_class",
                Cell::Utf8(opt_str(s, "acn.call.error_class")?),
            ),
        ]));
    }
    Ok(rows)
}

fn link_rows(ix: &Index<'_>) -> Result<Vec<Row>> {
    let links = ix.named("acn.link");
    if links.is_empty() {
        return Ok(Vec::new());
    }
    let Some(run) = run_id(ix)? else {
        return invalid("link spans without a session: the run id is unknown (TRC-35)");
    };
    let scenarios = ix.named("acn.scenario");
    let [sc] = scenarios.as_slice() else {
        return invalid(format!(
            "a run with link spans has exactly one acn.scenario span, not {} (TRC-16)",
            scenarios.len()
        ));
    };
    // Steps in the order they took effect: by time, then by the order the scenario
    // emitted them (`seq`), never by name.
    let mut steps: Vec<(i64, u32, String)> = Vec::new();
    for e in ix.events_of(*sc, "acn.scenario.step") {
        let Some(AttrValue::String(step)) = e.attrs.get("step") else {
            return invalid("an acn.scenario.step event lacks `step`");
        };
        steps.push((e.time_ns, e.seq, step.clone()));
    }
    let mut outages: Vec<(i64, i64)> = Vec::new();
    for e in ix.events_of(*sc, "acn.scenario.outage") {
        let (Some(AttrValue::Int(a)), Some(AttrValue::Int(b))) =
            (e.attrs.get("start_ns"), e.attrs.get("end_ns"))
        else {
            return invalid("an acn.scenario.outage event lacks start_ns or end_ns");
        };
        if b < a {
            return invalid(format!("an outage ends ({b}) before it starts ({a})"));
        }
        outages.push((*a, *b));
    }
    steps.sort();
    outages.sort_unstable();
    let mut rows = Vec::new();
    for l in links {
        let s = ix.span(l);
        let call = match ix.parent(l) {
            None => None,
            Some(p) => {
                let ps = ix.span(p);
                match ps.name.as_str() {
                    "chat" => Some(ps.span_id),
                    "execute_tool" if req_str(ps, "acn.tool.placement")? == "remote" => {
                        Some(ps.span_id)
                    }
                    other => {
                        return invalid(format!(
                            "an acn.link is a child of `{other}`; it carries the traffic of a chat or a remote tool (TRC-15)"
                        ));
                    }
                }
            }
        };
        let enqueue = req_int(s, "acn.link.enqueue_ns")?;
        let step = steps
            .iter()
            .rev()
            .find(|(t, _, _)| *t <= enqueue)
            .map(|(_, _, st)| st.clone());
        let outage = match outages
            .iter()
            .position(|(a, b)| *a <= enqueue && enqueue < *b)
        {
            Some(i) => Some(count(i)?),
            None => None,
        };
        rows.push(Row::from([
            ("run_id", Cell::Utf8(Some(run.clone()))),
            ("link_span_id", Cell::Id(Some(s.span_id))),
            ("call_id", Cell::Id(call)),
            ("link_id", Cell::Utf8(Some(req_str(s, "acn.link.id")?))),
            (
                "link_model",
                Cell::Utf8(Some(req_str(s, "acn.link.model")?)),
            ),
            (
                "direction",
                Cell::Utf8(Some(req_str(s, "acn.link.direction")?)),
            ),
            ("bytes", Cell::Int(Some(req_int(s, "acn.link.bytes")?))),
            ("enqueue_ns", Cell::Int(Some(enqueue))),
            (
                "dequeue_ns",
                Cell::Int(Some(req_int(s, "acn.link.dequeue_ns")?)),
            ),
            (
                "applied_delay_ns",
                Cell::Int(Some(req_ms(s, "acn.link.applied_delay_ms")?)),
            ),
            (
                "dropped",
                Cell::Bool(Some(req_bool(s, "acn.link.dropped")?)),
            ),
            (
                "reordered",
                Cell::Bool(Some(req_bool(s, "acn.link.reordered")?)),
            ),
            (
                "rate_limited_ns",
                Cell::Int(Some(req_ms(s, "acn.link.rate_limited_ms")?)),
            ),
            ("scenario_step", Cell::Utf8(step)),
            ("outage_id", Cell::Int(outage)),
        ]));
    }
    Ok(rows)
}

fn tool_rows(ix: &Index<'_>) -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    for t in ix.named("execute_tool") {
        let s = ix.span(t);
        let Place {
            run_id: run,
            session,
            turn_index,
            lineage,
            ..
        } = place(ix, t)?;
        rows.push(Row::from([
            ("run_id", Cell::Utf8(Some(run))),
            ("session_id", Cell::Id(Some(session))),
            ("turn_index", Cell::Int(Some(turn_index))),
            ("tool_span_id", Cell::Id(Some(s.span_id))),
            ("lineage_id", Cell::Id(lineage.map(|l| ix.span(l).span_id))),
            (
                "requesting_call",
                Cell::Int(Some(req_int(s, "acn.tool.requesting_call")?)),
            ),
            (
                "tool_name",
                Cell::Utf8(Some(req_str(s, "gen_ai.tool.name")?)),
            ),
            (
                "tool_class",
                Cell::Utf8(Some(req_str(s, "acn.tool.class")?)),
            ),
            (
                "placement",
                Cell::Utf8(Some(req_str(s, "acn.tool.placement")?)),
            ),
            ("duration_ns", Cell::Int(Some(dur(s)?))),
            (
                "result_bytes",
                Cell::Int(Some(req_int(s, "acn.tool.result_bytes")?)),
            ),
        ]));
    }
    Ok(rows)
}

/// The Arrow type of a `views.toml` column type.
fn arrow_type(ty: &str) -> Result<DataType> {
    Ok(match ty {
        "utf8" => DataType::Utf8,
        "int64" => DataType::Int64,
        "float64" => DataType::Float64,
        "bool" => DataType::Boolean,
        "fixed_size_binary(8)" => DataType::FixedSizeBinary(8),
        "map<utf8,int64>" => map_type(),
        other => return invalid(format!("view column type `{other}` has no Arrow type")),
    })
}

fn map_type() -> DataType {
    DataType::Map(
        Arc::new(Field::new(
            "entries",
            DataType::Struct(Fields::from(vec![
                Field::new("key", DataType::Utf8, false),
                Field::new("value", DataType::Int64, true),
            ])),
            false,
        )),
        false,
    )
}

/// The Arrow schema of a view, generated from `views.toml` (TRC-37).
pub fn view_schema(view: &View) -> Result<Schema> {
    let mut fields = Vec::new();
    for c in &view.columns {
        fields.push(Field::new(&c.name, arrow_type(&c.ty)?, c.nullable));
    }
    Ok(Schema::new(fields))
}

/// Type the rows of `view` against `views.toml`: every declared column must be
/// produced for every row, with its type, and a non-nullable column is never null.
fn batch(view: &View, rows: &[Row]) -> Result<RecordBatch> {
    let mut columns: Vec<ArrayRef> = Vec::new();
    for row in rows {
        if let Some(extra) = row.keys().find(|k| view.column(k).is_none()) {
            return invalid(format!(
                "view `{}` has no column `{extra}` (TRC-37)",
                view.name
            ));
        }
    }
    for col in &view.columns {
        let at = |r: &Row| -> Result<Cell> {
            r.get(col.name.as_str()).cloned().ok_or_else(|| {
                IngestError::Invalid(format!(
                    "view `{}`: column `{}` was not produced (TRC-37)",
                    view.name, col.name
                ))
            })
        };
        let null_err = || {
            IngestError::Invalid(format!(
                "view `{}`: `{}` is not nullable but a row has no value",
                view.name, col.name
            ))
        };
        let mismatch = |c: &Cell| {
            IngestError::Invalid(format!(
                "view `{}`: `{}` is {} but the row holds {c:?}",
                view.name, col.name, col.ty
            ))
        };
        let array: ArrayRef = match col.ty.as_str() {
            "utf8" => {
                let mut b = StringBuilder::new();
                for r in rows {
                    match at(r)? {
                        Cell::Utf8(v) => {
                            if v.is_none() && !col.nullable {
                                return Err(null_err());
                            }
                            b.append_option(v);
                        }
                        c => return Err(mismatch(&c)),
                    }
                }
                Arc::new(b.finish())
            }
            "int64" => {
                let mut b = Int64Builder::new();
                for r in rows {
                    match at(r)? {
                        Cell::Int(v) => {
                            if v.is_none() && !col.nullable {
                                return Err(null_err());
                            }
                            b.append_option(v);
                        }
                        c => return Err(mismatch(&c)),
                    }
                }
                Arc::new(b.finish())
            }
            "float64" => {
                let mut b = Float64Builder::new();
                for r in rows {
                    match at(r)? {
                        Cell::Float(v) => {
                            if v.is_none() && !col.nullable {
                                return Err(null_err());
                            }
                            b.append_option(v);
                        }
                        c => return Err(mismatch(&c)),
                    }
                }
                Arc::new(b.finish())
            }
            "bool" => {
                let mut b = BooleanBuilder::new();
                for r in rows {
                    match at(r)? {
                        Cell::Bool(v) => {
                            if v.is_none() && !col.nullable {
                                return Err(null_err());
                            }
                            b.append_option(v);
                        }
                        c => return Err(mismatch(&c)),
                    }
                }
                Arc::new(b.finish())
            }
            "fixed_size_binary(8)" => {
                let mut b = FixedSizeBinaryBuilder::new(8);
                for r in rows {
                    match at(r)? {
                        Cell::Id(Some(v)) => b.append_value(v)?,
                        Cell::Id(None) if col.nullable => b.append_null(),
                        Cell::Id(None) => return Err(null_err()),
                        c => return Err(mismatch(&c)),
                    }
                }
                Arc::new(b.finish())
            }
            "map<utf8,int64>" => {
                let mut b = MapBuilder::new(None, StringBuilder::new(), Int64Builder::new());
                for r in rows {
                    match at(r)? {
                        Cell::Map(entries) => {
                            for (k, v) in entries {
                                b.keys().append_value(k);
                                b.values().append_value(v);
                            }
                            b.append(true)?;
                        }
                        c => return Err(mismatch(&c)),
                    }
                }
                Arc::new(b.finish())
            }
            other => return invalid(format!("view column type `{other}` is not writable")),
        };
        columns.push(array);
    }
    Ok(RecordBatch::try_new(Arc::new(view_schema(view)?), columns)?)
}

/// The five views of a trace, in `views.toml` order, each with its view (TRC-30..34,
/// TRC-38). The trace must be in stored order.
pub fn views(inv: &Inventory, views: &Views, trace: &Trace) -> Result<Vec<(View, RecordBatch)>> {
    if !trace.is_sorted() {
        return invalid("the trace is not in stored order (TRC-25)");
    }
    let ix = Index::new(trace)?;
    check_call_indices(&ix)?;
    let mut out = Vec::new();
    for view in views.iter() {
        let rows = match view.name.as_str() {
            "session" => session_rows(&ix, inv)?,
            "turn" => turn_rows(&ix)?,
            "call" => call_rows(&ix)?,
            "link" => link_rows(&ix)?,
            "tool" => tool_rows(&ix)?,
            other => return invalid(format!("no ingester for view `{other}`")),
        };
        out.push((view.clone(), batch(view, &rows)?));
    }
    Ok(out)
}
