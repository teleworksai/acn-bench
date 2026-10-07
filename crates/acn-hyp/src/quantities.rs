//! The quantity table of HYP-12: every quantity a predicate may read, with its
//! unit and its documented formula over named view columns and promoted
//! `acn.*` columns (TRC-3). A quantity's value for a replicate is computed from
//! that replicate's view rows; its value for an arm in a cell is the mean over
//! replicates. [`value`] evaluates each formula over one replicate's rows
//! ([`Replicate`]); the table is also what files resolve their names against and
//! what `cargo xtask docs-inventory` renders. A sum over values that may be absent
//! is undefined when any term is absent, as in the views (`views.toml`): a partial
//! total never passes for a complete one, and a call is never dropped from a
//! replicate for lacking a value (HYP-11's survivorship rule, ADR-19).

use acn_attrib::core::{Cause, Parts};

/// One quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quantity {
    pub name: &'static str,
    /// The unit; `+`, `-`, comparison, `min` and `max` need equal units (HYP-11).
    pub unit: &'static str,
    /// The formula, per replicate, in terms of view columns (`views.toml`).
    pub formula: &'static str,
    /// The spec that defines it.
    pub source: &'static str,
}

/// The table. Names are unique and are not reserved words (HYP-6).
pub const QUANTITIES: &[Quantity] = &[
    Quantity {
        name: "cached_token_ratio",
        unit: "ratio",
        formula: "sum(call.cache_read_tokens) / sum(call.input_tokens), over the replicate's calls; undefined when any call lacks either count, or the denominator is 0",
        source: "SPEC 010 Appendix A `c.cached_token_ratio`",
    },
    Quantity {
        name: "ttft_p50_ms",
        unit: "ms",
        formula: "the nearest-rank 50th percentile (rank ceil(0.5 n)) of call.ttft_ns / 1e6, over the replicate's n calls; undefined when n = 0 or any call has no ttft (a call that never produced a token is not dropped, HYP-11)",
        source: "SPEC 010 Appendix A `h.ttft`",
    },
    Quantity {
        name: "ttft_p99_ms",
        unit: "ms",
        formula: "the nearest-rank 99th percentile (rank ceil(0.99 n)) of call.ttft_ns / 1e6, over the replicate's n calls; undefined when n = 0 or any call has no ttft (a call that never produced a token is not dropped, HYP-11)",
        source: "SPEC 010 Appendix A `h.ttft`",
    },
    Quantity {
        name: "cost_per_success",
        unit: "cost",
        formula: "sum over the replicate's calls of in·(call.input_tokens − call.cache_read_tokens − call.cache_write_tokens) + read·call.cache_read_tokens + write·call.cache_write_tokens + out·call.output_tokens, with the weights of the provider's row in PRICES, divided by count(turn where outcome = success); undefined when any call lacks a count, the provider has no row, or no turn succeeded",
        source: "SPEC 100 (POC 4); the price table is PRICES in this crate (HYP-12, ADR-19)",
    },
    Quantity {
        name: "input_tokens_per_turn",
        unit: "tokens",
        formula: "sum(call.input_tokens) / count(turn), over the replicate; undefined when any call lacks input_tokens or there is no turn",
        source: "SPEC 010 Appendix A `c.input_length_by_call_index`",
    },
    Quantity {
        name: "compactions_per_session",
        unit: "count",
        formula: "count(turn where compaction != none) / count(session), over the replicate; undefined when there is no session",
        source: "SPEC 010 Appendix A `t.compaction_vs_length`",
    },
    Quantity {
        name: "network_attributable_share",
        unit: "ratio",
        formula: "sum(network_ns) / sum(duration_ns) over the replicate's turns, as acn-attrib splits them (time on the emulated network on each turn's critical path); undefined when there is no turn or no time",
        source: "SPEC 090 ATR-20",
    },
    Quantity {
        name: "model_share",
        unit: "ratio",
        formula: "sum(model_ns) / sum(duration_ns) over the replicate's turns, as acn-attrib splits them; undefined as network_attributable_share",
        source: "SPEC 090 ATR-20",
    },
    Quantity {
        name: "tool_share",
        unit: "ratio",
        formula: "sum(tool_ns) / sum(duration_ns) over the replicate's turns, as acn-attrib splits them; undefined as network_attributable_share",
        source: "SPEC 090 ATR-20",
    },
    Quantity {
        name: "retry_share",
        unit: "ratio",
        formula: "sum(retry_ns) / sum(duration_ns) over the replicate's turns, as acn-attrib splits them; undefined as network_attributable_share",
        source: "SPEC 090 ATR-20",
    },
    Quantity {
        name: "other_share",
        unit: "ratio",
        formula: "sum(other_ns) / sum(duration_ns) over the replicate's turns, as acn-attrib splits them; undefined as network_attributable_share",
        source: "SPEC 090 ATR-20",
    },
    Quantity {
        name: "tail_network_share_p99",
        unit: "ratio",
        formula: "network_attributable_share over the replicate's tail turns: duration_ns at or above its nearest-rank 99th percentile (rank max(1, ceil(0.99 n))), ties included",
        source: "SPEC 090 ATR-21",
    },
    Quantity {
        name: "tail_model_share_p99",
        unit: "ratio",
        formula: "model_share over the replicate's tail turns (as tail_network_share_p99)",
        source: "SPEC 090 ATR-21",
    },
    Quantity {
        name: "tail_tool_share_p99",
        unit: "ratio",
        formula: "tool_share over the replicate's tail turns (as tail_network_share_p99)",
        source: "SPEC 090 ATR-21",
    },
    Quantity {
        name: "tail_retry_share_p99",
        unit: "ratio",
        formula: "retry_share over the replicate's tail turns (as tail_network_share_p99)",
        source: "SPEC 090 ATR-21",
    },
    Quantity {
        name: "tail_other_share_p99",
        unit: "ratio",
        formula: "other_share over the replicate's tail turns (as tail_network_share_p99)",
        source: "SPEC 090 ATR-21",
    },
];

/// The cause and, for a tail share, the percentile of an attribution quantity
/// (SPEC 090 ATR-20, ATR-21); `None` for any other name.
#[must_use]
pub fn attribution(name: &str) -> Option<(Cause, Option<usize>)> {
    let cause = |c: &str| Cause::ALL.into_iter().find(|x| x.as_str() == c);
    if name == "network_attributable_share" {
        return Some((Cause::Network, None));
    }
    if let Some(c) = name
        .strip_prefix("tail_")
        .and_then(|n| n.strip_suffix("_share_p99"))
    {
        return cause(c).map(|c| (c, Some(99)));
    }
    let c = name.strip_suffix("_share").and_then(cause)?;
    (c != Cause::Network).then_some((c, None))
}

/// The relative price of each class of token for one provider, in units of one
/// uncached input token (ADR-19). Only ratios enter a verdict: `provider` is never
/// pooled (CON-26), so every comparison is within one provider, and scaling a row
/// changes an effect and its noise floor alike.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prices {
    /// The `vary.provider` value, or the backend when the file has no `provider`.
    pub provider: &'static str,
    /// The models the ratios were taken from, and when.
    pub reference: &'static str,
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
}

/// The price table behind `cost_per_success` (HYP-12). A provider with no row
/// (self-hosted `vllm` and `sglang`, which have no list price) makes the quantity
/// undefined, and its slices inconclusive, until a Class C PR adds one.
pub const PRICES: &[Prices] = &[
    Prices {
        provider: "anthropic",
        reference: "Claude 4.x list prices, 5-minute cache writes, October 2026",
        input: 1.0,
        cache_read: 0.1,
        cache_write: 1.25,
        output: 5.0,
    },
    Prices {
        provider: "openai",
        reference: "GPT-5 family list prices (automatic caching, no write surcharge), October 2026",
        input: 1.0,
        cache_read: 0.1,
        cache_write: 1.0,
        output: 8.0,
    },
];

/// The price row of `provider`.
#[must_use]
pub fn prices(provider: &str) -> Option<&'static Prices> {
    PRICES.iter().find(|p| p.provider == provider)
}

/// One session of a replicate: its `session` view row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub session_id: [u8; 8],
}

/// One turn: the `turn` view columns the formulas read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub session_id: [u8; 8],
    pub outcome: String,
    pub compaction: String,
}

/// One call: the `call` view columns the formulas read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Call {
    pub session_id: [u8; 8],
    pub input_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub ttft_ns: Option<i64>,
}

/// The view rows of one replicate of one arm: its sessions, and their turns and
/// calls, with each turn's attribution (SPEC 090).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replicate {
    pub sessions: Vec<Session>,
    pub turns: Vec<Turn>,
    pub calls: Vec<Call>,
    /// Each turn's parts, as `acn-attrib` splits them (ATR-10).
    pub parts: Vec<Parts>,
    /// The key of its row in [`PRICES`]: `vary.provider`, or the backend.
    pub price_key: String,
}

#[allow(clippy::cast_precision_loss)] // token counts are far below 2^53
fn f(v: i64) -> f64 {
    v as f64
}

fn count(n: usize) -> f64 {
    crate::bootstrap::as_f64(n)
}

fn ratio(num: f64, den: f64) -> Option<f64> {
    (den != 0.0).then(|| num / den).filter(|v| v.is_finite())
}

/// The nearest-rank `p`th percentile (`ceil(p/100 · n)`, from 1) of `xs`.
fn nearest_rank(mut xs: Vec<i64>, p: u32) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_unstable();
    let n = xs.len();
    let rank = (n * p as usize).div_ceil(100).max(1);
    xs.get(rank - 1).map(|v| f(*v) / 1e6)
}

/// The value of quantity `name` for one replicate, or `None` when it is
/// undefined (HYP-11) or `name` is not in the table.
#[must_use]
pub fn value(name: &str, r: &Replicate) -> Option<f64> {
    match name {
        "cached_token_ratio" => {
            let (mut read, mut input) = (0.0, 0.0);
            for c in &r.calls {
                read += f(c.cache_read_tokens?);
                input += f(c.input_tokens?);
            }
            ratio(read, input)
        }
        "ttft_p50_ms" => nearest_rank(
            r.calls.iter().map(|c| c.ttft_ns).collect::<Option<_>>()?,
            50,
        ),
        "ttft_p99_ms" => nearest_rank(
            r.calls.iter().map(|c| c.ttft_ns).collect::<Option<_>>()?,
            99,
        ),
        "cost_per_success" => {
            let p = prices(&r.price_key)?;
            let mut cost = 0.0;
            for c in &r.calls {
                let (i, rd, w, o) = (
                    c.input_tokens?,
                    c.cache_read_tokens?,
                    c.cache_write_tokens?,
                    c.output_tokens?,
                );
                let uncached = i.checked_sub(rd)?.checked_sub(w)?;
                if uncached < 0 {
                    return None;
                }
                cost += p.input * f(uncached)
                    + p.cache_read * f(rd)
                    + p.cache_write * f(w)
                    + p.output * f(o);
            }
            let ok = r.turns.iter().filter(|t| t.outcome == "success").count();
            ratio(cost, count(ok))
        }
        "input_tokens_per_turn" => {
            let mut sum = 0.0;
            for c in &r.calls {
                sum += f(c.input_tokens?);
            }
            ratio(sum, count(r.turns.len()))
        }
        "compactions_per_session" => {
            let n = r.turns.iter().filter(|t| t.compaction != "none").count();
            ratio(count(n), count(r.sessions.len()))
        }
        // SPEC 090: computed by acn-attrib, never here (ATR-22, ATR-31). A
        // verdict reads it through `attribution_value`, which refuses an
        // overflow (ATR-30); here it is only undefined.
        other => attribution_value(other, r).ok().flatten(),
    }
}

/// The value of attribution quantity `name` for one replicate, from
/// `acn-attrib` (SPEC 090 ATR-22): `Ok(None)` when it is undefined or `name`
/// is not one, and an error when a sum overflows, which a verdict refuses
/// (ATR-30).
pub fn attribution_value(name: &str, r: &Replicate) -> Result<Option<f64>, String> {
    let Some((c, p)) = attribution(name) else {
        return Ok(None);
    };
    match p {
        None => acn_attrib::core::share(&r.parts, c),
        Some(p) => acn_attrib::core::tail_share(&r.parts, c, p),
    }
    .map_err(|e| e.to_string())
}

/// The quantity named `name`.
#[must_use]
pub fn get(name: &str) -> Option<&'static Quantity> {
    QUANTITIES.iter().find(|q| q.name == name)
}

/// The table as Markdown, for `docs/generated/quantities.md` (HYP-12).
#[must_use]
pub fn markdown() -> String {
    let mut out = String::from(
        "# Quantities\n\n\
         The quantities a hypothesis file may read (SPEC 080, HYP-12). A quantity's value for an arm in a cell is the mean over replicates of its per-replicate value.\n\n\
         | Quantity | Unit | Per replicate | Defined by |\n|---|---|---|---|\n",
    );
    for q in QUANTITIES {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            q.name,
            q.unit,
            q.formula.replace('|', "\\|"),
            q.source
        ));
    }
    out.push_str(
        "\n## Prices\n\n\
         The weights behind `cost_per_success`, per token, in units of one uncached input token (ADR-19). A provider with no row makes the quantity undefined.\n\n\
         | Provider | Input | Cache read | Cache write | Output | Taken from |\n|---|---|---|---|---|---|\n",
    );
    for p in PRICES {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} |\n",
            p.provider, p.input, p.cache_read, p.cache_write, p.output, p.reference
        ));
    }
    out
}
