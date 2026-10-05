//! Measured traces (SPEC 020 §6): every committed trace loads, and each way a
//! trace or its provenance can break EMU-60 to EMU-63 is refused by name.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use acn_emu::trace::load;

/// A named edit of a file's text, and the refusal reason it must cause.
type Case = (&'static str, Box<dyn Fn(&str) -> String>, &'static str);

fn measured_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios/measured")
}

fn reference() -> PathBuf {
    measured_root().join("5g-iana-2023-01-29")
}

/// Copy the reference trace into a fresh `<tmp>/t/`, applying `edit` to the
/// named file's text, and load it.
fn load_edited(file: &str, edit: impl Fn(&str) -> String) -> acn_emu::trace::TraceError {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("t");
    std::fs::create_dir(&dir).unwrap();
    for name in ["trace.toml", "provenance.toml"] {
        let text = std::fs::read_to_string(reference().join(name)).unwrap();
        let text = if name == file { edit(&text) } else { text };
        std::fs::write(dir.join(name), text).unwrap();
    }
    load(&dir).expect_err("the edited trace loaded")
}

fn replace(from: &'static str, to: &'static str) -> impl Fn(&str) -> String {
    move |s| {
        assert!(s.contains(from), "`{from}` is not in the file");
        s.replacen(from, to, 1)
    }
}

/// Cites: EMU-60, EMU-61, EMU-62, EMU-63, EMU-64
#[test]
fn every_committed_trace_loads() {
    let mut n = 0;
    for e in std::fs::read_dir(measured_root()).unwrap() {
        let dir = e.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        let m = load(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        let bytes = std::fs::read(dir.join("trace.toml")).unwrap();
        assert_eq!(m.hash, blake3::hash(&bytes).to_hex().as_str());
        assert!(m.trace.samples.len() >= 2);
        n += 1;
    }
    assert!(n >= 1, "no measured trace");
}

/// Cites: EMU-62, EMU-65
#[test]
fn the_5g_iana_trace_holds_its_outages() {
    let m = load(&reference()).unwrap();
    assert_eq!(m.slug, "5g-iana-2023-01-29");
    assert_eq!(m.trace.samples.len(), 198);
    let outages = m.trace.samples.iter().filter(|s| s.loss >= 1.0).count();
    assert_eq!(outages, 19);
    assert!(
        m.trace
            .samples
            .iter()
            .all(|s| (s.loss >= 1.0) == s.rtt_ms.is_none())
    );
    assert_eq!(m.provenance.licence.spdx, "CC-BY-4.0");
}

/// Cites: EMU-60, EMU-64
#[test]
fn a_broken_layout_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("t");
    std::fs::create_dir(&dir).unwrap();
    std::fs::copy(reference().join("trace.toml"), dir.join("trace.toml")).unwrap();
    assert_eq!(load(&dir).unwrap_err().reason, "layout");

    std::fs::copy(
        reference().join("provenance.toml"),
        dir.join("provenance.toml"),
    )
    .unwrap();
    std::fs::write(dir.join("notes.md"), "x").unwrap();
    assert_eq!(load(&dir).unwrap_err().reason, "layout");

    let upper = tmp.path().join("Upper");
    std::fs::create_dir(&upper).unwrap();
    assert_eq!(load(&upper).unwrap_err().reason, "layout");
}

/// Cites: EMU-60, EMU-62, EMU-63, EMU-64
#[test]
fn a_malformed_trace_is_refused_by_name() {
    let cases: [Case; 9] = [
        (
            "unknown key",
            Box::new(replace("loss = 0.0\n", "loss = 0.0\njitter_ms = 1.0\n")),
            "parse",
        ),
        (
            "coordinate",
            Box::new(replace("t_s = 25.0\n", "t_s = 25.0\nlat = 48.4\n")),
            "parse",
        ),
        (
            "schema",
            Box::new(replace("schema_version = 1", "schema_version = 2")),
            "parse",
        ),
        (
            "first t_s",
            Box::new(replace("t_s = 0.0", "t_s = 1.0")),
            "samples",
        ),
        (
            "decreasing t_s",
            Box::new(replace("t_s = 52.0", "t_s = 20.0")),
            "samples",
        ),
        (
            "min above avg",
            Box::new(replace("avg = 36.0, min = 14.0", "avg = 36.0, min = 40.0")),
            "range",
        ),
        (
            "loss above 1",
            Box::new(replace("loss = 0.0", "loss = 1.5")),
            "range",
        ),
        (
            "not finite",
            Box::new(replace("dl_kbps = 120858.0", "dl_kbps = inf")),
            "range",
        ),
        (
            "rtt below total loss missing",
            Box::new(replace(", min = 14.0, max = 57.0, stdev = 13.0 }", " }")),
            "parse",
        ),
    ];
    for (what, edit, reason) in cases {
        let e = load_edited("trace.toml", edit);
        assert_eq!(e.reason, reason, "{what}: {e}");
    }
}

/// Cites: EMU-62
#[test]
fn rtt_is_present_exactly_below_total_loss() {
    // A sample with loss below 1 and no rtt_ms.
    let e = load_edited(
        "trace.toml",
        replace(
            "rtt_ms = { avg = 36.0, min = 14.0, max = 57.0, stdev = 13.0 }\n",
            "",
        ),
    );
    assert_eq!(e.reason, "samples", "{e}");
    // A sample at total loss with an rtt_ms.
    let e = load_edited("trace.toml", |s| {
        let i = s.find("loss = 1.0\n").unwrap() + "loss = 1.0\n".len();
        format!(
            "{}rtt_ms = {{ avg = 1.0, min = 1.0, max = 1.0, stdev = 0.0 }}\n{}",
            &s[..i],
            &s[i..]
        )
    });
    assert_eq!(e.reason, "samples", "{e}");
}

/// Cites: EMU-61, EMU-64
#[test]
fn a_provenance_that_breaks_emu_61_is_refused_by_name() {
    let cases: [Case; 6] = [
        (
            "not redistributable",
            Box::new(replace("spdx = \"CC-BY-4.0\"", "spdx = \"CC-BY-NC-4.0\"")),
            "licence",
        ),
        (
            "missing field",
            Box::new(replace("mobility = \"drive\"\n", "")),
            "parse",
        ),
        (
            "unknown mobility",
            Box::new(replace("mobility = \"drive\"", "mobility = \"fly\"")),
            "provenance",
        ),
        (
            "empty attribution",
            Box::new(|s: &str| {
                let i = s.find("attribution = ").unwrap();
                let j = i + s[i..].find('\n').unwrap();
                format!("{}attribution = \"\"{}", &s[..i], &s[j..])
            }),
            "provenance",
        ),
        (
            "coordinates",
            Box::new(replace(
                "[collection]\n",
                "[collection]\nlatitude = 48.43\n",
            )),
            "parse",
        ),
        (
            "source hash",
            Box::new(|s: &str| {
                let i = s.rfind("blake3 = \"").unwrap() + "blake3 = \"".len();
                format!("{}xyz{}", &s[..i], &s[i + 3..])
            }),
            "provenance",
        ),
    ];
    for (what, edit, reason) in cases {
        let e = load_edited("provenance.toml", edit);
        assert_eq!(e.reason, reason, "{what}: {e}");
    }
}
