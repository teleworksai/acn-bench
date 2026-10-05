//! The evidence chain (SPEC 085: LOOP-1, LOOP-2, LOOP-4). `verify` walks
//! from a loop report or a verdict down to the L1 bundles it rests on and checks
//! every link: each bundle verifies with its views, each verdict recomputes to
//! the bytes on disk, each recorded layer is the derived one, and the report
//! regenerates. The two gates between layers are exposed here, for the `twin`
//! and `promote` commands of LOOP-12 to call.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use acn_trace::identity::Digest;

use crate::Hypothesis;
use crate::layer::{self, Layer};
use crate::loop_run::{self, Binary, Code, Executor, LoopError, REPORT_JSON, Regenerated};
use crate::read;
use crate::slice::key;
use crate::verdict::{self, ReasonId, Verdict};

/// One broken link of a chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub code: Code,
    pub message: String,
}

/// What `acn evidence verify` established (LOOP-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// The loop_id or verdict_id asked about.
    pub target: String,
    /// The loop reports walked: the one named, or every one whose final
    /// verdict is the verdict named, in loop_id order.
    pub loops: Vec<String>,
    /// The bundles and verdicts checked.
    pub bundles: usize,
    pub verdicts: usize,
    /// Where each report regenerated (LOOP-14).
    pub regenerated: Vec<PathBuf>,
    pub findings: Vec<Finding>,
}

impl Checked {
    /// Every link holds, and there was at least one chain to walk.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.findings.is_empty() && !self.loops.is_empty()
    }
}

fn finding(code: Code, message: impl Into<String>) -> Finding {
    Finding {
        code,
        message: message.into(),
    }
}

fn is_hex_id(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The loop reports under `runs/loop/` whose final verdict is `verdict_id`,
/// in loop_id order.
fn loops_naming(runs: &Path, verdict_id: &str) -> Result<Vec<String>, LoopError> {
    let dir = runs.join("loop");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for e in entries {
        let e = e.map_err(|e| loop_run::io_err(&dir, e))?;
        let name = e.file_name().to_string_lossy().into_owned();
        if !is_hex_id(&name) {
            continue; // a staging directory, never a report (ADR-23)
        }
        let Ok(text) = std::fs::read_to_string(e.path().join(REPORT_JSON)) else {
            continue;
        };
        let Ok(r) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if r["verdict_id"] == verdict_id {
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

/// `acn evidence verify <loop_id | verdict_id>` (LOOP-2): walk every chain
/// the id names under `runs`, and list every link that does not hold.
pub fn verify(
    runs: &Path,
    id: &str,
    bin: Binary,
    exec: &mut dyn Executor,
) -> Result<Checked, LoopError> {
    let mut c = Checked {
        target: id.to_owned(),
        loops: Vec::new(),
        bundles: 0,
        verdicts: 0,
        regenerated: Vec::new(),
        findings: Vec::new(),
    };
    if !is_hex_id(id) {
        return loop_run::err(
            Code::Report,
            format!("`{id}` is not a loop_id or a verdict_id (64 lowercase hex digits)"),
        );
    }
    c.loops = if runs.join("loop").join(id).join(REPORT_JSON).is_file() {
        vec![id.to_owned()]
    } else {
        loops_naming(runs, id)?
    };
    if c.loops.is_empty() {
        c.findings.push(finding(
            Code::NoLoopReport,
            format!(
                "no loop report under {} is {id} or names it as its final verdict: a verdict made outside a loop has no recorded inputs to regenerate from (LOOP-2)",
                runs.join("loop").display()
            ),
        ));
        return Ok(c);
    }
    for loop_id in c.loops.clone() {
        let report = runs.join("loop").join(&loop_id).join(REPORT_JSON);
        verify_loop(runs, &report, bin, exec, &mut c)?;
    }
    Ok(c)
}

/// Every link of one loop report's chain (LOOP-2).
fn verify_loop(
    runs: &Path,
    report: &Path,
    bin: Binary,
    exec: &mut dyn Executor,
    c: &mut Checked,
) -> Result<(), LoopError> {
    let text = std::fs::read_to_string(report).map_err(|e| loop_run::io_err(report, e))?;
    let r: serde_json::Value = serde_json::from_str(&text).map_err(|e| LoopError {
        code: Code::Report,
        message: format!("{}: {e}", report.display()),
    })?;
    let name = report.display().to_string();
    // LOOP-1: a report is L1, and says so.
    if r["layer"] != layer::REPORT.as_str() {
        c.findings.push(finding(
            Code::LayerMismatch,
            format!(
                "{name} records layer {}, but a loop report is {} (LOOP-1)",
                r["layer"],
                layer::REPORT.as_str()
            ),
        ));
    }
    // Every bundle verifies with its views and is the one recorded (TRC-23,
    // TRC-35), and is L1.
    let mut data = Vec::new();
    for b in r["bundles"].as_array().into_iter().flatten() {
        c.bundles += 1;
        let run_id = b["run_id"].as_str().unwrap_or_default();
        let digest = b["bundle_digest"].as_str().unwrap_or_default();
        if !is_hex_id(run_id) {
            c.findings.push(finding(
                Code::Report,
                format!("{name} lists a bundle without a run_id"),
            ));
            continue;
        }
        match read::read(&runs.join(run_id)) {
            Err(e) => c
                .findings
                .push(finding(Code::BundleInvalid, format!("{run_id}: {e}"))),
            Ok(d) => {
                if d.bundle_digest.to_hex() != digest {
                    c.findings.push(finding(
                        Code::BundleInvalid,
                        format!(
                            "{run_id}: bundle_digest {} is not the recorded {digest} (TRC-23)",
                            d.bundle_digest.to_hex()
                        ),
                    ));
                }
                match layer::of_bundle(&d.manifest) {
                    Ok(Layer::L1) => {}
                    Ok(l) => c.findings.push(finding(
                        Code::LayerMismatch,
                        format!(
                            "{run_id} is an {} bundle in an L1 report (LOOP-1)",
                            l.as_str()
                        ),
                    )),
                    Err(e) => c.findings.push(finding(Code::LayerMismatch, e)),
                }
                data.push(d);
            }
        }
    }
    // Regeneration and recomputation need the binary that made the report
    // (CON-31).
    let made_by = (
        r["build_hash"].as_str().unwrap_or_default(),
        r["engine_hash"].as_str().unwrap_or_default(),
    );
    if made_by
        != (
            bin.build_hash.to_hex().as_str(),
            bin.engine_hash.to_hex().as_str(),
        )
    {
        c.findings.push(finding(
            Code::NotRegenerable,
            format!(
                "{name} was made by build {} and engine {}; this binary is {} and {} (CON-31, LOOP-2)",
                made_by.0,
                made_by.1,
                bin.build_hash.to_hex(),
                bin.engine_hash.to_hex()
            ),
        ));
        return Ok(());
    }
    // The final verdict recomputes to the bytes on disk (HYP-15, HYP-20).
    c.verdicts += 1;
    let verdict_id = r["verdict_id"].as_str().unwrap_or_default();
    let base = match runs.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let hyp_path = base.join(r["hypothesis"]["path"].as_str().unwrap_or_default());
    match crate::load_in(&hyp_path, &base) {
        Err(e) => c.findings.push(finding(Code::InputChanged, e.to_string())),
        Ok(h) if h.hash().to_hex() != r["hypothesis"]["hash"] => c.findings.push(finding(
            Code::InputChanged,
            format!(
                "{} no longer has the recorded hash (LOOP-14)",
                hyp_path.display()
            ),
        )),
        Ok(h) => match verdict::verdict(&h, data, bin.engine_hash) {
            Err(e) => c
                .findings
                .push(finding(Code::VerdictRefused, e.to_string())),
            Ok(v) => {
                let path = runs.join("verdicts").join(verdict_id).join("verdict.json");
                if v.verdict_id.to_hex() != verdict_id {
                    c.findings.push(finding(
                        Code::VerdictMismatch,
                        format!(
                            "the bundles give verdict_id {}, not the recorded {verdict_id} (HYP-15)",
                            v.verdict_id.to_hex()
                        ),
                    ));
                } else if std::fs::read_to_string(&path).ok().as_deref() != Some(v.text().as_str())
                {
                    c.findings.push(finding(
                        Code::VerdictMismatch,
                        format!(
                            "{} is missing or differs from the verdict its bundles give (HYP-15, LOOP-2)",
                            path.display()
                        ),
                    ));
                }
                if layer::of_verdict(&v) != Layer::L1 {
                    c.findings.push(finding(
                        Code::LayerMismatch,
                        format!("verdict {verdict_id} is not L1 (LOOP-1)"),
                    ));
                }
            }
        },
    }
    // The report regenerates, and with it every L1 bundle (LOOP-14).
    match loop_run::regenerate(report, bin, exec) {
        Ok(g) => {
            if !g.identical() {
                c.findings.push(finding(
                    Code::NotRegenerated,
                    format!(
                        "{name} does not regenerate: {} differ (LOOP-14)",
                        g.differ.join(", ")
                    ),
                ));
            }
            c.regenerated.push(g.dir);
        }
        Err(e) => c.findings.push(finding(e.code, e.message)),
    }
    Ok(())
}

/// LOOP-4, the gate into L2: an L2 twin starts only from an L1 report that
/// regenerates byte for byte.
pub fn twin_gate(
    report: &Path,
    bin: Binary,
    exec: &mut dyn Executor,
) -> Result<Regenerated, LoopError> {
    let g = loop_run::regenerate(report, bin, exec).map_err(|e| LoopError {
        code: Code::TwinRefused,
        message: format!("the L1 report does not regenerate: {e} (LOOP-4)"),
    })?;
    if !g.identical() {
        return loop_run::err(
            Code::TwinRefused,
            format!(
                "the L1 report does not regenerate: {} differ (LOOP-4)",
                g.differ.join(", ")
            ),
        );
    }
    Ok(g)
}

/// LOOP-4, the gate into L3. A hypothesis that declares `twin_required =
/// false` waives it. Otherwise `l2` must be an L2 verdict of `h`:
/// - over every bundle `l1` read;
/// - whose live bundles twin every decision cell of `l1` (HYP-22);
/// - whose reasons include no `twin_failed`.
///
/// Omitting a live bundle therefore cannot pass it.
pub fn promote_gate(h: &Hypothesis, l1: &Verdict, l2: Option<&Verdict>) -> Result<(), LoopError> {
    if !h.design().twin_required {
        return Ok(());
    }
    let refuse = |m: String| loop_run::err(Code::PromoteRefused, format!("{m} (LOOP-4)"));
    if layer::of_verdict(l1) != Layer::L1 {
        return refuse(format!(
            "verdict {} is not an L1 verdict",
            l1.verdict_id.to_hex()
        ));
    }
    let Some(l2) = l2 else {
        return refuse("no L2 verdict: the hypothesis requires a twin".into());
    };
    if layer::of_verdict(l2) != Layer::L2 {
        return refuse(format!(
            "verdict {} holds no live twin of the mock, so it is not L2",
            l2.verdict_id.to_hex()
        ));
    }
    let l2_runs: BTreeSet<Digest> = l2.bundles.iter().map(|b| b.0).collect();
    if let Some((missing, _, _)) = l1.bundles.iter().find(|b| !l2_runs.contains(&b.0)) {
        return refuse(format!(
            "the L2 verdict does not read the L1 bundle {}",
            missing.to_hex()
        ));
    }
    let twin_failed = l2.reasons.iter().any(|r| r.id == ReasonId::TwinFailed)
        || l2
            .slices
            .iter()
            .any(|s| s.reasons.iter().any(|r| r.id == ReasonId::TwinFailed));
    if twin_failed {
        return refuse(format!(
            "the L2 verdict {} records twin_failed",
            l2.verdict_id.to_hex()
        ));
    }
    let l2_slices: BTreeMap<&str, &verdict::SliceVerdict> =
        l2.slices.iter().map(|s| (s.key.as_str(), s)).collect();
    let mut untwinned = Vec::new();
    for s in &l1.slices {
        for &i in &s.eval.decision_cells {
            let Some(cell) = s.data.cells().get(i) else {
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
                untwinned.push(if s.key.is_empty() {
                    k
                } else {
                    format!("{}: {k}", s.key)
                });
            }
        }
    }
    if !untwinned.is_empty() {
        return refuse(format!(
            "decision cells the L2 verdict does not twin: {}",
            untwinned.join("; ")
        ));
    }
    Ok(())
}
