//! The L1 loop runner (SPEC 085: LOOP-10, LOOP-11, LOOP-13, LOOP-14, LOOP-15).
//! Every decision of a loop is made here, from the hypothesis file and the
//! bundles alone: which cell runs next, when the loop stops, every verdict and
//! every word of the report. The bundles come from an [`Executor`], which
//! `acn-cli` supplies with the harness; this crate does not depend on the
//! harness, so the frozen code decides and the run path only runs.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use acn_trace::bundle::MOCK_BACKEND;
use acn_trace::identity::{
    self, Digest, HypStatus, Mode, OptionDecl, Preimage, RunIdentity, RunParams,
};
use rand_chacha::ChaCha20Rng;
use rand_core::Rng as _;

use crate::Status;
use crate::bootstrap::below;
use crate::file::{Control, Domain, Hypothesis};
use crate::json::J;
use crate::loop_out;
use crate::read::{self, BundleData};
use crate::slice::{Cell, Value, cell_order, key};
use crate::verdict::{
    self, Controls, Role, V, Verdict, VerdictError, cell_json, product, reasons_json, sub,
    values_of,
};

/// The `format` of `report.json` (LOOP-11); a change to its layout takes a new
/// version.
pub const REPORT_FORMAT: &str = "acn-bench/loop-report/v1";
/// The sub-stream of the run seed `random` draws from (LOOP-14, CON-30(b)).
pub const SEARCH_STREAM: &str = "loop.search";
/// Consecutive draws of an already-run cell that exhaust `random` (LOOP-10(b)).
pub const MAX_REDRAWS: u32 = 1000;
/// `report.json` and its rendering, `report.md` (LOOP-11).
pub const REPORT_JSON: &str = "report.json";
pub const REPORT_MD: &str = "report.md";

/// Why a loop refused to start, or aborted (LOOP-10(e), LOOP-10(f)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    // Refusals, before anything runs.
    Search,
    NoControl,
    WorkloadControl,
    Backends,
    UnknownModel,
    BadMap,
    Workload,
    Pins,
    BudgetTooSmall,
    LoopExists,
    Path,
    Report,
    // Aborts.
    VerdictRefused,
    BuildMismatch,
    BundleInvalid,
    BundleIncomplete,
    BudgetRefused,
    NoLoopReport,
    LayerMismatch,
    VerdictMismatch,
    NotRegenerated,
    TwinRefused,
    PromoteRefused,
    ExecutorFailed,
    ExecutorMismatch,
    HypothesisChanged,
    InputChanged,
    VerdictConflict,
    NotRegenerable,
    Io,
    Internal,
}

impl Code {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search_refused",
            Self::NoControl => "no_control",
            Self::WorkloadControl => "workload_control",
            Self::Backends => "backend_refused",
            Self::UnknownModel => "unknown_model",
            Self::BadMap => "bad_map",
            Self::Workload => "workload_refused",
            Self::Pins => "pins_refused",
            Self::BudgetTooSmall => "budget_too_small",
            Self::LoopExists => "loop_exists",
            Self::Path => "path_refused",
            Self::Report => "report_refused",
            Self::VerdictRefused => "verdict_refused",
            Self::BuildMismatch => "build_mismatch",
            Self::BundleInvalid => "bundle_invalid",
            Self::BundleIncomplete => "bundle_incomplete",
            Self::BudgetRefused => "budget_refused",
            Self::NoLoopReport => "no_loop_report",
            Self::LayerMismatch => "layer_mismatch",
            Self::VerdictMismatch => "verdict_mismatch",
            Self::NotRegenerated => "not_regenerated",
            Self::TwinRefused => "twin_refused",
            Self::PromoteRefused => "promote_refused",
            Self::ExecutorFailed => "executor_failed",
            Self::ExecutorMismatch => "executor_mismatch",
            Self::HypothesisChanged => crate::HYPOTHESIS_CHANGED,
            Self::InputChanged => "input_changed",
            Self::VerdictConflict => "verdict_conflict",
            Self::NotRegenerable => "not_regenerable_with_this_build",
            Self::Io => "io",
            Self::Internal => "internal",
        }
    }
}

/// A loop that did not complete: its code and what happened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: {message}", code.as_str())]
pub struct LoopError {
    pub code: Code,
    pub message: String,
}

pub(crate) fn err<T>(code: Code, message: impl Into<String>) -> Result<T, LoopError> {
    Err(LoopError {
        code,
        message: message.into(),
    })
}

fn internal(e: impl std::fmt::Display) -> LoopError {
    LoopError {
        code: Code::Internal,
        message: e.to_string(),
    }
}

pub(crate) fn io_err(path: &Path, e: impl std::fmt::Display) -> LoopError {
    LoopError {
        code: Code::Io,
        message: format!("{}: {e}", path.display()),
    }
}

/// The binary a loop runs on: the `engine_hash` and `build_hash` it embeds
/// (CON-28, CON-31).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binary {
    pub engine_hash: Digest,
    pub build_hash: Digest,
}

/// One bundle the runner asks for: the inputs of HAR-50 for a hypothesis file,
/// whose seed the harness derives from the file (HYP-9), in `sim` on the mock
/// with every run option at its default (LOOP-10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub hypothesis: PathBuf,
    pub workload: PathBuf,
    pub model: String,
    /// `vary.<name>` for every `[varies]` parameter, as CON-27(c) text.
    pub vary: BTreeMap<String, String>,
    pub arm: Role,
    pub replicates: u32,
    /// The directory the bundle goes under, as `<runs_dir>/<run_id>/`.
    pub runs_dir: PathBuf,
    /// Where the workspace root is looked for (CON-28): the directory `runs/`
    /// lies in, so that the harness decides the file's status as the loop did.
    pub start_dir: PathBuf,
}

/// What makes the bundles (LOOP-15). It answers two questions the runner
/// cannot answer without the run path, and runs one request at a time.
pub trait Executor {
    /// Run `request` into one bundle and return its directory.
    fn run(&mut self, request: &Request) -> Result<PathBuf, String>;
    /// Whether `model` names a mock profile (MLM-50).
    fn check_model(&self, model: &str) -> Result<(), String>;
    /// Whether `path` loads as a workload (HAR-60).
    fn check_workload(&self, path: &Path) -> Result<(), String>;
}

/// The inputs of `acn loop run` beside the hypothesis, as given: each
/// `--workload` and `--model` argument, a path or profile, or `<value>=…` when
/// mapped per value of `workload` or `provider` (LOOP-10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub workloads: Vec<String>,
    pub models: Vec<String>,
    pub budget: u64,
}

/// A loop that completed (LOOP-11).
#[derive(Debug, Clone, PartialEq)]
pub struct Completed {
    pub loop_id: Digest,
    pub report: PathBuf,
    pub verdict_id: Digest,
    pub verdict: V,
    pub stop: Stop,
    /// Every bundle, ascending.
    pub run_ids: Vec<Digest>,
}

/// A regeneration's outcome (LOOP-14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Regenerated {
    pub loop_id: Digest,
    /// `runs/regen/<loop_id>/<n>/`.
    pub dir: PathBuf,
    /// What differs from the original: `report.json`, `report.md`,
    /// `verdict.json`, or `bundle <run_id>`; empty when identical.
    pub differ: Vec<String>,
}

impl Regenerated {
    #[must_use]
    pub fn identical(&self) -> bool {
        self.differ.is_empty()
    }
}

/// Why a loop stopped (LOOP-10(d)): never the value of the verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The next batch would exceed the budget.
    Budget,
    /// The strategy has no cell left.
    Exhausted,
}

impl Stop {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Budget => "budget",
            Self::Exhausted => "exhausted",
        }
    }
}

/// An input file the loop reads: its path relative to the base, where it is,
/// and its hash.
#[derive(Debug, Clone)]
struct Input {
    rel: String,
    abs: PathBuf,
    hash: Digest,
}

/// Everything a loop is decided from, checked before anything runs.
struct Setup<'h> {
    h: &'h Hypothesis,
    /// The directory `runs/` lies in: the workspace root inside a workspace
    /// (CON-28). Relative inputs resolve against it and are recorded relative
    /// to it, and the executor looks for the root from it.
    base: PathBuf,
    /// Relative to the directory `runs/` lies in: the workspace root inside a
    /// workspace (CON-28).
    hypothesis: Input,
    /// By `workload` value, or the one file under `""`.
    workloads: BTreeMap<String, Input>,
    /// By `provider` value, or the one profile under `""`.
    models: BTreeMap<String, String>,
    budget: u64,
    seed: u64,
    status: HypStatus,
    bin: Binary,
    loop_id: Digest,
    non_pooled: BTreeSet<String>,
    options: Vec<(String, acn_trace::schema::ValueType, String)>,
}

/// The parameter a workload map is keyed by, and a model map.
const WORKLOAD_PARAM: &str = "workload";
const PROVIDER_PARAM: &str = "provider";

/// A value's CON-27(c) text in a run's identity.
fn ident_value(v: &Value) -> identity::Value {
    match v {
        Value::Bool(b) => identity::Value::Bool(*b),
        Value::Enum(s) => identity::Value::Str(s.clone()),
        Value::Int(i) => identity::Value::Int(*i),
        Value::Float(f) => identity::Value::Float(f.get()),
    }
}

/// `path`, resolved against `base` when relative (CON-28), made absolute and
/// recorded relative to `base` with `/` separators; refused when it lies
/// outside `base`, since a report records paths relative to it. A file that
/// cannot be read is refused with `unreadable`.
fn input(base: &Path, path: &Path, what: &str, unreadable: Code) -> Result<Input, LoopError> {
    let abs = std::fs::canonicalize(base.join(path)).map_err(|e| LoopError {
        code: unreadable,
        message: format!("{what} {}: {e}", path.display()),
    })?;
    let Ok(rel) = abs.strip_prefix(base) else {
        return err(
            Code::Path,
            format!(
                "{what} {} lies outside {}, which the report's paths are relative to (CON-28)",
                abs.display(),
                base.display()
            ),
        );
    };
    let rel = acn_trace::env::rel_path(base, rel).map_err(internal)?;
    let hash = identity::file_hash(&abs).map_err(|e| io_err(&abs, e))?;
    Ok(Input { rel, abs, hash })
}

/// The values of a finite parameter as text, in declaration order.
fn value_texts(h: &Hypothesis, name: &str) -> Result<Option<Vec<String>>, LoopError> {
    let Some(p) = h.params().get(name) else {
        return Ok(None);
    };
    match values_of(name, &p.domain) {
        Ok(v) => Ok(Some(v.iter().map(Value::text).collect())),
        Err(_) => err(
            Code::BadMap,
            format!("`{name}` has no finite set of values to map (LOOP-10)"),
        ),
    }
}

/// A `--workload` or `--model` map (LOOP-10): one unmapped argument, or one
/// `<value>=…` per value of `param`, naming every value and nothing else.
fn parse_map(
    h: &Hypothesis,
    param: &str,
    args: &[String],
    required: bool,
    flag: &str,
) -> Result<BTreeMap<String, String>, LoopError> {
    let one = || match args {
        [a] => Ok(BTreeMap::from([(String::new(), a.clone())])),
        _ => err(
            Code::BadMap,
            format!("give exactly one {flag}, or one per value of `{param}` (LOOP-10)"),
        ),
    };
    let Some(values) = value_texts(h, param)? else {
        return one();
    };
    if !required && matches!(args, [a] if !a.contains('=')) {
        return one();
    }
    let mut map = BTreeMap::new();
    for a in args {
        let Some((v, x)) = a.split_once('=') else {
            return err(
                Code::BadMap,
                format!(
                    "{flag} `{a}` is not `<{param} value>=…`: the file varies `{param}` (LOOP-10)"
                ),
            );
        };
        if map.insert(v.to_owned(), x.to_owned()).is_some() {
            return err(Code::BadMap, format!("{flag} maps `{v}` twice"));
        }
    }
    let keys: BTreeSet<&String> = map.keys().collect();
    let want: BTreeSet<&String> = values.iter().collect();
    if keys != want {
        return err(
            Code::BadMap,
            format!(
                "{flag} must map every value of `{param}` and nothing else: {want:?}, given {keys:?} (LOOP-10)"
            ),
        );
    }
    Ok(map)
}

/// The control's configuration a treatment cell maps to (HYP-8).
fn control_of(s: &Setup<'_>, cell: &Cell) -> Result<Cell, LoopError> {
    Controls::new(s.h, &s.non_pooled)
        .of_treatment(cell)
        .ok_or_else(|| internal("a checked control config maps no cell"))
}

/// Every grid cell, slices in key order and cells in HYP-14 order (LOOP-10(b)).
fn grid(h: &Hypothesis, non_pooled: &BTreeSet<String>) -> Result<Vec<Cell>, LoopError> {
    let params = h
        .params()
        .iter()
        .map(|(n, p)| values_of(n, &p.domain).map(|v| (n.clone(), v)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
    let mut cells = product(&params);
    cells.sort_by(|a, b| {
        key(&sub(a, non_pooled))
            .cmp(&key(&sub(b, non_pooled)))
            .then_with(|| cell_order(h, a, b))
    });
    Ok(cells)
}

/// One draw of `random` (LOOP-10(b)): each parameter in bytewise name order.
fn draw(h: &Hypothesis, rng: &mut ChaCha20Rng) -> Result<Cell, LoopError> {
    let index = |rng: &mut ChaCha20Rng, n: usize| -> Result<usize, LoopError> {
        let n = NonZeroU64::new(u64::try_from(n).map_err(internal)?)
            .ok_or_else(|| internal("a parameter with no values"))?;
        usize::try_from(below(rng, n)).map_err(internal)
    };
    let mut cell = Cell::new();
    for (name, p) in h.params() {
        let v = match &p.domain {
            Domain::Bool => Value::Bool(index(rng, 2)? == 1),
            Domain::Enum(vals) => Value::Enum(vals[index(rng, vals.len())?].clone()),
            Domain::Range {
                levels: Some(l), ..
            } => Value::float(l[index(rng, l.len())?])
                .ok_or_else(|| internal("a non-finite level"))?,
            Domain::IntRange {
                levels: Some(l), ..
            } => Value::Int(l[index(rng, l.len())?]),
            Domain::IntRange {
                min,
                max,
                levels: None,
            } => {
                // max − min + 1 values; the whole of i64 is 2^64 of them.
                let span = max.abs_diff(*min).checked_add(1);
                let off = match span.and_then(NonZeroU64::new) {
                    Some(n) => below(rng, n),
                    None => rng.next_u64(),
                };
                Value::Int(
                    min.checked_add_unsigned(off)
                        .ok_or_else(|| internal("an int_range draw overflowed"))?,
                )
            }
            Domain::Range {
                min,
                max,
                levels: None,
            } => {
                #[allow(clippy::cast_precision_loss)] // 53 bits: exact
                let u = (rng.next_u64() >> 11) as f64 * f64::powi(2.0, -53);
                // LOOP-10(b)'s formula; its width is finite (checked at setup),
                // and the result is clamped against rounding past `max` (ADR-23).
                let x = (min + (max - min) * u).clamp(*min, *max);
                Value::float(x).ok_or_else(|| LoopError {
                    code: Code::Internal,
                    message: format!("`{name}`: a draw over [{min}, {max}] is not finite"),
                })?
            }
        };
        cell.insert(name.clone(), v);
    }
    Ok(cell)
}

/// HYP-9's run seed: a candidate's `[design].seed`, or else the seed derived
/// from the file's hash, which is a frozen file's always.
fn run_seed(h: &Hypothesis) -> Result<u64, LoopError> {
    match h.design().seed {
        Some(s) => Ok(s),
        None => identity::hypothesis_seed(&h.hash()).map_err(internal),
    }
}

/// The first `n` draws of `random` for `h`, repeats included (LOOP-10(b)):
/// what a known-answer test pins.
pub fn random_draws(h: &Hypothesis, n: usize) -> Result<Vec<Cell>, LoopError> {
    let mut rng = identity::substream_rng(run_seed(h)?, SEARCH_STREAM).map_err(internal)?;
    (0..n).map(|_| draw(h, &mut rng)).collect()
}

/// The cells a loop runs, in order.
enum Strategy {
    Grid(std::vec::IntoIter<Cell>),
    Random {
        rng: Box<ChaCha20Rng>,
        seen: BTreeSet<String>,
    },
}

impl Strategy {
    fn new(s: &Setup<'_>) -> Result<Self, LoopError> {
        match s.h.design().search.as_str() {
            "grid" => Ok(Self::Grid(grid(s.h, &s.non_pooled)?.into_iter())),
            "random" => Ok(Self::Random {
                rng: Box::new(identity::substream_rng(s.seed, SEARCH_STREAM).map_err(internal)?),
                seen: BTreeSet::new(),
            }),
            other => err(
                Code::Search,
                format!("search `{other}` is not run (LOOP-10(b))"),
            ),
        }
    }

    /// The next cell, or `None` when the strategy is exhausted.
    fn next(&mut self, h: &Hypothesis) -> Result<Option<Cell>, LoopError> {
        match self {
            Self::Grid(it) => Ok(it.next()),
            Self::Random { rng, seen } => {
                for _ in 0..=MAX_REDRAWS {
                    let c = draw(h, rng)?;
                    if seen.insert(key(&c)) {
                        return Ok(Some(c));
                    }
                }
                Ok(None)
            }
        }
    }
}

/// `loop_id` (LOOP-11): `blake3("acn-bench/loop_id/v1\0" ‖ hypothesis_hash ‖
/// scenario_hash ‖ engine_hash ‖ hyp_status ‖ workloads ‖ models ‖ strategy ‖
/// budget)`, each map its entry count (u32) and then, by value bytewise, the
/// value and the file's hash or the profile.
pub fn loop_id(
    hypothesis_hash: &Digest,
    engine_hash: &Digest,
    hyp_status: &str,
    workloads: &BTreeMap<String, Digest>,
    models: &BTreeMap<String, String>,
    strategy: &str,
    budget: u64,
) -> Result<Digest, identity::IdentityError> {
    let mut p = Preimage::new("acn-bench/loop_id/v1")?
        .digest(hypothesis_hash)
        .digest(&Digest::ZERO)
        .digest(engine_hash)
        .str(hyp_status)?;
    let count = |n: usize| {
        u32::try_from(n)
            .map_err(|_| identity::IdentityError::Invalid("a map of 2^32 entries".into()))
    };
    p = p.u32(count(workloads.len())?);
    for (v, h) in workloads {
        p = p.str(v)?.digest(h);
    }
    p = p.u32(count(models.len())?);
    for (v, m) in models {
        p = p.str(v)?.str(m)?;
    }
    Ok(p.str(strategy)?.u64(budget).finish())
}

impl<'h> Setup<'h> {
    /// Check everything LOOP-10(e) refuses before anything runs.
    fn new(
        h: &'h Hypothesis,
        args: &Args,
        runs_dir: &Path,
        bin: Binary,
        exec: &dyn Executor,
    ) -> Result<Self, LoopError> {
        let d = h.design();
        let frozen = h.status() == Status::Frozen;
        match d.search.as_str() {
            "grid" => {}
            "random" if !frozen => {}
            "random" => {
                return err(
                    Code::Search,
                    "a frozen hypothesis runs as a grid (HYP-9, LOOP-10(e))",
                );
            }
            other => {
                return err(
                    Code::Search,
                    format!(
                        "search `{other}` is not specified yet (SPEC 085 §6 question 2, LOOP-10(b))"
                    ),
                );
            }
        }
        if d.search == "random"
            && let Some((name, _)) = h.params().iter().find(|(_, p)| {
                matches!(p.domain, Domain::Range { min, max, levels: None } if !(max - min).is_finite())
            })
        {
            return err(
                Code::Search,
                format!("`{name}`: random cannot draw over a range whose width max − min overflows (LOOP-10(b))"),
            );
        }
        match h.control() {
            Control::Config(_) => {}
            Control::Missing => {
                return err(
                    Code::NoControl,
                    "a loop without a control measures nothing (CON-18, LOOP-10(e))",
                );
            }
            Control::Workload { .. } => {
                return err(
                    Code::WorkloadControl,
                    "a workload control runs once SPEC 050's generator modes exist (LOOP-10(e))",
                );
            }
        }
        if !d.backends.is_empty() && !d.backends.iter().any(|b| b == MOCK_BACKEND) {
            return err(
                Code::Backends,
                "[design].backends does not admit the mock (HYP-9, LOOP-10(e))",
            );
        }
        // The base: the directory runs/ lies in (CON-28).
        verdict::check_runs_dir(runs_dir).map_err(|e| LoopError {
            code: Code::Path,
            message: e.to_string(),
        })?;
        let parent = match runs_dir.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let base = std::fs::canonicalize(&parent).map_err(|e| io_err(&parent, e))?;
        // A file loaded but no longer readable has changed (HYP-4).
        let hypothesis = input(&base, h.path(), "hypothesis", Code::HypothesisChanged)?;
        if hypothesis.hash != h.hash() {
            return err(
                Code::HypothesisChanged,
                format!("{} changed after it was loaded (HYP-4)", h.path().display()),
            );
        }

        let wmap = parse_map(
            h,
            WORKLOAD_PARAM,
            &args.workloads,
            h.params().contains_key(WORKLOAD_PARAM),
            "--workload",
        )?;
        let mut workloads = BTreeMap::new();
        for (v, path) in &wmap {
            let w = input(&base, Path::new(path), "workload", Code::Workload)?;
            exec.check_workload(&w.abs).map_err(|e| LoopError {
                code: Code::Workload,
                message: format!("{path}: {e}"),
            })?;
            workloads.insert(v.clone(), w);
        }
        let models = parse_map(h, PROVIDER_PARAM, &args.models, false, "--model")?;
        for m in models.values() {
            exec.check_model(m).map_err(|e| LoopError {
                code: Code::UnknownModel,
                message: format!("`{m}`: {e} (MLM-50)"),
            })?;
        }
        if models.len() > 1 && h.params().get(PROVIDER_PARAM).is_some_and(|p| p.pooled) {
            return err(
                Code::BadMap,
                "`provider` is pooled, so one slice would mix models (HYP-20)",
            );
        }
        if let Some(pins) = &d.pins {
            for (v, w) in &workloads {
                if !pins.workload.contains(&w.hash.to_hex()) {
                    return err(
                        Code::Pins,
                        format!(
                            "workload `{v}` ({}) is not among pins.workload (HYP-20)",
                            w.rel
                        ),
                    );
                }
            }
            if !pins.scenario.contains(&Digest::ZERO.to_hex()) {
                return err(
                    Code::Pins,
                    "the loop runs with no scenario (hash zero), which pins.scenario does not list (HYP-20)",
                );
            }
            let keys: Vec<String> = match value_texts(h, PROVIDER_PARAM)? {
                Some(v) => v,
                None => vec![MOCK_BACKEND.to_owned()],
            };
            for k in keys {
                let m = models.get(&k).or_else(|| models.get("")).cloned();
                if pins.models.get(&k) != m.as_ref() {
                    return err(
                        Code::Pins,
                        format!(
                            "`{k}` would run `{}`, which pins.models does not name for it (HYP-20)",
                            m.unwrap_or_default()
                        ),
                    );
                }
            }
        }
        if args.budget > u64::try_from(i64::MAX).unwrap_or(u64::MAX) {
            return err(
                Code::BudgetRefused,
                "a budget beyond 2^63 − 1 bundles cannot be recorded (LOOP-11)",
            );
        }
        let status = match h.status() {
            Status::Frozen => HypStatus::Frozen,
            Status::Candidate => HypStatus::Candidate,
        };
        let seed = run_seed(h)?;
        let loop_id = loop_id(
            &h.hash(),
            &bin.engine_hash,
            status.as_str(),
            &workloads.iter().map(|(v, w)| (v.clone(), w.hash)).collect(),
            &models,
            &d.search,
            args.budget,
        )
        .map_err(internal)?;
        let inv = acn_trace::schema::inventory().map_err(internal)?;
        let options = identity::options(&inv)
            .iter()
            .map(|o| (o.name.to_owned(), o.ty, o.default.to_owned()))
            .collect();
        let non_pooled = h
            .params()
            .iter()
            .filter(|(_, p)| !p.pooled)
            .map(|(n, _)| n.clone())
            .collect();
        let s = Self {
            h,
            base,
            hypothesis,
            workloads,
            models,
            budget: args.budget,
            seed,
            status,
            bin,
            loop_id,
            non_pooled,
            options,
        };
        // The budget: at least the first batch, and for a frozen file the whole
        // grid, which its verdict reads (HYP-21).
        if s.budget < 2 {
            return err(
                Code::BudgetTooSmall,
                format!(
                    "budget {} is smaller than the first batch, 2 bundles (LOOP-10(e))",
                    s.budget
                ),
            );
        }
        if frozen {
            let cells = grid(h, &s.non_pooled)?;
            let mut controls = BTreeSet::new();
            for c in &cells {
                controls.insert(key(&control_of(&s, c)?));
            }
            let need = u64::try_from(cells.len() + controls.len()).map_err(internal)?;
            if s.budget < need {
                return err(
                    Code::BudgetTooSmall,
                    format!(
                        "budget {} is smaller than the grid, {need} bundles, which a frozen verdict reads (HYP-21, LOOP-10(e))",
                        s.budget
                    ),
                );
            }
        }
        Ok(s)
    }

    fn workload_of(&self, cell: &Cell) -> Result<&Input, LoopError> {
        let k = if self.workloads.contains_key("") {
            String::new()
        } else {
            cell.get(WORKLOAD_PARAM)
                .map(Value::text)
                .unwrap_or_default()
        };
        self.workloads
            .get(&k)
            .ok_or_else(|| internal(format!("no workload for `{k}`")))
    }

    fn model_of(&self, cell: &Cell) -> Result<&String, LoopError> {
        let k = if self.models.contains_key("") {
            String::new()
        } else {
            cell.get(PROVIDER_PARAM)
                .map(Value::text)
                .unwrap_or_default()
        };
        self.models
            .get(&k)
            .ok_or_else(|| internal(format!("no model for `{k}`")))
    }

    /// The run_id the harness will give `cell` and `arm` (CON-29), computed
    /// here so that every bundle returned can be checked against it.
    fn run_id(&self, cell: &Cell, arm: Role) -> Result<Digest, LoopError> {
        let options: Vec<OptionDecl<'_>> = self
            .options
            .iter()
            .map(|(n, t, d)| OptionDecl {
                name: n,
                ty: *t,
                default: d,
            })
            .collect();
        let params = RunParams {
            backend: MOCK_BACKEND.into(),
            model: self.model_of(cell)?.clone(),
            hyp_status: self.status,
            arms: vec![arm.as_str().into()],
            replicates: self.h.design().replicates,
            vary: cell
                .iter()
                .map(|(k, v)| (k.clone(), ident_value(v)))
                .collect(),
            opts: BTreeMap::new(),
        };
        let pairs = params.pairs(&options).map_err(internal)?;
        RunIdentity {
            seed: self.seed,
            scenario_hash: Digest::ZERO,
            workload_hash: self.workload_of(cell)?.hash,
            hypothesis_hash: self.h.hash(),
            engine_hash: self.bin.engine_hash,
            mode: Mode::Sim,
            params_hash: identity::params_hash(&pairs).map_err(internal)?,
        }
        .run_id()
        .map_err(internal)
    }

    /// LOOP-13: the hypothesis and every workload file still have the bytes
    /// the loop started with.
    fn check_inputs(&self) -> Result<(), LoopError> {
        self.h.check_unchanged().map_err(|e| LoopError {
            code: Code::HypothesisChanged,
            message: e.to_string(),
        })?;
        for w in self.workloads.values() {
            match identity::file_hash(&w.abs) {
                Ok(h) if h == w.hash => {}
                _ => {
                    return err(
                        Code::InputChanged,
                        format!("workload {} changed during the loop (LOOP-13)", w.rel),
                    );
                }
            }
        }
        Ok(())
    }

    /// The bundle of `cell` and `arm` that already exists under `out`, checked
    /// for reuse (LOOP-11): it verifies (TRC-23) and comes from this binary's
    /// build and engine. `None` when there is none, or when reuse is off.
    fn existing(
        &self,
        out: &Path,
        reuse: bool,
        cell: &Cell,
        arm: Role,
    ) -> Result<Option<BundleData>, LoopError> {
        let expected = self.run_id(cell, arm)?;
        let dir = out.join(expected.to_hex());
        if !reuse || std::fs::symlink_metadata(&dir).is_err() {
            return Ok(None);
        }
        if std::fs::symlink_metadata(dir.join(acn_trace::bundle::MANIFEST)).is_err() {
            return err(
                Code::BundleIncomplete,
                format!(
                    "{} has no {}: a run that did not finish. Remove the directory and run the loop again (CON-29)",
                    dir.display(),
                    acn_trace::bundle::MANIFEST
                ),
            );
        }
        let b = read::read(&dir).map_err(|e| LoopError {
            code: Code::BundleInvalid,
            message: format!("an existing bundle does not verify (TRC-23): {e}"),
        })?;
        self.check(&b, expected, cell, false)?;
        Ok(Some(b))
    }

    /// The executor's bundle of `cell` and `arm`, checked against what was
    /// asked (LOOP-15).
    fn make(
        &self,
        exec: &mut dyn Executor,
        out: &Path,
        cell: &Cell,
        arm: Role,
    ) -> Result<BundleData, LoopError> {
        let expected = self.run_id(cell, arm)?;
        let dir = out.join(expected.to_hex());
        let request = Request {
            hypothesis: self.hypothesis.abs.clone(),
            workload: self.workload_of(cell)?.abs.clone(),
            model: self.model_of(cell)?.clone(),
            vary: cell.iter().map(|(k, v)| (k.clone(), v.text())).collect(),
            arm,
            replicates: self.h.design().replicates,
            runs_dir: out.to_path_buf(),
            start_dir: self.base.clone(),
        };
        let got = exec.run(&request).map_err(|e| LoopError {
            code: Code::ExecutorFailed,
            message: format!("{} {}: {e}", arm.as_str(), key(cell)),
        })?;
        let b = read::read(&got).map_err(|e| LoopError {
            code: Code::ExecutorMismatch,
            message: format!("the bundle returned does not verify: {e}"),
        })?;
        self.check(&b, expected, cell, true)?;
        if std::fs::canonicalize(&got).ok() != std::fs::canonicalize(&dir).ok() {
            return err(
                Code::ExecutorMismatch,
                format!(
                    "the executor returned {}, not {} (LOOP-15)",
                    got.display(),
                    dir.display()
                ),
            );
        }
        Ok(b)
    }

    /// A bundle is the one expected: made from the file and workload the loop
    /// read (LOOP-13), with the expected run_id, by this binary (LOOP-15,
    /// CON-31).
    fn check(
        &self,
        b: &BundleData,
        expected: Digest,
        cell: &Cell,
        made: bool,
    ) -> Result<(), LoopError> {
        let m = &b.manifest;
        if m.hypothesis.hash != self.h.hash().to_hex() {
            return err(
                Code::HypothesisChanged,
                format!(
                    "{} was made from hypothesis {}, not {} (LOOP-13)",
                    m.run_id,
                    m.hypothesis.hash,
                    self.h.hash().to_hex()
                ),
            );
        }
        let workload = self.workload_of(cell)?;
        if m.workload_hash != workload.hash.to_hex() {
            return err(
                Code::InputChanged,
                format!(
                    "{} was made from workload {}, not {}'s {} (LOOP-13)",
                    m.run_id,
                    m.workload_hash,
                    workload.rel,
                    workload.hash.to_hex()
                ),
            );
        }
        let what = if made {
            "the executor returned"
        } else {
            "an existing bundle is"
        };
        if b.run_id != expected {
            return err(
                if made {
                    Code::ExecutorMismatch
                } else {
                    Code::BundleInvalid
                },
                format!(
                    "{what} run {}, not the expected {} (CON-29, LOOP-15)",
                    m.run_id,
                    expected.to_hex()
                ),
            );
        }
        let mismatch = if made {
            Code::ExecutorMismatch
        } else {
            Code::BuildMismatch
        };
        if m.build.build_hash != self.bin.build_hash.to_hex()
            || m.engine_hash != self.bin.engine_hash.to_hex()
        {
            return err(
                mismatch,
                format!(
                    "{what} {} from build {} and engine {}, not this binary's {} and {} (CON-31, LOOP-11)",
                    m.run_id,
                    m.build.build_hash,
                    m.engine_hash,
                    self.bin.build_hash.to_hex(),
                    self.bin.engine_hash.to_hex()
                ),
            );
        }
        if m.seed != self.seed.to_string() {
            return err(
                mismatch,
                format!("{what} {} under seed {}", m.run_id, m.seed),
            );
        }
        Ok(())
    }
}

/// One batch as the report records it.
#[derive(Debug, Clone)]
struct Batch {
    cell: Cell,
    run_ids: Vec<Digest>,
    verdict: Verdict,
}

/// A loop run to its stop.
struct Done {
    batches: Vec<Batch>,
    bundles: Vec<BundleData>,
    stop: Stop,
}

impl Done {
    fn verdict(&self) -> Result<&Verdict, LoopError> {
        self.batches
            .last()
            .map(|b| &b.verdict)
            .ok_or_else(|| internal("no batch ran"))
    }
}

/// Run batches until the budget or the strategy stops the loop (LOOP-10).
fn execute(
    s: &Setup<'_>,
    out: &Path,
    reuse: bool,
    exec: &mut dyn Executor,
) -> Result<Done, LoopError> {
    let mut strategy = Strategy::new(s)?;
    let mut spent = 0u64;
    let mut controls: BTreeSet<String> = BTreeSet::new();
    let mut bundles: Vec<BundleData> = Vec::new();
    let mut batches = Vec::new();
    let stop = loop {
        let Some(cell) = strategy.next(s.h)? else {
            break Stop::Exhausted;
        };
        let control = control_of(s, &cell)?;
        let need_control = !controls.contains(&key(&control));
        let cost = 1 + u64::from(need_control);
        if spent + cost > s.budget {
            break Stop::Budget;
        }
        s.check_inputs()?;
        // LOOP-10(f): every existing bundle the batch would use is checked
        // before any of the batch runs.
        let mut wanted = vec![(cell.clone(), Role::Treatment)];
        if need_control {
            wanted.push((control.clone(), Role::Control));
        }
        let found = wanted
            .iter()
            .map(|(c, arm)| s.existing(out, reuse, c, *arm))
            .collect::<Result<Vec<_>, _>>()?;
        let mut run_ids = Vec::new();
        for ((c, arm), have) in wanted.iter().zip(found) {
            let b = match have {
                Some(b) => b,
                None => s.make(exec, out, c, *arm)?,
            };
            run_ids.push(b.run_id);
            bundles.push(b);
        }
        if need_control {
            controls.insert(key(&control));
        }
        spent += cost;
        // LOOP-10(c): the verdict over every bundle so far, by the one function
        // `acn hyp verdict` uses (HYP-20); never written, never a stop rule.
        let v =
            verdict::verdict(s.h, bundles.clone(), s.bin.engine_hash).map_err(|e| LoopError {
                code: Code::VerdictRefused,
                message: e.to_string(),
            })?;
        batches.push(Batch {
            cell,
            run_ids,
            verdict: v,
        });
    };
    if batches.is_empty() {
        // Unreachable: setup refuses a budget below the first batch.
        return err(Code::Internal, "no batch fitted the budget");
    }
    Ok(Done {
        batches,
        bundles,
        stop,
    })
}

/// The best and worst configurations, and the control effect (LOOP-11).
fn effects(h: &Hypothesis, v: &Verdict) -> (J, J, J) {
    let first = h.primary().first().cloned().unwrap_or_default();
    let mut best: Option<(f64, J)> = None;
    let mut worst: Option<(f64, J)> = None;
    let mut all = Vec::new();
    for s in &v.slices {
        for (i, c) in s.data.cells().iter().enumerate() {
            if c.treatment.is_none() {
                continue;
            }
            let Some(per_q) = s.effects.get(&i) else {
                continue;
            };
            for q in h.primary() {
                let Some(e) = per_q.get(q) else {
                    continue;
                };
                all.push(J::obj([
                    ("slice", J::str(s.key.clone())),
                    ("cell", J::str(key(&c.cell))),
                    ("quantity", J::str(q.clone())),
                    ("effect", J::num(e.value)),
                    ("ci_low", J::num(e.interval.map(|x| x.0))),
                    ("ci_high", J::num(e.interval.map(|x| x.1))),
                ]));
                if *q != first {
                    continue;
                }
                let Some(x) = e.value.filter(|x| x.is_finite()) else {
                    continue;
                };
                let entry = || {
                    J::obj([
                        ("slice", J::str(s.key.clone())),
                        ("cell", cell_json(&c.cell)),
                        ("quantity", J::str(q.clone())),
                        ("effect", J::Float(x)),
                    ])
                };
                if best.as_ref().is_none_or(|(b, _)| x > *b) {
                    best = Some((x, entry()));
                }
                if worst.as_ref().is_none_or(|(w, _)| x < *w) {
                    worst = Some((x, entry()));
                }
            }
        }
    }
    (
        best.map_or(J::Null, |b| b.1),
        worst.map_or(J::Null, |w| w.1),
        J::Arr(all),
    )
}

fn hex(d: &Digest) -> J {
    J::str(d.to_hex())
}

/// `report.json` (LOOP-11).
fn report(s: &Setup<'_>, done: &Done) -> Result<J, LoopError> {
    let v = done.verdict()?;
    let h = s.h;
    let (best, worst, control_effect) = effects(h, v);
    let mut bundles: Vec<(Digest, Digest)> = done
        .bundles
        .iter()
        .map(|b| (b.run_id, b.bundle_digest))
        .collect();
    bundles.sort_by_key(|b| b.0.0);
    let mut varied: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for b in &done.batches {
        for (k, v) in &b.cell {
            let seen = varied.entry(k.clone()).or_default();
            if !seen.contains(&v.text()) {
                seen.push(v.text());
            }
        }
    }
    let reason_ids =
        |v: &Verdict| J::Arr(v.reasons.iter().map(|r| J::str(r.id.as_str())).collect());
    let budget = i64::try_from(s.budget).map_err(internal)?;
    Ok(J::obj([
        ("format", J::str(REPORT_FORMAT)),
        ("layer", J::str(crate::layer::REPORT.as_str())),
        ("loop_id", hex(&s.loop_id)),
        (
            "hypothesis",
            J::obj([
                ("id", J::str(h.id())),
                ("status", J::str(s.status.as_str())),
                ("hash", hex(&h.hash())),
                ("path", J::str(s.hypothesis.rel.clone())),
            ]),
        ),
        (
            "inputs",
            J::obj([
                (
                    "workloads",
                    J::obj(s.workloads.iter().map(|(v, w)| {
                        (
                            v.clone(),
                            J::obj([("path", J::str(w.rel.clone())), ("hash", hex(&w.hash))]),
                        )
                    })),
                ),
                (
                    "models",
                    J::obj(s.models.iter().map(|(v, m)| (v.clone(), J::str(m.clone())))),
                ),
                ("strategy", J::str(h.design().search.clone())),
                ("budget", J::Int(budget)),
            ]),
        ),
        ("seed", J::str(s.seed.to_string())),
        ("engine_hash", hex(&s.bin.engine_hash)),
        ("build_hash", hex(&s.bin.build_hash)),
        (
            "batches",
            J::Arr(
                done.batches
                    .iter()
                    .map(|b| {
                        J::obj([
                            ("cell", cell_json(&b.cell)),
                            ("run_ids", J::Arr(b.run_ids.iter().map(hex).collect())),
                            ("verdict", J::str(b.verdict.verdict.as_str())),
                            ("reasons", reasons_json(&b.verdict.reasons)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "bundles",
            J::Arr(
                bundles
                    .iter()
                    .map(|(r, d)| J::obj([("run_id", hex(r)), ("bundle_digest", hex(d))]))
                    .collect(),
            ),
        ),
        ("stop", J::str(done.stop.as_str())),
        ("verdict_id", hex(&v.verdict_id)),
        ("verdict", J::str(v.verdict.as_str())),
        ("reasons", reasons_json(&v.reasons)),
        ("best", best),
        ("worst", worst),
        ("control_effect", control_effect),
        (
            "lab_note",
            J::obj([
                ("question", J::str(h.statement())),
                (
                    "varied",
                    J::obj(
                        varied
                            .into_iter()
                            .map(|(k, vs)| (k, J::Arr(vs.into_iter().map(J::str).collect()))),
                    ),
                ),
                (
                    "observed",
                    J::obj([
                        ("verdict", J::str(v.verdict.as_str())),
                        ("reasons", reason_ids(v)),
                        ("batches", J::count(done.batches.len())),
                        ("bundles", J::count(done.bundles.len())),
                        ("stop", J::str(done.stop.as_str())),
                    ]),
                ),
                (
                    "next_layer",
                    J::str(if h.design().twin_required { "L2" } else { "L3" }),
                ),
            ]),
        ),
    ]))
}

/// `report.md`, made from `report.json`'s text alone (LOOP-11).
pub fn markdown(report_json: &str) -> Result<String, LoopError> {
    use std::fmt::Write as _;
    let r: serde_json::Value = serde_json::from_str(report_json).map_err(|e| LoopError {
        code: Code::Report,
        message: format!("report.json: {e}"),
    })?;
    let s = |v: &serde_json::Value| -> String {
        match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => "undefined".into(),
            other => other.to_string(),
        }
    };
    let cell = |v: &serde_json::Value| -> String {
        v.as_object().map_or_else(
            || s(v),
            |o| {
                o.iter()
                    .map(|(k, v)| format!("{k}={}", s(v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        )
    };
    let slice = |v: &serde_json::Value| -> String {
        let k = s(v);
        if k.is_empty() {
            "(the one slice)".into()
        } else {
            format!("`{k}`")
        }
    };
    let reasons = |v: &serde_json::Value| -> String {
        let r: Vec<String> = v
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| s(x.get("reason").unwrap_or(x)))
            .collect();
        if r.is_empty() {
            "none".into()
        } else {
            r.join(", ")
        }
    };
    let mut o = String::new();
    let h = &r["hypothesis"];
    let i = &r["inputs"];
    let _ = writeln!(o, "# Loop report `{}`\n", s(&r["loop_id"]));
    let _ = writeln!(
        o,
        "Hypothesis `{}` ({}, `{}`), file `{}`. Layer {}. Generated from `report.json` ({}).\n",
        s(&h["id"]),
        s(&h["status"]),
        s(&h["hash"]),
        s(&h["path"]),
        s(&r["layer"]),
        s(&r["format"])
    );
    let _ = writeln!(o, "## Inputs\n");
    let _ = writeln!(
        o,
        "- Strategy `{}`, budget {} bundles, seed {}.",
        s(&i["strategy"]),
        s(&i["budget"]),
        s(&r["seed"])
    );
    for (v, w) in i["workloads"].as_object().into_iter().flatten() {
        let label = if v.is_empty() {
            String::new()
        } else {
            format!(" for `{v}`")
        };
        let _ = writeln!(
            o,
            "- Workload{label}: `{}` (`{}`).",
            s(&w["path"]),
            s(&w["hash"])
        );
    }
    for (v, m) in i["models"].as_object().into_iter().flatten() {
        let label = if v.is_empty() {
            String::new()
        } else {
            format!(" for `{v}`")
        };
        let _ = writeln!(o, "- Mock profile{label}: `{}`.", s(m));
    }
    let _ = writeln!(
        o,
        "- engine_hash `{}`, build_hash `{}`.\n",
        s(&r["engine_hash"]),
        s(&r["build_hash"])
    );
    let _ = writeln!(o, "## Result\n");
    let _ = writeln!(
        o,
        "Final verdict **{}** (`{}`), reasons: {}. Stopped: {}.\n",
        s(&r["verdict"]),
        s(&r["verdict_id"]),
        reasons(&r["reasons"]),
        s(&r["stop"])
    );
    for which in ["best", "worst"] {
        let b = &r[which];
        if b.is_null() {
            let _ = writeln!(o, "- {which}: none (no defined effect).");
        } else {
            let _ = writeln!(
                o,
                "- {which}: {} in slice {}, effect of `{}` {}.",
                cell(&b["cell"]),
                slice(&b["slice"]),
                s(&b["quantity"]),
                s(&b["effect"])
            );
        }
    }
    let _ = writeln!(o, "\n## Trajectory\n");
    let _ = writeln!(o, "| # | cell | bundles | verdict | reasons |");
    let _ = writeln!(o, "|---|---|---|---|---|");
    for (n, b) in r["batches"].as_array().into_iter().flatten().enumerate() {
        let ids: Vec<String> = b["run_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| format!("`{}`", s(x).chars().take(12).collect::<String>()))
            .collect();
        let _ = writeln!(
            o,
            "| {} | {} | {} | {} | {} |",
            n + 1,
            cell(&b["cell"]),
            ids.join(" "),
            s(&b["verdict"]),
            reasons(&b["reasons"])
        );
    }
    let _ = writeln!(o, "\n## Control effect\n");
    let _ = writeln!(o, "| slice | cell | quantity | effect | 95% interval |");
    let _ = writeln!(o, "|---|---|---|---|---|");
    for e in r["control_effect"].as_array().into_iter().flatten() {
        let _ = writeln!(
            o,
            "| {} | {} | {} | {} | [{}, {}] |",
            slice(&e["slice"]),
            s(&e["cell"]),
            s(&e["quantity"]),
            s(&e["effect"]),
            s(&e["ci_low"]),
            s(&e["ci_high"])
        );
    }
    let n = &r["lab_note"];
    let varied: Vec<String> = n["varied"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| {
            let vals: Vec<String> = v.as_array().into_iter().flatten().map(s).collect();
            format!("`{k}` over {}", vals.join(", "))
        })
        .collect();
    let ob = &n["observed"];
    let _ = writeln!(o, "\n## Lab note (draft)\n");
    let _ = writeln!(o, "- **Question.** {}", s(&n["question"]));
    let _ = writeln!(o, "- **What was varied.** {}.", varied.join("; "));
    let _ = writeln!(
        o,
        "- **What was observed.** {} batches, {} bundles, stopped by {}; verdict {}, reasons: {}.",
        s(&ob["batches"]),
        s(&ob["bundles"]),
        s(&ob["stop"]),
        s(&ob["verdict"]),
        reasons(&ob["reasons"])
    );
    let _ = writeln!(o, "- **Suggested next layer.** {}.", s(&n["next_layer"]));
    Ok(o)
}

/// `acn loop run` (LOOP-10, LOOP-11): run L1 under `runs_dir`, write the final
/// verdict under `runs/verdicts/` and the report under
/// `runs/loop/<loop_id>/`.
pub fn run(
    h: &Hypothesis,
    args: &Args,
    runs_dir: &Path,
    bin: Binary,
    exec: &mut dyn Executor,
) -> Result<Completed, LoopError> {
    let s = Setup::new(h, args, runs_dir, bin, &*exec)?;
    let loop_dir = runs_dir.join("loop").join(s.loop_id.to_hex());
    if std::fs::symlink_metadata(&loop_dir).is_ok() {
        return err(
            Code::LoopExists,
            format!(
                "{} exists; a loop report is never overwritten (LOOP-11)",
                loop_dir.display()
            ),
        );
    }
    let done = execute(&s, runs_dir, true, exec)?;
    let v = done.verdict()?;
    // Rendered first, so that the verdict and the report are written one
    // after the other with nothing between them that can fail but I/O.
    let text = report(&s, &done)?.render();
    let md = markdown(&text)?;
    // LOOP-13: re-read before the verdict is written ...
    s.check_inputs()?;
    match verdict::write(runs_dir, v) {
        Ok(_) => {}
        Err(VerdictError::Exists(dir)) => {
            // Two loops can end on the same bundle set (LOOP-10(c)).
            let path = dir.join("verdict.json");
            let on_disk = std::fs::read(&path).map_err(|e| io_err(&path, e))?;
            if on_disk != v.text().as_bytes() {
                return err(
                    Code::VerdictConflict,
                    format!("{} exists with other bytes (LOOP-10(c))", path.display()),
                );
            }
        }
        Err(e) => {
            return err(Code::Io, e.to_string());
        }
    }
    // ... and before the report is.
    s.check_inputs()?;
    let report = loop_out::write_report(runs_dir, &[], &s.loop_id, &text, &md)?;
    let mut run_ids: Vec<Digest> = done.bundles.iter().map(|b| b.run_id).collect();
    run_ids.sort_by_key(|d| d.0);
    Ok(Completed {
        loop_id: s.loop_id,
        report,
        verdict_id: v.verdict_id,
        verdict: v.verdict,
        stop: done.stop,
        run_ids,
    })
}

fn bad_report(m: impl Into<String>) -> LoopError {
    LoopError {
        code: Code::Report,
        message: m.into(),
    }
}

/// `acn loop run --from-report` (LOOP-14): re-run the loop the report
/// records into a fresh `runs/regen/<loop_id>/<n>/`, reusing no bundle and
/// judging in memory, and compare every bundle, both report files and the final
/// verdict with the originals.
pub fn regenerate(
    report_path: &Path,
    bin: Binary,
    exec: &mut dyn Executor,
) -> Result<Regenerated, LoopError> {
    // Absolute, so `runs/` and its parent are found however the path is given.
    let report_path = &std::fs::canonicalize(report_path).map_err(|e| LoopError {
        code: Code::Report,
        message: format!("{}: {e}", report_path.display()),
    })?;
    let text = std::fs::read_to_string(report_path).map_err(|e| io_err(report_path, e))?;
    let r: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| bad_report(format!("{}: {e}", report_path.display())))?;
    if r["format"] != REPORT_FORMAT {
        return Err(bad_report(format!("format is not {REPORT_FORMAT}")));
    }
    let id = r["loop_id"].as_str().unwrap_or_default().to_owned();
    // runs/loop/<loop_id>/report.json
    let loop_dir = report_path.parent().unwrap_or(Path::new(""));
    let named = |p: &Path, n: &str| p.file_name().and_then(|x| x.to_str()) == Some(n);
    let loop_root = loop_dir.parent().unwrap_or(Path::new(""));
    let runs = loop_root.parent().unwrap_or(Path::new("")).to_path_buf();
    if !named(report_path, REPORT_JSON) || !named(loop_dir, &id) || !named(loop_root, "loop") {
        return Err(bad_report(format!(
            "{} is not runs/loop/<loop_id>/report.json",
            report_path.display()
        )));
    }
    let str_at = |v: &serde_json::Value, what: &str| -> Result<String, LoopError> {
        v.as_str()
            .map(str::to_owned)
            .ok_or_else(|| bad_report(format!("no `{what}`")))
    };
    let build = str_at(&r["build_hash"], "build_hash")?;
    let engine = str_at(&r["engine_hash"], "engine_hash")?;
    if build != bin.build_hash.to_hex() || engine != bin.engine_hash.to_hex() {
        return err(
            Code::NotRegenerable,
            format!(
                "the report was made by build {build} and engine {engine}; this binary is {} and {} (CON-31, LOOP-14)",
                bin.build_hash.to_hex(),
                bin.engine_hash.to_hex()
            ),
        );
    }
    let parent = match runs.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let base = std::fs::canonicalize(&parent).map_err(|e| io_err(&parent, e))?;
    // LOOP-14: every input file still has its recorded hash, checked before
    // anything else reads it, so that a change is named `input_changed`.
    let unchanged = |rel: String, hash: String, what: &str| -> Result<PathBuf, LoopError> {
        let path = base.join(&rel);
        match identity::file_hash(&path) {
            Ok(h) if h.to_hex() == hash => Ok(path),
            Ok(_) => err(
                Code::InputChanged,
                format!("{what} {rel} no longer has the hash the report records (LOOP-14)"),
            ),
            Err(e) => err(
                Code::InputChanged,
                format!("{what} {rel} can no longer be read (LOOP-14): {e}"),
            ),
        }
    };
    let hyp_path = unchanged(
        str_at(&r["hypothesis"]["path"], "hypothesis.path")?,
        str_at(&r["hypothesis"]["hash"], "hypothesis.hash")?,
        "hypothesis",
    )?;
    for (v, w) in r["inputs"]["workloads"].as_object().into_iter().flatten() {
        unchanged(
            str_at(&w["path"], "inputs.workloads path")?,
            str_at(&w["hash"], "inputs.workloads hash")?,
            &format!("workload `{v}`"),
        )?;
    }
    // The bytes are the recorded ones, so a load failure is the report's.
    let h = crate::load_in(&hyp_path, &base).map_err(|e| bad_report(e.to_string()))?;
    if h.status().as_str() != str_at(&r["hypothesis"]["status"], "hypothesis.status")? {
        return err(
            Code::InputChanged,
            format!(
                "{} no longer has the status the report records (LOOP-14)",
                hyp_path.display()
            ),
        );
    }
    let i = &r["inputs"];
    let entries =
        |v: &serde_json::Value, path_key: Option<&str>| -> Result<Vec<String>, LoopError> {
            let mut out = Vec::new();
            for (k, x) in v.as_object().into_iter().flatten() {
                let val = match path_key {
                    Some(p) => base
                        .join(str_at(&x[p], "inputs path")?)
                        .to_string_lossy()
                        .into_owned(),
                    None => str_at(x, "inputs model")?,
                };
                out.push(if k.is_empty() {
                    val
                } else {
                    format!("{k}={val}")
                });
            }
            Ok(out)
        };
    let args = Args {
        workloads: entries(&i["workloads"], Some("path"))?,
        models: entries(&i["models"], None)?,
        budget: i["budget"]
            .as_u64()
            .ok_or_else(|| bad_report("no `inputs.budget`"))?,
    };
    let s = Setup::new(&h, &args, &runs, bin, &*exec)?;
    if s.loop_id.to_hex() != id {
        return err(
            Code::InputChanged,
            format!(
                "the inputs give loop_id {}, not the report's {id} (LOOP-14)",
                s.loop_id.to_hex()
            ),
        );
    }
    let dir = loop_out::fresh_regen_dir(&runs, &s.loop_id)?;
    let done = execute(&s, &dir, false, exec)?;
    let new_text = report(&s, &done)?.render();
    let new_md = markdown(&new_text)?;
    let n = dir
        .file_name()
        .and_then(|x| x.to_str())
        .unwrap_or_default()
        .to_owned();
    loop_out::write_report(&runs, &["regen", &id, &n], &s.loop_id, &new_text, &new_md)?;

    let mut differ = Vec::new();
    if new_text != text {
        differ.push(REPORT_JSON.to_owned());
    }
    let md_path = loop_dir.join(REPORT_MD);
    if std::fs::read_to_string(&md_path).ok().as_deref() != Some(new_md.as_str()) {
        differ.push(REPORT_MD.to_owned());
    }
    let v = done.verdict()?;
    let vpath = runs
        .join("verdicts")
        .join(str_at(&r["verdict_id"], "verdict_id")?)
        .join("verdict.json");
    if std::fs::read_to_string(&vpath).ok().as_deref() != Some(v.text().as_str()) {
        differ.push("verdict.json".to_owned());
    }
    let regen: BTreeMap<String, String> = done
        .bundles
        .iter()
        .map(|b| (b.run_id.to_hex(), b.bundle_digest.to_hex()))
        .collect();
    for b in r["bundles"].as_array().into_iter().flatten() {
        let run_id = str_at(&b["run_id"], "bundles.run_id")?;
        let digest = str_at(&b["bundle_digest"], "bundles.bundle_digest")?;
        let original = acn_trace::bundle::verify(&runs.join(&run_id))
            .ok()
            .map(|v| v.bundle_digest.to_hex());
        if original.as_deref() != Some(digest.as_str()) || regen.get(&run_id) != Some(&digest) {
            differ.push(format!("bundle {run_id}"));
        }
    }
    let recorded: BTreeSet<String> = r["bundles"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| b["run_id"].as_str().map(str::to_owned))
        .collect();
    for run_id in regen.keys().filter(|k| !recorded.contains(*k)) {
        differ.push(format!("bundle {run_id}"));
    }
    Ok(Regenerated {
        loop_id: s.loop_id,
        dir,
        differ,
    })
}
