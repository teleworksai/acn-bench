//! `cargo xtask import-5g-iana` (SPEC 020 EMU-65). The fixture is four
//! placemarks of the published PING file (the first, two with a ping test and
//! one with total loss), with coordinates and cell identifiers zeroed: the
//! published structure without the identifiers EMU-63 keeps out of the repo.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::{Path, PathBuf};

use common::{repo_root, xtask};
use xtask::import_5g_iana::{SOURCE_NAME, TOOL_PATH, convert};

const TRACE_DIR: &str = "scenarios/measured/5g-iana-2023-01-29";

fn excerpt() -> PathBuf {
    common::fixture("5g_iana/PING-excerpt.kml")
}

fn hash(path: &Path) -> String {
    blake3::hash(&std::fs::read(path).unwrap())
        .to_hex()
        .to_string()
}

/// A trace directory whose provenance records `source_hash` for the source and
/// the real tool's hash.
fn trace_dir(source_hash: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let tool = hash(&repo_root().join(TOOL_PATH));
    std::fs::write(
        tmp.path().join("provenance.toml"),
        format!(
            "[conversion]\ntool_path = \"{TOOL_PATH}\"\ntool_blake3 = \"{tool}\"\n\
             [[conversion.sources]]\nname = \"{SOURCE_NAME}\"\nblake3 = \"{source_hash}\"\n"
        ),
    )
    .unwrap();
    tmp
}

const EXPECTED: &str = "\
# A measured trace (SPEC 020 EMU-62), written by `cargo xtask import-5g-iana`
# from PING.kml of Zenodo record 12664724. Do not edit: see provenance.toml.
schema_version = 1
sample_interval_s = 25.0

[[sample]]
t_s = 0.0
loss = 0.0
dl_kbps = 120858.0
ul_kbps = 2403.0
rtt_ms = { avg = 36.0, min = 14.0, max = 57.0, stdev = 13.0 }

[[sample]]
t_s = 25.0
loss = 0.0
dl_kbps = 100234.0
ul_kbps = 1761.0
rtt_ms = { avg = 42.0, min = 15.0, max = 61.0, stdev = 13.0 }

[[sample]]
t_s = 79.0
loss = 1.0
dl_kbps = 18.0
ul_kbps = 381.0
";

/// Cites: EMU-62, EMU-63, EMU-65
#[test]
fn the_excerpt_converts_to_the_documented_samples() {
    let kml = std::fs::read_to_string(excerpt()).unwrap();
    let c = convert(&kml).unwrap();
    assert_eq!(c.trace, EXPECTED);
    assert_eq!((c.samples, c.skipped), (3, 1));
    // Nothing of the identifying fields survives.
    for gone in [
        "coordinates",
        "CELL",
        "2023.01.29",
        "MAX PING\"",
        "TEST DL MAX",
    ] {
        assert!(!c.trace.contains(gone), "{gone} leaked into the trace");
    }
}

/// Cites: EMU-65
#[test]
fn a_value_off_the_published_shape_is_refused() {
    let kml = std::fs::read_to_string(excerpt()).unwrap();
    for (from, to) in [
        ("36 ms", "36 s"),
        ("0 %", "zero %"),
        ("2023.01.29_12.06.55", "2023.01.29_12.06.20"),
    ] {
        assert!(kml.contains(from), "{from}");
        let bad = kml.replacen(from, to, 1);
        assert!(convert(&bad).is_err(), "{from} -> {to} was accepted");
    }
}

/// Cites: EMU-65
#[test]
fn the_importer_writes_then_checks_its_output() {
    let dir = trace_dir(&hash(&excerpt()));
    let d = dir.path().to_str().unwrap();
    let ping = excerpt();
    let p = ping.to_str().unwrap();
    let r = xtask(&["import-5g-iana", "--ping", p, "--dir", d]);
    assert!(r.ok(), "{}", r.json);
    assert_eq!(r.json["samples"], 3);
    let written = std::fs::read_to_string(dir.path().join("trace.toml")).unwrap();
    assert_eq!(written, EXPECTED);
    assert_eq!(
        r.json["hash"],
        blake3::hash(EXPECTED.as_bytes()).to_hex().as_str()
    );

    let r = xtask(&["import-5g-iana", "--ping", p, "--dir", d, "--check"]);
    assert!(r.ok(), "{}", r.json);

    std::fs::write(
        dir.path().join("trace.toml"),
        written.replace("36.0", "35.0"),
    )
    .unwrap();
    let r = xtask(&["import-5g-iana", "--ping", p, "--dir", d, "--check"]);
    assert!(!r.ok());
    assert!(
        r.json["error"]
            .as_str()
            .unwrap()
            .contains("not this tool's output")
    );
}

/// Cites: EMU-65
#[test]
fn a_source_with_another_hash_is_refused() {
    let dir = trace_dir(&"0".repeat(64));
    let ping = excerpt();
    let r = xtask(&[
        "import-5g-iana",
        "--ping",
        ping.to_str().unwrap(),
        "--dir",
        dir.path().to_str().unwrap(),
    ]);
    assert!(!r.ok());
    assert!(
        r.json["error"]
            .as_str()
            .unwrap()
            .contains("not the 0000000000000000000000000000000000000000000000000000000000000000")
    );
    assert!(!dir.path().join("trace.toml").exists());
}

/// Cites: EMU-61, EMU-65
#[test]
fn the_committed_provenance_names_this_tool_by_its_hash() {
    let root = repo_root();
    let prov: toml::Value = toml::from_str(
        &std::fs::read_to_string(root.join(TRACE_DIR).join("provenance.toml")).unwrap(),
    )
    .unwrap();
    let c = &prov["conversion"];
    assert_eq!(c["tool_path"].as_str(), Some(TOOL_PATH));
    assert_eq!(
        c["tool_blake3"].as_str(),
        Some(hash(&root.join(TOOL_PATH)).as_str()),
        "the importer changed: re-run it on the source and record its new hash"
    );
}

/// Run on the published source (download it from the record and set
/// `ACN_5G_IANA_PING` to its path): the committed trace is the tool's output.
///
/// Cites: EMU-65
#[test]
#[ignore = "live: needs the published source file"]
fn the_committed_trace_is_the_importers_output_on_the_source() {
    let ping = std::env::var("ACN_5G_IANA_PING").expect("ACN_5G_IANA_PING");
    let dir = repo_root().join(TRACE_DIR);
    let r = xtask(&[
        "import-5g-iana",
        "--ping",
        &ping,
        "--dir",
        dir.to_str().unwrap(),
        "--check",
    ]);
    assert!(r.ok(), "{}", r.json);
    assert_eq!(r.json["samples"], 198);
}
