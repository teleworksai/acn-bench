//! The quantity table of HYP-12: every quantity a predicate may read, with its
//! unit and its documented formula over named view columns and promoted
//! `acn.*` columns (TRC-3). A quantity's value for a replicate is computed from
//! that replicate's view rows; its value for an arm in a cell is the mean over
//! replicates. The formulas are evaluated by the verdict engine (T05.2, ADR-18);
//! this table is what files resolve their names against and what
//! `cargo xtask docs-inventory` renders.

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
        formula: "sum(call.cache_read_tokens) / sum(call.input_tokens), over the replicate's calls with both present",
        source: "SPEC 010 Appendix A `c.cached_token_ratio`",
    },
    Quantity {
        name: "ttft_p50_ms",
        unit: "ms",
        formula: "the nearest-rank 50th percentile of call.ttft_ns / 1e6, over the replicate's calls with a ttft",
        source: "SPEC 010 Appendix A `h.ttft`",
    },
    Quantity {
        name: "ttft_p99_ms",
        unit: "ms",
        formula: "the nearest-rank 99th percentile of call.ttft_ns / 1e6, over the replicate's calls with a ttft",
        source: "SPEC 010 Appendix A `h.ttft`",
    },
    Quantity {
        name: "cost_per_success",
        unit: "cost",
        formula: "sum over the replicate's calls of the provider's price of its uncached input, cache read, cache write and output tokens, divided by the number of the replicate's turns with outcome `success`",
        source: "SPEC 100 (POC 4); the price table lives in this crate (HYP-12)",
    },
    Quantity {
        name: "input_tokens_per_turn",
        unit: "tokens",
        formula: "sum(call.input_tokens) / count(turn), over the replicate",
        source: "SPEC 010 Appendix A `c.input_length_by_call_index`",
    },
    Quantity {
        name: "compactions_per_session",
        unit: "count",
        formula: "count(turn where compaction != none) / count(session), over the replicate",
        source: "SPEC 010 Appendix A `t.compaction_vs_length`",
    },
];

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
    out
}
