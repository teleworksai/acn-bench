//! Attribution statistics (SPEC 090). This module is in the frozen set
//! (CON-7): integer nanoseconds throughout, one `f64` division last, and every
//! reduction in a fixed order (ATR-30).
//!
//! [`decompose`] splits each turn of a bundle into network, model, tool, retry
//! and other time (ATR-10 to ATR-14); [`share`] and [`tail_share`] are the
//! quantities a verdict reads (ATR-20, ATR-21).

use std::collections::BTreeMap;

pub use acn_trace::ingest::{Leaf, LeafKind, TurnPath};

/// Why a bundle's attribution failed (ATR-15): it names the turn.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("attribution: {0}")]
pub struct AttribError(pub String);

type Result<T> = std::result::Result<T, AttribError>;

fn err<T>(m: impl Into<String>) -> Result<T> {
    Err(AttribError(m.into()))
}

fn hex(id: &[u8; 8]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

fn add(a: i64, b: i64) -> Result<i64> {
    a.checked_add(b)
        .ok_or_else(|| AttribError("a sum of nanoseconds overflows (ATR-30)".into()))
}

fn sub(a: i64, b: i64) -> Result<i64> {
    a.checked_sub(b)
        .ok_or_else(|| AttribError("a difference of nanoseconds overflows (ATR-30)".into()))
}

/// The `turn` view columns attribution reads (TRC-32).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnRow {
    pub session_id: [u8; 8],
    pub turn_index: i64,
    pub duration_ns: i64,
    pub tool_wait_ns: i64,
    pub model_wait_ns: i64,
    pub queue_wait_ns: Option<i64>,
    pub stalls: i64,
    pub retries: i64,
}

/// The `link` view columns attribution reads (TRC-34).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkRow {
    pub call_id: Option<[u8; 8]>,
    pub link_id: String,
    pub direction: String,
    pub enqueue_ns: i64,
    pub dequeue_ns: i64,
    pub dropped: bool,
    pub applied_delay_ns: i64,
    pub rate_limited_ns: i64,
}

/// One link in one direction, ordered bytewise by link, then direction
/// (SPEC 090 §0).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hop {
    pub link_id: String,
    pub direction: String,
}

impl Hop {
    /// `<link_id>/<direction>`, the key of `hop_ns` (ATR-40).
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}/{}", self.link_id, self.direction)
    }
}

/// A cause of ATR-10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cause {
    Network,
    Model,
    Tool,
    Retry,
    Other,
}

impl Cause {
    pub const ALL: [Cause; 5] = [
        Cause::Network,
        Cause::Model,
        Cause::Tool,
        Cause::Retry,
        Cause::Other,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Cause::Network => "network",
            Cause::Model => "model",
            Cause::Tool => "tool",
            Cause::Retry => "retry",
            Cause::Other => "other",
        }
    }
}

/// A turn's duration and its five parts (ATR-10): what the quantities read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Parts {
    pub duration_ns: i64,
    pub network_ns: i64,
    pub model_ns: i64,
    pub tool_ns: i64,
    pub retry_ns: i64,
    pub other_ns: i64,
}

impl Parts {
    #[must_use]
    pub fn of(&self, c: Cause) -> i64 {
        match c {
            Cause::Network => self.network_ns,
            Cause::Model => self.model_ns,
            Cause::Tool => self.tool_ns,
            Cause::Retry => self.retry_ns,
            Cause::Other => self.other_ns,
        }
    }
}

/// One turn's attribution (ATR-10 to ATR-14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decomposition {
    pub session_id: [u8; 8],
    pub turn_index: i64,
    pub parts: Parts,
    /// `network_ns` by hop, in hop order; the values sum to it (ATR-13).
    pub hop_ns: BTreeMap<Hop, i64>,
    pub queue_wait_ns: Option<i64>,
    pub stalls: i64,
    pub retries: i64,
    /// Link time removed by clipping to the leaves (ATR-14).
    pub clipped_ns: i64,
    /// Time of link rows with no call that overlaps the turn (ATR-14).
    pub unattributed_link_ns: i64,
}

/// A leaf's parts while they are summed.
#[derive(Default)]
struct Acc {
    network: i64,
    server: i64,
    retry: i64,
    other: i64,
    hops: BTreeMap<Hop, i64>,
    clipped: i64,
}

impl Acc {
    fn net(&mut self, row: &LinkRow, ns: i64) -> Result<()> {
        self.network = add(self.network, ns)?;
        let h = self
            .hops
            .entry(Hop {
                link_id: row.link_id.clone(),
                direction: row.direction.clone(),
            })
            .or_insert(0);
        *h = add(*h, ns)?;
        Ok(())
    }
}

/// A link row named for a message: its hop and recorded send time.
fn row_name(r: &LinkRow) -> String {
    format!(
        "link row {}/{} sent at {}",
        r.link_id, r.direction, r.enqueue_ns
    )
}

/// ATR-11 and ATR-12: the parts of one chat or remote tool leaf. `server` is
/// the model's time for a chat and the tool's for a remote tool.
fn leaf_parts(leaf: &Leaf, rows: &[&LinkRow], what: &str) -> Result<Acc> {
    let mut acc = Acc::default();
    let (ls, le) = (leaf.start_ns, leaf.end_ns);
    for r in rows {
        if r.dequeue_ns < r.enqueue_ns {
            return err(format!(
                "{what}: {} is received before it is sent (ATR-11)",
                row_name(r)
            ));
        }
        if r.dropped && r.dequeue_ns != r.enqueue_ns {
            return err(format!(
                "{what}: {} is dropped but not empty (ATR-11)",
                row_name(r)
            ));
        }
        if r.direction != "up" && r.direction != "down" {
            return err(format!(
                "{what}: {} has direction `{}`",
                row_name(r),
                r.direction
            ));
        }
    }
    if ls == le {
        return Ok(acc);
    }
    if rows.is_empty() {
        acc.server = sub(le, ls)?;
        return Ok(acc);
    }
    // Attempts and their answers on the recorded times, before any clipping:
    // uplink rows in send order, ties by row order (ATR-11).
    let mut ups: Vec<usize> = (0..rows.len())
        .filter(|&i| rows[i].direction == "up")
        .collect();
    ups.sort_by_key(|&i| (rows[i].enqueue_ns, i));
    let (Some(&first), Some(&last)) = (ups.first(), ups.last()) else {
        return err(format!("{what}: a downlink row with no request (ATR-11)"));
    };
    // The last attempt's answer: its latest-sent downlink row, the last in row
    // order of those sent together.
    let mut answer: Option<usize> = None;
    for (i, d) in rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.direction == "down")
    {
        let Some(&owner) = ups
            .iter()
            .rev()
            .find(|&&u| rows[u].enqueue_ns <= d.enqueue_ns)
        else {
            return err(format!(
                "{what}: {} is sent before the first request (ATR-11)",
                row_name(d)
            ));
        };
        let up = rows[owner];
        if up.dropped {
            return err(format!(
                "{what}: {} answers a lost request (ATR-11)",
                row_name(d)
            ));
        }
        if d.enqueue_ns < up.dequeue_ns {
            return err(format!(
                "{what}: {} is sent before its request was received (ATR-11)",
                row_name(d)
            ));
        }
        if owner == last && answer.is_none_or(|a| d.enqueue_ns >= rows[a].enqueue_ns) {
            answer = Some(i);
        }
    }
    // Measured on times clipped to the leaf; what clipping removes is recorded.
    let clip = |t: i64| t.clamp(ls, le);
    for r in rows {
        let removed = sub(
            sub(r.dequeue_ns, r.enqueue_ns)?,
            sub(clip(r.dequeue_ns), clip(r.enqueue_ns))?,
        )?;
        acc.clipped = add(acc.clipped, removed)?;
    }
    let (f, l) = (rows[first], rows[last]);
    acc.other = sub(clip(f.enqueue_ns), ls)?;
    acc.retry = sub(clip(l.enqueue_ns), clip(f.enqueue_ns))?;
    if l.dropped {
        acc.net(l, sub(le, clip(l.enqueue_ns))?)?;
        return Ok(acc);
    }
    acc.net(l, sub(clip(l.dequeue_ns), clip(l.enqueue_ns))?)?;
    match answer.map(|a| rows[a]) {
        None => acc.server = sub(le, clip(l.dequeue_ns))?,
        Some(r) => {
            acc.server = sub(clip(r.enqueue_ns), clip(l.dequeue_ns))?;
            if r.dropped {
                acc.net(r, sub(le, clip(r.enqueue_ns))?)?;
            } else {
                acc.net(r, sub(clip(r.dequeue_ns), clip(r.enqueue_ns))?)?;
                acc.other = add(acc.other, sub(le, clip(r.dequeue_ns))?)?;
            }
        }
    }
    Ok(acc)
}

/// Split every turn of a bundle (ATR-10 to ATR-14). `turns` are the turn view's
/// rows and `paths` the critical paths, both in turn-view order; `links` are the
/// link view's rows in view order. Any error is the whole bundle's (ATR-15).
pub fn decompose(
    turns: &[TurnRow],
    paths: &[TurnPath],
    links: &[LinkRow],
) -> Result<Vec<Decomposition>> {
    if turns.len() != paths.len() {
        return err("the turn view and the critical paths have different turns (ATR-2)");
    }
    let mut by_call: BTreeMap<[u8; 8], Vec<&LinkRow>> = BTreeMap::new();
    let mut unowned: Vec<&LinkRow> = Vec::new();
    for l in links {
        match l.call_id {
            Some(c) => by_call.entry(c).or_default().push(l),
            None => unowned.push(l),
        }
    }
    let mut out = Vec::with_capacity(turns.len());
    for (t, p) in turns.iter().zip(paths) {
        let name = format!("turn {} of session {}", t.turn_index, hex(&t.session_id));
        if t.session_id != p.session_id || t.turn_index != p.turn_index {
            return err(format!(
                "{name}: the critical paths are in another order (ATR-2)"
            ));
        }
        if sub(p.end_ns, p.start_ns)? != t.duration_ns {
            return err(format!("{name}: its duration is not its span's (ATR-2)"));
        }
        let mut parts = Parts {
            duration_ns: t.duration_ns,
            ..Parts::default()
        };
        let mut hop_ns: BTreeMap<Hop, i64> = BTreeMap::new();
        let (mut leaves_ns, mut clipped) = (0i64, 0i64);
        let (mut tool_leaves, mut chat_leaves, mut chat_wait) = (0i64, 0i64, 0i64);
        let mut prev_end = p.start_ns;
        for leaf in &p.leaves {
            let what = format!("{name}, leaf {}", hex(&leaf.span_id));
            if leaf.start_ns < prev_end || leaf.end_ns > p.end_ns || leaf.end_ns < leaf.start_ns {
                return err(format!(
                    "{what}: leaves overlap or reach outside the turn (ATR-10)"
                ));
            }
            prev_end = leaf.end_ns;
            let time = sub(leaf.end_ns, leaf.start_ns)?;
            leaves_ns = add(leaves_ns, time)?;
            let rows: &[&LinkRow] = by_call.get(&leaf.span_id).map_or(&[], Vec::as_slice);
            let remote = leaf.placement.as_deref() == Some("remote");
            let acc = match leaf.kind {
                LeafKind::Tool if !remote => {
                    if !rows.is_empty() {
                        return err(format!("{what}: a local tool carries link rows (ATR-12)"));
                    }
                    Acc {
                        server: time,
                        ..Acc::default()
                    }
                }
                _ => leaf_parts(leaf, rows, &what)?,
            };
            match leaf.kind {
                LeafKind::Chat => {
                    parts.model_ns = add(parts.model_ns, acc.server)?;
                    chat_leaves = add(chat_leaves, time)?;
                    for r in rows {
                        chat_wait = add(chat_wait, add(r.applied_delay_ns, r.rate_limited_ns)?)?;
                    }
                }
                LeafKind::Tool => {
                    parts.tool_ns = add(parts.tool_ns, acc.server)?;
                    tool_leaves = add(tool_leaves, time)?;
                }
            }
            parts.network_ns = add(parts.network_ns, acc.network)?;
            parts.retry_ns = add(parts.retry_ns, acc.retry)?;
            parts.other_ns = add(parts.other_ns, acc.other)?;
            clipped = add(clipped, acc.clipped)?;
            for (h, ns) in acc.hops {
                let v = hop_ns.entry(h).or_insert(0);
                *v = add(*v, ns)?;
            }
        }
        // The turn's time outside its leaves (ATR-10).
        parts.other_ns = add(parts.other_ns, sub(t.duration_ns, leaves_ns)?)?;
        // ATR-14: the views read the same path.
        if tool_leaves != t.tool_wait_ns {
            return err(format!(
                "{name}: its tool leaves take {tool_leaves} ns, the turn view says {} (ATR-14)",
                t.tool_wait_ns
            ));
        }
        if chat_leaves != add(t.model_wait_ns, chat_wait)? {
            return err(format!(
                "{name}: its chat leaves take {chat_leaves} ns, the turn view says {} plus {chat_wait} of link wait (ATR-14)",
                t.model_wait_ns
            ));
        }
        let mut unattributed = 0i64;
        for l in &unowned {
            let lo = l.enqueue_ns.max(p.start_ns);
            let hi = l.dequeue_ns.min(p.end_ns);
            if hi > lo {
                unattributed = add(unattributed, sub(hi, lo)?)?;
            }
        }
        let total = [
            parts.network_ns,
            parts.model_ns,
            parts.tool_ns,
            parts.retry_ns,
            parts.other_ns,
        ]
        .into_iter()
        .try_fold(0i64, add)?;
        if total != parts.duration_ns || Cause::ALL.iter().any(|c| parts.of(*c) < 0) {
            return err(format!(
                "{name}: its parts do not split its duration (ATR-10)"
            ));
        }
        out.push(Decomposition {
            session_id: t.session_id,
            turn_index: t.turn_index,
            parts,
            hop_ns,
            queue_wait_ns: t.queue_wait_ns,
            stalls: t.stalls,
            retries: t.retries,
            clipped_ns: clipped,
            unattributed_link_ns: unattributed,
        });
    }
    Ok(out)
}

#[allow(clippy::cast_precision_loss)] // one division, last (ATR-30)
fn divide(num: i64, den: i64) -> Option<f64> {
    (den > 0).then(|| num as f64 / den as f64)
}

/// The sum of `cause` over `turns` divided by the sum of their durations
/// (ATR-20): `Ok(None)` when there is no turn or no time, and an error when a
/// sum overflows (ATR-30), which a verdict refuses.
pub fn share(turns: &[Parts], cause: Cause) -> Result<Option<f64>> {
    let (mut num, mut den) = (0i64, 0i64);
    for t in turns {
        num = add(num, t.of(cause))?;
        den = add(den, t.duration_ns)?;
    }
    Ok(divide(num, den))
}

/// [`share`] over the tail turns: those at or above the nearest-rank `p`th
/// percentile of the turns' durations, rank `max(1, ceil(n·p/100))` in
/// integers, ties included (ATR-21). `p` is 1 to 100.
pub fn tail_share(turns: &[Parts], cause: Cause, p: usize) -> Result<Option<f64>> {
    if !(1..=100).contains(&p) {
        return err(format!("a tail percentile is 1 to 100, not {p} (ATR-21)"));
    }
    if turns.is_empty() {
        return Ok(None);
    }
    let mut d: Vec<i64> = turns.iter().map(|t| t.duration_ns).collect();
    d.sort_unstable();
    let rank = turns
        .len()
        .checked_mul(p)
        .ok_or_else(|| AttribError("the tail's rank overflows (ATR-30)".into()))?
        .div_ceil(100)
        .max(1);
    let threshold = d[rank - 1];
    let tail: Vec<Parts> = turns
        .iter()
        .copied()
        .filter(|t| t.duration_ns >= threshold)
        .collect();
    share(&tail, cause)
}
