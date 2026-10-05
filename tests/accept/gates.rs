//! Milestone gate records (SPEC 095 §1): every `docs/gates/M<n>.md` lists
//! exactly its gate's criteria, each `met` with evidence that exists or
//! `deferred` under an ADR to a later gate, and carries earlier deferrals
//! forward. The checker runs on the real records and on fixtures that break
//! each rule.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const SPEC: &str = "specs/095-gates.md";
const RECORDS: &str = "docs/gates";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// The criteria of each gate whose section SPEC 095 writes: the `**GATE-n**`
/// definitions under a `## <k>. Gate M<g>` heading.
fn criteria(spec: &str) -> BTreeMap<u32, Vec<String>> {
    let mut out: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    let mut gate = None;
    for line in spec.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            gate = h
                .split_once(". Gate M")
                .and_then(|(_, rest)| rest.split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|n| n.parse::<u32>().ok());
            if let Some(g) = gate {
                out.entry(g).or_default();
            }
            continue;
        }
        if let (Some(g), Some(rest)) = (gate, line.strip_prefix("**GATE-"))
            && let Some((n, _)) = rest.split_once("**")
        {
            out.entry(g).or_default().push(format!("GATE-{n}"));
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

fn rows(record: &str) -> Vec<Row> {
    record
        .lines()
        .filter(|l| l.starts_with("| GATE-"))
        .map(|l| {
            let cells: Vec<&str> = l.trim_matches('|').split('|').map(str::trim).collect();
            Row {
                id: cells.first().copied().unwrap_or_default().to_owned(),
                status: cells.get(1).copied().unwrap_or_default().to_owned(),
                evidence: cells[2.min(cells.len())..].join("|"),
            }
        })
        .collect()
}

/// The backticked spans of `text`.
fn ticks(text: &str) -> Vec<&str> {
    text.split('`').skip(1).step_by(2).collect()
}

/// A backticked span that names a repository path: it holds a `/`, no
/// whitespace and no URL scheme.
fn is_path(span: &str) -> bool {
    span.contains('/') && !span.contains(char::is_whitespace) && !span.contains("://")
}

/// Alphanumeric words of `text`, with `-` kept inside them (`ADR-29`, `M1`).
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|w| !w.is_empty())
}

fn gate_number(word: &str) -> Option<u32> {
    word.strip_prefix('M')?.parse().ok()
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

/// The next gate a deferred row is due at: its first `M<k>` with k > `gate`.
fn due_at(evidence: &str, gate: u32) -> Option<u32> {
    words(evidence).filter_map(gate_number).find(|k| *k > gate)
}

/// Every breach of SPEC 095 §1 by the records under `root`.
fn check(root: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    let spec = std::fs::read_to_string(root.join(SPEC)).unwrap();
    let written = criteria(&spec);
    let runs: String = std::fs::read_dir(root.join("docs/runs"))
        .map(|d| {
            d.filter_map(|e| std::fs::read_to_string(e.ok()?.path()).ok())
                .collect()
        })
        .unwrap_or_default();

    let mut records: BTreeMap<u32, String> = BTreeMap::new();
    let Ok(dir) = std::fs::read_dir(root.join(RECORDS)) else {
        return problems;
    };
    for e in dir {
        let path = e.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        match name.strip_suffix(".md").and_then(gate_number) {
            Some(g) => {
                records.insert(g, std::fs::read_to_string(&path).unwrap());
            }
            None => problems.push(format!("{RECORDS}/{name} is not a record M<n>.md")),
        }
    }

    // GATE-5: criteria deferred to each gate by an earlier record.
    let mut carried: BTreeMap<u32, BTreeSet<String>> = BTreeMap::new();
    for (g, text) in &records {
        for r in rows(text) {
            if r.status == "deferred"
                && let Some(k) = due_at(&r.evidence, *g)
            {
                carried.entry(k).or_default().insert(r.id);
            }
        }
    }

    for (g, text) in &records {
        let m = format!("M{g}");
        // GATE-6: a record needs its gate's section.
        let Some(own) = written.get(g) else {
            problems.push(format!("{m}: SPEC 095 has no section for gate {m}"));
            continue;
        };
        // GATE-4: the commit the evidence was checked at.
        let commit = text
            .lines()
            .find_map(|l| l.strip_prefix("**Commit:**"))
            .and_then(|l| ticks(l).into_iter().next());
        if !commit.is_some_and(is_commit) {
            problems.push(format!("{m}: no **Commit:** line naming a commit"));
        }
        // GATE-1: exactly the gate's criteria, plus those carried to it.
        let mut want: BTreeSet<String> = own.iter().cloned().collect();
        want.extend(carried.get(g).cloned().unwrap_or_default());
        let mut seen = BTreeSet::new();
        for r in rows(text) {
            if !seen.insert(r.id.clone()) {
                problems.push(format!("{m}: {} has two rows", r.id));
            }
            if !want.contains(&r.id) {
                problems.push(format!("{m}: {} is not a criterion of {m}", r.id));
            }
            match r.status.as_str() {
                "met" => {
                    // GATE-2: a met row names a path.
                    if !ticks(&r.evidence).into_iter().any(is_path) {
                        problems.push(format!("{m}: {} is met without a path", r.id));
                    }
                }
                "deferred" => {
                    // GATE-3: an existing ADR and a later gate.
                    let adr = words(&r.evidence).find(|w| {
                        w.strip_prefix("ADR-").is_some_and(|n| {
                            n.parse::<u32>().is_ok()
                                && root.join(format!("docs/decisions/{w}.md")).is_file()
                        })
                    });
                    if adr.is_none() {
                        problems.push(format!("{m}: {} is deferred without an ADR", r.id));
                    }
                    if due_at(&r.evidence, *g).is_none() {
                        problems.push(format!("{m}: {} is deferred to no later gate", r.id));
                    }
                }
                other => problems.push(format!("{m}: {} has status `{other}`", r.id)),
            }
        }
        for id in want.difference(&seen) {
            // GATE-5 names a missing carried row; GATE-1 a missing own one.
            problems.push(format!("{m}: {id} has no row"));
        }
        // GATE-2: every path exists, every 64-hex ID has a run record.
        for span in ticks(text) {
            if is_path(span) && !root.join(span.trim_end_matches('/')).exists() {
                problems.push(format!("{m}: `{span}` does not exist"));
            }
            for w in words(span) {
                if is_hex64(w) && !runs.contains(w) {
                    problems.push(format!(
                        "{m}: {w} appears in no run record under docs/runs/"
                    ));
                }
            }
        }
    }
    problems
}

/// Cites: GATE-1, GATE-2, GATE-3, GATE-4, GATE-5, GATE-6, GATE-10, GATE-11,
/// GATE-12, GATE-13, GATE-14, GATE-15, GATE-16
#[test]
fn every_gate_record_meets_spec_095() {
    let root = repo_root();
    let problems = check(&root);
    assert!(problems.is_empty(), "{problems:#?}");
    assert!(root.join(RECORDS).join("M0.md").is_file(), "no M0 record");
}

/// Cites: GATE-1, GATE-6
#[test]
fn spec_095_writes_gate_m0() {
    let spec = std::fs::read_to_string(repo_root().join(SPEC)).unwrap();
    let c = criteria(&spec);
    let m0: Vec<String> = (10..=16).map(|n| format!("GATE-{n}")).collect();
    assert_eq!(c.get(&0), Some(&m0));
}

const ID: &str = "f3afab225f575fdab86fb088a2fa9d06b472aca3fbccc91747615bef16128344";

/// A repository with the real SPEC 095, one run record, ADR-29 and the
/// record `M0.md` given.
fn fixture(m0: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    for d in ["specs", "docs/gates", "docs/runs", "docs/decisions"] {
        std::fs::create_dir_all(r.join(d)).unwrap();
    }
    std::fs::copy(repo_root().join(SPEC), r.join(SPEC)).unwrap();
    std::fs::write(r.join("docs/runs/r.md"), format!("loop {ID}\n")).unwrap();
    std::fs::write(r.join("docs/decisions/ADR-29.md"), "# ADR-29\n").unwrap();
    std::fs::write(r.join("docs/gates/M0.md"), m0).unwrap();
    tmp
}

/// A record that meets every rule.
fn good() -> String {
    let mut s =
        String::from("**Commit:** `6422d3e`\n\n| Criterion | Status | Evidence |\n|---|---|---|\n");
    for n in 10..=16 {
        if n == 14 {
            s.push_str("| GATE-14 | deferred | ADR-29; next due at M1. |\n");
        } else {
            s.push_str(&format!(
                "| GATE-{n} | met | `docs/runs/r.md`, loop `{ID}` |\n"
            ));
        }
    }
    s
}

/// Cites: GATE-1, GATE-2, GATE-3, GATE-4, GATE-5, GATE-6
#[test]
fn each_breach_of_section_1_is_found() {
    let ok = fixture(&good());
    assert_eq!(check(ok.path()), Vec::<String>::new());

    let g = good();
    let cases: Vec<(&str, String, &str)> = vec![
        (
            "missing criterion",
            g.replace("| GATE-15 | met | `docs/runs/r.md`, loop `", "| x | "),
            "GATE-15 has no row",
        ),
        (
            "extra criterion",
            format!("{g}| GATE-20 | met | `docs/runs/r.md` |\n"),
            "GATE-20 is not a criterion",
        ),
        (
            "two rows",
            format!("{g}| GATE-10 | met | `docs/runs/r.md` |\n"),
            "GATE-10 has two rows",
        ),
        (
            "unknown status",
            g.replace("| GATE-10 | met |", "| GATE-10 | done |"),
            "status `done`",
        ),
        (
            "met without a path",
            g.replace(
                "| GATE-10 | met | `docs/runs/r.md`, loop",
                "| GATE-10 | met | see loop",
            ),
            "GATE-10 is met without a path",
        ),
        (
            "missing path",
            g.replacen("`docs/runs/r.md`", "`docs/runs/gone.md`", 1),
            "does not exist",
        ),
        (
            "unrecorded id",
            g.replacen(ID, &"a".repeat(64), 1),
            "appears in no run record",
        ),
        (
            "deferral without an ADR",
            g.replace("ADR-29;", "the maintainer said so;"),
            "deferred without an ADR",
        ),
        (
            "deferral to an ADR that does not exist",
            g.replace("ADR-29;", "ADR-99;"),
            "deferred without an ADR",
        ),
        (
            "deferral to no later gate",
            g.replace("next due at M1", "next due at M0"),
            "deferred to no later gate",
        ),
        (
            "no commit",
            g.replace("**Commit:** `6422d3e`", "**Commit:** soon"),
            "no **Commit:** line",
        ),
    ];
    for (what, record, expect) in cases {
        let dir = fixture(&record);
        let p = check(dir.path());
        assert!(p.iter().any(|x| x.contains(expect)), "{what}: {p:#?}");
    }

    // GATE-5: a deferral to M1 must be carried into M1's record, once M1 has a
    // section and a record. SPEC 095 has no M1 section yet, so the fixture
    // spec gains one.
    let dir = fixture(&good());
    let spec = dir.path().join(SPEC);
    let text = std::fs::read_to_string(&spec).unwrap();
    std::fs::write(
        &spec,
        format!("{text}\n## 3. Gate M1 — test\n\n**GATE-20** A criterion.\n"),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("docs/gates/M1.md"),
        "**Commit:** `abcdef1`\n\n| GATE-20 | met | `docs/runs/r.md` |\n",
    )
    .unwrap();
    let p = check(dir.path());
    assert!(
        p.iter().any(|x| x.contains("M1: GATE-14 has no row")),
        "{p:#?}"
    );
    std::fs::write(
        dir.path().join("docs/gates/M1.md"),
        "**Commit:** `abcdef1`\n\n| GATE-20 | met | `docs/runs/r.md` |\n| GATE-14 | deferred | ADR-29, due at M2. |\n",
    )
    .unwrap();
    assert_eq!(check(dir.path()), Vec::<String>::new());

    // GATE-6: a record for a gate with no section.
    let dir = fixture(&good());
    std::fs::write(
        dir.path().join("docs/gates/M3.md"),
        "**Commit:** `abcdef1`\n",
    )
    .unwrap();
    let p = check(dir.path());
    assert!(
        p.iter().any(|x| x.contains("no section for gate M3")),
        "{p:#?}"
    );
}
