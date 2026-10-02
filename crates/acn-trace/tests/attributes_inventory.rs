//! TRC-20: the `acn.*` inventory is complete, typed, and strict about what it accepts.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeSet;
use std::path::PathBuf;

use acn_trace::schema::{self, Inventory, ValueType};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

/// Every back-ticked `acn.…` name in SPEC 010, which is how the spec names
/// spans, events and attributes.
fn names_in_spec() -> BTreeSet<String> {
    let text =
        std::fs::read_to_string(repo_root().join("specs/010-trace-schema.md")).expect("spec");
    let mut out = BTreeSet::new();
    for piece in text.split('`').skip(1).step_by(2) {
        // `acn.keep_content = true` names the attribute and then a value.
        let piece = piece.split([' ', '=']).next().unwrap_or(piece);
        let ok = piece.starts_with("acn.")
            && piece
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_');
        // `acn.*` and `acn.cache.*` are wildcards in prose, not names.
        if ok && !piece.ends_with('.') {
            out.insert(piece.to_owned());
        }
    }
    out
}

/// Cites: TRC-20
#[test]
fn the_embedded_inventory_loads_and_every_name_in_the_spec_is_listed() {
    let inv = schema::inventory().expect("the embedded inventory is valid");
    let listed: BTreeSet<String> = inv.all_names().map(str::to_owned).collect();
    let spec = names_in_spec();
    let missing: Vec<&String> = spec.iter().filter(|n| !listed.contains(*n)).collect();
    assert!(
        missing.is_empty(),
        "named in SPEC 010 but not in acn_attributes.toml: {missing:?}"
    );
    // And nothing is listed that the spec never mentions: the spec is the source.
    let extra: Vec<&String> = listed.iter().filter(|n| !spec.contains(*n)).collect();
    assert!(
        extra.is_empty(),
        "in acn_attributes.toml but not in SPEC 010: {extra:?}"
    );
}

/// Cites: TRC-20
#[test]
fn every_attribute_has_a_type_a_unit_that_matches_its_suffix_and_a_producer() {
    let inv = schema::inventory().expect("inventory");
    assert!(inv.attributes().len() >= 70, "{}", inv.attributes().len());
    for a in inv.attributes() {
        assert!(!a.producers.is_empty(), "{}: no producer", a.name);
        assert!(
            !a.on.is_empty(),
            "{}: not attached to any span, event or resource",
            a.name
        );
        let (unit, ty) = (a.unit.as_str(), a.ty);
        let expect = if a.name.ends_with("_ms") {
            Some(("ms", ValueType::Float))
        } else if a.name.ends_with("_ns") {
            Some(("ns", ValueType::Int))
        } else if a.name.ends_with("_tokens") {
            Some(("tokens", ValueType::Int))
        } else if a.name.ends_with("bytes")
            || a.name.ends_with("_bytes_up")
            || a.name.ends_with("_bytes_down")
        {
            Some(("bytes", ValueType::Int))
        } else if a.name.ends_with("_kbps") {
            Some(("kbps", ValueType::Float))
        } else {
            None
        };
        if let Some((u, t)) = expect {
            assert_eq!((unit, ty), (u, t), "{}", a.name);
        }
        if ty == ValueType::Bool {
            assert_eq!(unit, "", "{}: booleans are unsuffixed and unitless", a.name);
        }
        assert_eq!(
            a.required,
            a.when.is_none(),
            "{}: optional attributes say when they are present",
            a.name
        );
    }
}

/// Cites: TRC-20, TRC-25
#[test]
fn every_promoted_attribute_has_a_typed_column() {
    let inv = schema::inventory().expect("inventory");
    let promoted: Vec<_> = inv.promoted().collect();
    assert!(promoted.len() >= 50, "{}", promoted.len());
    let mut columns = BTreeSet::new();
    for a in &promoted {
        let col = a.column_name();
        assert!(col.starts_with("acn_") && !col.contains('.'), "{col}");
        assert!(
            columns.insert(col.clone()),
            "two attributes promote to `{col}`"
        );
        assert_ne!(
            a.ty,
            ValueType::Bytes,
            "{}: a promoted column is a scalar",
            a.name
        );
        assert!(
            a.on.iter().all(|o| o != "resource"),
            "{}: resource attributes live in resources.parquet",
            a.name
        );
    }
    // The two large strings stay in the attribute map.
    for big in ["acn.scenario.toml", "acn.harness.knobs"] {
        assert!(!inv.attribute(big).expect(big).promoted, "{big}");
    }
    // What SPEC 080 quantities and the views read is promoted.
    for needed in [
        "acn.run_id",
        "acn.role",
        "acn.replicate",
        "acn.turn.outcome",
        "acn.call.index",
        "acn.call.input_tokens",
        "acn.call.new_input_tokens",
        "acn.cache.read_tokens",
        "acn.tool.requesting_call",
        "acn.link.applied_delay_ms",
    ] {
        assert!(inv.attribute(needed).expect(needed).promoted, "{needed}");
    }
}

/// Cites: TRC-3, TRC-12
#[test]
fn closed_value_sets_match_the_spec() {
    let inv = schema::inventory().expect("inventory");
    let values = |n: &str| inv.attribute(n).expect(n).values.clone();
    assert_eq!(values("acn.mode"), ["sim", "live", "netem"]);
    assert_eq!(values("acn.role"), ["treatment", "control", "researcher"]);
    assert_eq!(values("acn.hypothesis.status"), ["candidate", "frozen"]);
    assert_eq!(
        values("acn.turn.outcome"),
        ["success", "failure", "timeout", "aborted"]
    );
    assert_eq!(
        values("acn.call.stop_reason"),
        [
            "end_turn",
            "tool_use",
            "max_tokens",
            "stop_sequence",
            "content_filter",
            "client_abort",
            "transport_error",
            "other"
        ]
    );
    assert_eq!(
        values("acn.call.new_input_tokens_method"),
        ["tokens", "bytes_scaled"]
    );
    assert_eq!(
        values("acn.turn.compaction"),
        ["none", "window_full", "read_cost_threshold"]
    );
    assert_eq!(
        values("acn.tool.class"),
        [
            "file", "shell", "search", "http", "subagent", "testbed", "other"
        ]
    );
    assert_eq!(values("acn.tool.placement"), ["local", "remote"]);
    assert_eq!(values("acn.link.direction"), ["up", "down"]);
    let outage = inv
        .events()
        .iter()
        .find(|e| e.name == "acn.scenario.outage")
        .expect("outage event");
    let cause = outage
        .fields
        .iter()
        .find(|f| f.name == "cause")
        .expect("cause");
    assert_eq!(cause.values, ["handover", "scheduled", "trace"]);
    // The backend list is open: new providers arrive without a schema change.
    assert!(values("acn.backend").is_empty());
}

/// Cites: TRC-2
#[test]
fn the_semconv_pin_is_a_version_and_is_what_the_inventory_reports() {
    let pin =
        std::fs::read_to_string(repo_root().join("crates/acn-trace/src/schema/SEMCONV_VERSION"))
            .expect("pin");
    let inv = schema::inventory().expect("inventory");
    assert_eq!(inv.semconv_version(), pin.trim());
    assert_eq!(pin.trim(), "1.41.0");
}

/// Cites: CON-29
#[test]
fn run_options_are_listed_with_their_defaults() {
    let inv = schema::inventory().expect("inventory");
    let opt = inv
        .option("opt.stall_threshold_ms")
        .expect("the stall threshold is a run option");
    assert_eq!(
        opt.default, "250.0",
        "defaults are written in the text form of CON-27(c)"
    );
    for o in inv.options() {
        assert!(o.name.starts_with("opt."), "{}", o.name);
    }
}

fn reject(toml_text: &str, needle: &str) {
    let err = Inventory::parse(toml_text, "1.41.0")
        .expect_err("must be rejected")
        .to_string();
    assert!(err.contains(needle), "expected `{needle}` in: {err}");
}

const HEAD: &str = "schema_version = 1\n[[span]]\nname = \"acn.session\"\nkind = [\"internal\"]\nparents = [\"root\"]\nproducers = [\"acn-harness\"]\nrequirement = \"TRC-10\"\n";

fn attr(extra: &str) -> String {
    format!(
        "{HEAD}[[attribute]]\nname = \"acn.x_ms\"\non = [\"acn.session\"]\ntype = \"float\"\nunit = \"ms\"\nproducers = [\"acn-harness\"]\nrequired = true\npromoted = true\nrequirement = \"TRC-10\"\n{extra}"
    )
}

/// Cites: TRC-20
#[test]
fn a_malformed_inventory_is_rejected_with_the_reason() {
    assert!(
        Inventory::parse(&attr(""), "1.41.0").is_ok(),
        "the baseline fixture is valid"
    );
    reject(&attr("surprise = 1\n"), "unknown field");
    reject(&attr("").replace("acn.x_ms", "gen_ai.x_ms"), "acn.");
    reject(&attr("").replace("unit = \"ms\"", "unit = \"ns\""), "unit");
    reject(
        &attr("").replace("type = \"float\"", "type = \"int\""),
        "_ms",
    );
    reject(
        &attr("").replace("on = [\"acn.session\"]", "on = [\"acn.nowhere\"]"),
        "acn.nowhere",
    );
    reject(
        &attr("").replace("required = true", "required = false"),
        "when",
    );
    reject(&attr("when = \"always\"\n"), "when");
    reject(&attr("values = [\"a\"]\n"), "values");
    reject(
        &attr("").replace(
            "promoted = true\nrequirement = \"TRC-10\"",
            "promoted = true\nrequirement = \"nope\"",
        ),
        "requirement",
    );
    let twice = format!("{}{}", attr(""), attr("").replace(HEAD, ""));
    reject(&twice, "duplicate");
    reject(
        &attr("").replace(
            "producers = [\"acn-harness\"]\nrequired",
            "producers = []\nrequired",
        ),
        "producer",
    );
}

/// What is optional is part of the frozen contract: a producer may omit exactly these.
/// The cross-check with the spec covers names only, so the set is pinned here.
///
/// Cites: TRC-11, TRC-12, TRC-14, TRC-18, TRC-26
#[test]
fn exactly_these_attributes_are_optional_and_key_placements_hold() {
    let inv = schema::inventory().expect("inventory");
    let optional: BTreeSet<&str> = inv
        .attributes()
        .iter()
        .filter(|a| !a.required)
        .map(|a| a.name.as_str())
        .collect();
    let expected: BTreeSet<&str> = [
        "acn.turn.deadline_ms",
        "acn.turn.first_useful_result_ms",
        "acn.call.input_tokens",
        "acn.call.new_input_tokens",
        "acn.call.output_tokens",
        "acn.call.stop_reason",
        "acn.call.stop_reason_raw",
        "acn.cache.read_tokens",
        "acn.cache.write_tokens",
        "acn.call.ttft_ms",
        "acn.call.itl_p50_ms",
        "acn.call.itl_p99_ms",
        "acn.call.error_class",
        "acn.server.queue_ms",
        "acn.server.prefill_ms",
        "acn.server.decode_ms",
        "acn.fanout.parent_lineage",
        "acn.ingest.clock_offset_ns",
    ]
    .into_iter()
    .collect();
    assert_eq!(optional, expected);

    let count_on = |span: &str| {
        inv.attributes()
            .iter()
            .filter(|a| a.on.iter().any(|o| o == span))
            .count()
    };
    for (span, n) in [
        ("acn.session", 13),
        ("acn.turn", 5),
        ("chat", 20),
        ("execute_tool", 4),
        ("invoke_agent", 6),
        ("acn.link", 10),
        ("acn.scenario", 2),
        ("acn.replay.roundtrip", 9),
        ("resource", 2),
        ("*", 1),
    ] {
        assert_eq!(count_on(span), n, "attributes on `{span}`");
    }
    let a = |n: &str| inv.attribute(n).expect(n);
    assert_eq!(a("acn.tool.requesting_call").on, ["execute_tool"]);
    assert_eq!(a("acn.seed").ty, ValueType::Int);
    assert!(a("acn.turn.outcome").required);
    assert!(
        a("acn.call.new_input_tokens_method").required,
        "one method per run, stated on every call"
    );
    let link = inv
        .spans()
        .iter()
        .find(|s| s.name == "acn.link")
        .expect("acn.link");
    assert_eq!(
        link.parents,
        ["chat", "execute_tool"],
        "a call is a model or a tool invocation"
    );
}

/// A full valid fixture with one option and one provider, for the rejection cases below.
const FULL: &str = r#"schema_version = 1
[[span]]
name = "acn.session"
kind = ["internal"]
parents = ["root"]
producers = ["acn-harness"]
requirement = "TRC-10"
[[span]]
name = "chat"
kind = ["client"]
parents = ["acn.session"]
producers = ["acn-harness"]
requirement = "TRC-12"
[[event]]
name = "acn.stream.stall"
on = "chat"
producers = ["acn-harness"]
requirement = "TRC-12"
fields = [{ name = "gap_ms", type = "float", unit = "ms" }]
[[attribute]]
name = "acn.stall_threshold_ms"
on = ["acn.session"]
type = "float"
unit = "ms"
producers = ["acn-harness"]
required = true
promoted = true
requirement = "TRC-12"
[[attribute]]
name = "acn.call.stop_reason"
on = ["chat"]
type = "string"
unit = ""
producers = ["acn-harness"]
required = false
when = "the call ended"
promoted = true
values = ["end_turn", "max_tokens", "client_abort", "transport_error", "other"]
requirement = "TRC-12"
[[option]]
name = "opt.stall_threshold_ms"
type = "float"
default = "250.0"
attribute = "acn.stall_threshold_ms"
[[provider]]
name = "p"
input_tokens = ["usage.a", "usage.b"]
input_tokens_base = "usage.a"
output_tokens = "usage.o"
cache_read = "usage.b"
cache_read_absent = "absent"
stop_reason_field = "stop"
stop_reason = { stop = "end_turn", length = "max_tokens" }
"#;

fn reject_full(from: &str, to: &str, needle: &str) {
    assert!(FULL.contains(from), "fixture lacks `{from}`");
    reject(&FULL.replacen(from, to, 1), needle);
}

/// Every case below loaded cleanly before the adversarial review of T02a.
///
/// Cites: TRC-20, TRC-21, CON-29
#[test]
fn mutations_that_would_change_meaning_silently_are_rejected() {
    assert!(
        Inventory::parse(FULL, "1.41.0").is_ok(),
        "the full fixture is valid"
    );
    // Option defaults feed params_hash: one spelling only (CON-27c), of the declared type.
    for bad in [
        "250", "2.5e2", "250.00", "banana", "inf", "NaN", "1e+16", "-0.0",
    ] {
        reject_full(
            "default = \"250.0\"",
            &format!("default = \"{bad}\""),
            "default",
        );
    }
    // The one spelling is ryu's, the form `run_id` is built from (CON-27(c),
    // ADR-13), even where serde_json writes another (`1e+16`).
    for good in ["1e16", "0.05", "1e-7"] {
        assert!(
            Inventory::parse(
                &FULL.replacen("default = \"250.0\"", &format!("default = \"{good}\""), 1),
                "1.41.0"
            )
            .is_ok(),
            "{good}"
        );
    }
    // The option's attribute must sit on the session alone, and be required.
    reject_full(
        "name = \"acn.stall_threshold_ms\"\non = [\"acn.session\"]",
        "name = \"acn.stall_threshold_ms\"\non = [\"chat\"]",
        "acn.session",
    );
    let shared = format!(
        "{FULL}[[option]]\nname = \"opt.other\"\ntype = \"float\"\ndefault = \"1.0\"\nattribute = \"acn.stall_threshold_ms\"\n"
    );
    reject(&shared, "one option");
    // A provider cannot claim what only the harness observes.
    reject_full(
        "length = \"max_tokens\"",
        "length = \"client_abort\"",
        "client_abort",
    );
    reject_full(
        "length = \"max_tokens\"",
        "length = \"transport_error\"",
        "transport_error",
    );
    reject_full(
        "input_tokens_base = \"usage.a\"",
        "input_tokens_base = \"usage.z\"",
        "input_tokens_base",
    );
    reject_full(
        "cache_read_absent = \"absent\"",
        "cache_read_absent = \"maybe\"",
        "maybe",
    );
    // Closed sets, targets, units, spans.
    reject_full(
        "\"end_turn\", \"max_tokens\"",
        "\"end_turn\", \"end_turn\"",
        "duplicate",
    );
    reject_full(
        "on = [\"chat\"]\ntype = \"string\"",
        "on = [\"chat\", \"chat\"]\ntype = \"string\"",
        "duplicate",
    );
    reject_full(
        "type = \"string\"\nunit = \"\"",
        "type = \"string\"\nunit = \"ms\"",
        "unit",
    );
    reject_full(
        "{ name = \"gap_ms\", type = \"float\", unit = \"ms\" }",
        "{ name = \"gap_ms\", type = \"int\", unit = \"ms\" }",
        "_ms",
    );
    reject_full("kind = [\"client\"]", "kind = [\"banana\"]", "banana");
    reject_full(
        "parents = [\"acn.session\"]",
        "parents = [\"acn.nowhere\"]",
        "acn.nowhere",
    );
    reject_full("schema_version = 1", "schema_version = 2", "schema_version");
}

/// The `other` fallback would hide a typo in a mapping table, so the tables are pinned.
///
/// Cites: TRC-21
#[test]
fn the_provider_stop_tables_are_exactly_these() {
    let inv = schema::inventory().expect("inventory");
    let table = |p: &str| -> Vec<(String, String)> {
        inv.provider(p)
            .expect(p)
            .stop_reason
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    };
    let pairs = |v: &[(&str, &str)]| -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = v
            .iter()
            .map(|(k, x)| ((*k).to_owned(), (*x).to_owned()))
            .collect();
        out.sort();
        out
    };
    assert_eq!(
        table("anthropic"),
        pairs(&[
            ("end_turn", "end_turn"),
            ("tool_use", "tool_use"),
            ("max_tokens", "max_tokens"),
            ("stop_sequence", "stop_sequence"),
            ("refusal", "content_filter"),
            ("pause_turn", "other"),
            ("model_context_window_exceeded", "max_tokens"),
        ])
    );
    assert_eq!(
        table("openai"),
        pairs(&[
            ("stop", "end_turn"),
            ("tool_calls", "tool_use"),
            ("function_call", "tool_use"),
            ("length", "max_tokens"),
            ("content_filter", "content_filter")
        ])
    );
    for p in ["vllm", "sglang"] {
        assert_eq!(
            table(p),
            pairs(&[
                ("stop", "end_turn"),
                ("tool_calls", "tool_use"),
                ("length", "max_tokens"),
                ("abort", "other")
            ]),
            "{p}: a server-side abort is not a client abort"
        );
    }
    assert_eq!(
        table("mockllm"),
        pairs(&[
            ("stop", "end_turn"),
            ("tool_calls", "tool_use"),
            ("length", "max_tokens"),
            ("content_filter", "content_filter")
        ])
    );
    for (p, absent) in [
        ("anthropic", "zero"),
        ("mockllm", "zero"),
        ("openai", "absent"),
        ("vllm", "absent"),
        ("sglang", "absent"),
    ] {
        assert_eq!(
            inv.provider(p).expect(p).cache_read_absent.as_str(),
            absent,
            "{p}"
        );
    }
}
