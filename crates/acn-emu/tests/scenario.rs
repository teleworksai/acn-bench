//! Synthetic scenarios (SPEC 020 §3): every committed scenario loads and
//! builds, and each malformed variant is refused by name.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use acn_emu::link::{
    Delay, Direction, LinkModel as _, LinkSpec, Loss, OutageCause, OutageMode, Rate, Reorder,
    Window,
};
use acn_emu::scenario::load;

fn synthetic() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios/synthetic")
}

/// Write `text` as `<tmp>/<name>.toml` and load it.
fn load_text(
    name: &str,
    text: &str,
) -> Result<acn_emu::scenario::Scenario, acn_emu::scenario::ScenarioError> {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(format!("{name}.toml"));
    std::fs::write(&path, text).unwrap();
    load(&path)
}

/// Cites: EMU-20, EMU-21
#[test]
fn every_committed_scenario_loads_and_builds() {
    let mut n = 0;
    for e in std::fs::read_dir(synthetic()).unwrap() {
        let path = e.unwrap().path();
        if path.extension().and_then(|x| x.to_str()) != Some("toml") {
            continue;
        }
        let s = load(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(
            s.hash,
            blake3::hash(&std::fs::read(&path).unwrap())
                .to_hex()
                .as_str()
        );
        let mut links = s.build(3).unwrap();
        assert_eq!(links.len(), s.links.len());
        for l in &mut links {
            l.transmit(0, 1_000).unwrap();
        }
        n += 1;
    }
    assert!(n >= 2, "fewer than two synthetic scenarios");
}

/// Cites: EMU-20, EMU-21
#[test]
fn the_cellular_scenario_reads_as_written() {
    let s = load(&synthetic().join("cellular-handover.toml")).unwrap();
    assert_eq!(s.name, "cellular-handover");
    let window = |mode| Window {
        start_ns: 10_000_000_000,
        end_ns: 10_300_000_000,
        mode,
        cause: OutageCause::Handover,
    };
    let mut up = LinkSpec::new("radio", Direction::Up);
    up.outage = Some(vec![window(OutageMode::Hold)]);
    up.loss = Some(Loss::GilbertElliott {
        p_good_bad_ppm: 5_000,
        p_bad_good_ppm: 200_000,
        loss_good_ppm: 1_000,
        loss_bad_ppm: 300_000,
    });
    up.rate = Some(Rate {
        rate_bps: 20_000_000,
        burst_bytes: 64_000,
        queue_bytes: 1_000_000,
    });
    up.delay = Some(Delay {
        delay_ns: 25_000_000,
        jitter_ns: 8_000_000,
    });
    up.reorder = Some(Reorder {
        reorder_ppm: 2_000,
        gap_ns: 15_000_000,
    });
    let mut down = LinkSpec::new("radio", Direction::Down);
    down.outage = Some(vec![window(OutageMode::Drop)]);
    down.loss = Some(Loss::Iid { loss_ppm: 2_000 });
    down.rate = Some(Rate {
        rate_bps: 100_000_000,
        burst_bytes: 256_000,
        queue_bytes: 4_000_000,
    });
    down.delay = Some(Delay {
        delay_ns: 25_000_000,
        jitter_ns: 5_000_000,
    });
    assert_eq!(s.links, vec![up, down]);
}

/// Cites: EMU-20, EMU-21
#[test]
fn every_key_of_the_base_maps_to_its_field() {
    let s = load_text("t", BASE).unwrap();
    let mut l = LinkSpec::new("radio", Direction::Up);
    l.loss = Some(Loss::Iid { loss_ppm: 1000 });
    l.rate = Some(Rate {
        rate_bps: 1_000_000,
        burst_bytes: 1000,
        queue_bytes: 10000,
    });
    l.delay = Some(Delay {
        delay_ns: 1_000_000,
        jitter_ns: 100_000,
    });
    l.reorder = Some(Reorder {
        reorder_ppm: 10,
        gap_ns: 50_000,
    });
    l.outage = Some(vec![Window {
        start_ns: 10_000_000,
        end_ns: 20_000_000,
        mode: OutageMode::Drop,
        cause: OutageCause::Scheduled,
    }]);
    assert_eq!(s.links, vec![l]);
}

const BASE: &str = r#"schema_version = 1
name = "t"

[[link]]
name = "radio"
direction = "up"

[link.loss]
kind = "iid"
loss_ppm = 1000

[link.rate]
rate_kbps = 1000
burst_bytes = 1000
queue_bytes = 10000

[link.delay]
delay_us = 1000
jitter_us = 100

[link.reorder]
reorder_ppm = 10
gap_us = 50

[[link.outage.window]]
start_ms = 10
end_ms = 20
mode = "drop"
cause = "scheduled"
"#;

/// Cites: EMU-20, EMU-21, EMU-22
#[test]
fn each_malformed_scenario_is_refused_by_name() {
    assert!(load_text("t", BASE).is_ok());
    let r = |from: &str, to: &str| {
        assert!(BASE.contains(from), "{from}");
        BASE.replacen(from, to, 1)
    };
    let window2 = "\n[[link.outage.window]]\nstart_ms = 15\nend_ms = 30\nmode = \"hold\"\ncause = \"handover\"\n";
    let cases: Vec<(&str, String, &str, &str)> = vec![
        (
            "unknown key",
            r("jitter_us = 100", "jitter_us = 100\njitter_ms = 1"),
            "t",
            "parse",
        ),
        (
            "unknown stage",
            format!("{BASE}\n[link.corrupt]\nppm = 1\n"),
            "t",
            "parse",
        ),
        (
            "schema",
            r("schema_version = 1", "schema_version = 2"),
            "t",
            "parse",
        ),
        ("stem", BASE.to_owned(), "other", "name"),
        (
            "link name",
            r("name = \"radio\"", "name = \"Radio\""),
            "t",
            "name",
        ),
        (
            "direction",
            r("direction = \"up\"", "direction = \"sideways\""),
            "t",
            "parse",
        ),
        (
            "no links",
            "schema_version = 1\nname = \"t\"\nlink = []\n".to_owned(),
            "t",
            "parse",
        ),
        (
            "duplicate",
            format!("{BASE}\n[[link]]\nname = \"radio\"\ndirection = \"up\"\n"),
            "t",
            "duplicate",
        ),
        (
            "ppm above a million",
            r("loss_ppm = 1000", "loss_ppm = 1000001"),
            "t",
            "range",
        ),
        (
            "zero rate",
            r("rate_kbps = 1000", "rate_kbps = 0"),
            "t",
            "range",
        ),
        (
            "zero burst",
            r("burst_bytes = 1000", "burst_bytes = 0"),
            "t",
            "range",
        ),
        (
            "zero queue",
            r("queue_bytes = 10000", "queue_bytes = 0"),
            "t",
            "range",
        ),
        ("zero gap", r("gap_us = 50", "gap_us = 0"), "t", "range"),
        (
            "empty window",
            r("end_ms = 20", "end_ms = 10"),
            "t",
            "range",
        ),
        (
            "overlapping windows",
            format!("{BASE}{window2}"),
            "t",
            "range",
        ),
        (
            "iid with a burst key",
            r("loss_ppm = 1000", "loss_ppm = 1000\nloss_bad_ppm = 5"),
            "t",
            "parse",
        ),
        (
            "burst without its keys",
            r(
                "kind = \"iid\"\nloss_ppm = 1000",
                "kind = \"gilbert_elliott\"\np_good_bad_ppm = 1",
            ),
            "t",
            "parse",
        ),
        (
            "unknown loss kind",
            r("kind = \"iid\"", "kind = \"bursty\""),
            "t",
            "parse",
        ),
        (
            "trace",
            format!("{BASE}\n[link.trace]\ndir = \"scenarios/measured/x\"\nblake3 = \"00\"\n"),
            "t",
            "trace",
        ),
    ];
    for (what, text, stem, reason) in cases {
        let e = load_text(stem, &text).expect_err(what);
        assert_eq!(e.reason, reason, "{what}: {e}");
    }
    assert_eq!(
        load(Path::new("/nonexistent/t.toml")).unwrap_err().reason,
        "layout"
    );
}
