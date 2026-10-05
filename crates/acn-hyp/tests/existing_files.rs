//! HYP-16: the two files that existed when SPEC 080 was accepted load under it
//! unedited, with the parse trees the spec describes. Byte-for-byte copies are
//! pinned by BLAKE3 here, because a candidate may be edited by anyone and a
//! frozen crate's test must not break when it is; `hypotheses/p4.toml` is also
//! loaded from its real path.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::path::{Path, PathBuf};

use acn_hyp::Status;
use acn_hyp::file::{Control, Domain};

const P4_BLAKE3: &str = "03cb953a764a83909bc31df27968341f89ab9fee00f88bf1a14f192c461d73ae";
const P17_BLAKE3: &str = "fbfa0e1e9594842226c06b4b529dd621554781741903867efeeafa7138bd2832";

fn fixture(name: &str, pinned: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        blake3::hash(&bytes).to_hex().to_string(),
        pinned,
        "{name} is the pinned copy"
    );
    path
}

/// Cites: HYP-16
#[test]
fn p4_loads_unedited_with_its_predicate_and_guard() {
    let h = acn_hyp::load(&fixture("p4.toml", P4_BLAKE3)).unwrap();
    assert_eq!(h.id(), "p4");
    assert_eq!(
        h.status(),
        Status::Candidate,
        "the copy lies outside hypotheses/"
    );
    assert_eq!(
        h.predicate().to_string(),
        "(max_over_knobs(abs(effect(cost_per_success))) < noise_floor(cost_per_success, control, ci = 0.95))",
        "a comparison of two slice-level numbers"
    );
    assert_eq!(
        h.guard().unwrap().to_string(),
        "((replicates < 20) or (providers_reported < 2))"
    );
    assert!(!h.params()["provider"].pooled);
    assert_eq!(h.design().min_providers_for_verdict, Some(2));
    assert!(
        h.design().pins.is_none(),
        "unpinned: its verdicts are labelled until pins are added (HYP-23)"
    );
    assert!(matches!(h.control(), Control::Config(c) if c.len() == 6));
    assert!(h.warnings().is_empty(), "{:?}", h.warnings());

    // From its real path it is frozen, recorded in env-hash.json.
    let real = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../hypotheses/p4.toml");
    let h = acn_hyp::load(&real).unwrap();
    assert_eq!(h.status(), Status::Frozen);
    assert_eq!(h.hash().to_hex(), P4_BLAKE3);
}

/// Cites: HYP-16
#[test]
fn p17_loads_unedited_with_its_two_selects_and_at_clause() {
    let h = acn_hyp::load(&fixture("p17-a2a.toml", P17_BLAKE3)).unwrap();
    assert_eq!(h.id(), "p17", "the `<id>-<slug>` stem rule");
    assert_eq!(h.status(), Status::Candidate);
    assert_eq!(
        h.predicate().to_string(),
        "((network_attributable_share(protocol = a2a) - network_attributable_share(control)) < 0.05) at all rtt_ms <= 150",
        "two selects, the first fixing `protocol` by its unique value `a2a`, normalised to `pname = value`"
    );
    assert_eq!(h.param_of_value("a2a").unwrap().name, "protocol");
    assert!(matches!(
        h.params()["rtt_ms"].domain,
        Domain::Range { levels: None, .. }
    ));
    // The candidate allowances of HYP-7, HYP-8 and HYP-9.
    assert!(matches!(h.control(), Control::Missing));
    assert!(
        h.warnings()
            .iter()
            .any(|w| w.contains("network_attributable_share"))
    );
    assert_eq!(h.design().search, "bisect");
    assert_eq!(h.design().replicates, 10);
}

/// Cites: P4-4, P4-5, P4-1
#[test]
fn p4_names_the_spec_100_providers_and_workloads_and_pins_them_when_pinned() {
    use acn_hyp::file::Domain;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let h = acn_hyp::load(&root.join("hypotheses/p4.toml")).unwrap();
    let values = |name: &str| match &h.params()[name].domain {
        Domain::Enum(v) => v.clone(),
        d => panic!("{name}: {d:?}"),
    };
    assert_eq!(
        values("provider"),
        ["anthropic", "openai", "vllm", "sglang"]
    );
    assert_eq!(values("workload"), ["coding", "retrieval", "fanout"]);
    // P4-1: each value has its workload file; P4-4: once pinned, the pins are
    // exactly their hashes.
    let mut hashes: Vec<String> = ["coding", "retrieval", "fanout"]
        .iter()
        .map(|v| {
            let bytes = std::fs::read(root.join(format!("workloads/p4-{v}.toml"))).unwrap();
            blake3::hash(&bytes).to_hex().to_string()
        })
        .collect();
    hashes.sort();
    if let Some(pins) = &h.design().pins {
        let mut pinned = pins.workload.clone();
        pinned.sort();
        assert_eq!(pinned, hashes, "P4-4");
    }
}
