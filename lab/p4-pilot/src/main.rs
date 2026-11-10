//! What a real-provider POC 4 would spend, read from a loop's mock bundles.
//!
//! For every bundle a loop report names, sum the tokens of its calls by kind
//! (uncached input, cache read, cache write, output), group the bundles by
//! provider slice, and price each group with a price sheet. Two totals per
//! provider: the full grid at the design's replicates, and a pilot of the
//! control and the cells one knob away from it, at fewer replicates.
//!
//! The counts come from the mock's model of each provider's caching and from
//! its reply lengths, so every figure is an estimate of an estimate. Nothing
//! here calls a provider.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use arrow_array::{Array, Int64Array};
use clap::Parser;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::Deserialize;

#[derive(Parser)]
struct Args {
    /// A loop's `report.json`.
    #[arg(long)]
    report: PathBuf,
    /// The runs directory that holds the loop's bundles.
    #[arg(long, default_value = "runs")]
    runs: PathBuf,
    /// The price sheet (TOML): per provider, USD per million tokens.
    #[arg(long)]
    prices: PathBuf,
    /// Replicates of each pilot cell.
    #[arg(long, default_value_t = 5)]
    pilot_replicates: u64,
}

#[derive(Deserialize)]
struct Report {
    bundles: Vec<BundleRef>,
}

#[derive(Deserialize)]
struct BundleRef {
    run_id: String,
}

#[derive(Deserialize)]
struct Manifest {
    model: String,
    params: BTreeMap<String, String>,
}

/// USD per million tokens. A provider with no write premium states the input
/// price for `cache_write`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Price {
    model: String,
    source: String,
    as_of: String,
    input: f64,
    cache_read: f64,
    cache_write: f64,
    output: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sheet {
    provider: BTreeMap<String, Price>,
}

#[derive(Clone, Copy, Default)]
struct Tokens {
    calls: u64,
    /// Prompt tokens neither read from nor written to the cache.
    uncached: u64,
    read: u64,
    write: u64,
    output: u64,
}

impl Tokens {
    fn add(&mut self, o: &Tokens) {
        self.calls += o.calls;
        self.uncached += o.uncached;
        self.read += o.read;
        self.write += o.write;
        self.output += o.output;
    }

    fn scaled(&self, num: u64, den: u64) -> Tokens {
        let s = |x: u64| x * num / den;
        Tokens {
            calls: s(self.calls),
            uncached: s(self.uncached),
            read: s(self.read),
            write: s(self.write),
            output: s(self.output),
        }
    }

    fn usd(&self, p: &Price) -> f64 {
        (self.uncached as f64 * p.input
            + self.read as f64 * p.cache_read
            + self.write as f64 * p.cache_write
            + self.output as f64 * p.output)
            / 1e6
    }
}

fn column(batch: &arrow_array::RecordBatch, name: &str) -> Result<Vec<i64>> {
    let col = batch
        .column_by_name(name)
        .with_context(|| format!("no column `{name}`"))?;
    let ints = col
        .as_any()
        .downcast_ref::<Int64Array>()
        .with_context(|| format!("`{name}` is not int64"))?;
    // A call with no usage reported counts as zero; the mock always reports.
    Ok((0..ints.len())
        .map(|i| if ints.is_null(i) { 0 } else { ints.value(i) })
        .collect())
}

fn tokens_of(bundle: &Path) -> Result<Tokens> {
    let file = File::open(bundle.join("views/call.parquet"))
        .with_context(|| format!("{}", bundle.display()))?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)?.build()?;
    let mut t = Tokens::default();
    for batch in reader {
        let batch = batch?;
        let input = column(&batch, "input_tokens")?;
        let read = column(&batch, "cache_read_tokens")?;
        let write = column(&batch, "cache_write_tokens")?;
        let output = column(&batch, "output_tokens")?;
        for i in 0..batch.num_rows() {
            let cached = read[i] + write[i];
            if cached > input[i] {
                bail!("{}: a call caches more than its prompt", bundle.display());
            }
            t.calls += 1;
            t.uncached += (input[i] - cached) as u64;
            t.read += read[i] as u64;
            t.write += write[i] as u64;
            t.output += output[i] as u64;
        }
    }
    Ok(t)
}

struct Bundle {
    provider: String,
    workload: String,
    model: String,
    control: bool,
    replicates: u64,
    /// The knob settings: every `vary.` parameter but provider and workload.
    knobs: BTreeMap<String, String>,
    tokens: Tokens,
}

fn load(runs: &Path, run_id: &str) -> Result<Bundle> {
    let dir = runs.join(run_id);
    let manifest: Manifest = serde_json::from_reader(
        File::open(dir.join("manifest.json")).with_context(|| format!("{}", dir.display()))?,
    )?;
    let p = &manifest.params;
    let get = |k: &str| {
        p.get(k)
            .cloned()
            .with_context(|| format!("{run_id}: no `{k}`"))
    };
    let knobs = p
        .iter()
        .filter_map(|(k, v)| {
            let knob = k.strip_prefix("vary.")?;
            (knob != "provider" && knob != "workload").then(|| (knob.to_owned(), v.clone()))
        })
        .collect();
    Ok(Bundle {
        provider: get("vary.provider")?,
        workload: get("vary.workload")?,
        model: manifest.model,
        control: get("arms")? == "control",
        replicates: get("replicates")?.parse()?,
        knobs,
        tokens: tokens_of(&dir)?,
    })
}

fn row(label: &str, t: &Tokens, price: Option<&Price>) {
    let usd = price.map_or("-".to_owned(), |p| format!("{:.2}", t.usd(p)));
    println!(
        "| {label} | {} | {} | {} | {} | {} | {usd} |",
        t.calls, t.uncached, t.read, t.write, t.output
    );
}

fn main() -> Result<()> {
    let args = Args::parse();
    let report: Report = serde_json::from_reader(File::open(&args.report)?)?;
    let sheet: Sheet = toml::from_str(&std::fs::read_to_string(&args.prices)?)?;
    let mut bundles = Vec::new();
    for b in &report.bundles {
        bundles.push(load(&args.runs, &b.run_id)?);
    }

    let mut providers: Vec<&str> = bundles.iter().map(|b| b.provider.as_str()).collect();
    providers.sort_unstable();
    providers.dedup();

    println!(
        "{} bundles; pilot at {} replicates of the control and of every cell one knob from it\n",
        bundles.len(),
        args.pilot_replicates
    );
    for provider in providers {
        let of: Vec<&Bundle> = bundles.iter().filter(|b| b.provider == provider).collect();
        let price = sheet.provider.get(provider);
        let mock = of.first().map_or("", |b| b.model.as_str());
        match price {
            Some(p) => println!(
                "### {provider} (mock profile `{mock}`; priced as `{}`, {} as of {})\n",
                p.model, p.source, p.as_of
            ),
            None => println!("### {provider} (mock profile `{mock}`; no price given)\n"),
        }
        println!("| what | calls | uncached in | cache read | cache write | output | USD |");
        println!("|---|---|---|---|---|---|---|");
        let mut grid = Tokens::default();
        let mut pilot = Tokens::default();
        let mut pilot_cells = 0;
        let mut workloads: Vec<&str> = of.iter().map(|b| b.workload.as_str()).collect();
        workloads.sort_unstable();
        workloads.dedup();
        for workload in workloads {
            let here: Vec<&&Bundle> = of.iter().filter(|b| b.workload == workload).collect();
            let Some(control) = here.iter().find(|b| b.control) else {
                bail!("{provider}/{workload}: no control bundle");
            };
            let mut w = Tokens::default();
            for b in &here {
                w.add(&b.tokens);
                let away = b
                    .knobs
                    .iter()
                    .filter(|(k, v)| control.knobs.get(*k) != Some(*v))
                    .count();
                if b.control || away == 1 {
                    pilot.add(&b.tokens.scaled(args.pilot_replicates, b.replicates));
                    pilot_cells += 1;
                }
            }
            row(
                &format!("grid, {workload} ({} bundles)", here.len()),
                &w,
                price,
            );
            grid.add(&w);
        }
        row(
            &format!("**grid, all ({} bundles)**", of.len()),
            &grid,
            price,
        );
        row(&format!("**pilot ({pilot_cells} bundles)**"), &pilot, price);
        println!();
    }
    Ok(())
}
