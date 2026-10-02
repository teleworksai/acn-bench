//! CON-27, CON-29, CON-30: the canonical encodings, run identity and seeds, pinned by
//! known-answer vectors. The vectors were computed outside this crate, from the
//! preimage layout as the constitution states it, so a test here cannot agree with
//! the implementation merely because it shares its mistakes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeMap;

use acn_trace::identity::{self, Digest, HypStatus, Mode, Preimage, RunIdentity, RunParams, Value};
use acn_trace::schema;
use rand_core::Rng as _;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Cites: CON-30, CON-27
#[test]
fn replicate_seed_matches_its_known_answer() {
    let p = Preimage::new("acn-bench/replicate/v1")
        .unwrap()
        .u64(42)
        .u32(3);
    assert_eq!(
        hex(p.bytes()),
        "61636e2d62656e63682f7265706c69636174652f7631002a0000000000000003000000",
        "context bare and zero-terminated, seed u64 LE, index u32 LE"
    );
    assert_eq!(
        identity::replicate_seed(42, 3).unwrap(),
        531_995_386_567_255_885
    );
}

/// Cites: CON-30, CON-27, CON-5
#[test]
fn a_named_substream_matches_its_known_answer_and_its_first_raw_outputs() {
    let s = identity::replicate_seed(42, 3).unwrap();
    let p = Preimage::new("acn-bench/rng/v1")
        .unwrap()
        .u64(s)
        .str("trace.ids")
        .unwrap();
    assert_eq!(
        hex(p.bytes()),
        "61636e2d62656e63682f726e672f7631004de77c46fc0662070900000074726163652e696473",
        "the name is a length-prefixed string and comes last"
    );
    let seed = identity::substream_seed(s, "trace.ids").unwrap();
    assert_eq!(
        hex(&seed),
        "35c6ef92aa427865bd0a8e44f4ac210540393d9e305fd76f047445ff4013eaa5"
    );
    // CON-5(a) golden vector: the first raw outputs of the generator, computed from
    // the ChaCha20 keystream (zero nonce, counter 0) independently of rand_chacha.
    let mut rng = identity::substream_rng(s, "trace.ids").unwrap();
    assert_eq!(
        [rng.next_u64(), rng.next_u64(), rng.next_u64()],
        [
            1_931_434_202_297_228_593,
            1_639_166_351_897_295_282,
            10_337_616_115_050_100_999
        ]
    );
}

/// Cites: CON-30
#[test]
fn adding_a_component_never_shifts_another_components_stream() {
    let a = identity::substream_seed(7, "emu.link0").unwrap();
    let b = identity::substream_seed(7, "emu.link1").unwrap();
    assert_ne!(a, b);
    assert_eq!(a, identity::substream_seed(7, "emu.link0").unwrap());
    assert!(identity::substream_seed(7, "").is_err());
    assert!(identity::substream_seed(7, "has space").is_err());
    assert!(identity::substream_seed(7, "nön-ascii").is_err());
}

/// Cites: CON-30
#[test]
fn every_derived_seed_fits_a_signed_64_bit_field() {
    for i in 0..2000 {
        let s = identity::replicate_seed(u64::MAX - u64::from(i), i).unwrap();
        assert!(i64::try_from(s).is_ok(), "{s} does not fit i64");
    }
    assert_eq!(
        identity::derived_seed(&Digest([0xff; 32])),
        i64::MAX.cast_unsigned()
    );
}

fn known_params() -> RunParams {
    RunParams {
        backend: "mockllm".into(),
        model: "mock-default".into(),
        hyp_status: HypStatus::Candidate,
        arms: vec!["treatment".into(), "control".into()],
        replicates: 20,
        vary: BTreeMap::from([
            ("timestamp_in_system_prompt".into(), Value::Bool(true)),
            ("compaction_threshold".into(), Value::Float(0.75)),
        ]),
        opts: BTreeMap::from([
            ("opt.stall_threshold_ms".into(), Value::Float(500.0)),
            // Equal to its default: it must not enter the pairs.
            ("opt.keep_content".into(), Value::Bool(false)),
        ]),
    }
}

/// Cites: CON-29, CON-27
#[test]
fn params_hash_and_run_id_match_their_known_answers() {
    let inv = schema::inventory().unwrap();
    let pairs = known_params().pairs(&identity::options(&inv)).unwrap();
    assert_eq!(
        pairs.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "arms",
            "backend",
            "hyp_status",
            "model",
            "opt.stall_threshold_ms",
            "replicates",
            "vary.compaction_threshold",
            "vary.timestamp_in_system_prompt"
        ],
        "bytewise key order; an option at its default is left out"
    );
    assert_eq!(pairs["arms"], "control,treatment", "sorted, comma-joined");
    let params_hash = identity::params_hash(&pairs).unwrap();
    assert_eq!(
        params_hash.to_hex(),
        "86c66c2e75fe32ebab800718a348e67b2b260d3947659357051bbfe1f9375660",
        "a float-valued parameter is part of the vector (CON-27(d))"
    );
    let id = RunIdentity {
        seed: 42,
        scenario_hash: Digest::of(b"scenario"),
        workload_hash: Digest::of(b"workload"),
        hypothesis_hash: Digest::ZERO,
        engine_hash: Digest::of(b"engine"),
        mode: Mode::Sim,
        params_hash,
    };
    assert_eq!(
        id.run_id().unwrap().to_hex(),
        "fe17f66dfbef6b04b0ceeb3fbadce38852c69b01236255ac62ad629065382e2b"
    );
}

/// Cites: CON-29
#[test]
fn every_identity_input_moves_the_run_id() {
    let base = RunIdentity {
        seed: 1,
        scenario_hash: Digest::of(b"s"),
        workload_hash: Digest::of(b"w"),
        hypothesis_hash: Digest::ZERO,
        engine_hash: Digest::of(b"e"),
        mode: Mode::Sim,
        params_hash: Digest::of(b"p"),
    };
    let id = base.run_id().unwrap();
    let variants = [
        RunIdentity { seed: 2, ..base },
        RunIdentity {
            scenario_hash: Digest::of(b"s2"),
            ..base
        },
        RunIdentity {
            workload_hash: Digest::of(b"w2"),
            ..base
        },
        RunIdentity {
            hypothesis_hash: Digest::of(b"h"),
            ..base
        },
        RunIdentity {
            engine_hash: Digest::of(b"e2"),
            ..base
        },
        RunIdentity {
            mode: Mode::Live,
            ..base
        },
        RunIdentity {
            params_hash: Digest::of(b"p2"),
            ..base
        },
    ];
    for v in variants {
        assert_ne!(v.run_id().unwrap(), id, "{v:?}");
    }
}

/// Cites: CON-29
#[test]
fn run_options_are_read_from_the_inventory_and_checked() {
    let inv = schema::inventory().unwrap();
    let options = identity::options(&inv);
    let mut p = known_params();
    p.opts = BTreeMap::from([("opt.not_listed".into(), Value::Bool(true))]);
    assert!(p.pairs(&options).is_err(), "an unlisted option is an error");
    p.opts = BTreeMap::from([("opt.keep_content".into(), Value::Str("yes".into()))]);
    assert!(
        p.pairs(&options).is_err(),
        "a value of the wrong type is an error"
    );
    // An integer literal for a float option is that float: 250 is the default 250.0.
    p.opts = BTreeMap::from([("opt.stall_threshold_ms".into(), Value::Int(250))]);
    assert!(
        !p.pairs(&options)
            .unwrap()
            .contains_key("opt.stall_threshold_ms")
    );
    p.opts = BTreeMap::from([("opt.keep_content".into(), Value::Bool(true))]);
    assert_eq!(p.pairs(&options).unwrap()["opt.keep_content"], "true");
}

/// Cites: CON-29
#[test]
fn malformed_parameters_are_errors_not_hashes() {
    let inv = schema::inventory().unwrap();
    let options = identity::options(&inv);
    let mut p = known_params();
    p.arms = vec!["control".into(), "control".into()];
    assert!(p.pairs(&options).is_err(), "an arm named twice");
    p.arms = vec!["researcher".into()];
    assert!(p.pairs(&options).is_err(), "not an arm");
    p.arms = vec![];
    assert!(p.pairs(&options).is_err(), "no arm");
    let mut p = known_params();
    p.replicates = 0;
    assert!(p.pairs(&options).is_err());
    let mut p = known_params();
    p.backend = String::new();
    assert!(p.pairs(&options).is_err());
    let mut p = known_params();
    p.vary.insert("Bad Name".into(), Value::Int(1));
    assert!(p.pairs(&options).is_err());
}

/// Cites: CON-27
#[test]
fn numbers_are_written_in_one_text_form() {
    for (v, text) in [
        (150.0, "150.0"),
        (0.05, "0.05"),
        (1e21, "1e21"),
        (1e-7, "1e-7"),
        (5.0, "5.0"),
        (-2.5, "-2.5"),
        (-0.0, "0.0"),
    ] {
        assert_eq!(identity::float_text(v).unwrap(), text);
    }
    // CON-27(c) names ryu's form. serde_json 1.0.151 no longer emits it for every
    // value (it writes `1e+21`), so no float reaches a hashed or manifest text
    // through serde_json's formatter (ADR-13).
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(identity::float_text(bad).is_err(), "{bad} must be rejected");
    }
    assert_eq!(Value::Int(-3).to_text().unwrap(), "-3");
    assert_eq!(Value::Int(0).to_text().unwrap(), "0");
    assert_eq!(Value::Bool(false).to_text().unwrap(), "false");
}

/// Cites: CON-27
#[test]
fn digests_have_one_text_form_and_contexts_are_checked() {
    let d = Digest::of(b"abc");
    assert_eq!(
        d.to_hex(),
        "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85",
        "plain BLAKE3, not keyed or derive_key"
    );
    assert_eq!(Digest::from_hex(&d.to_hex()).unwrap(), d);
    assert!(Digest::from_hex(&d.to_hex().to_uppercase()).is_err());
    assert!(Digest::from_hex("abc").is_err());
    assert!(Preimage::new("").is_err());
    assert!(Preimage::new("bad\0context").is_err());
}

/// Cites: CON-27
#[test]
fn a_file_hash_is_of_the_working_tree_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("h.toml");
    std::fs::write(&path, b"abc").unwrap();
    assert_eq!(identity::file_hash(&path).unwrap(), Digest::of(b"abc"));
    assert!(identity::file_hash(&dir.path().join("missing")).is_err());
}

/// Cites: CON-27
#[test]
fn hashed_input_directories_are_checked_out_byte_for_byte() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(root.join(".gitattributes"))
        .expect("CON-27(a): the repository carries a .gitattributes");
    let rules: Vec<Vec<&str>> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split_whitespace().collect())
        .collect();
    for dir in ["hypotheses/**", "lab/hypotheses/**", "scenarios/**"] {
        assert!(
            rules
                .iter()
                .any(|r| r[0] == dir && r[1..].contains(&"-text")),
            "{dir} must be marked -text"
        );
    }
}

/// Cites: CON-29
#[test]
fn modes_and_statuses_have_one_spelling() {
    for m in [Mode::Sim, Mode::Live, Mode::Netem] {
        assert_eq!(Mode::parse(m.as_str()).unwrap(), m);
    }
    assert_eq!(
        [Mode::Sim.byte(), Mode::Live.byte(), Mode::Netem.byte()],
        [0, 1, 2]
    );
    assert!(Mode::parse("Sim").is_err());
    for s in [HypStatus::Frozen, HypStatus::Candidate] {
        assert_eq!(HypStatus::parse(s.as_str()).unwrap(), s);
    }
}

/// Cites: CON-31, CON-27
#[test]
fn build_hash_matches_its_known_answer() {
    // Computed outside this crate from the CON-31 preimage; the rustflags carry the
    // 0x1f separator of CARGO_ENCODED_RUSTFLAGS and the features are non-empty.
    let parts = acn_trace::identity::BuildParts {
        cargo_lock: Digest::of(b"l"),
        rust_toolchain: Digest::of(b"t"),
        cargo_config: Digest::of(b"c"),
        source_hash: Digest::of(b"s"),
        target: "x86_64-unknown-linux-gnu",
        profile: "release",
        features: "a,b",
        rustflags: "-C\u{1f}opt-level=3",
    };
    assert_eq!(
        parts.build_hash().unwrap().to_hex(),
        "2c483389c9934360f5c36be1b2d8e17b64cce756cbb3fd9f794ac73c5955aa81"
    );
    let info = parts.info().unwrap();
    assert_eq!(info.check().unwrap().to_hex(), info.build_hash);
    let mut forged = info.clone();
    forged.rustflags = String::new();
    assert!(forged.check().is_err(), "a component that moved");
}
