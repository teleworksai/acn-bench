//! SPEC 090 acceptance (ATR-11, ATR-20, CON-18): a calibration with a
//! control. The same seeded `sim` workload runs over a link of one-way delay
//! `d` and over a link with none, the control. Each call then has exactly `2d`
//! of network time, the model's time is the control's, each turn is longer by
//! exactly its network time, and the share is the known value: streamed or not.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acn_attrib::core::{Cause, Decomposition, share};
use acn_harness::agent::Opts;
use acn_harness::run::{HypothesisArg, RunConfig, run_with_scenario};
use acn_harness::wire::Backend;
use acn_trace::identity::{BuildParts, Digest, Mode};

/// One way, in nanoseconds.
const D: i64 = 40_000_000;

/// Two tasks of plain question and answer: one call per turn, no tools.
fn workload(stream: bool) -> String {
    format!(
        r#"schema_version = 1

[agent]
system_prompt = "You answer briefly."
temperature = 0.0
max_tokens = 64
stream = {stream}
max_calls_per_turn = 2
compact_at_tokens = 100000
read_cost_threshold_tokens = 100000
summary_instruction = "Summarise."
summary_max_tokens = 16

[[task]]
id = "a"
tools = []

[[task.turn]]
user = "What is a monad?"
think_time_ns = {{ min = 0, max = 0 }}

[[task.turn]]
user = "And a functor?"
think_time_ns = {{ min = 0, max = 0 }}

[[task]]
id = "b"
tools = []

[[task.turn]]
user = "Name three sorting algorithms."
think_time_ns = {{ min = 0, max = 0 }}
"#
    )
}

fn scenario(dir: &Path, delay_ns: i64) -> PathBuf {
    let p = dir.join(format!("link-{delay_ns}.toml"));
    let delay = format!(
        "\n[link.delay]\ndelay_us = {}\njitter_us = 0\n",
        delay_ns / 1000
    );
    std::fs::write(
        &p,
        format!(
            "schema_version = 1\nname = \"link-{delay_ns}\"\n\n[[link]]\nname = \"p\"\ndirection = \"up\"\n{delay}\n[[link]]\nname = \"p\"\ndirection = \"down\"\n{delay}"
        ),
    )
    .unwrap();
    p
}

/// The most downlink messages any one call carried: one for a plain answer,
/// one per event for a streamed one.
fn most_answer_messages(bundle: &Path) -> usize {
    use arrow_array::Array as _;
    use arrow_array::cast::AsArray as _;
    let (_, views) = acn_trace::bundle::verify_views_read(bundle).unwrap();
    let link = &views.iter().find(|(v, _)| v.name == "link").unwrap().1;
    let call = link
        .column_by_name("call_id")
        .unwrap()
        .as_fixed_size_binary();
    let dir = link.column_by_name("direction").unwrap().as_string::<i32>();
    let mut per: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    for i in 0..link.num_rows() {
        if call.is_valid(i) && dir.value(i) == "down" {
            *per.entry(call.value(i).to_vec()).or_default() += 1;
        }
    }
    per.values().copied().max().unwrap_or(0)
}

/// Every turn's split, in turn-view order, of a `sim` run over `delay_ns`, and
/// the most answer messages a call carried.
fn run(dir: &Path, stream: bool, delay_ns: i64, arm: &str) -> (Vec<Decomposition>, usize) {
    let w = dir.join(format!("w-{stream}.toml"));
    std::fs::write(&w, workload(stream)).unwrap();
    let cfg = RunConfig {
        workload: w,
        backend: Backend::Mockllm,
        model: "mock-auto".into(),
        mode: Mode::Sim,
        arm: arm.into(),
        replicates: 2,
        vary: BTreeMap::new(),
        opts: Opts::default(),
        hypothesis: HypothesisArg::None { seed: 20_261_007 },
        runs_dir: dir.join(format!("runs-{arm}-{stream}")),
        start_dir: dir.to_path_buf(),
        engine_hash: Digest::of(b"engine"),
        build: BuildParts {
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
        .unwrap(),
        profiles: None,
    };
    let sc = scenario(dir, delay_ns);
    let b = run_with_scenario(&cfg, Some(&sc)).unwrap();
    (
        acn_hyp::read::read(&b.dir).unwrap().attrib.unwrap(),
        most_answer_messages(&b.dir),
    )
}

fn calibrate(stream: bool) {
    let dir = tempfile::tempdir().unwrap();
    let (treatment, messages) = run(dir.path(), stream, D, "treatment");
    let (control, _) = run(dir.path(), stream, 0, "control");
    // A streamed answer arrives as several messages, a plain one as one: the
    // two calibrations read different traces.
    if stream {
        assert!(messages > 1, "{messages}");
    } else {
        assert_eq!(messages, 1);
    }
    assert_eq!(treatment.len(), control.len());
    assert_eq!(treatment.len(), 6, "three turns, two replicates");
    for (t, c) in treatment.iter().zip(&control) {
        let (t, c) = (t.parts, c.parts);
        // One call per turn: its request and its last message, d each (ATR-11).
        assert_eq!(t.network_ns, 2 * D, "{t:?}");
        assert_eq!(c.network_ns, 0, "{c:?}");
        // The delay adds to the turn and takes nothing from the model.
        assert_eq!(t.model_ns, c.model_ns, "{t:?} {c:?}");
        assert_eq!(t.duration_ns - c.duration_ns, t.network_ns, "{t:?} {c:?}");
        assert_eq!((t.tool_ns, t.retry_ns), (0, 0));
    }
    // The share is the known value: 2d per turn over the control's time plus 2d
    // per turn (ATR-20); the control's is zero.
    let n = i64::try_from(treatment.len()).unwrap();
    let control_time: i64 = control.iter().map(|d| d.parts.duration_ns).sum();
    #[allow(clippy::cast_precision_loss)]
    let want = (2 * D * n) as f64 / (control_time + 2 * D * n) as f64;
    let tp: Vec<_> = treatment.iter().map(|d| d.parts).collect();
    let cp: Vec<_> = control.iter().map(|d| d.parts).collect();
    assert_eq!(share(&tp, Cause::Network).unwrap(), Some(want));
    assert_eq!(share(&cp, Cause::Network).unwrap(), Some(0.0));
}

/// Cites: ATR-11, ATR-20, CON-18
#[test]
fn a_known_delay_against_a_no_delay_control_gives_the_known_share() {
    calibrate(false);
}

/// Cites: ATR-11, ATR-20, CON-18
#[test]
fn a_streamed_answer_gives_the_same_known_share() {
    calibrate(true);
}
