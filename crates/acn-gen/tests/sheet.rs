//! The sheet (SPEC 050 GEN-1) and its distributions (GEN-2): the shipped
//! sheet loads, every refusal fires, and draws match known answers.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use acn_gen::sheet::{Dist, Sheet};
use acn_mockllm::profile::{Profiles, embedded};

fn shipped_text() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../workloads/gen/appendix-c.toml"
    ))
    .unwrap()
}

fn profiles() -> Profiles {
    embedded().unwrap()
}

fn refused(text: &str, want: &str) {
    let e = Sheet::parse(text, &profiles()).unwrap_err().to_string();
    assert!(e.contains(want), "expected `{want}` in: {e}");
}

/// Cites: GEN-1
#[test]
fn the_shipped_sheet_loads_and_is_a_placeholder() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../workloads/gen/appendix-c.toml");
    let s = Sheet::load(&path).unwrap();
    assert!(s.placeholder);
    assert_eq!(s.model, "mock-agentic");
    // Every drawable class has its per-class parameters, and tools are listed
    // in TRC-13's order, `subagent` included since a turn can fan out.
    assert_eq!(
        s.tool_classes(),
        ["file", "shell", "search", "http", "subagent", "other"]
    );
}

/// Cites: GEN-1
#[test]
fn a_sheet_is_refused_for_each_fault_gen_1_names() {
    let s = shipped_text();
    refused(&format!("extra = 1\n{s}"), "unknown field");
    refused(
        &s.replace("schema_version = 1", "schema_version = 2"),
        "schema_version",
    );
    refused(
        &s.replace("model = \"mock-agentic\"", "model = \"nope\""),
        "not an embedded mock profile",
    );
    refused(&s.replace("sessions = 8\n", ""), "missing field");
    refused(&s.replace("sessions = 8", "sessions = 0"), "`sessions`");
    refused(
        &s.replace("other = { uniform = [10, 500] }\n", ""),
        "no entry for class `other`",
    );
    refused(
        &s.replace(
            "[tool_duration_ns]\n",
            "[tool_duration_ns]\ntestbed = { const = 1 }\n",
        ),
        "`tool_duration_ns.testbed`",
    );
    refused(
        &s.replace("[\"other\", 5]", "[\"subagent\", 5]"),
        "cannot draw `subagent`",
    );
    refused(
        &s.replace("[\"other\", 5]", "[\"robot\", 5]"),
        "not a TRC-13 class",
    );
    // The spawn is a tool call: a possible empty chain cannot fan out.
    refused(
        &s.replace(
            "chain_length = { quantiles = [[0, 1],",
            "chain_length = { quantiles = [[0, 0],",
        ),
        "the spawn is a tool call",
    );
    // A chain longer than the profile's limit (MLM-40).
    refused(
        &s.replace("[1000000, 40]] }\nfanout", "[1000000, 65]] }\nfanout"),
        "allows 64 tool calls per turn",
    );
    refused(
        &s.replace("model = \"mock-agentic\"", "model = \"mock-auto\""),
        "allows 1 tool calls",
    );
}

/// Cites: GEN-2
#[test]
fn a_distribution_is_refused_unless_well_formed() {
    let s = shipped_text();
    let with = |d: &str| s.replace("other = { uniform = [10, 500] }", &format!("other = {d}"));
    refused(&with("{ uniform = [5, 4] }"), "min <= max");
    refused(&with("{ uniform = [1, 2], const = 3 }"), "exactly one of");
    refused(
        &with("{ quantiles = [[1, 0], [1000000, 9]] }"),
        "from p = 0",
    );
    refused(&with("{ quantiles = [[0, 0], [999999, 9]] }"), "from p = 0");
    refused(
        &with("{ quantiles = [[0, 5], [500000, 4], [1000000, 9]] }"),
        "non-decreasing",
    );
    refused(
        &with("{ quantiles = [[0, 0], [0, 4], [1000000, 9]] }"),
        "strictly increasing",
    );
    refused(&with("{ weighted = [[1, 0]] }"), "every weight positive");
    refused(&with("{ weighted = [[1, 2], [1, 3]] }"), "distinct");
}

fn rng(name: &str) -> rand_chacha::ChaCha20Rng {
    acn_trace::identity::substream_rng(7, name).unwrap()
}

/// Cites: GEN-2
#[test]
fn draws_follow_the_formulas_of_gen_2_exactly() {
    use acn_harness::agent::below;
    const PPM: u64 = 1_000_000;
    // A quantile table from (0, 0) to (PPM, PPM) is the draw of u itself.
    let identity = Dist::Quantiles(vec![(0, 0), (PPM, PPM)]);
    let (mut a, mut b) = (rng("gen.q.id"), rng("gen.q.id"));
    for _ in 0..1000 {
        assert_eq!(identity.draw(&mut a), below(&mut b, PPM + 1));
    }
    // Interpolation rounds down within the first segment holding u.
    let table = Dist::Quantiles(vec![(0, 0), (3, 1), (PPM, 1000)]);
    let (mut a, mut b) = (rng("gen.q.t"), rng("gen.q.t"));
    for _ in 0..1000 {
        let u = below(&mut b, PPM + 1);
        let want = if u <= 3 {
            u / 3
        } else {
            1 + (999 * (u - 3)) / (PPM - 3)
        };
        assert_eq!(table.draw(&mut a), want);
    }
    // A weighted choice follows listed order, not value order.
    let w = Dist::Weighted {
        entries: vec![(8, 3), (7, 1)],
        total: 4,
    };
    let (mut a, mut b) = (rng("gen.w"), rng("gen.w"));
    for _ in 0..1000 {
        let want = if below(&mut b, 4) < 3 { 8 } else { 7 };
        assert_eq!(w.draw(&mut a), want);
    }
    // A uniform range is min plus a draw below its width.
    let u = Dist::Uniform { min: 10, max: 20 };
    let (mut a, mut b) = (rng("gen.u"), rng("gen.u"));
    for _ in 0..1000 {
        assert_eq!(u.draw(&mut a), 10 + below(&mut b, 11));
    }
}

/// Cites: GEN-2
#[test]
fn the_first_draws_are_pinned() {
    // Golden vectors (CON-5(a)): a change to the sampler or to the generator
    // crates shows here first.
    let mut r = rng("gen.golden");
    let u = Dist::Uniform { min: 10, max: 20 };
    let q = Dist::Quantiles(vec![(0, 0), (500_000, 100), (1_000_000, 1000)]);
    let w = Dist::Weighted {
        entries: vec![(7, 1), (8, 3)],
        total: 4,
    };
    let got: Vec<u64> = (0..8)
        .flat_map(|_| [u.draw(&mut r), q.draw(&mut r), w.draw(&mut r)])
        .collect();
    assert_eq!(got, GOLDEN);
}

/// The first draws of `gen.golden` under seed 7: uniform 10..=20, the
/// quantile table, the weighted choice, eight times over.
const GOLDEN: [u64; 24] = [
    13, 946, 8, 12, 26, 8, 11, 81, 7, 16, 716, 7, 11, 975, 8, 17, 138, 7, 17, 775, 8, 11, 766, 8,
];

/// A minimal sheet with `tool_class` and its per-class tables as given.
fn minimal(tool_class: &str, per_class: &str) -> String {
    format!(
        r#"schema_version = 1
placeholder = true
doc = "t"
model = "mock-agentic"
sessions = 1
system_tokens = 10
summary_instruction_tokens = 4
summary_max_tokens = 8
compact_at_tokens = 0
session_start_ns = {{ const = 0 }}
turns_per_session = {{ const = 1 }}
think_time_ns = {{ const = 0 }}
chain_length = {{ const = 1 }}
fanout_width = {{ const = 0 }}
user_tokens = {{ const = 5 }}
answer_tokens = {{ const = 5 }}
tool_class = {tool_class}
[tool_result_tokens]
{per_class}
[tool_duration_ns]
{per_class}
"#
    )
}

/// Cites: GEN-1, GEN-2
#[test]
fn a_tool_class_is_drawn_by_name_whatever_the_order_written() {
    let s = Sheet::parse(
        &minimal(
            r#"{ weighted = [["search", 1], ["file", 1]] }"#,
            "file = { const = 1 }
search = { const = 2 }",
        ),
        &profiles(),
    )
    .unwrap();
    // Classes in TRC-13 order; the choice follows the order written.
    assert_eq!(s.classes, ["file", "search"]);
    let (mut a, mut b) = (rng("gen.c"), rng("gen.c"));
    for _ in 0..500 {
        let class = s.classes[s.tool_class.draw(&mut a) as usize];
        let want = if acn_harness::agent::below(&mut b, 2) == 0 {
            "search"
        } else {
            "file"
        };
        assert_eq!(class, want);
    }
}

/// Cites: GEN-1
#[test]
fn nested_unknown_keys_turns_below_one_and_oversized_values_are_refused() {
    let s = shipped_text();
    refused(
        &s.replace(
            "other = { uniform = [10, 500] }",
            "other = { unifrom = [10, 500] }",
        ),
        "unknown field",
    );
    refused(
        &s.replace(
            "tool_class = { weighted = [[",
            "tool_class = { x = 1, weighted = [[",
        ),
        "unknown field",
    );
    refused(
        &s.replace(
            "turns_per_session = { quantiles = [[0, 1],",
            "turns_per_session = { quantiles = [[0, 0],",
        ),
        "`turns_per_session` is at least 1",
    );
    refused(
        &s.replace(
            "[1000000, 4000]] }
answer_tokens",
            "[1000000, 99999999]] }
answer_tokens",
        ),
        "`user_tokens` can be 99999999",
    );
    refused(
        &s.replace(
            "[900000, 10], [1000000, 30]] }",
            "[900000, 10], [1000000, 3000000]] }",
        ),
        "`turns_per_session` can be 3000000",
    );
    refused(
        &s.replace("sessions = 8", "sessions = 2000000"),
        "`sessions` can be 2000000",
    );
}
