//! `acn attrib` (SPEC 090 ATR-40, ATR-41): one JSON object each (CON-8), the
//! same bytes on every run, the labels a citation needs, and the refusals.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

fn acn(dir: &Path, args: &[&str]) -> (Option<i32>, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_acn"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("spawn acn");
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    let json: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not one JSON object: {e}\n{stdout}"));
    (out.status.code(), json)
}

/// A `sim` bundle of the smoke workload over a link of 40 ms each way.
fn bundle(dir: &Path) -> String {
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../workloads/harness-smoke.toml"
        ),
        dir.join("w.toml"),
    )
    .unwrap();
    let delay = "\n[link.delay]\ndelay_us = 40000\njitter_us = 0\n";
    std::fs::write(
        dir.join("p.toml"),
        format!(
            "schema_version = 1\nname = \"p\"\n\n[[link]]\nname = \"p\"\ndirection = \"up\"\n{delay}\n[[link]]\nname = \"p\"\ndirection = \"down\"\n{delay}"
        ),
    )
    .unwrap();
    let (code, j) = acn(
        dir,
        &[
            "harness",
            "run",
            "--workload",
            "w.toml",
            "--backend",
            "mockllm",
            "--model",
            "mock-auto",
            "--seed",
            "5",
            "--replicates",
            "2",
            "--scenario",
            "p.toml",
        ],
    );
    assert_eq!(code, Some(0), "{j}");
    j["dir"].as_str().unwrap().to_owned()
}

/// Cites: ATR-40, CON-8
#[test]
fn turns_writes_one_row_per_turn_the_same_bytes_every_time() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let b = bundle(d);
    let (code, j) = acn(d, &["attrib", "turns", &b, "--out", "a"]);
    assert_eq!(code, Some(0), "{j}");
    assert_eq!(j["ok"], true);
    assert_eq!(
        (j["mode"].as_str(), j["backend"].as_str()),
        (Some("sim"), Some("mockllm"))
    );
    // The labels a verdict would give, so a mock sim number never reads as citable.
    assert_eq!(
        j["labels"],
        json!(["exploratory", "mock-gated", "sim-only"])
    );
    let rows = j["rows"].as_u64().unwrap();
    assert!(rows > 0);
    // One entry per (role, replicate), each with every attribution quantity.
    let reps = j["replicates"].as_array().unwrap();
    assert_eq!(reps.len(), 2);
    for (k, r) in reps.iter().enumerate() {
        assert_eq!(
            (r["role"].as_str(), r["replicate"].as_u64()),
            (Some("treatment"), Some(k as u64))
        );
        let share = r["network_attributable_share"].as_f64().unwrap();
        assert!(share > 0.0 && share < 1.0, "{r}");
        for q in [
            "model_share",
            "tool_share",
            "retry_share",
            "other_share",
            "tail_network_share_p99",
        ] {
            assert!(r[q].is_number(), "{q}: {r}");
        }
    }
    // The file: the declared schema, one row per turn, and the same bytes again.
    let file = d.join("a").join("attribution.parquet");
    let bytes = std::fs::read(&file).unwrap();
    assert_eq!(
        blake3::hash(&bytes).to_hex().as_str(),
        j["blake3"].as_str().unwrap()
    );
    let reader = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
        std::fs::File::open(&file).unwrap(),
    )
    .unwrap();
    // The columns, pinned here rather than by the function that writes them.
    let cols: Vec<(String, String, bool)> = reader
        .schema()
        .fields()
        .iter()
        .map(|f| {
            (
                f.name().clone(),
                format!("{:?}", f.data_type()),
                f.is_nullable(),
            )
        })
        .collect();
    let int = |n: &str| (n.to_owned(), "Int64".to_owned(), false);
    let mut want = vec![
        ("run_id".to_owned(), "Utf8".to_owned(), false),
        ("role".to_owned(), "Utf8".to_owned(), false),
        int("replicate"),
        (
            "session_id".to_owned(),
            "FixedSizeBinary(8)".to_owned(),
            false,
        ),
    ];
    for c in [
        "turn_index",
        "duration_ns",
        "network_ns",
        "model_ns",
        "tool_ns",
        "retry_ns",
        "other_ns",
    ] {
        want.push(int(c));
    }
    want.push(("queue_wait_ns".to_owned(), "Int64".to_owned(), true));
    for c in ["stalls", "retries", "clipped_ns", "unattributed_link_ns"] {
        want.push(int(c));
    }
    assert_eq!(cols[..cols.len() - 1], want[..]);
    assert_eq!(cols.last().unwrap().0, "hop_ns");
    assert!(cols.last().unwrap().1.starts_with("Map("), "{cols:?}");
    // The rows are acn-attrib's split of the bundle, in turn-view order.
    use arrow_array::Array as _;
    use arrow_array::cast::AsArray as _;
    use arrow_array::types::Int64Type;
    let batches: Vec<_> = reader.build().unwrap().map(|b| b.unwrap()).collect();
    let n: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(n as u64, rows);
    let ds = acn_hyp::read::read(&d.join(&b)).unwrap().attrib.unwrap();
    let col = |name: &str| -> Vec<i64> {
        batches
            .iter()
            .flat_map(|b| {
                let a = b
                    .column_by_name(name)
                    .unwrap()
                    .as_primitive::<Int64Type>()
                    .clone();
                (0..a.len()).map(move |i| a.value(i)).collect::<Vec<_>>()
            })
            .collect()
    };
    let parts = |f: fn(&acn_attrib::core::Parts) -> i64| -> Vec<i64> {
        ds.iter().map(|d| f(&d.parts)).collect()
    };
    assert_eq!(col("duration_ns"), parts(|p| p.duration_ns));
    assert_eq!(col("network_ns"), parts(|p| p.network_ns));
    assert_eq!(col("model_ns"), parts(|p| p.model_ns));
    assert_eq!(col("tool_ns"), parts(|p| p.tool_ns));
    assert_eq!(col("retry_ns"), parts(|p| p.retry_ns));
    assert_eq!(col("other_ns"), parts(|p| p.other_ns));
    assert_eq!(
        col("turn_index"),
        ds.iter().map(|d| d.turn_index).collect::<Vec<_>>()
    );
    // Hops are keyed `<link>/<direction>`: one link each way here.
    let hops = batches[0]
        .column_by_name("hop_ns")
        .unwrap()
        .as_map()
        .clone();
    let keys = hops.keys().as_string::<i32>().clone();
    let mut seen: Vec<String> = (0..keys.len()).map(|i| keys.value(i).to_owned()).collect();
    seen.sort();
    seen.dedup();
    assert_eq!(seen, ["p/down", "p/up"]);
    let (code, again) = acn(d, &["attrib", "turns", &b, "--out", "b"]);
    assert_eq!(code, Some(0), "{again}");
    assert_eq!(
        std::fs::read(d.join("b").join("attribution.parquet")).unwrap(),
        bytes
    );
    assert_eq!(again["replicates"], j["replicates"]);
    // Never into the bundle (TRC-23), and a directory that is no bundle is refused.
    // Directly, or by a path that only resolves into it.
    let id = b.rsplit('/').next().unwrap();
    for inside in [format!("{b}/x"), format!("{b}/../{id}/y")] {
        let (code, j) = acn(d, &["attrib", "turns", &b, "--out", &inside]);
        assert_eq!((code, &j["ok"]), (Some(1), &json!(false)), "{inside}: {j}");
        assert!(j["error"].as_str().unwrap().contains("immutable"), "{j}");
        assert!(!d.join(&b).join("x").exists() && !d.join(&b).join("y").exists());
    }
    // A link planted where the file goes is replaced, never written through.
    #[cfg(unix)]
    {
        let target = d.join(&b).join("events.parquet");
        let before = std::fs::read(&target).unwrap();
        std::fs::create_dir_all(d.join("l")).unwrap();
        std::os::unix::fs::symlink(&target, d.join("l").join("attribution.parquet")).unwrap();
        let (code, j) = acn(d, &["attrib", "turns", &b, "--out", "l"]);
        assert_eq!(code, Some(0), "{j}");
        assert_eq!(
            std::fs::read(&target).unwrap(),
            before,
            "the bundle is untouched"
        );
        let meta = std::fs::symlink_metadata(d.join("l").join("attribution.parquet")).unwrap();
        assert!(meta.file_type().is_file());
        assert_eq!(
            std::fs::read(d.join("l").join("attribution.parquet")).unwrap(),
            bytes
        );
    }
    let (code, j) = acn(d, &["attrib", "turns", "a", "--out", "c"]);
    assert_eq!((code, &j["ok"]), (Some(1), &json!(false)), "{j}");
}

fn cell(a: &str, b: &str, effect: Option<(f64, f64, f64)>) -> Value {
    let effects = match effect {
        Some((v, lo, hi)) => {
            json!({"network_attributable_share": {"value": v, "ci_low": lo, "ci_high": hi}})
        }
        None => {
            json!({"network_attributable_share": {"value": null, "ci_low": null, "ci_high": null}})
        }
    };
    json!({"key": format!("rtt={a},loss={b}"), "params": {"rtt": a, "loss": b}, "effects": effects})
}

fn verdict(cells: Vec<Value>) -> Value {
    json!({
        "format": "acn-bench/verdict/v1",
        "labels": ["exploratory", "mock-gated"],
        "slices": [{"key": "all", "labels": ["sim-only"], "cells": cells}]
    })
}

/// Cites: ATR-41, CON-8
#[test]
fn heatmap_draws_each_cell_with_its_interval_and_the_verdicts_labels() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let mut cells = vec![
        cell("10", "0", Some((0.05, 0.01, 0.09))),
        cell("50", "0", Some((0.25, 0.2, 0.3))),
        cell("10", "1", Some((-0.02, -0.04, 0.0))),
        cell("50", "1", None),
    ];
    // A real verdict's cells also carry the slice's constant parameters.
    for c in &mut cells {
        c["params"]["provider"] = "mockllm".into();
    }
    let v = verdict(cells);
    // A value whose shortest text a lossy float parse would change in its last
    // digit: the cell shows the verdict's own text.
    let text = v
        .to_string()
        .replace("\"value\":0.25", "\"value\":-0.22343495120599086");
    assert!(text.contains("-0.22343495120599086"));
    std::fs::write(d.join("verdict.json"), text).unwrap();
    let args = |out: &str| {
        vec![
            "attrib",
            "heatmap",
            "--verdict",
            "verdict.json",
            "--slice",
            "all",
            "--quantity",
            "network_attributable_share",
            "--x",
            "rtt",
            "--y",
            "loss",
            "--out",
        ]
        .into_iter()
        .map(str::to_owned)
        .chain([out.to_owned()])
        .collect::<Vec<_>>()
    };
    let run = |out: &str| {
        let a = args(out);
        acn(d, &a.iter().map(String::as_str).collect::<Vec<_>>())
    };
    let (code, j) = run("h1.svg");
    assert_eq!(code, Some(0), "{j}");
    assert_eq!(j["cells"], 4);
    let svg = std::fs::read_to_string(d.join("h1.svg")).unwrap();
    assert!(svg.starts_with("<svg"), "{svg}");
    // The title carries every label, so an exploratory mock result never looks citable.
    for label in ["exploratory", "mock-gated", "sim-only"] {
        assert!(svg.contains(label), "{label}");
    }
    // Values and intervals as CON-27(c) writes them; the empty cell is marked.
    for t in [
        "0.05",
        "[0.01, 0.09]",
        "-0.22343495120599086",
        "[0.2, 0.3]",
        "-0.02",
        "[-0.04, 0.0]",
        "no value",
    ] {
        assert!(svg.contains(t), "{t}");
    }
    // The same verdict and arguments give the same bytes.
    let (code, _) = run("h2.svg");
    assert_eq!(code, Some(0));
    assert_eq!(std::fs::read(d.join("h2.svg")).unwrap(), svg.as_bytes());
    assert_eq!(
        j["blake3"].as_str().unwrap(),
        blake3::hash(svg.as_bytes()).to_hex().as_str()
    );
}

/// Cites: ATR-41, CON-8
#[test]
fn heatmap_refuses_what_it_cannot_draw_faithfully() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::write(
        d.join("v.json"),
        verdict(vec![cell("10", "0", Some((0.1, 0.0, 0.2)))]).to_string(),
    )
    .unwrap();
    // `mode` varies across the slice, beside rtt and loss.
    let mut a = cell("10", "0", Some((0.1, 0.0, 0.2)));
    a["params"]["mode"] = "slow".into();
    let mut b = cell("50", "0", Some((0.1, 0.0, 0.2)));
    b["params"]["mode"] = "fast".into();
    std::fs::write(d.join("v4.json"), verdict(vec![a, b]).to_string()).unwrap();
    let dup = verdict(vec![
        cell("10", "0", Some((0.1, 0.0, 0.2))),
        cell("10", "0", Some((0.2, 0.0, 0.3))),
    ]);
    std::fs::write(d.join("dup.json"), dup.to_string()).unwrap();
    std::fs::write(d.join("bare.json"), json!({"slices": []}).to_string()).unwrap();
    for (file, slice, q, x, needle) in [
        ("v.json", "all", "ttft_p99_ms", "rtt", "records no effect"),
        (
            "v.json",
            "nope",
            "network_attributable_share",
            "rtt",
            "no slice",
        ),
        (
            "v4.json",
            "all",
            "network_attributable_share",
            "rtt",
            "varies `mode`",
        ),
        (
            "dup.json",
            "all",
            "network_attributable_share",
            "rtt",
            "two cells share",
        ),
        (
            "v.json",
            "all",
            "network_attributable_share",
            "loss",
            "the same parameter",
        ),
        (
            "bare.json",
            "all",
            "network_attributable_share",
            "rtt",
            "not a verdict.json",
        ),
    ] {
        let (code, j) = acn(
            d,
            &[
                "attrib",
                "heatmap",
                "--verdict",
                file,
                "--slice",
                slice,
                "--quantity",
                q,
                "--x",
                x,
                "--y",
                "loss",
                "--out",
                "x.svg",
            ],
        );
        assert_eq!((code, &j["ok"]), (Some(1), &json!(false)), "{j}");
        assert!(j["error"].as_str().unwrap().contains(needle), "{j}");
    }
    assert!(!d.join("x.svg").exists());
}
