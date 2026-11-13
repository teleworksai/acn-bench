//! lab/p1a-atlas — POC 1a by hand (CON-23): a 3×3 grid of inference placement
//! and link profile, one scenario file per cell, run through the substrate's
//! own `acn harness run` and judged by `acn hyp verdict`.
//!
//! `acn loop run` cannot do this yet: it runs with no scenario (ADR-34). So
//! this driver makes the bundles one by one. Every number is exploratory.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde_json::Value;

const PLACEMENTS: [&str; 3] = ["edge", "regional", "remote"];
const PROFILES: [&str; 3] = ["wired", "wifi", "cellular"];

#[derive(Parser)]
struct Args {
    /// The substrate's `acn` binary.
    #[arg(long, default_value = "target/debug/acn")]
    acn: PathBuf,
    /// The repository root; paths below are relative to it.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    #[arg(long, default_value = "lab/hypotheses/p1a.toml")]
    hypothesis: PathBuf,
    /// Where bundles and the verdict go. Not `runs/`: these are lab bundles.
    #[arg(long)]
    runs_dir: PathBuf,
    /// `sim`, `live` or both, comma-separated. Live serves the mock per replicate.
    #[arg(long, default_value = "sim", value_delimiter = ',')]
    modes: Vec<String>,
    /// Replicates a bundle. The verdict reads indices `0..[design].replicates`
    /// of the hypothesis file only, so raise both together.
    #[arg(long, default_value_t = 4)]
    replicates: u32,
    /// Draw the verdict's effect (agent minus stand-in control) as an SVG here.
    #[arg(long)]
    heatmap: Option<PathBuf>,
    /// The mock profile.
    #[arg(long, default_value = "mock-auto")]
    model: String,
}

/// Run `acn` and return the one JSON object it prints (CON-8).
fn acn(args: &Args, argv: &[String]) -> Result<Value> {
    let out = Command::new(&args.acn)
        .current_dir(&args.root)
        .args(argv)
        .output()
        .with_context(|| format!("running {}", args.acn.display()))?;
    let v: Value = serde_json::from_slice(&out.stdout).with_context(|| {
        format!(
            "acn {argv:?} printed no JSON object; stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })?;
    if v["ok"] != Value::Bool(true) {
        bail!("acn {argv:?} refused: {v}");
    }
    Ok(v)
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

fn main() -> Result<()> {
    let args = Args::parse();
    let started = std::time::Instant::now();
    let mut bundles = Vec::new();
    for mode in &args.modes {
        for placement in PLACEMENTS {
            for profile in PROFILES {
                for (arm, workload) in [("treatment", "agent"), ("control", "rpc")] {
                    let mut argv: Vec<String> = [
                        "harness",
                        "run",
                        "--backend",
                        "mockllm",
                        "--mode",
                        mode,
                        "--arm",
                        arm,
                    ]
                    .map(str::to_owned)
                    .into();
                    argv.extend([
                        "--model".into(),
                        args.model.clone(),
                        "--workload".into(),
                        format!("lab/p1a-atlas/workloads/{workload}.toml"),
                        "--scenario".into(),
                        format!("lab/p1a-atlas/scenarios/{placement}-{profile}.toml"),
                        "--hypothesis".into(),
                        s(&args.hypothesis),
                        "--vary".into(),
                        format!("placement={placement}"),
                        "--vary".into(),
                        format!("profile={profile}"),
                        "--replicates".into(),
                        args.replicates.to_string(),
                        "--runs-dir".into(),
                        s(&args.runs_dir),
                    ]);
                    if mode == "live" {
                        argv.extend(["--endpoint".into(), "acn-mock://loopback".into()]);
                    }
                    let t = std::time::Instant::now();
                    let v = acn(&args, &argv)?;
                    let dir = v["dir"].as_str().context("no bundle dir")?.to_owned();
                    eprintln!(
                        "{mode} {placement}-{profile} {arm}: {:.1}s",
                        t.elapsed().as_secs_f64()
                    );
                    bundles.push(dir);
                }
            }
        }
    }
    let mut argv = vec![
        "hyp".to_owned(),
        "verdict".into(),
        "--hypothesis".into(),
        s(&args.hypothesis),
        "--runs-dir".into(),
        s(&args.runs_dir),
    ];
    argv.extend(bundles);
    let v = acn(&args, &argv)?;
    let verdict = &v["verdict"];
    let slice = &verdict["slices"][0];
    let value = |arm: &Value, q: &str| arm["values"][q].clone();
    let rows: Vec<Value> = slice["cells"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let control = slice["controls"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|k| k["key"] == c["control"])
                .map_or(Value::Null, |k| k["arm"].clone());
            let t = &c["treatment"];
            serde_json::json!({
                "cell": c["key"],
                "agent": {
                    "network": value(t, "network_attributable_share"),
                    "retry": value(t, "retry_share"),
                    "model": value(t, "model_share"),
                    "ttft_p50_ms": value(t, "ttft_p50_ms"),
                },
                "control": {
                    "network": value(&control, "network_attributable_share"),
                    "retry": value(&control, "retry_share"),
                },
                "twinned": c["twinned"],
                "divergence": c["divergence"],
            })
        })
        .collect();
    if let Some(out) = &args.heatmap {
        let argv = [
            "attrib",
            "heatmap",
            "--verdict",
            v["verdict_path"].as_str().context("no verdict path")?,
            "--slice",
            slice["key"].as_str().unwrap_or(""),
            "--quantity",
            "network_attributable_share",
            "--x",
            "placement",
            "--y",
            "profile",
            "--out",
            &s(out),
        ]
        .map(str::to_owned);
        acn(&args, &argv)?;
    }
    println!(
        "{}",
        serde_json::json!({
            "ok": true,
            "verdict_path": v["verdict_path"],
            "verdict": verdict["verdict"],
            "labels": verdict["labels"],
            "reasons": slice["reasons"],
            "decision_cells": slice["decision_cells"],
            "cells": rows,
            "seconds": started.elapsed().as_secs_f64(),
        })
    );
    Ok(())
}
