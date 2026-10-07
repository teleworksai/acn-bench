//! The L2 twin (SPEC 085 LOOP-12, LOOP-16): which cells of a loop are run again
//! in `live`, on the mock the harness serves (HAR-26), and what the verdict
//! over both modes says about them. Every choice is made here, from the
//! hypothesis file and the loop's final verdict alone (LOOP-15).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use acn_trace::identity::{Digest, Mode};

use crate::Hypothesis;
use crate::file::Tolerance;
use crate::json::J;
use crate::layer;
use crate::loop_out;
use crate::loop_run::{self, Binary, Code, Executor, LoopError, Setup};
use crate::read::{self, BundleData};
use crate::slice::{Cell, key};
use crate::verdict::{self, Label, ReasonId, Role, SliceVerdict, Verdict};

/// The twin object's format and file name (LOOP-16).
pub const TWIN_FORMAT: &str = "acn-bench/loop-twin/v1";
pub const TWIN_JSON: &str = "twin.json";

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
/// primary quantity, each cell once, in slice-key and then HYP-14 order.
///
/// `l1` must be an L1 verdict of `h`, or the choice is refused with
/// `twin_refused`. A verdict with no such cell is refused with
/// `nothing_to_twin`.
pub fn choose(h: &Hypothesis, l1: &Verdict, top: u32) -> Result<Vec<Chosen>, LoopError> {
    if !crate::evidence::judged_by(&h.hash(), l1) {
        return loop_run::err(
            Code::TwinRefused,
            format!(
                "verdict {} is not a verdict of {} (LOOP-12)",
                l1.verdict_id.to_hex(),
                h.id()
            ),
        );
    }
    if crate::layer::of_verdict(l1) != crate::layer::Layer::L1 {
        return loop_run::err(
            Code::TwinRefused,
            format!(
                "verdict {} is not an L1 verdict: a twin starts from a loop's final verdict (LOOP-12)",
                l1.verdict_id.to_hex()
            ),
        );
    }
    let mut chosen: BTreeMap<(usize, usize), BTreeSet<Why>> = BTreeMap::new();
    for (si, s) in l1.slices.iter().enumerate() {
        for &ci in &s.eval.decision_cells {
            // A decision cell the slice does not hold is the verdict's
            // fault: the choice fails closed rather than dropping it.
            if ci >= s.data.cells().len() {
                return loop_run::err(
                    Code::Internal,
                    format!(
                        "decision cell #{ci} of slice `{}` is not among its cells",
                        s.key
                    ),
                );
            }
            chosen.entry((si, ci)).or_default().insert(Why::Decision);
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
/// twinned, so the check fails closed. This is not a gate by itself: it
/// checks neither hypothesis nor layer, which [`crate::evidence::promote_gate`]
/// and the twin check first.
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

/// One arm of a chosen cell: the L1 bundle it twins, when the report holds
/// one, and the run_id its live twin has (CON-29, HAR-26).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlannedArm {
    pub role: Role,
    /// The cell the arm runs: the chosen cell, or its control's.
    pub cell: Cell,
    /// The L1 bundle twinned (`derived_from`); `None` when the report holds
    /// none for this arm, which is then not run and the cell not twinned.
    pub derived_from: Option<Digest>,
    pub live_run_id: Digest,
}

/// A chosen cell and its two arms (LOOP-12).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Planned {
    pub chosen: Chosen,
    pub arms: Vec<PlannedArm>,
}

/// The arms of every chosen cell, treatment before control.
pub(crate) fn plan(
    s: &Setup<'_>,
    chosen: Vec<Chosen>,
    l1_runs: &BTreeSet<Digest>,
) -> Result<Vec<Planned>, LoopError> {
    let mut out = Vec::with_capacity(chosen.len());
    for c in chosen {
        let control = loop_run::control_of(s, &c.cell)?;
        let mut arms = Vec::with_capacity(2);
        for (role, cell) in [(Role::Treatment, c.cell.clone()), (Role::Control, control)] {
            let sim = s.run_id(&cell, role, Mode::Sim)?;
            arms.push(PlannedArm {
                role,
                derived_from: l1_runs.contains(&sim).then_some(sim),
                live_run_id: s.run_id(&cell, role, Mode::Live)?,
                cell,
            });
        }
        out.push(Planned { chosen: c, arms });
    }
    Ok(out)
}

/// The distinct L1 bundles a plan twins, in order: a control shared by
/// several cells is run once (LOOP-12).
pub(crate) fn runs_of(plan: &[Planned]) -> Vec<&PlannedArm> {
    let mut seen = BTreeSet::new();
    plan.iter()
        .flat_map(|p| &p.arms)
        .filter(|a| a.derived_from.is_some() && seen.insert(a.live_run_id))
        .collect()
}

fn tolerance_json(t: Tolerance) -> J {
    match t {
        Tolerance::Absolute(x) => J::obj([("abs", J::Float(x))]),
        Tolerance::Relative(x) => J::obj([("relative", J::Float(x))]),
    }
}

/// The twin object (LOOP-16), a function of the report, the L2 verdict, the
/// live bundles and `top` alone, so that `acn evidence verify` regenerates it
/// byte for byte.
#[allow(clippy::too_many_arguments)]
pub(crate) fn object(
    h: &Hypothesis,
    loop_id: &str,
    l1: &Verdict,
    l2: &Verdict,
    plan: &[Planned],
    live: &BTreeMap<Digest, BundleData>,
    top: u32,
    live_dir: &str,
) -> Result<J, LoopError> {
    let hex = |d: &Digest| J::str(d.to_hex());
    let cells: Vec<J> = plan
        .iter()
        .map(|p| {
            let arms = p.arms.iter().map(|a| {
                let b = a.derived_from.and_then(|_| live.get(&a.live_run_id));
                (
                    a.role.as_str(),
                    J::obj([
                        ("derived_from", a.derived_from.as_ref().map_or(J::Null, hex)),
                        ("run_id", b.map_or(J::Null, |b| hex(&b.run_id))),
                        (
                            "bundle_digest",
                            b.map_or(J::Null, |b| hex(&b.bundle_digest)),
                        ),
                    ]),
                )
            });
            J::obj([
                ("slice", J::str(p.chosen.slice.clone())),
                ("key", J::str(key(&p.chosen.cell))),
                ("params", verdict::cell_json(&p.chosen.cell)),
                (
                    "reasons",
                    J::Arr(
                        p.chosen
                            .reasons
                            .iter()
                            .map(|r| J::str(r.as_str()))
                            .collect(),
                    ),
                ),
                ("arms", J::obj(arms)),
            ])
        })
        .collect();
    let tolerances = &h.design().sim_live_tolerance;
    let mut divergence = Vec::new();
    for s in &l2.slices {
        let Some(t) = &s.twin else {
            continue;
        };
        let mut twinned_cells = Vec::new();
        for (ci, c) in s.data.cells().iter().enumerate() {
            if t.twinned.get(&ci).copied() != Some(true) {
                continue;
            }
            let quantities = t
                .divergences
                .get(&ci)
                .into_iter()
                .flatten()
                .filter_map(|(q, d)| {
                    let tol = *tolerances.get(q)?;
                    let f = |x: Option<Option<f64>>| x.map_or(J::Null, J::num);
                    Some((
                        q.clone(),
                        J::obj([
                            ("tolerance", tolerance_json(tol)),
                            ("treatment", f(d.treatment)),
                            ("control", f(d.control)),
                            ("effect", f(d.effect)),
                            ("within", J::Bool(d.within(tol))),
                        ]),
                    ))
                });
            twinned_cells.push(J::obj([
                ("key", J::str(key(&c.cell))),
                ("quantities", J::obj(quantities)),
            ]));
        }
        divergence.push(J::obj([
            ("slice", J::str(s.key.clone())),
            ("cells", J::Arr(twinned_cells)),
        ]));
    }
    let Some(first) = live.values().next() else {
        return loop_run::err(Code::Internal, "a twin object with no live bundle");
    };
    Ok(J::obj([
        ("format", J::str(TWIN_FORMAT)),
        ("layer", J::str(layer::of_verdict(l2).as_str())),
        ("loop_id", J::str(loop_id)),
        ("l1_verdict_id", hex(&l1.verdict_id)),
        ("top", J::Int(i64::from(top))),
        ("live_dir", J::str(live_dir)),
        ("cells", J::Arr(cells)),
        ("verdict_id", hex(&l2.verdict_id)),
        ("verdict", J::str(l2.verdict.as_str())),
        ("reasons", verdict::reasons_json(&l2.reasons)),
        (
            "twin_label",
            if l2.labels.contains(&Label::PartiallyTwinned) {
                J::str(Label::PartiallyTwinned.as_str())
            } else {
                J::Null
            },
        ),
        ("divergence", J::Arr(divergence)),
        ("engine_hash", J::str(first.manifest.engine_hash.clone())),
        (
            "build_hash",
            J::str(first.manifest.build.build_hash.clone()),
        ),
    ]))
}

/// A twin that completed (LOOP-12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Twinned {
    pub loop_id: Digest,
    /// `runs/loop/<loop_id>/twin/<verdict_id>/twin.json`.
    pub twin: PathBuf,
    /// The L2 verdict's.
    pub verdict_id: Digest,
    /// The live bundles, ascending.
    pub run_ids: Vec<Digest>,
    /// Every decision cell of the L1 final verdict is twinned (HYP-22).
    pub twinned: bool,
    /// The L2 verdict records `twin_failed` (HYP-21).
    pub twin_failed: bool,
}

/// The L1 bundles a report lists, each verified (TRC-23) and the one recorded.
pub(crate) fn report_bundles(
    runs: &Path,
    r: &serde_json::Value,
) -> Result<Vec<BundleData>, LoopError> {
    let mut out = Vec::new();
    for b in r["bundles"].as_array().into_iter().flatten() {
        let (Some(run_id), Some(digest)) = (b["run_id"].as_str(), b["bundle_digest"].as_str())
        else {
            return loop_run::err(Code::Report, "a report bundle without run_id and digest");
        };
        if !crate::evidence::is_hex_id(run_id) {
            return loop_run::err(Code::Report, format!("`{run_id}` is not a run_id"));
        }
        let d = read::read(&runs.join(run_id)).map_err(|e| LoopError {
            code: Code::BundleInvalid,
            message: format!("{run_id}: {e} (TRC-23)"),
        })?;
        if d.bundle_digest.to_hex() != digest {
            return loop_run::err(
                Code::BundleInvalid,
                format!("{run_id}: bundle_digest is not the recorded {digest} (TRC-23)"),
            );
        }
        out.push(d);
    }
    Ok(out)
}

/// `acn loop twin --loop <loop_id> [--top k]` (LOOP-12): the twin of the loop
/// report `runs/loop/<loop_id>/report.json`, its live bundles under a fresh
/// `runs/live/<n>/`, its L2 verdict and its twin object (LOOP-16).
pub fn twin(
    runs: &Path,
    loop_id: &str,
    top: u32,
    bin: Binary,
    exec: &mut dyn Executor,
) -> Result<Twinned, LoopError> {
    if !crate::evidence::is_hex_id(loop_id) {
        return loop_run::err(
            Code::BadId,
            format!("`{loop_id}` is not a loop_id (64 lowercase hex digits)"),
        );
    }
    let report = runs.join("loop").join(loop_id).join(loop_run::REPORT_JSON);
    if std::fs::symlink_metadata(&report).is_err() {
        return loop_run::err(
            Code::NoLoopReport,
            format!("no loop report at {} (LOOP-12)", report.display()),
        );
    }
    // LOOP-4: nothing runs unless the report regenerates byte for byte.
    let gate = crate::evidence::twin_gate(&report, bin, exec)?;
    let rec = loop_run::read_report(&report, bin)?;
    // The report read now is the one that just regenerated: its bytes are
    // the regeneration's, which the gate compared with the original.
    let regenerated = gate
        .dir
        .join("loop")
        .join(loop_id)
        .join(loop_run::REPORT_JSON);
    if std::fs::read_to_string(&regenerated).ok().as_deref() != Some(rec.text.as_str()) {
        return loop_run::err(
            Code::TwinRefused,
            format!("{} changed after it regenerated (LOOP-4)", report.display()),
        );
    }
    let s = rec.setup(bin, &*exec)?;
    let l1_data = report_bundles(&rec.runs, &rec.r)?;
    let l1 = verdict::verdict(&rec.h, l1_data.clone(), bin.engine_hash).map_err(|e| LoopError {
        code: Code::VerdictRefused,
        message: e.to_string(),
    })?;
    if rec.r["verdict_id"].as_str() != Some(l1.verdict_id.to_hex().as_str()) {
        return loop_run::err(
            Code::VerdictMismatch,
            format!(
                "the report's bundles give verdict {}, not its recorded final verdict (HYP-15)",
                l1.verdict_id.to_hex()
            ),
        );
    }
    let l1_runs: BTreeSet<Digest> = l1_data.iter().map(|b| b.run_id).collect();
    let planned = plan(&s, choose(&rec.h, &l1, top)?, &l1_runs)?;
    let to_run: Vec<PlannedArm> = runs_of(&planned).into_iter().cloned().collect();
    if to_run.is_empty() {
        return loop_run::err(
            Code::NothingToTwin,
            "no chosen cell has an L1 bundle in the report (LOOP-12)",
        );
    }
    let (live_dir, live_rel) = loop_out::fresh_live_dir(&rec.runs)?;
    let mut live = BTreeMap::new();
    for a in &to_run {
        // LOOP-13: the inputs are the loop's own, before every run.
        s.check_inputs()?;
        let b = s.make(exec, &live_dir, &a.cell, a.role, Mode::Live)?;
        live.insert(b.run_id, b);
    }
    let mut set = l1_data;
    set.extend(live.values().cloned());
    let l2 = verdict::verdict(&rec.h, set, bin.engine_hash).map_err(|e| LoopError {
        code: Code::VerdictRefused,
        message: e.to_string(),
    })?;
    let path = loop_out::twin_path(&rec.runs, &s.loop_id, &l2.verdict_id);
    if std::fs::symlink_metadata(&path).is_ok() {
        return loop_run::err(
            Code::TwinExists,
            format!(
                "{} exists; a twin object is never overwritten (LOOP-16)",
                path.display()
            ),
        );
    }
    let text = object(&rec.h, &rec.id, &l1, &l2, &planned, &live, top, &live_rel)?.render();
    // LOOP-13, once before both writes, so nothing can fail between them but
    // I/O and the existence check (LOOP-16).
    s.check_inputs()?;
    loop_run::write_or_keep(&rec.runs, &l2)?;
    let twin = loop_out::write_twin(&rec.runs, &s.loop_id, &l2.verdict_id, &text)?;
    Ok(Twinned {
        loop_id: s.loop_id,
        twin,
        verdict_id: l2.verdict_id,
        run_ids: live.keys().copied().collect(),
        twinned: untwinned(&l1, &l2).is_empty(),
        twin_failed: records_twin_failed(&l2),
    })
}

/// LOOP-16's walk of one twin object, `runs/loop/<loop_id>/twin/<name>/`,
/// after its loop's L1 chain: every finding goes to `c`.
pub(crate) fn verify_twin(
    runs: &Path,
    rec: &loop_run::Recorded,
    s: &Setup<'_>,
    l1: &Verdict,
    l1_data: &[BundleData],
    name: &str,
    c: &mut crate::evidence::Checked,
) {
    use crate::evidence::finding;
    let path = runs
        .join("loop")
        .join(&rec.id)
        .join("twin")
        .join(name)
        .join(TWIN_JSON);
    let shown = path.display().to_string();
    let mismatch = |c: &mut crate::evidence::Checked, m: String| {
        c.findings
            .push(finding(Code::TwinMismatch, format!("{shown}: {m}")));
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return mismatch(c, e.to_string()),
    };
    let t: serde_json::Value = match serde_json::from_str(&text) {
        Ok(t) => t,
        Err(e) => return mismatch(c, e.to_string()),
    };
    if t["format"] != TWIN_FORMAT {
        return mismatch(c, format!("format is not {TWIN_FORMAT}"));
    }
    // The object is the one its directory names, of this loop and its final
    // verdict.
    if t["verdict_id"].as_str() != Some(name) {
        return mismatch(
            c,
            "its verdict_id is not its directory's name (LOOP-16)".into(),
        );
    }
    if t["loop_id"].as_str() != Some(rec.id.as_str())
        || t["l1_verdict_id"].as_str() != Some(l1.verdict_id.to_hex().as_str())
    {
        return mismatch(
            c,
            "it extends another loop or another final verdict (LOOP-2)".into(),
        );
    }
    let live_rel = t["live_dir"].as_str().unwrap_or_default();
    let live_ok = live_rel.strip_prefix("live/").is_some_and(|n| {
        !n.is_empty() && !n.starts_with('0') && n.bytes().all(|b| b.is_ascii_digit())
    });
    if !live_ok || !crate::evidence::inside(live_rel) {
        return mismatch(
            c,
            format!("live_dir `{live_rel}` is not live/<n> (LOOP-12)"),
        );
    }
    let Some(top) = t["top"].as_u64().and_then(|x| u32::try_from(x).ok()) else {
        return mismatch(c, "no `top`".into());
    };
    // The cells and the L1 bundles they twin are LOOP-12's for this report.
    let l1_runs: BTreeSet<Digest> = l1_data.iter().map(|b| b.run_id).collect();
    let planned = match choose(&rec.h, l1, top).and_then(|ch| plan(s, ch, &l1_runs)) {
        Ok(p) => p,
        Err(e) => return mismatch(c, e.to_string()),
    };
    // The live directory is read from runs/ itself (HYP-4), and is this
    // object's alone: a live measurement is never shared (LOOP-12).
    let live_dir = runs.join(live_rel);
    let linked = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink());
    if linked(&runs.join("live")) || linked(&live_dir) {
        return mismatch(
            c,
            format!(
                "runs/live or runs/{live_rel} is a symbolic link; a chain is read from runs/ itself"
            ),
        );
    }
    for other in claimants(runs, live_rel) {
        if other != path {
            mismatch(
                c,
                format!(
                    "{} claims the same live_dir {live_rel} (LOOP-12)",
                    other.display()
                ),
            );
        }
    }
    let planned_ids: BTreeSet<String> = runs_of(&planned)
        .iter()
        .map(|a| a.live_run_id.to_hex())
        .collect();
    if let Ok(rd) = std::fs::read_dir(&live_dir) {
        for e in rd.filter_map(Result::ok) {
            let n = e.file_name().to_string_lossy().into_owned();
            if !planned_ids.contains(&n) {
                mismatch(
                    c,
                    format!("{live_rel}/{n} is not a live bundle this twin runs (LOOP-12)"),
                );
            }
        }
    }
    let recorded = (
        rec.r["build_hash"].as_str().unwrap_or_default(),
        rec.r["engine_hash"].as_str().unwrap_or_default(),
    );
    // Each live bundle verifies, is L2, comes from the report's build and
    // engine (CON-31), and has the run_id its L1 bundle's inputs give in
    // `live` (CON-29, HAR-26).
    let mut live = BTreeMap::new();
    for a in runs_of(&planned) {
        c.bundles += 1;
        let id = a.live_run_id.to_hex();
        if linked(&live_dir.join(&id)) {
            mismatch(c, format!("{live_rel}/{id} is a symbolic link"));
            continue;
        }
        match read::read(&live_dir.join(&id)) {
            Err(e) => c.findings.push(finding(
                Code::BundleInvalid,
                format!(
                    "{live_rel}/{id}, the twin of {}: {e}",
                    a.derived_from.map(|d| d.to_hex()).unwrap_or_default()
                ),
            )),
            Ok(b) => {
                let made = (
                    b.manifest.build.build_hash.as_str(),
                    b.manifest.engine_hash.as_str(),
                );
                if made != recorded {
                    c.findings.push(finding(
                        Code::BuildMismatch,
                        format!(
                            "{live_rel}/{id} was made by build {} and engine {}, not the report's {} and {} (CON-31)",
                            made.0, made.1, recorded.0, recorded.1
                        ),
                    ));
                }
                match layer::of_bundle(&b.manifest) {
                    Ok(layer::Layer::L2) => {}
                    Ok(l) => c.findings.push(finding(
                        Code::LayerMismatch,
                        format!(
                            "{live_rel}/{id} is an {} bundle, not a twin (LOOP-1)",
                            l.as_str()
                        ),
                    )),
                    Err(e) => c.findings.push(finding(Code::LayerMismatch, e)),
                }
                live.insert(b.run_id, b);
            }
        }
    }
    // The L2 verdict is over exactly the L1 bundles and the live ones, and
    // recomputes to the bytes on disk (HYP-15).
    let mut set = l1_data.to_vec();
    set.extend(live.values().cloned());
    let l2 = match verdict::verdict(&rec.h, set, l1.engine_hash) {
        Ok(v) => v,
        Err(e) => {
            c.findings
                .push(finding(Code::VerdictRefused, e.to_string()));
            return;
        }
    };
    c.verdicts += 1;
    if l2.verdict_id.to_hex() != name {
        c.findings.push(finding(
            Code::VerdictMismatch,
            format!(
                "{shown}: its L1 and live bundles give verdict {}, not {name} (HYP-15)",
                l2.verdict_id.to_hex()
            ),
        ));
        return;
    }
    let vpath = runs.join("verdicts").join(name).join("verdict.json");
    if std::fs::read_to_string(&vpath).ok().as_deref() != Some(l2.text().as_str()) {
        c.findings.push(finding(
            Code::VerdictMismatch,
            format!(
                "{} is missing or differs from the verdict its bundles give (HYP-15, LOOP-16)",
                vpath.display()
            ),
        ));
    }
    if t["layer"].as_str() != Some(layer::of_verdict(&l2).as_str()) {
        c.findings.push(finding(
            Code::LayerMismatch,
            format!(
                "{shown} records layer {}, but its bundles make it {} (LOOP-1)",
                t["layer"],
                layer::of_verdict(&l2).as_str()
            ),
        ));
    }
    // Every byte of the object is a function of what was just checked.
    match object(&rec.h, &rec.id, l1, &l2, &planned, &live, top, live_rel) {
        Ok(j) if j.render() == text => {}
        Ok(_) => mismatch(
            c,
            "it does not regenerate from the report, its verdict and its live bundles (LOOP-16)"
                .into(),
        ),
        Err(e) => mismatch(c, e.to_string()),
    }
}

/// Every twin object under `runs/loop/` whose `live_dir` is `live_rel`.
fn claimants(runs: &Path, live_rel: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let loops = runs.join("loop");
    for l in std::fs::read_dir(&loops)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
    {
        for t in std::fs::read_dir(l.path().join("twin"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            let p = t.path().join(TWIN_JSON);
            let named = std::fs::read_to_string(&p)
                .ok()
                .and_then(|x| serde_json::from_str::<serde_json::Value>(&x).ok())
                .is_some_and(|j| j["live_dir"].as_str() == Some(live_rel));
            if named {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}
