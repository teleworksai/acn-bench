//! Helpers of the generator's run tests: a small sheet, a run of it, and the
//! spans of its bundle.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::Path;

use acn_gen::run::{GenConfig, GenWritten, run};
use acn_harness::agent::Opts;
use acn_harness::run::HypothesisArg;
use acn_trace::identity::{BuildParts, Digest, Mode};
use acn_trace::model::{AttrValue, SpanRow, Trace};

/// A sheet small enough to read whole: three sessions of two or three
/// turns, chains of one to four tool calls, a fan-out of two now and then.
pub const SMALL: &str = r#"schema_version = 1
placeholder = true
doc = "a small sheet for tests"
model = "mock-agentic"
sessions = 3
system_tokens = 300
summary_instruction_tokens = 8
summary_max_tokens = 32
compact_at_tokens = 0
session_start_ns = { uniform = [0, 3_000_000_000] }
turns_per_session = { uniform = [2, 3] }
think_time_ns = { uniform = [500_000_000, 2_000_000_000] }
chain_length = { uniform = [1, 4] }
fanout_width = { weighted = [[0, 2], [2, 1]] }
user_tokens = { uniform = [10, 40] }
answer_tokens = { uniform = [16, 64] }
tool_class = { weighted = [["file", 2], ["search", 1]] }

[tool_result_tokens]
file = { uniform = [50, 200] }
search = { uniform = [20, 80] }

[tool_duration_ns]
file = { uniform = [1_000_000, 5_000_000] }
search = { uniform = [5_000_000, 20_000_000] }
"#;

pub fn build() -> acn_trace::identity::BuildInfo {
    BuildParts {
        cargo_lock: Digest::of(b"lock"),
        rust_toolchain: Digest::of(b"toolchain"),
        cargo_config: Digest::of(b"config"),
        source_hash: Digest::of(b"build"),
        target: "test",
        profile: "debug",
        features: "",
        rustflags: "",
    }
    .info()
    .unwrap()
}

/// A config for `sheet` (written to a temporary directory) in `sim`, seed 7,
/// one replicate.
pub fn config(dir: &Path, sheet: &str) -> GenConfig {
    let path = dir.join("sheet.toml");
    std::fs::write(&path, sheet).unwrap();
    GenConfig {
        sheet: path,
        mode: Mode::Sim,
        arm: "treatment".into(),
        replicates: 1,
        vary: BTreeMap::new(),
        opts: Opts::default(),
        hypothesis: HypothesisArg::None { seed: 7 },
        runs_dir: dir.join("runs"),
        start_dir: dir.to_path_buf(),
        engine_hash: Digest::of(b"engine"),
        build: build(),
        profiles: None,
    }
}

pub fn run_ok(cfg: &GenConfig) -> GenWritten {
    run(cfg, None).unwrap()
}

pub fn read(dir: &Path) -> Trace {
    acn_trace::parquet_io::read_trace(dir, &acn_trace::schema::inventory().unwrap()).unwrap()
}

pub fn spans<'a>(t: &'a Trace, name: &str) -> Vec<&'a SpanRow> {
    let mut v: Vec<&SpanRow> = t.spans.iter().filter(|s| s.name == name).collect();
    v.sort_by_key(|s| (s.start_ns, s.span_id));
    v
}

pub fn int(s: &SpanRow, key: &str) -> Option<i64> {
    match s.attrs.get(key) {
        Some(AttrValue::Int(v)) => Some(*v),
        _ => None,
    }
}

pub fn text<'a>(s: &'a SpanRow, key: &str) -> Option<&'a str> {
    match s.attrs.get(key) {
        Some(AttrValue::String(v)) => Some(v),
        _ => None,
    }
}

/// The spans whose parent is `parent`, by start time.
pub fn children<'a>(t: &'a Trace, parent: &SpanRow, name: &str) -> Vec<&'a SpanRow> {
    let mut v: Vec<&SpanRow> = t
        .spans
        .iter()
        .filter(|s| s.name == name && s.parent_span_id == Some(parent.span_id))
        .collect();
    v.sort_by_key(|s| (s.start_ns, s.span_id));
    v
}
