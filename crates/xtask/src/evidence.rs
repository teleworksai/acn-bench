//! Evidence pages (SPEC 085 LOOP-30, SPEC 140 P16-20, P16-21): one page per
//! hypothesis under `docs/evidence/`, rendered from the loop reports, twins and
//! verdicts committed under `docs/runs/`. Numbers are written as the committed
//! JSON writes them, never re-formatted; ids are checked against what they
//! name; free text is escaped, so a committed file cannot write the page.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use acn_trace::identity::Digest;
use serde_json::Value;
use serde_json::value::RawValue;

use crate::workspace::read;
use crate::{Error, Result};

/// Where the committed copies live (P16-20).
pub const RUNS_DIR: &str = "docs/runs";
/// Where the pages go (LOOP-30).
pub const EVIDENCE_DIR: &str = "docs/evidence";

/// Labels that keep a verdict from being cited (HYP-23). `sim-only` and
/// `partially-twinned` disqualify only when the twin is required, which the
/// verdict does not record, so they are counted here too: a page never shows
/// as citable what might not be.
const NOT_CITABLE: [&str; 5] = [
    "exploratory",
    "mock-gated",
    "unpinned-inputs",
    "sim-only",
    "partially-twinned",
];
const VERDICTS: [&str; 3] = ["pass", "fail", "inconclusive"];

fn invalid<T>(m: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(m.into()))
}

/// A control effect, its numbers as written (LOOP-11).
#[derive(serde::Deserialize)]
struct ControlEffect {
    slice: String,
    cell: String,
    quantity: String,
    effect: Box<RawValue>,
    ci_low: Box<RawValue>,
    ci_high: Box<RawValue>,
    treatment_replicates: Box<RawValue>,
    control_replicates: Box<RawValue>,
}

#[derive(serde::Deserialize)]
struct Effects {
    control_effect: Vec<ControlEffect>,
}

/// One twin's divergence entry: a slice, and per cell its quantities' figures.
#[derive(serde::Deserialize)]
struct Divergence {
    slice: String,
    cells: Vec<DivergenceCell>,
}

#[derive(serde::Deserialize)]
struct DivergenceCell {
    key: String,
    quantities: BTreeMap<String, Box<RawValue>>,
}

#[derive(serde::Deserialize)]
struct TwinDivergence {
    divergence: Vec<Divergence>,
}

struct Twin {
    id: String,
    json: Value,
    divergence: Vec<Divergence>,
}

struct Loop {
    id: String,
    report: Value,
    effects: Vec<ControlEffect>,
    twins: Vec<Twin>,
}

fn json(path: &Path) -> Result<(String, Value)> {
    let text = read(path)?;
    let v = serde_json::from_str(&text)
        .map_err(|e| Error::Invalid(format!("{}: {e}", path.display())))?;
    Ok((text, v))
}

fn s<'v>(v: &'v Value, k: &str) -> &'v str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn is_hex64(t: &str) -> bool {
    t.len() == 64
        && t.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A field that must be a 64-hex id or hash.
fn hex<'v>(v: &'v Value, k: &str, file: &Path) -> Result<&'v str> {
    let t = s(v, k);
    if is_hex64(t) {
        Ok(t)
    } else {
        invalid(format!("{}: `{k}` is not 64 lowercase hex", file.display()))
    }
}

/// A field that must be a word of lowercase letters, digits, `_` and `-`.
fn word<'v>(v: &'v Value, k: &str, file: &Path) -> Result<&'v str> {
    let t = s(v, k);
    if !t.is_empty()
        && t.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        Ok(t)
    } else {
        invalid(format!("{}: `{k}` is not a plain word", file.display()))
    }
}

/// Free text in a table cell: Markdown's active characters escaped and line
/// breaks flattened, so committed text can only ever be text.
fn esc(t: &str) -> String {
    let mut o = String::with_capacity(t.len());
    for c in t.chars() {
        match c {
            '\\' | '|' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' => {
                o.push('\\');
                o.push(c);
            }
            '\n' | '\r' | '\t' => o.push(' '),
            c if c.is_control() => {}
            c => o.push(c),
        }
    }
    if o.is_empty() { "(none)".to_owned() } else { o }
}

/// Raw JSON with the whitespace outside strings removed: the committed text,
/// compact, every digit as written.
fn compact(raw: &str) -> String {
    let mut o = String::with_capacity(raw.len());
    let (mut in_str, mut escaped) = (false, false);
    for c in raw.chars() {
        if in_str {
            o.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            o.push(c);
        } else if !c.is_whitespace() {
            o.push(c);
        }
    }
    o
}

fn dirs(p: &Path) -> Result<Vec<PathBuf>> {
    if !p.is_dir() {
        return Ok(Vec::new());
    }
    let mut out: Vec<PathBuf> = std::fs::read_dir(p)
        .map_err(|e| Error::io(p, e))?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    Ok(out)
}

fn name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A verdict's id recomputed from its hypothesis hash and bundles (HYP-15).
fn recomputed(v: &Value, file: &Path) -> Result<String> {
    let bad = |e: &dyn std::fmt::Display| Error::Invalid(format!("{}: {e}", file.display()));
    let h = v.get("hypothesis").cloned().unwrap_or(Value::Null);
    let hash = Digest::from_hex(hex(&h, "hash", file)?).map_err(|e| bad(&e))?;
    let mut bundles = Vec::new();
    for b in v
        .get("bundles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        bundles.push((
            Digest::from_hex(hex(b, "run_id", file)?).map_err(|e| bad(&e))?,
            Digest::from_hex(hex(b, "bundle_digest", file)?).map_err(|e| bad(&e))?,
        ));
    }
    Ok(acn_hyp::verdict::verdict_id(&hash, &bundles)
        .map_err(|e| bad(&e))?
        .to_hex())
}

/// The committed loops, twins and verdicts, checked against each other.
fn load(root: &Path) -> Result<(Vec<Loop>, BTreeMap<String, Value>)> {
    let runs = root.join(RUNS_DIR);
    let mut verdicts = BTreeMap::new();
    for d in dirs(&runs.join("verdicts"))? {
        let file = d.join("verdict.json");
        let (_, v) = json(&file)?;
        let id = hex(&v, "verdict_id", &file)?.to_owned();
        if id != name(&d) {
            return invalid(format!(
                "{}: its verdict_id is not its directory's name (P16-20)",
                d.display()
            ));
        }
        if recomputed(&v, &file)? != id {
            return invalid(format!(
                "{}: its verdict_id does not recompute from its hypothesis and bundles (HYP-15)",
                file.display()
            ));
        }
        if !VERDICTS.contains(&s(&v, "verdict")) {
            return invalid(format!("{}: `verdict` is not a verdict", file.display()));
        }
        verdicts.insert(id, v);
    }
    let mut named: BTreeSet<String> = BTreeSet::new();
    let mut loops = Vec::new();
    for d in dirs(&runs.join("loop"))? {
        let file = d.join("report.json");
        let (text, report) = json(&file)?;
        if hex(&report, "loop_id", &file)? != name(&d) {
            return invalid(format!(
                "{}: its loop_id is not its directory's name (P16-20)",
                d.display()
            ));
        }
        for k in ["engine_hash", "build_hash", "verdict_id"] {
            hex(&report, k, &file)?;
        }
        word(&report, "stop", &file)?;
        if !VERDICTS.contains(&s(&report, "verdict")) {
            return invalid(format!("{}: `verdict` is not a verdict", file.display()));
        }
        if !s(&report, "seed").bytes().all(|b| b.is_ascii_digit()) {
            return invalid(format!("{}: `seed` is not a decimal", file.display()));
        }
        let h = report.get("hypothesis").cloned().unwrap_or(Value::Null);
        hex(&h, "hash", &file)?;
        word(&h, "status", &file)?;
        for b in report
            .get("bundles")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            hex(b, "run_id", &file)?;
        }
        let effects: Effects = serde_json::from_str(&text)
            .map_err(|e| Error::Invalid(format!("{}: {e}", file.display())))?;
        // Each verdict this loop names: committed, and of this hypothesis.
        let need = |id: &str, what: &str| -> Result<()> {
            let Some(v) = verdicts.get(id) else {
                return invalid(format!(
                    "{}: {what} {id} is not committed under {RUNS_DIR}/verdicts/ (P16-20)",
                    d.display()
                ));
            };
            let vh = v.get("hypothesis").cloned().unwrap_or(Value::Null);
            if s(&vh, "id") != s(&h, "id") || s(&vh, "hash") != s(&h, "hash") {
                return invalid(format!(
                    "{}: {what} {id} is another hypothesis's (P16-20)",
                    d.display()
                ));
            }
            Ok(())
        };
        let l1 = s(&report, "verdict_id").to_owned();
        need(&l1, "its final verdict")?;
        named.insert(l1.clone());
        let mut twins = Vec::new();
        for t in dirs(&d.join("twin"))? {
            let tf = t.join("twin.json");
            let (text, twin) = json(&tf)?;
            if hex(&twin, "verdict_id", &tf)? != name(&t) || s(&twin, "loop_id") != name(&d) {
                return invalid(format!(
                    "{}: its verdict_id or loop_id is not its path's (P16-20)",
                    t.display()
                ));
            }
            if hex(&twin, "l1_verdict_id", &tf)? != l1 {
                return invalid(format!(
                    "{}: it twins verdict {}, not its loop's final verdict {l1} (LOOP-12)",
                    t.display(),
                    s(&twin, "l1_verdict_id")
                ));
            }
            need(s(&twin, "verdict_id"), "its twin's L2 verdict")?;
            named.insert(s(&twin, "verdict_id").to_owned());
            for c in twin
                .get("cells")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                for arm in c
                    .get("arms")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|o| o.values())
                {
                    hex(arm, "run_id", &tf)?;
                    hex(arm, "derived_from", &tf)?;
                }
            }
            let div: TwinDivergence = serde_json::from_str(&text)
                .map_err(|e| Error::Invalid(format!("{}: {e}", tf.display())))?;
            twins.push(Twin {
                id: name(&t),
                json: twin,
                divergence: div.divergence,
            });
        }
        loops.push(Loop {
            id: name(&d),
            report,
            effects: effects.control_effect,
            twins,
        });
    }
    if let Some(orphan) = verdicts.keys().find(|k| !named.contains(*k)) {
        return invalid(format!(
            "{RUNS_DIR}/verdicts/{orphan} is named by no committed report or twin (P16-20)"
        ));
    }
    Ok((loops, verdicts))
}

fn labels(v: &Value) -> Vec<String> {
    v.get("labels")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(esc).collect())
        .unwrap_or_default()
}

fn citable(ls: &[String]) -> String {
    let against: Vec<&str> = ls
        .iter()
        .map(String::as_str)
        .filter(|l| NOT_CITABLE.contains(l))
        .collect();
    if against.is_empty() {
        "yes".to_owned()
    } else {
        format!("no ({})", against.join(", "))
    }
}

fn list(ls: &[String]) -> String {
    if ls.is_empty() {
        "none".to_owned()
    } else {
        ls.join(", ")
    }
}

/// The reasons of a verdict or twin, as their ids.
fn reasons(v: &Value) -> String {
    let rs: Vec<String> = v
        .get("reasons")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|r| esc(s(r, "reason"))).collect())
        .unwrap_or_default();
    list(&rs)
}

/// Every quantity name a loop's page shows.
fn quantities(l: &Loop) -> Vec<String> {
    let mut q: Vec<String> = l.effects.iter().map(|e| e.quantity.clone()).collect();
    for t in &l.twins {
        for d in &t.divergence {
            for c in &d.cells {
                q.extend(c.quantities.keys().cloned());
            }
        }
    }
    q.sort();
    q.dedup();
    q
}

fn verdict_row(p: &mut String, layer: &str, v: &Value) {
    let ls = labels(v);
    let _ = writeln!(
        p,
        "| {layer} | {} | `{}` | {} | {} | {} |",
        s(v, "verdict"),
        s(v, "verdict_id"),
        list(&ls),
        citable(&ls),
        reasons(v)
    );
}

fn page(id: &str, loops: &[&Loop], verdicts: &BTreeMap<String, Value>) -> String {
    let mut p = String::new();
    let _ = writeln!(p, "# Evidence: {}\n", esc(id));
    let _ = writeln!(
        p,
        "Rendered by `cargo xtask docs-inventory` from the loop reports, twins and verdicts committed under `{RUNS_DIR}/` (SPEC 085 LOOP-30, SPEC 140 P16-21). This page is the citation target for this hypothesis. A verdict marked not citable is shown, never cited (HYP-23). Loops and twins are listed in the order of their ids; none is marked current.\n"
    );
    let attribution = loops
        .iter()
        .flat_map(|l| quantities(l))
        .any(|q| acn_hyp::quantities::attribution(&q).is_some());
    if attribution {
        let _ = writeln!(
            p,
            "Network time on this page is time on the emulated network of each run's scenario, not a real provider's WAN (SPEC 090 ATR-42).\n"
        );
    }
    for l in loops {
        let r = &l.report;
        let h = r.get("hypothesis").cloned().unwrap_or(Value::Null);
        let _ = writeln!(p, "## Loop `{}`\n", l.id);
        let _ = writeln!(p, "| Item | Value |\n|---|---|");
        let _ = writeln!(
            p,
            "| Hypothesis | {} ({}), `{}` |",
            esc(s(&h, "id")),
            s(&h, "status"),
            s(&h, "hash")
        );
        let _ = writeln!(p, "| Seed | `{}` |", s(r, "seed"));
        let _ = writeln!(p, "| engine_hash | `{}` |", s(r, "engine_hash"));
        let _ = writeln!(p, "| build_hash | `{}` |", s(r, "build_hash"));
        let _ = writeln!(p, "| Stopped | {} |\n", s(r, "stop"));
        let _ = writeln!(p, "### Verdicts\n");
        let _ = writeln!(
            p,
            "| Layer | Verdict | verdict_id | Labels | Citable | Reasons |\n|---|---|---|---|---|---|"
        );
        if let Some(v) = verdicts.get(s(r, "verdict_id")) {
            verdict_row(&mut p, "L1", v);
        }
        for t in &l.twins {
            if let Some(v) = verdicts.get(&t.id) {
                verdict_row(&mut p, "L2", v);
            }
        }
        let _ = writeln!(p, "| L3 | none yet | | | | |\n");
        let _ = writeln!(p, "### Control effects (CON-18)\n");
        if l.effects.is_empty() {
            let _ = writeln!(p, "None recorded.\n");
        } else {
            let _ = writeln!(
                p,
                "| Slice | Cell | Quantity | Effect | Interval | Replicates (treatment / control) |\n|---|---|---|---|---|---|"
            );
            for e in &l.effects {
                let _ = writeln!(
                    p,
                    "| {} | {} | {} | {} | [{}, {}] | {} / {} |",
                    esc(&e.slice),
                    esc(&e.cell),
                    esc(&e.quantity),
                    esc(&compact(e.effect.get())),
                    esc(&compact(e.ci_low.get())),
                    esc(&compact(e.ci_high.get())),
                    esc(&compact(e.treatment_replicates.get())),
                    esc(&compact(e.control_replicates.get()))
                );
            }
            let _ = writeln!(p);
        }
        let _ = writeln!(p, "### Divergence, sim against live (LOOP-12)\n");
        let mut rows = Vec::new();
        for t in &l.twins {
            for d in &t.divergence {
                for c in &d.cells {
                    for (q, fig) in &c.quantities {
                        rows.push(format!(
                            "| `{}` | {} | {} | {} | {} |",
                            &t.id[..12],
                            esc(&d.slice),
                            esc(&c.key),
                            esc(q),
                            esc(&compact(fig.get()))
                        ));
                    }
                }
            }
        }
        if rows.is_empty() {
            let _ = writeln!(
                p,
                "None recorded: {}.\n",
                if l.twins.is_empty() {
                    "no twin is committed"
                } else {
                    "the twin compared no quantity with a tolerance"
                }
            );
        } else {
            let _ = writeln!(
                p,
                "| Twin | Slice | Cell | Quantity | Figures |\n|---|---|---|---|---|"
            );
            for row in rows {
                let _ = writeln!(p, "{row}");
            }
            let _ = writeln!(p);
        }
        let _ = writeln!(p, "### Providers (HYP-24)\n");
        match verdicts
            .get(s(r, "verdict_id"))
            .and_then(|v| v.get("providers"))
            .and_then(Value::as_object)
        {
            Some(ps) if !ps.is_empty() => {
                let _ = writeln!(p, "| Provider | Status |\n|---|---|");
                for (k, v) in ps {
                    let status = v.as_str().map_or_else(|| v.to_string(), str::to_owned);
                    let _ = writeln!(p, "| {} | {} |", esc(k), esc(&status));
                }
                let _ = writeln!(p);
            }
            _ => {
                let _ = writeln!(p, "The hypothesis has no `provider` parameter.\n");
            }
        }
        let _ = writeln!(p, "### Chain\n");
        let _ = writeln!(p, "- L1 verdict: `{}`", s(r, "verdict_id"));
        let ids: Vec<&str> = r
            .get("bundles")
            .and_then(Value::as_array)
            .map(|b| b.iter().map(|x| s(x, "run_id")).collect())
            .unwrap_or_default();
        let _ = writeln!(p, "- L1 bundles ({}):\n", ids.len());
        let _ = writeln!(p, "```");
        for id in ids {
            let _ = writeln!(p, "{id}");
        }
        let _ = writeln!(p, "```\n");
        for t in &l.twins {
            let label = s(&t.json, "twin_label");
            let _ = writeln!(
                p,
                "- L2 verdict: `{}`, the twin of L1 verdict `{}`; its reasons: {}{}",
                t.id,
                s(&t.json, "l1_verdict_id"),
                reasons(&t.json),
                if label.is_empty() {
                    String::new()
                } else {
                    format!("; label: {}", esc(label))
                }
            );
            let _ = writeln!(p, "- L2 bundles, each beside the L1 bundle it twins:\n");
            let _ = writeln!(
                p,
                "| Cell | Arm | Live run_id | Twin of |\n|---|---|---|---|"
            );
            for c in t
                .json
                .get("cells")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let key = esc(s(c, "key"));
                for (arm, a) in c
                    .get("arms")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                {
                    let _ = writeln!(
                        p,
                        "| {key} | {} | `{}` | `{}` |",
                        esc(arm),
                        s(a, "run_id"),
                        s(a, "derived_from")
                    );
                }
            }
            let _ = writeln!(p);
        }
    }
    p.trim_end().to_owned() + "\n"
}

/// Every page, keyed by its path under the root (P16-21).
pub fn pages(root: &Path) -> Result<BTreeMap<String, String>> {
    let (loops, verdicts) = load(root)?;
    let mut by: BTreeMap<String, Vec<&Loop>> = BTreeMap::new();
    for l in &loops {
        let id = l
            .report
            .get("hypothesis")
            .and_then(|h| h.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if id.is_empty()
            || !id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        {
            return invalid(format!(
                "loop {}: hypothesis id `{id}` cannot name a page (lowercase letters, digits, `-` and `_`)",
                l.id
            ));
        }
        by.entry(id).or_default().push(l);
    }
    Ok(by
        .into_iter()
        .map(|(id, ls)| (format!("{EVIDENCE_DIR}/{id}.md"), page(&id, &ls, &verdicts)))
        .collect())
}
