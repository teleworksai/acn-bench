//! The L2 twin (SPEC 085 LOOP-12, LOOP-16): which cells of a loop are run again
//! in `live`, on the mock the harness serves (HAR-26), and what the verdict
//! over both modes says about them. Every choice is made here, from the
//! hypothesis file and the loop's final verdict alone (LOOP-15).

use std::collections::{BTreeMap, BTreeSet};

use crate::Hypothesis;
use crate::loop_run::{self, Code, LoopError};
use crate::slice::{Cell, key};
use crate::verdict::{ReasonId, SliceVerdict, Verdict};

/// Why a cell is twinned (LOOP-12), in the order the reasons are listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Why {
    /// A decision cell of its slice (HYP-22).
    Decision,
    /// Among the *k* best by LOOP-11's ranking.
    Best,
    /// Among the *k* worst.
    Worst,
}

impl Why {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Best => "best",
            Self::Worst => "worst",
        }
    }
}

/// A cell chosen for the twin, with every reason it was chosen.
#[derive(Debug, Clone, PartialEq)]
pub struct Chosen {
    /// The slice's index in the verdict, and its key.
    pub slice_index: usize,
    pub slice: String,
    /// The cell's index in its slice (HYP-14 order), and its values.
    pub cell_index: usize,
    pub cell: Cell,
    /// In the order of [`Why`].
    pub reasons: Vec<Why>,
}

/// LOOP-12's cells: the decision cells of every slice of the L1 final verdict
/// `l1`, and the `top` best and `top` worst cells by the effect of the first
/// primary quantity, each cell once, in slice-key and then HYP-14 order. A
/// verdict with no such cell is refused with `nothing_to_twin`.
pub fn choose(h: &Hypothesis, l1: &Verdict, top: u32) -> Result<Vec<Chosen>, LoopError> {
    let mut chosen: BTreeMap<(usize, usize), BTreeSet<Why>> = BTreeMap::new();
    for (si, s) in l1.slices.iter().enumerate() {
        for &ci in &s.eval.decision_cells {
            if ci < s.data.cells().len() {
                chosen.entry((si, ci)).or_default().insert(Why::Decision);
            }
        }
    }
    let k = usize::try_from(top).unwrap_or(usize::MAX);
    let ranked = loop_run::ranking(h, l1);
    for (order, why) in [
        (loop_run::best_first(&ranked), Why::Best),
        (loop_run::worst_first(&ranked), Why::Worst),
    ] {
        for r in order.iter().take(k) {
            chosen.entry((r.slice, r.cell)).or_default().insert(why);
        }
    }
    if chosen.is_empty() {
        return loop_run::err(
            Code::NothingToTwin,
            format!(
                "verdict {} has no decision cell and no cell with a defined effect (LOOP-12)",
                l1.verdict_id.to_hex()
            ),
        );
    }
    let mut out = Vec::with_capacity(chosen.len());
    for ((si, ci), reasons) in chosen {
        let (Some(s), Some(c)) = (
            l1.slices.get(si),
            l1.slices.get(si).and_then(|s| s.data.cells().get(ci)),
        ) else {
            return loop_run::err(Code::Internal, "a chosen cell outside its verdict");
        };
        out.push(Chosen {
            slice_index: si,
            slice: s.key.clone(),
            cell_index: ci,
            cell: c.cell.clone(),
            reasons: reasons.into_iter().collect(),
        });
    }
    Ok(out)
}

/// Whether `v` records `twin_failed`, at file or slice level (HYP-21).
#[must_use]
pub fn records_twin_failed(v: &Verdict) -> bool {
    v.reasons.iter().any(|r| r.id == ReasonId::TwinFailed)
        || v.slices
            .iter()
            .any(|s| s.reasons.iter().any(|r| r.id == ReasonId::TwinFailed))
}

/// The decision cells of `l1` that `l2` does not twin (HYP-22), labelled by
/// slice and cell key. A decision cell that cannot be found counts as not
/// twinned, so the check fails closed.
#[must_use]
pub fn untwinned(l1: &Verdict, l2: &Verdict) -> Vec<String> {
    let l2_slices: BTreeMap<&str, &SliceVerdict> =
        l2.slices.iter().map(|s| (s.key.as_str(), s)).collect();
    let mut out = Vec::new();
    for s in &l1.slices {
        for &i in &s.eval.decision_cells {
            let label = |k: String| {
                if s.key.is_empty() {
                    k
                } else {
                    format!("{}: {k}", s.key)
                }
            };
            let Some(cell) = s.data.cells().get(i) else {
                out.push(label(format!("#{i}")));
                continue;
            };
            let k = key(&cell.cell);
            let twinned = l2_slices.get(s.key.as_str()).is_some_and(|t| {
                t.data
                    .cells()
                    .iter()
                    .position(|c| key(&c.cell) == k)
                    .and_then(|j| t.twin.as_ref()?.twinned.get(&j).copied())
                    == Some(true)
            });
            if !twinned {
                out.push(label(k));
            }
        }
    }
    out
}
