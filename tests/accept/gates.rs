//! Milestone gate records (SPEC 095 §1): every `docs/gates/M<g>.md` lists
//! exactly its gate's criteria, each `met` with files that exist or `deferred`
//! under an ADR to a later gate, and carries earlier deferrals forward. The
//! checker runs on the real records, and on fixtures that each break one rule
//! and must produce exactly one problem. A second test checks what a test can
//! see of M0's criteria themselves.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const SPEC: &str = "specs/095-gates.md";
const RECORDS: &str = "docs/gates";
/// The last gate SPEC 095 plans (M0 to M4).
const LAST_GATE: u32 = 4;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// `w` as a decimal number with no sign and no leading zero.
fn canonical_number(w: &str) -> Option<u32> {
    let ok =
        !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit()) && (w == "0" || !w.starts_with('0'));
    if ok { w.parse().ok() } else { None }
}

/// The criteria of each gate whose section SPEC 095 writes: the
/// `**GATE-n**` paragraphs under a `## <k>. Gate M<g>` heading. Problems with
/// the spec itself (a section with no criteria, an ID out of its band) are
/// pushed to `problems`.
fn criteria(spec: &str, problems: &mut Vec<String>) -> BTreeMap<u32, Vec<String>> {
    let mut out: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    let mut gate = None;
    for line in spec.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            gate = h.split_once(". Gate M").and_then(|(_, rest)| {
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                canonical_number(&digits)
            });
            if let Some(g) = gate {
                out.entry(g).or_default();
            }
            continue;
        }
        if let (Some(g), Some(rest)) = (gate, line.strip_prefix("**GATE-"))
            && let Some((n, _)) = rest.split_once("**")
        {
            let band = (g + 1) * 10..(g + 2) * 10;
            match canonical_number(n) {
                Some(k) if band.contains(&k) => out.entry(g).or_default().push(format!("GATE-{n}")),
                _ => problems.push(format!("SPEC 095: GATE-{n} is outside gate M{g}'s band")),
            }
        }
    }
    for (g, c) in &out {
        if c.is_empty() {
            problems.push(format!("SPEC 095: gate M{g} has no criteria"));
        }
    }
    out
}

/// One row of a record's criteria table.
struct Row {
    id: String,
    status: String,
    evidence: String,
}

/// The rows of the table under `## Exit criteria`, and problems with it.
fn rows(record: &str, m: &str, problems: &mut Vec<String>) -> Vec<Row> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in record.lines() {
        if line.starts_with("## ") {
            inside = line.trim_end() == "## Exit criteria";
            continue;
        }
        if !inside || !line.trim_start().starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        let first = cells.first().copied().unwrap_or_default();
        if first == "Criterion" || first.chars().all(|c| c == '-' || c == ':') {
            continue;
        }
        let is_id = first
            .strip_prefix("GATE-")
            .and_then(canonical_number)
            .is_some();
        if !is_id || cells.len() < 3 {
            problems.push(format!("{m}: `{line}` is not a criterion row"));
            continue;
        }
        out.push(Row {
            id: first.to_owned(),
            status: cells[1].to_owned(),
            evidence: cells[2..].join("|"),
        });
    }
    out
}

/// The backticked spans of one line; `None` when a backtick is unpaired.
fn ticks(line: &str) -> Option<Vec<&str>> {
    if line.matches('`').count() % 2 == 1 {
        return None;
    }
    Some(line.split('`').skip(1).step_by(2).collect())
}

/// The backticked spans of `text`, line by line (unpaired lines give none).
fn all_ticks(text: &str) -> Vec<&str> {
    text.lines().filter_map(ticks).flatten().collect()
}

/// A backticked span that names a repository path: a `/`, no whitespace, no
/// URL scheme.
fn is_path(span: &str) -> bool {
    span.contains('/') && !span.contains(char::is_whitespace) && !span.contains("://")
}

/// Why a path span is not a repository file under `root`, if it is not.
fn bad_path(root: &Path, span: &str) -> Option<&'static str> {
    let p = Path::new(span);
    if p.is_absolute() || span.starts_with('/') {
        return Some("is absolute");
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Some("climbs out with `..`");
    }
    if !root.join(p).is_file() {
        return Some("is not a file in the repository");
    }
    None
}

/// Words of `text`: runs of ASCII letters, digits and `-`.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|w| !w.is_empty())
}

fn is_hex64(w: &str) -> bool {
    w.len() == 64
        && w.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_commit(w: &str) -> bool {
    (7..=40).contains(&w.len())
        && w.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The gate a deferral is due at: `due at M<k>`.
fn due_at(evidence: &str) -> Option<u32> {
    let rest = &evidence[evidence.find("due at M")? + "due at M".len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    canonical_number(&digits)
}

/// The ADRs a row names that exist and mention `id`.
fn adrs<'a>(root: &Path, evidence: &'a str, id: &str) -> Vec<&'a str> {
    words(evidence)
        .filter(|w| {
            w.strip_prefix("ADR-").and_then(canonical_number).is_some()
                && std::fs::read_to_string(root.join(format!("docs/decisions/{w}.md")))
                    .is_ok_and(|t| words(&t).any(|x| x == id))
        })
        .collect()
}

/// Whether a commit is an ancestor of the commit under test: `Some(false)`
/// refuses, `None` means it cannot be resolved here (a shallow clone).
type Ancestry<'a> = &'a dyn Fn(&str) -> Option<bool>;

fn git_ancestry(root: &Path) -> impl Fn(&str) -> Option<bool> + '_ {
    move |c: &str| {
        let git = |args: &[&str]| {
            Command::new("git")
                .current_dir(root)
                .args(args)
                .status()
                .ok()
        };
        if !git(&["cat-file", "-e", &format!("{c}^{{commit}}")])?.success() {
            return None;
        }
        Some(git(&["merge-base", "--is-ancestor", c, "HEAD"])?.success())
    }
}

/// Every breach of SPEC 095 §1 by the records under `root`.
fn check(root: &Path, ancestry: Ancestry<'_>) -> Vec<String> {
    let mut problems = Vec::new();
    let spec = std::fs::read_to_string(root.join(SPEC)).unwrap();
    let written = criteria(&spec, &mut problems);
    let runs: Vec<BTreeSet<String>> = std::fs::read_dir(root.join("docs/runs"))
        .map(|d| {
            d.filter_map(|e| std::fs::read_to_string(e.ok()?.path()).ok())
                .map(|t| words(&t).map(str::to_owned).collect())
                .collect()
        })
        .unwrap_or_default();

    let mut records: BTreeMap<u32, String> = BTreeMap::new();
    let Ok(dir) = std::fs::read_dir(root.join(RECORDS)) else {
        return problems;
    };
    let mut names: Vec<String> = dir
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    for name in names {
        let g = name
            .strip_prefix('M')
            .and_then(|n| n.strip_suffix(".md"))
            .and_then(canonical_number);
        match g {
            Some(g) => {
                records.insert(
                    g,
                    std::fs::read_to_string(root.join(RECORDS).join(&name)).unwrap(),
                );
            }
            None => problems.push(format!("{RECORDS}/{name} is not a record M<g>.md")),
        }
    }

    // Parse every record first: GATE-5 needs the deferrals of earlier ones.
    let mut parsed: BTreeMap<u32, Vec<Row>> = BTreeMap::new();
    for (g, text) in &records {
        parsed.insert(*g, rows(text, &format!("M{g}"), &mut problems));
    }
    // GATE-5: (criterion, ADRs) deferred to each gate by an earlier record.
    let mut carried: BTreeMap<u32, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for (g, rs) in &parsed {
        for r in rs.iter().filter(|r| r.status == "deferred") {
            if let Some(k) = due_at(&r.evidence).filter(|k| k > g) {
                let a = adrs(root, &r.evidence, &r.id)
                    .into_iter()
                    .map(str::to_owned)
                    .collect();
                carried.entry(k).or_default().insert(r.id.clone(), a);
            }
        }
    }

    for (g, text) in &records {
        let m = format!("M{g}");
        let Some(own) = written.get(g) else {
            problems.push(format!("{m}: SPEC 095 has no section for gate {m}"));
            continue;
        };
        // GATE-4: the base, and where it resolves, an ancestor.
        let base = text
            .lines()
            .find_map(|l| l.strip_prefix("**Commit:**"))
            .and_then(|l| ticks(l).and_then(|t| t.into_iter().next()));
        match base {
            Some(c) if is_commit(c) => {
                if ancestry(c) == Some(false) {
                    problems.push(format!(
                        "{m}: base {c} is not an ancestor of the commit under test"
                    ));
                }
            }
            _ => problems.push(format!("{m}: no **Commit:** line naming the base commit")),
        }
        // GATE-1: exactly the gate's criteria and those carried to it.
        let to_here = carried.get(g).cloned().unwrap_or_default();
        let want: BTreeSet<String> = own.iter().cloned().chain(to_here.keys().cloned()).collect();
        let mut seen = BTreeSet::new();
        for r in &parsed[g] {
            if !seen.insert(r.id.clone()) {
                problems.push(format!("{m}: {} has two rows", r.id));
                continue;
            }
            if !want.contains(&r.id) {
                problems.push(format!("{m}: {} is not a criterion of {m}", r.id));
            }
            match r.status.as_str() {
                "met" => {
                    let mut files = r
                        .evidence
                        .lines()
                        .filter_map(ticks)
                        .flatten()
                        .filter(|s| is_path(s));
                    if !files.any(|s| bad_path(root, s).is_none()) {
                        problems.push(format!("{m}: {} is met without a repository file", r.id));
                    }
                }
                "deferred" => {
                    let named = adrs(root, &r.evidence, &r.id);
                    if named.is_empty() {
                        problems.push(format!(
                            "{m}: {} is deferred without an ADR that mentions it",
                            r.id
                        ));
                    }
                    match due_at(&r.evidence) {
                        Some(k) if k > *g && k <= LAST_GATE => {}
                        _ => problems.push(format!(
                            "{m}: {} is not `due at` a later gate up to M{LAST_GATE}",
                            r.id
                        )),
                    }
                    // GATE-5: a second deferral is its own decision.
                    if let Some(before) = to_here.get(&r.id)
                        && named.iter().all(|a| before.iter().any(|b| b == a))
                    {
                        problems.push(format!(
                            "{m}: {} is deferred again under the same ADR",
                            r.id
                        ));
                    }
                }
                other => problems.push(format!("{m}: {} has status `{other}`", r.id)),
            }
        }
        for id in want.difference(&seen) {
            problems.push(format!("{m}: {id} has no row"));
        }
        // GATE-2: backticks pair within a line; paths are repository files.
        for (n, line) in text.lines().enumerate() {
            if ticks(line).is_none() {
                problems.push(format!("{m}: line {} has an unpaired backtick", n + 1));
            }
        }
        for span in all_ticks(text).into_iter().filter(|s| is_path(s)) {
            if let Some(why) = bad_path(root, span) {
                problems.push(format!("{m}: `{span}` {why}"));
            }
        }
        // GATE-2: every 64-hex ID is a word of one run record.
        for w in words(text).filter(|w| is_hex64(w)) {
            if !runs.iter().any(|r| r.contains(w)) {
                problems.push(format!(
                    "{m}: {w} appears in no run record under docs/runs/"
                ));
            }
        }
    }
    problems
}

/// Cites: GATE-1, GATE-2, GATE-3, GATE-4, GATE-5, GATE-6
#[test]
fn every_gate_record_meets_spec_095() {
    let root = repo_root();
    let problems = check(&root, &git_ancestry(&root));
    assert!(problems.is_empty(), "{problems:#?}");
    assert!(root.join(RECORDS).join("M0.md").is_file(), "no M0 record");
}

/// The rows of the real M0 record, by ID.
fn m0_rows() -> BTreeMap<String, Row> {
    let text = std::fs::read_to_string(repo_root().join(RECORDS).join("M0.md")).unwrap();
    rows(&text, "M0", &mut Vec::new())
        .into_iter()
        .map(|r| (r.id.clone(), r))
        .collect()
}

/// What a test can see of M0's criteria (SPEC 095 §9). GATE-10 is the CI run
/// that runs this; GATE-14 is deferred; GATE-16 is the maintainer's merge.
///
/// Cites: GATE-10, GATE-11, GATE-12, GATE-13, GATE-14, GATE-15, GATE-16
#[test]
fn m0_criteria_hold_where_a_test_can_see_them() {
    let root = repo_root();
    let spec = std::fs::read_to_string(root.join(SPEC)).unwrap();
    let m0: Vec<String> = (10..=16).map(|n| format!("GATE-{n}")).collect();
    assert_eq!(criteria(&spec, &mut Vec::new()).get(&0), Some(&m0));
    let rows = m0_rows();

    // GATE-11: the substrate specs are in scope (trace-check proves the citing).
    let scope: toml::Value =
        toml::from_str(&std::fs::read_to_string(root.join("trace-scope.toml")).unwrap()).unwrap();
    let specs: BTreeSet<&str> = scope["implemented"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["spec"].as_str())
        .collect();
    for s in ["010", "030", "040", "080", "085"] {
        assert!(specs.contains(s), "SPEC {s} is not in trace-scope.toml");
    }

    // GATE-12: the POC 4 suite cites the control rule.
    let p4 = std::fs::read_to_string(root.join("tests/accept/p4.rs")).unwrap();
    assert!(
        p4.lines()
            .any(|l| l.starts_with("/// Cites:") && l.contains("CON-18"))
    );

    // GATE-13: the cited run record holds the cited IDs and is mock-gated.
    let r13 = &rows["GATE-13"];
    let runs: Vec<&str> = all_ticks(&r13.evidence)
        .into_iter()
        .filter(|s| s.starts_with("docs/runs/"))
        .collect();
    assert!(!runs.is_empty(), "GATE-13 cites no run record");
    let ids: Vec<&str> = words(&r13.evidence).filter(|w| is_hex64(w)).collect();
    assert!(ids.len() >= 2, "GATE-13 cites no loop and verdict IDs");
    for run in runs {
        let text = std::fs::read_to_string(root.join(run)).unwrap();
        assert!(
            text.contains("mock-gated"),
            "{run} is not labelled mock-gated"
        );
        for id in &ids {
            assert!(text.contains(id), "{run} does not hold {id}");
        }
    }

    // GATE-14 is deferred, and GATE-15's traces load under EMU-64.
    assert_eq!(rows["GATE-14"].status, "deferred");
    let traces: BTreeSet<PathBuf> = all_ticks(&rows["GATE-15"].evidence)
        .into_iter()
        .filter(|s| s.starts_with("scenarios/measured/"))
        .filter_map(|s| root.join(s).parent().map(Path::to_path_buf))
        .collect();
    assert!(!traces.is_empty(), "GATE-15 cites no measured trace");
    for dir in traces {
        acn_emu::trace::load(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    }
    assert_eq!(rows["GATE-16"].status, "met");
}

/// The rows of the real M1 record, by ID; `None` until the record is written.
fn m1_rows() -> Option<BTreeMap<String, Row>> {
    let text = std::fs::read_to_string(repo_root().join(RECORDS).join("M1.md")).ok()?;
    Some(
        rows(&text, "M1", &mut Vec::new())
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect(),
    )
}

/// The `/// Cites:` IDs of a suite under `tests/accept/`.
fn cited(root: &Path, suite: &str) -> BTreeSet<String> {
    let path = root.join("tests/accept").join(suite);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .filter_map(|l| l.strip_prefix("/// Cites:"))
        .flat_map(|l| l.split(','))
        .map(|id| id.trim().to_owned())
        .collect()
}

fn json(path: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The spans of a row's evidence that are repository paths under `prefix`.
fn cited_paths<'a>(row: &'a Row, prefix: &str) -> Vec<&'a str> {
    all_ticks(&row.evidence)
        .into_iter()
        .filter(|s| is_path(s) && s.starts_with(prefix))
        .collect()
}

/// The value of an `| Item | \`value\` |` row of a run record.
fn item<'a>(record: &'a str, name: &str) -> Option<&'a str> {
    record.lines().find_map(|l| {
        let mut cells = l.trim().trim_matches('|').split('|').map(str::trim);
        (cells.next()? == name).then_some(())?;
        cells.next()?.strip_prefix('`')?.strip_suffix('`')
    })
}

/// What a test can see of M1's criteria (SPEC 095 §9). GATE-20 is the CI run
/// that runs this; GATE-27 and the substance of GATE-26 are the maintainer's.
/// What depends on the record is checked once the record is written.
///
/// Cites: GATE-14, GATE-20, GATE-21, GATE-22, GATE-23, GATE-24, GATE-25, GATE-26, GATE-27
#[test]
fn m1_criteria_hold_where_a_test_can_see_them() {
    let root = repo_root();
    let spec = std::fs::read_to_string(root.join(SPEC)).unwrap();
    let m1: Vec<String> = (20..=27).map(|n| format!("GATE-{n}")).collect();
    assert_eq!(criteria(&spec, &mut Vec::new()).get(&1), Some(&m1));

    // GATE-21: the substrate specs are in scope (trace-check proves the citing).
    let scope: toml::Value =
        toml::from_str(&std::fs::read_to_string(root.join("trace-scope.toml")).unwrap()).unwrap();
    let specs: BTreeSet<&str> = scope["implemented"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["spec"].as_str())
        .collect();
    for s in ["020", "050", "070", "090", "140"] {
        assert!(specs.contains(s), "SPEC {s} is not in trace-scope.toml");
    }

    // GATE-22: the suites exist, each cites CON-5, and the kit's cites P16-6.
    for suite in [
        "trace_determinism.rs",
        "emu_sim.rs",
        "harness_sim.rs",
        "gen_sim.rs",
        "kit.rs",
    ] {
        assert!(cited(&root, suite).contains("CON-5"), "{suite}: CON-5");
    }
    assert!(cited(&root, "kit.rs").contains("P16-6"), "kit.rs: P16-6");

    let Some(rows) = m1_rows() else {
        return;
    };
    for suite in [
        "tests/accept/trace_determinism.rs",
        "tests/accept/emu_sim.rs",
        "tests/accept/harness_sim.rs",
        "tests/accept/gen_sim.rs",
        "tests/accept/kit.rs",
    ] {
        assert!(
            cited_paths(&rows["GATE-22"], "tests/accept/").contains(&suite),
            "GATE-22 does not cite {suite}"
        );
    }

    // GATE-23: the cited twin is committed with its loop's report and both
    // verdicts, its page shows the loop, and no twinned quantity is outside
    // its tolerance. Read from the twin itself, not from the page (issue 84).
    let r23 = &rows["GATE-23"];
    let twins: Vec<&str> = cited_paths(r23, "docs/runs/loop/")
        .into_iter()
        .filter(|p| p.ends_with("/twin.json"))
        .collect();
    assert!(!twins.is_empty(), "GATE-23 cites no committed twin");
    let page = cited_paths(r23, "docs/evidence/");
    assert_eq!(page.len(), 1, "GATE-23 cites one evidence page");
    let page = std::fs::read_to_string(root.join(page[0])).unwrap();
    let mut loops = BTreeSet::new();
    for twin_path in twins {
        let twin = json(&root.join(twin_path));
        let loop_id = twin["loop_id"].as_str().unwrap().to_owned();
        let report = root.join(format!("docs/runs/loop/{loop_id}/report.json"));
        assert_eq!(json(&report)["loop_id"], twin["loop_id"], "{twin_path}");
        for key in ["verdict_id", "l1_verdict_id"] {
            let id = twin[key].as_str().unwrap();
            let verdict = root.join(format!("docs/runs/verdicts/{id}/verdict.json"));
            assert_eq!(
                json(&verdict)["verdict_id"],
                twin[key],
                "{twin_path}: {key}"
            );
        }
        assert!(page.contains(&loop_id), "the page lacks loop {loop_id}");
        let mut compared = 0;
        for slice in twin["divergence"].as_array().unwrap() {
            for cell in slice["cells"].as_array().unwrap() {
                for (quantity, figures) in cell["quantities"].as_object().unwrap() {
                    assert_eq!(
                        figures["within"], true,
                        "{twin_path}: {} / {quantity} is outside its tolerance",
                        cell["key"]
                    );
                    compared += 1;
                }
            }
        }
        assert!(compared > 0, "{twin_path}: no twinned quantity");
        loops.insert(loop_id);
    }

    // GATE-24 (P16-11): the cited records name the tag, the target, the
    // build_hash and `identical`; every reference manifest of that target is
    // canonical, of `sim`, of that build, and regenerated in the record.
    let records = cited_paths(&rows["GATE-24"], "docs/runs/");
    assert!(!records.is_empty(), "GATE-24 cites no regeneration record");
    let mut manifests = BTreeMap::new();
    for e in std::fs::read_dir(root.join("kit/manifests")).unwrap() {
        let path = e.unwrap().path();
        let bytes = std::fs::read(&path).unwrap();
        let m: acn_trace::bundle::Manifest =
            serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(
            m.to_bytes().unwrap(),
            bytes,
            "{}: not canonical",
            path.display()
        );
        assert_eq!(m.mode, "sim", "{}", path.display());
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            format!("{}.json", m.run_id),
            "a reference manifest is named by its run_id"
        );
        manifests.insert(m.run_id.clone(), m);
    }
    let mut regenerated = BTreeSet::new();
    for record in &records {
        let text = std::fs::read_to_string(root.join(record)).unwrap();
        let tag = item(&text, "Tag").unwrap_or_else(|| panic!("{record}: no tag"));
        assert!(
            !tag.is_empty() && tag != "HEAD",
            "{record}: `{tag}` is not a tag"
        );
        let target = item(&text, "Target").unwrap_or_else(|| panic!("{record}: no target"));
        let build = item(&text, "build_hash").unwrap_or_else(|| panic!("{record}: no build_hash"));
        assert!(is_hex64(build), "{record}: build_hash");
        assert!(
            record.ends_with(&format!("-p16-{target}.md")),
            "{record} is not named for {target} (P16-11)"
        );
        let mut of_target = 0;
        for (id, m) in manifests.iter().filter(|(_, m)| m.build.target == target) {
            assert_eq!(
                m.build.build_hash, build,
                "{id}: another build than {record}"
            );
            assert!(
                text.lines().any(|l| l == format!("{id} identical")),
                "{record}: {id} is not recorded identical"
            );
            regenerated.insert(id.clone());
            of_target += 1;
        }
        assert!(of_target > 0, "{record}: no reference manifest of {target}");
    }
    assert_eq!(
        regenerated.len(),
        manifests.len(),
        "a reference manifest is of a target no record covers"
    );

    // GATE-25 (P16-10, P16-12): the kit holds the manifest of every sim bundle
    // of GATE-23's loop, the quickstart, and a cross-target comparison record.
    let r25 = &rows["GATE-25"];
    assert!(
        !cited_paths(r25, "kit/manifests/").is_empty(),
        "GATE-25 cites no reference manifest"
    );
    assert!(
        all_ticks(&r25.evidence).contains(&"README.md"),
        "GATE-25 does not cite the quickstart"
    );
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    assert!(
        readme.contains("```sh quickstart"),
        "no quickstart (P16-30)"
    );
    assert!(
        !cited_paths(r25, "docs/runs/").is_empty(),
        "GATE-25 cites no cross-target comparison record"
    );
    for loop_id in &loops {
        let report = json(&root.join(format!("docs/runs/loop/{loop_id}/report.json")));
        for b in report["bundles"].as_array().unwrap() {
            let id = b["run_id"].as_str().unwrap();
            assert!(
                manifests.contains_key(id),
                "the kit lacks the manifest of {id}"
            );
        }
    }

    // GATE-26: met with an account under docs/wg/, or deferred. GATE-14 is
    // carried, deferred again. GATE-27 is the merge.
    let r26 = &rows["GATE-26"];
    match r26.status.as_str() {
        "met" => assert!(
            !cited_paths(r26, "docs/wg/").is_empty(),
            "GATE-26 is met without an account under docs/wg/"
        ),
        other => assert_eq!(other, "deferred"),
    }
    assert_eq!(rows["GATE-14"].status, "deferred");
    assert_eq!(rows["GATE-27"].status, "met");
}

const ID: &str = "f3afab225f575fdab86fb088a2fa9d06b472aca3fbccc91747615bef16128344";

/// A repository with the real SPEC 095, one run record, ADRs 29, 31 and 40
/// (each mentioning GATE-14) and ADR-1 (not), and the record `M0.md` given.
fn fixture(m0: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    for d in ["specs", "docs/gates", "docs/runs", "docs/decisions"] {
        std::fs::create_dir_all(r.join(d)).unwrap();
    }
    std::fs::copy(repo_root().join(SPEC), r.join(SPEC)).unwrap();
    std::fs::write(r.join("docs/runs/r.md"), format!("loop {ID}\n")).unwrap();
    for n in [29, 31, 40] {
        std::fs::write(
            r.join(format!("docs/decisions/ADR-{n}.md")),
            "Defers GATE-14.\n",
        )
        .unwrap();
    }
    std::fs::write(r.join("docs/decisions/ADR-1.md"), "Unrelated.\n").unwrap();
    std::fs::write(r.join("docs/gates/M0.md"), m0).unwrap();
    std::fs::write(r.join("docs/gates/.gitkeep"), "").unwrap();
    tmp
}

/// A record that meets every rule.
fn good() -> String {
    let mut s = String::from(
        "**Commit:** `6422d3e`\n\n## Exit criteria\n\n| Criterion | Status | Evidence |\n|---|---|---|\n",
    );
    for n in 10..=16 {
        if n == 14 {
            s.push_str("| GATE-14 | deferred | ADR-29 and ADR-31; due at M1. |\n");
        } else {
            s.push_str(&format!(
                "| GATE-{n} | met | `docs/runs/r.md`, loop `{ID}` |\n"
            ));
        }
    }
    s.push_str("\n## Notes\n\n| Item | Lands with |\n|---|---|\n| x | y |\n");
    s
}

fn no_ancestry(_: &str) -> Option<bool> {
    None
}

/// Cites: GATE-1, GATE-2, GATE-3, GATE-4, GATE-5, GATE-6
#[test]
fn each_breach_of_section_1_is_found_alone() {
    let ok = fixture(&good());
    assert_eq!(check(ok.path(), &no_ancestry), Vec::<String>::new());

    let g = good();
    let row10 = format!("| GATE-10 | met | `docs/runs/r.md`, loop `{ID}` |\n");
    let without = |id: &str| -> String {
        g.lines()
            .filter(|l| !l.starts_with(&format!("| {id} ")))
            .map(|l| format!("{l}\n"))
            .collect()
    };
    let hex = "b".repeat(64);
    let late = format!("M0: GATE-14 is not `due at` a later gate up to M{LAST_GATE}");
    let cases: Vec<(&str, String, String)> = vec![
        (
            "missing criterion",
            without("GATE-15"),
            "M0: GATE-15 has no row".into(),
        ),
        (
            "extra criterion",
            g.replace(
                &row10,
                &format!("{row10}| GATE-20 | met | `docs/runs/r.md` |\n"),
            ),
            "M0: GATE-20 is not a criterion of M0".into(),
        ),
        (
            "two rows",
            g.replace(&row10, &format!("{row10}{row10}")),
            "M0: GATE-10 has two rows".into(),
        ),
        (
            "not a row",
            g.replace(&row10, &format!("{row10}| gate-10 | met | x |\n")),
            "M0: `| gate-10 | met | x |` is not a criterion row".into(),
        ),
        (
            "unknown status",
            g.replace("| GATE-10 | met |", "| GATE-10 | done |"),
            "M0: GATE-10 has status `done`".into(),
        ),
        (
            "met without a file",
            g.replace(
                "| GATE-10 | met | `docs/runs/r.md`, loop",
                "| GATE-10 | met | see loop",
            ),
            "M0: GATE-10 is met without a repository file".into(),
        ),
        (
            "missing path",
            g.replacen(
                "`docs/runs/r.md`, loop",
                "`docs/runs/r.md`, `docs/runs/gone.md`, loop",
                1,
            ),
            "M0: `docs/runs/gone.md` is not a file in the repository".into(),
        ),
        (
            "absolute path",
            g.replacen(
                "`docs/runs/r.md`, loop",
                "`docs/runs/r.md`, `/etc/hosts`, loop",
                1,
            ),
            "M0: `/etc/hosts` is absolute".into(),
        ),
        (
            "climbing path",
            g.replacen(
                "`docs/runs/r.md`, loop",
                "`docs/runs/r.md`, `../x/y`, loop",
                1,
            ),
            "M0: `../x/y` climbs out with `..`".into(),
        ),
        (
            "a directory",
            g.replacen(
                "`docs/runs/r.md`, loop",
                "`docs/runs/r.md`, `docs/`, loop",
                1,
            ),
            "M0: `docs/` is not a file in the repository".into(),
        ),
        (
            "unpaired backtick",
            format!("A ` stray\n{g}"),
            "M0: line 1 has an unpaired backtick".into(),
        ),
        (
            "unrecorded id in backticks",
            g.replacen(ID, &hex, 1),
            format!("M0: {hex} appears in no run record under docs/runs/"),
        ),
        (
            "unrecorded id in prose",
            format!("{g}\nAlso {hex}.\n"),
            format!("M0: {hex} appears in no run record under docs/runs/"),
        ),
        (
            "deferral without an ADR",
            g.replace("ADR-29 and ADR-31;", "the maintainer said so;"),
            "M0: GATE-14 is deferred without an ADR that mentions it".into(),
        ),
        (
            "deferral to an unrelated ADR",
            g.replace("ADR-29 and ADR-31;", "ADR-1;"),
            "M0: GATE-14 is deferred without an ADR that mentions it".into(),
        ),
        (
            "deferral with no due gate",
            g.replace("due at M1", "later"),
            late.clone(),
        ),
        (
            "deferral to no later gate",
            g.replace("due at M1", "due at M0"),
            late.clone(),
        ),
        (
            "deferral past M4",
            g.replace("due at M1", "due at M9"),
            late.clone(),
        ),
        (
            "no commit",
            g.replace("**Commit:** `6422d3e`", "**Commit:** soon"),
            "M0: no **Commit:** line naming the base commit".into(),
        ),
    ];
    for (what, record, expect) in cases {
        let dir = fixture(&record);
        assert_eq!(check(dir.path(), &no_ancestry), vec![expect], "{what}");
    }

    // GATE-4: a base that resolves but is not an ancestor.
    let dir = fixture(&g);
    let not_ancestor = |_: &str| Some(false);
    assert_eq!(
        check(dir.path(), &not_ancestor),
        vec!["M0: base 6422d3e is not an ancestor of the commit under test".to_owned()]
    );

    // GATE-1: a record named with a leading zero.
    let dir = fixture(&g);
    std::fs::write(dir.path().join("docs/gates/M00.md"), &g).unwrap();
    assert_eq!(
        check(dir.path(), &no_ancestry),
        vec!["docs/gates/M00.md is not a record M<g>.md".to_owned()]
    );

    // GATE-5: a deferral to M1 is carried into M1's record, once M1 has a
    // section (SPEC 095 §3) and a record; a second deferral needs another ADR.
    let dir = fixture(&g);
    let spec = std::fs::read_to_string(dir.path().join(SPEC)).unwrap();
    let m1_ids = criteria(&spec, &mut Vec::new())
        .remove(&1)
        .expect("SPEC 095 writes gate M1");
    let m1 = |rows: &str| {
        let met: String = m1_ids
            .iter()
            .map(|id| format!("| {id} | met | `docs/runs/r.md` |\n"))
            .collect();
        std::fs::write(
            dir.path().join("docs/gates/M1.md"),
            format!("**Commit:** `abcdef1`\n\n## Exit criteria\n\n{met}{rows}"),
        )
        .unwrap();
    };
    m1("");
    assert_eq!(
        check(dir.path(), &no_ancestry),
        vec!["M1: GATE-14 has no row".to_owned()]
    );
    m1("| GATE-14 | deferred | ADR-31, due at M2. |\n");
    assert_eq!(
        check(dir.path(), &no_ancestry),
        vec!["M1: GATE-14 is deferred again under the same ADR".to_owned()]
    );
    m1("| GATE-14 | deferred | ADR-40, due at M2. |\n");
    assert_eq!(check(dir.path(), &no_ancestry), Vec::<String>::new());

    // GATE-1, GATE-6: a section with no criteria, and a record with no section.
    let dir = fixture(&g);
    let spec = dir.path().join(SPEC);
    let text = std::fs::read_to_string(&spec).unwrap();
    std::fs::write(
        &spec,
        format!("{text}\n## 4. Gate M2 — empty\n\nNothing yet.\n"),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("docs/gates/M3.md"),
        "**Commit:** `abcdef1`\n",
    )
    .unwrap();
    assert_eq!(
        check(dir.path(), &no_ancestry),
        vec![
            "SPEC 095: gate M2 has no criteria".to_owned(),
            "M3: SPEC 095 has no section for gate M3".to_owned(),
        ]
    );
}
