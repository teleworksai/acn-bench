//! Synthetic bundles for the verdict tests (SPEC 080 §5: "on synthetic bundles"):
//! a manifest as a run writes it and view rows whose `cached_token_ratio` per
//! replicate is chosen by the test. `read` is not involved; it is tested on real
//! bundles in the acceptance suite.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use acn_hyp::Hypothesis;
use acn_hyp::quantities::{Call, Turn};
use acn_hyp::read::{BundleData, SessionRow};
use acn_hyp::verdict::hypothesis_seed;
use acn_trace::bundle::{Manifest, ManifestHypothesis};
use acn_trace::identity::{BuildInfo, BuildParts, Digest};

pub const ENGINE: &[u8] = b"engine";

pub fn engine() -> Digest {
    Digest::of(ENGINE)
}

pub fn build(tag: &str) -> BuildInfo {
    BuildParts {
        cargo_lock: Digest::of(b"lock"),
        rust_toolchain: Digest::of(b"toolchain"),
        cargo_config: Digest::of(b"config"),
        source_hash: Digest::of(tag.as_bytes()),
        target: "test",
        profile: "debug",
        features: "",
        rustflags: "",
    }
    .info()
    .unwrap()
}

/// One synthetic bundle: one cell, one arm, one mode.
#[derive(Clone)]
pub struct Spec {
    pub name: String,
    pub vary: Vec<(String, String)>,
    pub arm: String,
    pub mode: String,
    pub backend: String,
    pub model: String,
    pub scenario: String,
    pub workload: String,
    pub status: Option<String>,
    pub seed: Option<u64>,
    pub build: String,
    pub method: String,
    /// The `cached_token_ratio` of replicate `i`: `None` runs no session for it;
    /// NaN runs one whose call reports no usage (an undefined value).
    pub ratios: Vec<Option<f64>>,
}

impl Spec {
    pub fn new(name: &str, vary: &[(&str, &str)], arm: &str, ratios: Vec<Option<f64>>) -> Self {
        Self {
            name: name.to_owned(),
            vary: vary
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            arm: arm.to_owned(),
            mode: "sim".into(),
            backend: "mockllm".into(),
            model: "mock-explicit".into(),
            scenario: Digest::of(b"scenario").to_hex(),
            workload: Digest::of(b"workload").to_hex(),
            status: None,
            seed: None,
            build: "build".into(),
            method: "tokens".into(),
            ratios,
        }
    }

    pub fn mode(mut self, m: &str) -> Self {
        self.mode = m.into();
        self
    }

    pub fn backend(mut self, b: &str, model: &str) -> Self {
        self.backend = b.into();
        self.model = model.into();
        self
    }
}

/// `n` replicates, all with ratio `x`.
pub fn flat(n: usize, x: f64) -> Vec<Option<f64>> {
    vec![Some(x); n]
}

/// `n` replicates alternating `a` and `b`: a nonzero noise floor.
pub fn alt(n: usize, a: f64, b: f64) -> Vec<Option<f64>> {
    (0..n)
        .map(|i| Some(if i % 4 < 2 { a } else { b } + (i % 3) as f64 * 0.001))
        .collect()
}

/// The bundle `s` describes, for hypothesis `h`.
pub fn bundle(h: &Hypothesis, s: &Spec) -> BundleData {
    let hash = h.hash();
    let mut params: BTreeMap<String, String> = BTreeMap::from([
        ("arms".into(), s.arm.clone()),
        ("backend".into(), s.backend.clone()),
        ("model".into(), s.model.clone()),
        (
            "hyp_status".into(),
            s.status
                .clone()
                .unwrap_or_else(|| h.status().as_str().into()),
        ),
        ("replicates".into(), s.ratios.len().to_string()),
    ]);
    for (k, v) in &s.vary {
        params.insert(format!("vary.{k}"), v.clone());
    }
    let run_id = Digest::of(format!("run:{}", s.name).as_bytes());
    let manifest = Manifest {
        run_id: run_id.to_hex(),
        seed: s
            .seed
            .unwrap_or_else(|| hypothesis_seed(&hash).unwrap())
            .to_string(),
        mode: s.mode.clone(),
        backend: s.backend.clone(),
        model: s.model.clone(),
        endpoint_host: None,
        scenario_hash: s.scenario.clone(),
        workload_hash: s.workload.clone(),
        hypothesis: ManifestHypothesis {
            id: h.id.clone(),
            status: s
                .status
                .clone()
                .unwrap_or_else(|| h.status().as_str().into()),
            hash: hash.to_hex(),
        },
        engine_hash: engine().to_hex(),
        build: build(&s.build),
        params,
        execution_order: None,
        semconv_version: "1.40.0".into(),
        producers: vec!["acn-harness".into()],
        started_at: None,
        replicates: u32::try_from(s.ratios.len()).unwrap(),
        files: BTreeMap::new(),
    };
    let (mut sessions, mut turns, mut calls) = (Vec::new(), Vec::new(), Vec::new());
    for (i, r) in s.ratios.iter().enumerate() {
        let Some(x) = r else { continue };
        let mut sid = [0u8; 8];
        sid[..4].copy_from_slice(&u32::try_from(i).unwrap().to_le_bytes());
        sid[4..].copy_from_slice(&run_id.0[..4]);
        sessions.push(SessionRow {
            session_id: sid,
            role: s.arm.clone(),
            replicate: i64::try_from(i).unwrap(),
        });
        turns.push(Turn {
            session_id: sid,
            outcome: "success".into(),
            compaction: "none".into(),
        });
        let defined = x.is_finite();
        calls.push(Call {
            session_id: sid,
            input_tokens: defined.then_some(100_000),
            cache_read_tokens: defined.then(|| (x * 100_000.0).round() as i64),
            cache_write_tokens: defined.then_some(0),
            output_tokens: defined.then_some(10),
            ttft_ns: Some(1_000_000),
        });
    }
    BundleData {
        dir: PathBuf::from(format!("runs/{}", run_id.to_hex())),
        manifest,
        run_id,
        bundle_digest: Digest::of(format!("digest:{}", s.name).as_bytes()),
        sessions,
        turns,
        calls,
        methods: BTreeSet::from([s.method.clone()]),
    }
}

/// Add a session for replicate index `i` (at or beyond the design: ignored).
pub fn extra_replicate(b: &mut BundleData, i: i64) {
    let sid = [0xee, 0, 0, 0, 0, 0, 0, u8::try_from(i).unwrap()];
    b.sessions.push(SessionRow {
        session_id: sid,
        role: b.sessions[0].role.clone(),
        replicate: i,
    });
    b.turns.push(Turn {
        session_id: sid,
        outcome: "success".into(),
        compaction: "none".into(),
    });
    b.calls.push(Call {
        session_id: sid,
        input_tokens: Some(100_000),
        cache_read_tokens: Some(99_000),
        cache_write_tokens: Some(0),
        output_tokens: Some(10),
        ttft_ns: Some(1_000_000),
    });
    b.bundle_digest = Digest::of(format!("{}+{i}", b.bundle_digest.to_hex()).as_bytes());
}
