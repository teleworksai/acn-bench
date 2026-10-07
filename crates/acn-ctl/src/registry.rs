//! The registry and its worker (SPEC 070 CTL-11 to CTL-13): requests named by
//! their resolved inputs, their status on disk, and one worker that runs them
//! in submission order through the CLI's own run paths.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use acn_harness::HarnessError;
use acn_harness::agent::Opts;
use acn_harness::run::{HypothesisArg, Planned, RunConfig};
use acn_harness::wire::Backend;
use acn_mockllm::profile::Profiles;
use acn_trace::identity::{BuildInfo, Digest, Mode};
use serde::{Deserialize, Serialize};

use crate::Refusal;
use crate::request::{Kind, ScenarioRef, Submit};
use crate::resolve::{self, Resolved};

/// What the control plane runs with.
#[derive(Debug, Clone)]
pub struct CtlConfig {
    /// The canonical workspace root (CON-28).
    pub root: PathBuf,
    /// The runs directory, relative to the root (CTL-1).
    pub runs_dir: PathBuf,
    pub engine_hash: Digest,
    pub build: BuildInfo,
    /// The mock's profiles; `None` means the embedded ones (MLM-50).
    pub profiles: Option<Profiles>,
}

/// A request's state (CTL-11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Queued,
    Running,
    Done,
    Failed,
}

/// A request's `status.json` (CTL-11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub seq: u64,
    pub state: State,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reused: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub submitted_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
}

/// A submission's answer (CTL-21): `new` is false for a request already
/// known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submitted {
    pub request_id: String,
    pub state: State,
    pub new: bool,
}

/// What one worker step did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub request_id: String,
    pub status: Status,
}

struct Inner {
    cfg: CtlConfig,
    /// `<root>/<runs_dir>`.
    runs: PathBuf,
    /// `<runs>/ctl`.
    ctl: PathBuf,
    statuses: Mutex<BTreeMap<String, Status>>,
    queue: Mutex<VecDeque<String>>,
    ready: Condvar,
    stopping: AtomicBool,
    run_ids: Mutex<BTreeSet<String>>,
    fault: AtomicBool,
    /// Held across every run: one at a time, whoever calls (CTL-12).
    exec: Mutex<()>,
    /// Called on the run's thread before it runs; tests use it.
    before_run: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Held while this registry is open: one control plane per runs
    /// directory (CTL-12). The OS releases it if the process dies.
    _lock: std::fs::File,
}

/// The control plane's registry and worker (CTL-11 to CTL-13).
#[derive(Clone)]
pub struct Ctl {
    inner: Arc<Inner>,
}

fn now() -> String {
    acn_emu::clock::wall_time_utc()
}

/// `p` written whole: to a temporary file beside it, then renamed (CTL-11).
fn replace(p: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = p.with_extension("json.tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, p)
}

/// The directories a runs directory may not lie in (LOOP-20).
const PROTECTED: [&str; 4] = ["hypotheses", "specs", "scenarios/measured", "crates"];

fn code_of(e: &HarnessError) -> &'static str {
    match e {
        HarnessError::Workload(_) => "workload",
        HarnessError::Knob(_) => "knob",
        HarnessError::Config(_) => "config",
        HarnessError::BackendMismatch(_) => "backend_mismatch",
        HarnessError::Backend(_) => "backend",
        HarnessError::Env(_) => "preflight",
        HarnessError::Identity(_) => "config",
        HarnessError::Bundle(_) => "bundle",
        _ => "internal",
    }
}

/// A run's config, harness or generator, as the CLI would build it.
enum Config {
    Harness(RunConfig),
    Generator(acn_gen::run::GenConfig),
}

impl Ctl {
    /// Open the registry under `<root>/<runs_dir>/ctl/`, recovering from a
    /// restart (CTL-11): a `running` request is `failed` with `interrupted`,
    /// its unfinished bundle removed, and `queued` ones are queued again by
    /// `seq`.
    pub fn open(cfg: CtlConfig) -> Result<Self, Refusal> {
        let root = std::fs::canonicalize(&cfg.root).map_err(Refusal::internal)?;
        let rel = cfg.runs_dir.clone();
        let refuse = || {
            Refusal::bad(
                "runs_dir_refused",
                format!(
                    "--runs-dir `{}` must lie inside the workspace and outside its protected paths (CTL-1, LOOP-20)",
                    rel.display()
                ),
            )
        };
        if rel.as_os_str().is_empty()
            || rel.is_absolute()
            || rel
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(refuse());
        }
        std::fs::create_dir_all(root.join(&rel)).map_err(Refusal::internal)?;
        // Canonical, so a link or `.` cannot hide where it lies.
        let runs = std::fs::canonicalize(root.join(&rel)).map_err(Refusal::internal)?;
        let Ok(under) = runs.strip_prefix(&root) else {
            return Err(refuse());
        };
        let parts: Vec<String> = under
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect();
        let protected = parts.is_empty()
            || PROTECTED.iter().any(|p| {
                let pp: Vec<&str> = p.split('/').collect();
                parts.len() >= pp.len() && parts.iter().zip(&pp).all(|(a, b)| a == b)
            });
        if protected {
            return Err(refuse());
        }
        let ctl = runs.join("ctl");
        std::fs::create_dir_all(ctl.join("requests")).map_err(Refusal::internal)?;
        // Before anything is read or recovered: a second server on the same
        // runs directory would take over a live one's requests (CTL-12).
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(ctl.join("lock"))
            .map_err(Refusal::internal)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(Refusal::new(
                    409,
                    "registry_in_use",
                    format!(
                        "another control plane holds {} (CTL-12)",
                        ctl.join("lock").display()
                    ),
                ));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(Refusal::internal(e)),
        }
        let mut statuses = BTreeMap::new();
        let mut queued: Vec<(u64, String)> = Vec::new();
        let mut orphans: Vec<String> = Vec::new();
        let mut broken: Vec<(String, Status)> = Vec::new();
        let mut next = 0u64;
        for e in std::fs::read_dir(ctl.join("requests")).map_err(Refusal::internal)? {
            let e = e.map_err(Refusal::internal)?;
            let id = e.file_name().to_string_lossy().into_owned();
            if !resolve::is_hex64(&id) {
                continue;
            }
            // A request whose file is missing or is not its id's is not a
            // request (CTL-11).
            let sound = std::fs::read(e.path().join("request.json"))
                .ok()
                .is_some_and(|b| request_id_of(&b) == id);
            let sp = e.path().join("status.json");
            let st = std::fs::read(&sp)
                .ok()
                .and_then(|b| serde_json::from_slice::<Status>(&b).ok());
            match (sound, st) {
                (true, Some(st)) => {
                    next = next.max(st.seq + 1);
                    statuses.insert(id, st);
                }
                (true, None) => orphans.push(id),
                (false, Some(mut st)) => {
                    next = next.max(st.seq + 1);
                    if !matches!(st.state, State::Done | State::Failed) {
                        st.state = State::Failed;
                        st.code = Some("internal".into());
                        st.error = Some("request.json is missing or does not match its id".into());
                        st.ended_at = Some(now());
                    }
                    broken.push((id, st));
                }
                (false, None) => {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        let c = Self {
            inner: Arc::new(Inner {
                cfg: CtlConfig { root, ..cfg },
                runs,
                ctl,
                statuses: Mutex::new(BTreeMap::new()),
                queue: Mutex::new(VecDeque::new()),
                ready: Condvar::new(),
                stopping: AtomicBool::new(false),
                run_ids: Mutex::new(BTreeSet::new()),
                fault: AtomicBool::new(false),
                exec: Mutex::new(()),
                before_run: None,
                _lock: lock,
            }),
        };
        for (id, mut st) in statuses {
            match st.state {
                State::Running => {
                    // A bundle that verifies is kept for a retry to adopt;
                    // one that does not is removed, as HAR-23 removes it.
                    if let Some(run_id) = st.run_id.clone() {
                        let dir = c.inner.runs.join(&run_id);
                        if dir.exists() && acn_trace::bundle::verify(&dir).is_err() {
                            let _ = std::fs::remove_dir_all(&dir);
                        }
                    }
                    st.state = State::Failed;
                    st.code = Some("interrupted".into());
                    st.error =
                        Some("the server stopped while the run was in progress (CTL-11)".into());
                    st.ended_at = Some(now());
                    c.write_status(&id, &st)?;
                }
                State::Queued => queued.push((st.seq, id.clone())),
                State::Done | State::Failed => {}
            }
            c.lock_statuses()?.insert(id, st);
        }
        for (id, st) in broken {
            c.write_status(&id, &st)?;
            c.lock_statuses()?.insert(id, st);
        }
        // A request written without its status: queued, last.
        orphans.sort();
        for id in orphans {
            let st = Status {
                seq: next,
                state: State::Queued,
                run_id: None,
                bundle_digest: None,
                reused: None,
                code: None,
                error: None,
                submitted_at: now(),
                started_at: None,
                ended_at: None,
            };
            next += 1;
            c.write_status(&id, &st)?;
            queued.push((st.seq, id.clone()));
            c.lock_statuses()?.insert(id, st);
        }
        queued.sort();
        c.lock_queue()?.extend(queued.into_iter().map(|(_, id)| id));
        Ok(c)
    }

    /// Call `f` on each run's thread before it runs. For tests: a hook that
    /// panics stands for a run that does.
    #[doc(hidden)]
    #[must_use]
    pub fn with_before_run(mut self, f: Arc<dyn Fn() + Send + Sync>) -> Self {
        if let Some(inner) = Arc::get_mut(&mut self.inner) {
            inner.before_run = Some(f);
        }
        self
    }

    fn lock_statuses(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, Status>>, Refusal> {
        self.inner
            .statuses
            .lock()
            .map_err(|_| Refusal::internal("the registry's lock is poisoned"))
    }

    fn lock_queue(&self) -> Result<std::sync::MutexGuard<'_, VecDeque<String>>, Refusal> {
        self.inner
            .queue
            .lock()
            .map_err(|_| Refusal::internal("the queue's lock is poisoned"))
    }

    fn request_dir(&self, id: &str) -> PathBuf {
        self.inner.ctl.join("requests").join(id)
    }

    fn write_status(&self, id: &str, st: &Status) -> Result<(), Refusal> {
        let bytes = serde_json::to_vec_pretty(st).map_err(Refusal::internal)?;
        replace(&self.request_dir(id).join("status.json"), &bytes).map_err(|e| {
            self.inner.fault.store(true, Ordering::SeqCst);
            Refusal::internal(e)
        })
    }

    fn set(&self, id: &str, st: Status) -> Result<(), Refusal> {
        self.write_status(id, &st)?;
        self.lock_statuses()?.insert(id.to_owned(), st);
        Ok(())
    }

    /// `<runs>/ctl/`.
    #[must_use]
    pub fn ctl_dir(&self) -> &Path {
        &self.inner.ctl
    }

    /// `<root>/<runs_dir>`.
    #[must_use]
    pub fn runs_dir(&self) -> &Path {
        &self.inner.runs
    }

    /// The workspace root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.inner.cfg.root
    }

    /// Submit a request (CTL-13): resolved, named, written once, and queued,
    /// or the status of the request already known.
    pub fn submit(&self, r: &Submit) -> Result<Submitted, Refusal> {
        if self.inner.stopping.load(Ordering::SeqCst) {
            return Err(Refusal::new(
                503,
                "shutting_down",
                "the server is stopping (CTL-21)",
            ));
        }
        // Under the registry's lock from resolution on, so an endpoint cannot
        // be deleted between its lookup and the request's queuing (CTL-30).
        let mut statuses = self.lock_statuses()?;
        let resolved = resolve::resolve(&self.inner.cfg.root, &self.inner.ctl, r)?;
        let id = resolved.request_id()?;
        if let Some(st) = statuses.get(&id).cloned() {
            return match (st.state, r.retry) {
                (State::Failed, true) => {
                    // Under the lock: two retries queue it once (CTL-13).
                    let seq = statuses.values().map(|s| s.seq + 1).max().unwrap_or(0);
                    let st = Status {
                        seq,
                        state: State::Queued,
                        run_id: None,
                        bundle_digest: None,
                        reused: None,
                        code: None,
                        error: None,
                        submitted_at: now(),
                        started_at: None,
                        ended_at: None,
                    };
                    self.write_status(&id, &st)?;
                    statuses.insert(id.clone(), st);
                    drop(statuses);
                    self.enqueue(&id)?;
                    Ok(Submitted {
                        request_id: id,
                        state: State::Queued,
                        new: true,
                    })
                }
                (State::Running, true) => Err(Refusal::new(
                    409,
                    "running",
                    "the request is running; it can be retried once it has failed (CTL-13)",
                )),
                (state, _) => Ok(Submitted {
                    request_id: id,
                    state,
                    new: false,
                }),
            };
        }
        let seq = statuses.values().map(|s| s.seq + 1).max().unwrap_or(0);
        let dir = self.request_dir(&id);
        std::fs::create_dir_all(&dir).map_err(Refusal::internal)?;
        let text = resolved.text()?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join("request.json"))
        {
            Ok(mut f) => {
                f.write_all(text.as_bytes()).map_err(Refusal::internal)?;
                f.sync_all().map_err(Refusal::internal)?;
            }
            // Written by an earlier call that stopped before its status:
            // kept if whole, written again if not.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let path = dir.join("request.json");
                if std::fs::read(&path).ok().as_deref() != Some(text.as_bytes()) {
                    replace(&path, text.as_bytes()).map_err(Refusal::internal)?;
                }
            }
            Err(e) => return Err(Refusal::internal(e)),
        }
        let st = Status {
            seq,
            state: State::Queued,
            run_id: None,
            bundle_digest: None,
            reused: None,
            code: None,
            error: None,
            submitted_at: now(),
            started_at: None,
            ended_at: None,
        };
        self.write_status(&id, &st)?;
        statuses.insert(id.clone(), st);
        drop(statuses);
        self.enqueue(&id)?;
        Ok(Submitted {
            request_id: id,
            state: State::Queued,
            new: true,
        })
    }

    fn enqueue(&self, id: &str) -> Result<(), Refusal> {
        self.lock_queue()?.push_back(id.to_owned());
        self.inner.ready.notify_all();
        Ok(())
    }

    /// A request and its status (CTL-21).
    pub fn get(&self, id: &str) -> Result<(Resolved, Status), Refusal> {
        let unknown = || Refusal::new(404, "unknown_request", format!("no request {id} (CTL-21)"));
        if !resolve::is_hex64(id) {
            return Err(Refusal::bad(
                "bad_id",
                format!("`{id}` is not a request_id (CTL-3)"),
            ));
        }
        let st = self.lock_statuses()?.get(id).cloned().ok_or_else(unknown)?;
        let bytes =
            std::fs::read(self.request_dir(id).join("request.json")).map_err(|_| unknown())?;
        let r: Resolved = serde_json::from_slice(&bytes).map_err(Refusal::internal)?;
        Ok((r, st))
    }

    /// Every request's id and state, in `seq` order (CTL-21).
    pub fn list(&self) -> Result<Vec<(String, State)>, Refusal> {
        let mut v: Vec<(u64, String, State)> = self
            .lock_statuses()?
            .iter()
            .map(|(id, s)| (s.seq, id.clone(), s.state))
            .collect();
        v.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        Ok(v.into_iter().map(|(_, id, st)| (id, st)).collect())
    }

    /// The runs started or adopted so far, ascending (CTL-1).
    pub fn run_ids(&self) -> Vec<String> {
        self.inner
            .run_ids
            .lock()
            .map(|r| r.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Whether a status could not be written (CTL-1: the server's `ok`).
    #[must_use]
    pub fn faulted(&self) -> bool {
        self.inner.fault.load(Ordering::SeqCst)
    }

    /// Whether a queued or running request names endpoint `name`, with the
    /// registry's lock held (CTL-30).
    fn in_use(&self, statuses: &BTreeMap<String, Status>, name: &str) -> bool {
        statuses
            .iter()
            .filter(|(_, s)| matches!(s.state, State::Queued | State::Running))
            .any(|(id, _)| {
                std::fs::read(self.request_dir(id).join("request.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Resolved>(&b).ok())
                    .is_some_and(|r| r.request.opt.endpoint_name.as_deref() == Some(name))
            })
    }

    /// Whether a queued or running request names endpoint `name` (CTL-30).
    pub fn endpoint_in_use(&self, name: &str) -> Result<bool, Refusal> {
        let statuses = self.lock_statuses()?;
        Ok(self.in_use(&statuses, name))
    }

    /// Remove endpoint `name`, unless a queued or running request names it:
    /// checked and removed under the lock submissions take (CTL-30).
    pub fn delete_endpoint(&self, name: &str) -> Result<(), Refusal> {
        if !resolve::is_endpoint_name(name) {
            return Err(Refusal::bad(
                "bad_id",
                "an endpoint name is 1 to 64 of a-z, 0-9 and - (CTL-30)",
            ));
        }
        let statuses = self.lock_statuses()?;
        let file = self
            .inner
            .ctl
            .join("endpoints")
            .join(format!("{name}.json"));
        if !file.exists() {
            return Err(Refusal::new(
                404,
                "unknown_endpoint",
                format!("no endpoint `{name}` (CTL-30)"),
            ));
        }
        if self.in_use(&statuses, name) {
            return Err(Refusal::new(
                409,
                "endpoint_in_use",
                format!("a queued or running request names `{name}` (CTL-30)"),
            ));
        }
        std::fs::remove_file(&file).map_err(Refusal::internal)
    }

    /// Stop taking requests; the worker finishes the run in progress, and
    /// queued requests stay queued (CTL-21).
    pub fn stop(&self) {
        // Under the queue's lock, so a worker about to wait cannot miss it.
        let _q = self.inner.queue.lock();
        self.inner.stopping.store(true, Ordering::SeqCst);
        self.inner.ready.notify_all();
    }

    #[must_use]
    pub fn stopping(&self) -> bool {
        self.inner.stopping.load(Ordering::SeqCst)
    }

    /// The worker (CTL-12): runs requests in `seq` order until stopped, on
    /// the calling thread.
    pub fn work(&self) {
        loop {
            let next = {
                let Ok(mut q) = self.inner.queue.lock() else {
                    return;
                };
                loop {
                    if self.stopping() {
                        return;
                    }
                    if let Some(id) = q.pop_front() {
                        break id;
                    }
                    let Ok(g) = self.inner.ready.wait(q) else {
                        return;
                    };
                    q = g;
                }
            };
            self.execute(&next);
        }
    }

    /// One worker step, on the calling thread: the next queued request, run
    /// (CTL-12, CTL-13). `None` when nothing is queued.
    pub fn run_next(&self) -> Option<Outcome> {
        let id = self.inner.queue.lock().ok()?.pop_front()?;
        self.execute(&id);
        let st = self.inner.statuses.lock().ok()?.get(&id).cloned()?;
        Some(Outcome {
            request_id: id,
            status: st,
        })
    }

    fn fail(&self, id: &str, mut st: Status, code: &str, error: String) {
        st.state = State::Failed;
        st.code = Some(code.to_owned());
        st.error = Some(error);
        st.ended_at = Some(now());
        tracing::warn!(request = id, code, "acn ctl run failed");
        if let Err(e) = self.set(id, st) {
            tracing::error!(request = id, "status: {e}");
        }
    }

    fn done(&self, id: &str, mut st: Status, run_id: &Digest, digest: &Digest, reused: bool) {
        st.state = State::Done;
        st.run_id = Some(run_id.to_hex());
        st.bundle_digest = Some(digest.to_hex());
        st.reused = Some(reused);
        st.ended_at = Some(now());
        if let Ok(mut r) = self.inner.run_ids.lock() {
            r.insert(run_id.to_hex());
        }
        tracing::info!(request = id, run_id = %run_id.to_hex(), reused, "acn ctl run done");
        if let Err(e) = self.set(id, st) {
            tracing::error!(request = id, "status: {e}");
        }
    }

    fn config(&self, r: &Resolved) -> Result<(Config, Option<PathBuf>), Refusal> {
        let root = &self.inner.cfg.root;
        let q = &r.request;
        let hypothesis = match (&q.hypothesis, &q.seed) {
            (Some(h), _) => HypothesisArg::File(root.join(h)),
            (None, Some(s)) => HypothesisArg::None {
                seed: s.parse().map_err(|_| Refusal::bad("bad_request", "seed"))?,
            },
            (None, None) => return Err(Refusal::bad("bad_request", "no hypothesis or seed")),
        };
        let d = Opts::default();
        let opts = Opts {
            endpoint: r
                .endpoint_url
                .clone()
                .or_else(|| q.opt.endpoint.clone())
                .unwrap_or(d.endpoint),
            max_retries: q.opt.max_retries.unwrap_or(d.max_retries),
            retry_base_ms: q.opt.retry_base_ms.unwrap_or(d.retry_base_ms),
            request_timeout_ms: q.opt.request_timeout_ms.unwrap_or(d.request_timeout_ms),
            stall_threshold_ms: q.opt.stall_threshold_ms.unwrap_or(d.stall_threshold_ms),
        };
        let mode = Mode::parse(&q.mode).map_err(|e| Refusal::bad("bad_request", e.to_string()))?;
        let scenario = match &q.scenario {
            Some(ScenarioRef::Path(p)) => Some(root.join(&p.path)),
            Some(ScenarioRef::Hash(h)) => Some(resolve::stored_scenario(&self.inner.ctl, &h.hash)?),
            None => None,
        };
        let runs_dir = self.inner.runs.clone();
        let c = match q.kind {
            Kind::Harness => Config::Harness(RunConfig {
                workload: root.join(q.workload.clone().unwrap_or_default()),
                backend: Backend::parse(q.backend.as_deref().unwrap_or_default())
                    .map_err(|e| Refusal::bad("bad_request", e.to_string()))?,
                model: q.model.clone().unwrap_or_default(),
                mode,
                arm: q.arm.clone(),
                replicates: q.replicates,
                vary: q.vary.clone(),
                opts,
                hypothesis,
                runs_dir,
                start_dir: root.clone(),
                engine_hash: self.inner.cfg.engine_hash,
                build: self.inner.cfg.build.clone(),
                profiles: self.inner.cfg.profiles.clone(),
            }),
            Kind::Generator => Config::Generator(acn_gen::run::GenConfig {
                sheet: root.join(q.sheet.clone().unwrap_or_default()),
                mode,
                arm: q.arm.clone(),
                replicates: q.replicates,
                vary: q.vary.clone(),
                opts,
                hypothesis,
                runs_dir,
                start_dir: root.clone(),
                engine_hash: self.inner.cfg.engine_hash,
                build: self.inner.cfg.build.clone(),
                profiles: self.inner.cfg.profiles.clone(),
            }),
        };
        Ok((c, scenario))
    }

    /// Run request `id` (CTL-12, CTL-13), one at a time whoever calls.
    #[allow(clippy::too_many_lines)]
    fn execute(&self, id: &str) {
        let Ok(_one) = self.inner.exec.lock() else {
            return;
        };
        let (r, mut st) = match self.get(id) {
            Ok(x) => x,
            Err(e) => {
                // A request that cannot be read fails, rather than staying
                // queued.
                if let Some(st) = self
                    .inner
                    .statuses
                    .lock()
                    .ok()
                    .and_then(|m| m.get(id).cloned())
                {
                    self.fail(id, st, "internal", e.to_string());
                }
                return;
            }
        };
        // CTL-12: the inputs are still the ones resolved.
        let changed = r.changed(&self.inner.cfg.root);
        if !changed.is_empty() {
            return self.fail(
                id,
                st,
                "input_changed",
                format!(
                    "{} changed after the request was submitted (CTL-12)",
                    changed.join(", ")
                ),
            );
        }
        let (config, scenario) = match self.config(&r) {
            Ok(x) => x,
            Err(e) => return self.fail(id, st, e.code, e.error),
        };
        let planned: Result<Planned, HarnessError> = match &config {
            Config::Harness(c) => acn_harness::run::plan(c, scenario.as_deref()),
            Config::Generator(c) => acn_gen::run::plan_run(c, scenario.as_deref()),
        };
        let planned = match planned {
            Ok(p) => p,
            Err(e) => return self.fail(id, st, code_of(&e), e.to_string()),
        };
        // The plan read the inputs again: they must be the ones resolved.
        let q = &r.request;
        let input = q.workload.as_ref().or(q.sheet.as_ref());
        let mut expected: Vec<(&str, Option<String>, Digest)> = vec![(
            "workload",
            input.and_then(|p| r.hashes.get(p).cloned()),
            planned.workload_hash,
        )];
        if let Some(h) = &q.hypothesis {
            expected.push((
                "hypothesis",
                r.hashes.get(h).cloned(),
                planned.hypothesis_hash,
            ));
        }
        match &q.scenario {
            Some(ScenarioRef::Path(p)) => {
                expected.push((
                    "scenario",
                    r.hashes.get(&p.path).cloned(),
                    planned.scenario_hash,
                ));
            }
            Some(ScenarioRef::Hash(h)) => {
                expected.push(("scenario", Some(h.hash.clone()), planned.scenario_hash));
            }
            None => {}
        }
        if let Some((what, _, _)) = expected
            .iter()
            .find(|(_, want, got)| want.as_deref() != Some(got.to_hex().as_str()))
        {
            return self.fail(
                id,
                st,
                "input_changed",
                format!("the {what} the run read is not the one resolved (CTL-12)"),
            );
        }
        // CTL-13: an existing bundle is adopted when it verifies.
        if planned.dir.exists() {
            return match acn_trace::bundle::verify(&planned.dir) {
                Ok(v) => self.done(id, st, &planned.run_id, &v.bundle_digest, true),
                Err(e) => self.fail(id, st, "bundle_invalid", e.to_string()),
            };
        }
        st.state = State::Running;
        st.run_id = Some(planned.run_id.to_hex());
        st.started_at = Some(now());
        if let Err(e) = self.set(id, st.clone()) {
            tracing::error!(request = id, "status: {e}");
            return;
        }
        // A run builds its own runtime, so it runs on a thread of its own;
        // a panic there fails the request and nothing else (CTL-12).
        let hook = self.inner.before_run.clone();
        let ran = std::thread::Builder::new()
            .name("acn-ctl-run".into())
            .spawn(move || {
                if let Some(h) = hook {
                    h();
                }
                match config {
                    Config::Harness(c) => {
                        acn_harness::run::run_with_scenario(&c, scenario.as_deref())
                    }
                    Config::Generator(c) => {
                        acn_gen::run::run(&c, scenario.as_deref()).map(|w| w.written)
                    }
                }
            })
            .map_err(|e| e.to_string())
            .and_then(|h| h.join().map_err(|_| "the run panicked".to_owned()));
        match ran {
            Ok(Ok(w)) if w.run_id == planned.run_id => {
                self.done(id, st, &w.run_id, &w.bundle_digest, false);
            }
            Ok(Ok(w)) => self.fail(
                id,
                st,
                "internal",
                format!(
                    "the run made {}, not the planned {} (CON-29)",
                    w.run_id.to_hex(),
                    planned.run_id.to_hex()
                ),
            ),
            // Made by someone else since the plan: adopt it if it verifies.
            Ok(Err(HarnessError::Bundle(acn_trace::bundle::BundleError::Refused(m)))) => {
                if planned.dir.exists() {
                    match acn_trace::bundle::verify(&planned.dir) {
                        Ok(v) => self.done(id, st, &planned.run_id, &v.bundle_digest, true),
                        Err(e) => self.fail(id, st, "bundle_invalid", e.to_string()),
                    }
                } else {
                    self.fail(id, st, "bundle", m);
                }
            }
            Ok(Err(e)) => self.fail(id, st, code_of(&e), e.to_string()),
            Err(e) => {
                // The run did not clean up after itself (HAR-23): its
                // unfinished bundle would refuse every retry.
                if planned.dir.exists() && acn_trace::bundle::verify(&planned.dir).is_err() {
                    let _ = std::fs::remove_dir_all(&planned.dir);
                }
                self.fail(id, st, "internal", e);
            }
        }
    }
}

/// The `request_id` of `request.json`'s bytes (CTL-11).
fn request_id_of(bytes: &[u8]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(resolve::REQUEST_CONTEXT);
    h.update(bytes);
    h.finalize().to_hex().to_string()
}
