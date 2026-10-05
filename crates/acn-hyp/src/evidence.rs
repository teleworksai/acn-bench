//! The evidence chain (SPEC 085: LOOP-1, LOOP-2, LOOP-4). `verify` walks
//! from a loop report or a verdict down to the L1 bundles it rests on and checks
//! every link: each bundle verifies with its views, each verdict recomputes to
//! the bytes on disk, each recorded layer is the derived one, and the report
//! regenerates. The two gates between layers are exposed here, for the `twin`
//! and `promote` commands of LOOP-12 to call.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use acn_trace::identity::{Digest, Mode};

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
    /// The bundles checked, and the verdicts recomputed.
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

/// A loop_id, verdict_id or run_id: 64 lowercase hex digits.
fn is_hex_id(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A string field of a report, or the `report_refused` finding that names it.
fn field(r: &serde_json::Value, path: &[&str], report: &str) -> Result<String, Finding> {
    let mut v = r;
    for p in path {
        v = &v[*p];
    }
    v.as_str().map(str::to_owned).ok_or_else(|| {
        finding(
            Code::Report,
            format!("{report} has no `{}`", path.join(".")),
        )
    })
}

/// A recorded path inside the base: relative, with no `..`.
fn inside(rel: &str) -> bool {
    let p = Path::new(rel);
    !rel.is_empty()
        && p.components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// The loop reports under `runs/loop/` whose final verdict is `verdict_id`,
/// in loop_id order, and a finding for every loop directory whose report
/// cannot be read: it might have named the verdict.
fn loops_naming(runs: &Path, verdict_id: &str) -> Result<(Vec<String>, Vec<Finding>), LoopError> {
    let dir = runs.join("loop");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok((Vec::new(), Vec::new()));
    };
    let mut found = Vec::new();
    let mut unreadable = Vec::new();
    for e in entries {
        let e = e.map_err(|e| loop_run::io_err(&dir, e))?;
        let name = e.file_name().to_string_lossy().into_owned();
        if !is_hex_id(&name) {
            continue; // a staging directory, never a report (ADR-23)
        }
        let r = std::fs::read_to_string(e.path().join(REPORT_JSON))
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|e| e.to_string()));
        match r {
            Ok(r) if r["verdict_id"] == verdict_id => found.push(name),
            Ok(_) => {}
            Err(why) => unreadable.push(finding(
                Code::Report,
                format!(
                    "{}: {why}; a loop report that cannot be read may name the verdict (LOOP-2)",
                    e.path().join(REPORT_JSON).display()
                ),
            )),
        }
    }
    found.sort();
    unreadable.sort_by(|a, b| a.message.cmp(&b.message));
    Ok((found, unreadable))
}

/// `acn evidence verify <loop_id | verdict_id>` (LOOP-2): walk every chain
/// the id names under `runs`, and list every link that does not hold.
pub fn verify(
    runs: &Path,
    id: &str,
    bin: Binary,
    exec: &mut dyn Executor,
) -> Result<Checked, LoopError> {
    if !is_hex_id(id) {
        return loop_run::err(
            Code::BadId,
            format!("`{id}` is not a loop_id or a verdict_id (64 lowercase hex digits)"),
        );
    }
    let runs = std::fs::canonicalize(runs).map_err(|e| loop_run::io_err(runs, e))?;
    let mut c = Checked {
        target: id.to_owned(),
        loops: Vec::new(),
        bundles: 0,
        verdicts: 0,
        regenerated: Vec::new(),
        findings: Vec::new(),
    };
    if std::fs::symlink_metadata(runs.join("loop").join(id)).is_ok() {
        c.loops = vec![id.to_owned()];
    } else {
        let (found, unreadable) = loops_naming(&runs, id)?;
        c.loops = found;
        c.findings = unreadable;
    }
    if c.loops.is_empty() && c.findings.is_empty() {
        c.findings.push(finding(
            Code::NoLoopReport,
            format!(
                "no loop report under {} is {id} or names it as its final verdict: a verdict made outside a loop has no recorded inputs to regenerate from (LOOP-2)",
                runs.join("loop").display()
            ),
        ));
    }
    for loop_id in c.loops.clone() {
        verify_loop(&runs, &loop_id, bin, exec, &mut c)?;
    }
    Ok(c)
}

/// Every link of one loop report's chain (LOOP-2). `runs` is canonical.
fn verify_loop(
    runs: &Path,
    loop_id: &str,
    bin: Binary,
    exec: &mut dyn Executor,
    c: &mut Checked,
) -> Result<(), LoopError> {
    let dir = runs.join("loop").join(loop_id);
    let report = dir.join(REPORT_JSON);
    let name = report.display().to_string();
    // The chain is the one under `runs`: neither the loop directory nor its
    // report may lead elsewhere (HYP-4).
    for p in [&dir, &report] {
        if std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) {
            c.findings.push(finding(
                Code::Report,
                format!(
                    "{} is a symbolic link; a chain is read from runs/ itself",
                    p.display()
                ),
            ));
            return Ok(());
        }
    }
    let r: serde_json::Value = match std::fs::read_to_string(&report)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
    {
        Ok(r) => r,
        Err(e) => {
            c.findings
                .push(finding(Code::Report, format!("{name}: {e}")));
            return Ok(());
        }
    };
    let fields = (|| {
        Ok::<_, Finding>((
            field(&r, &["verdict_id"], &name)?,
            field(&r, &["build_hash"], &name)?,
            field(&r, &["engine_hash"], &name)?,
            field(&r, &["hypothesis", "path"], &name)?,
            field(&r, &["hypothesis", "hash"], &name)?,
        ))
    })();
    let (verdict_id, build, engine, hyp_rel, hyp_hash) = match fields {
        Ok(f) => f,
        Err(f) => {
            c.findings.push(f);
            return Ok(());
        }
    };
    if !is_hex_id(&verdict_id) || !inside(&hyp_rel) {
        c.findings.push(finding(
            Code::Report,
            format!("{name}: its verdict_id or hypothesis path is not one a loop writes"),
        ));
        return Ok(());
    }
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
        let (run_id, digest) = match (
            field(b, &["run_id"], &name),
            field(b, &["bundle_digest"], &name),
        ) {
            (Ok(r), Ok(d)) if is_hex_id(&r) => (r, d),
            _ => {
                c.findings.push(finding(
                    Code::Report,
                    format!("{name} lists a bundle without a run_id and bundle_digest"),
                ));
                continue;
            }
        };
        match read::read(&runs.join(&run_id)) {
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
    if (build.as_str(), engine.as_str())
        != (
            bin.build_hash.to_hex().as_str(),
            bin.engine_hash.to_hex().as_str(),
        )
    {
        c.findings.push(finding(
            Code::NotRegenerable,
            format!(
                "{name} was made by build {build} and engine {engine}; this binary is {} and {} (CON-31, LOOP-2)",
                bin.build_hash.to_hex(),
                bin.engine_hash.to_hex()
            ),
        ));
        return Ok(());
    }
    // The hypothesis is the one recorded; a changed file ends the walk here,
    // since nothing below can hold for it (LOOP-13).
    let base = runs
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let hyp_path = base.join(&hyp_rel);
    match identity_hash(&hyp_path) {
        Some(h) if h == hyp_hash => {}
        _ => {
            c.findings.push(finding(
                Code::HypothesisChanged,
                format!(
                    "{} no longer has the recorded hash {hyp_hash} (LOOP-13, LOOP-14)",
                    hyp_path.display()
                ),
            ));
            return Ok(());
        }
    }
    let h = match crate::load_in(&hyp_path, &base) {
        Ok(h) => h,
        Err(e) => {
            c.findings.push(finding(Code::Report, e.to_string()));
            return Ok(());
        }
    };
    // The final verdict recomputes to the bytes on disk (HYP-15, HYP-20).
    match verdict::verdict(&h, data, bin.engine_hash) {
        Err(e) => c
            .findings
            .push(finding(Code::VerdictRefused, e.to_string())),
        Ok(v) => {
            c.verdicts += 1;
            let path = runs.join("verdicts").join(&verdict_id).join("verdict.json");
            if v.verdict_id.to_hex() != verdict_id {
                c.findings.push(finding(
                    Code::VerdictMismatch,
                    format!(
                        "the bundles give verdict_id {}, not the recorded {verdict_id} (HYP-15)",
                        v.verdict_id.to_hex()
                    ),
                ));
            } else if std::fs::read_to_string(&path).ok().as_deref() != Some(v.text().as_str()) {
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
    }
    // The report regenerates, and with it every L1 bundle (LOOP-14).
    match loop_run::regenerate(&report, bin, exec) {
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

/// The BLAKE3 of a file as hex, or `None` when it cannot be read.
fn identity_hash(path: &Path) -> Option<String> {
    acn_trace::identity::file_hash(path)
        .ok()
        .map(|d| d.to_hex())
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

/// Whether `v` is a verdict of the file whose hash is `hash`: its verdict_id is
/// recomputed from that hash and its bundles (HYP-15).
fn judged_by(hash: &Digest, v: &Verdict) -> bool {
    let pairs: Vec<(Digest, Digest)> = v.bundles.iter().map(|b| (b.0, b.1)).collect();
    verdict::verdict_id(hash, &pairs).is_ok_and(|id| id == v.verdict_id)
}

/// LOOP-4, the gate into L3. A hypothesis that declares `twin_required =
/// false` waives it. Otherwise all of these must hold:
/// - `l1` is an L1 verdict of `h`;
/// - `l2` is an L2 verdict of `h` whose sim bundles are exactly `l1`'s, so it
///   twins the verdict being promoted;
/// - `l2` records no `twin_failed`;
/// - `l2`'s live bundles twin every decision cell of `l1` (HYP-22).
///
/// Omitting a live bundle therefore cannot pass it. Both verdicts must be ones
/// `evidence::verify` accepts; the caller checks that (ADR-24).
pub fn promote_gate(h: &Hypothesis, l1: &Verdict, l2: Option<&Verdict>) -> Result<(), LoopError> {
    if !h.design().twin_required {
        return Ok(());
    }
    let refuse = |m: String| loop_run::err(Code::PromoteRefused, format!("{m} (LOOP-4)"));
    let hash = h.hash();
    if !judged_by(&hash, l1) {
        return refuse(format!(
            "verdict {} is not a verdict of {}",
            l1.verdict_id.to_hex(),
            h.id()
        ));
    }
    if layer::of_verdict(l1) != Layer::L1 {
        return refuse(format!(
            "verdict {} is not an L1 verdict",
            l1.verdict_id.to_hex()
        ));
    }
    let Some(l2) = l2 else {
        return refuse("no L2 verdict: the hypothesis requires a twin".into());
    };
    if !judged_by(&hash, l2) {
        return refuse(format!(
            "verdict {} is not a verdict of {}",
            l2.verdict_id.to_hex(),
            h.id()
        ));
    }
    if layer::of_verdict(l2) != Layer::L2 {
        return refuse(format!(
            "verdict {} holds no live twin of the mock, so it is not L2",
            l2.verdict_id.to_hex()
        ));
    }
    let l1_runs: BTreeSet<Digest> = l1.bundles.iter().map(|b| b.0).collect();
    let l2_sim: BTreeSet<Digest> = l2
        .bundles
        .iter()
        .filter(|b| b.2 == Mode::Sim)
        .map(|b| b.0)
        .collect();
    if l1_runs != l2_sim {
        return refuse(format!(
            "the L2 verdict's sim bundles are not exactly those of the L1 verdict {}",
            l1.verdict_id.to_hex()
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
            let label = |k: String| {
                if s.key.is_empty() {
                    k
                } else {
                    format!("{}: {k}", s.key)
                }
            };
            // A decision cell that cannot be found is not twinned: the gate
            // fails closed.
            let Some(cell) = s.data.cells().get(i) else {
                untwinned.push(label(format!("#{i}")));
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
                untwinned.push(label(k));
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
