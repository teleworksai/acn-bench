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
fn draws_are_integer_and_match_known_answers() {
    // Golden vectors (CON-5(a)): a change to a sampler or to the generator
    // crates shows here first.
    let mut r = rng("gen.test");
    let u = Dist::Uniform { min: 10, max: 20 };
    let got: Vec<u64> = (0..6).map(|_| u.draw(&mut r)).collect();
    let mut r2 = rng("gen.test");
    let again: Vec<u64> = (0..6).map(|_| u.draw(&mut r2)).collect();
    assert_eq!(got, again);
    assert!(got.iter().all(|x| (10..=20).contains(x)));
    // A quantile table interpolates in integers, rounding down, and reaches
    // its last value.
    let q = Dist::Quantiles(vec![(0, 0), (500_000, 100), (1_000_000, 1000)]);
    let mut r = rng("gen.test.q");
    let draws: Vec<u64> = (0..2000).map(|_| q.draw(&mut r)).collect();
    assert!(draws.iter().all(|x| *x <= 1000));
    let below_median = draws.iter().filter(|x| **x <= 100).count();
    assert!((900..=1100).contains(&below_median), "{below_median}");
    // A weighted choice takes the first entry whose cumulative weight exceeds
    // the draw.
    let w = Dist::Weighted {
        entries: vec![(7, 1), (8, 3)],
        total: 4,
    };
    let mut r = rng("gen.test.w");
    let sevens = (0..4000).filter(|_| w.draw(&mut r) == 7).count();
    assert!((850..=1150).contains(&sevens), "{sevens}");
    // The pinned first values of each.
    let mut r = rng("gen.golden");
    let golden = [
        u.draw(&mut r),
        q.draw(&mut r),
        w.draw(&mut r),
        Dist::Const(3).draw(&mut r),
    ];
    assert_eq!(golden, GOLDEN);
}

/// The first draws of `gen.golden` under seed 7: uniform 10..=20, the
/// quantile table, the weighted choice, a constant.
const GOLDEN: [u64; 4] = [13, 946, 8, 3];
