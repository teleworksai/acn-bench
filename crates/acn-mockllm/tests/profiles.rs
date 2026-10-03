//! MLM-50: the shipped profiles load and state every parameter; a malformed
//! profile is refused.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_mockllm::profile::{self, CacheModel, Profiles};

/// Cites: MLM-50, MLM-51
#[test]
fn the_shipped_profiles_cover_the_three_models_and_are_placeholders() {
    let p = profile::embedded().unwrap();
    for (name, model) in [
        ("mock-explicit", CacheModel::ExplicitBreakpoints),
        ("mock-auto", CacheModel::AutomaticPrefix),
        ("mock-blocks", CacheModel::BlockGranular),
    ] {
        let pr = p.get(name).unwrap();
        assert_eq!(pr.cache_model, model);
        assert!(pr.placeholder, "{name} is uncalibrated");
    }
    assert_eq!(
        p.blake3,
        blake3::hash(profile::PROFILES_TOML.as_bytes())
            .to_hex()
            .to_string()
    );
}

/// Cites: MLM-50
#[test]
fn a_profile_must_state_everything_and_make_sense() {
    let good = common::profile_toml("x", "automatic_prefix", &[]);
    let parse = |t: &str| Profiles::parse(&format!("schema_version = 1\n{t}"));
    assert!(parse(&good).is_ok());
    let cases = [
        good.replace("itl_ns = 100\n", ""), // a missing parameter
        format!("{good}surprise = 1\n"),    // an unknown one
        format!("{good}\n{good}"),          // a name twice
        good.replace("output_tokens_min = 5", "output_tokens_min = 9"), // min above max
        good.replace("itl_jitter_ns = 0", "itl_jitter_ns = 100"), // jitter not below itl
        good.replace("\"system\", \"messages\"", "\"system\", \"system\""), // a segment twice
        good.replace("fault_429_ppm = 0", "fault_429_ppm = 999999")
            .replace("fault_500_ppm = 0", "fault_500_ppm = 2"),
        good.replace("\"automatic_prefix\"", "\"lru\""), // an unknown cache model
        good.replace("\"tools\",", "\"prompt\","),       // an unknown segment
        good.replace("ttl_ns = 1000000", "ttl_ns = 0"),  // a parameter that must be positive
        good.replace("itl_ns = 100", "itl_ns = 0"),
        good.replace("retry_after_s_max = 5", "retry_after_s_max = 0"),
        good.replace("prefill_base_ns = 1000", "prefill_base_ns = -1"), // a negative constant
    ];
    for bad in cases {
        assert!(parse(&bad).is_err(), "{bad}");
    }
    assert!(Profiles::parse(&format!("schema_version = 2\n{good}")).is_err());
}

/// Cites: MLM-50
#[test]
fn a_mock_checks_profiles_it_is_handed_directly() {
    let good = common::profile_toml("x", "automatic_prefix", &[]);
    let mut p = Profiles::parse(&format!("schema_version = 1\n{good}")).unwrap();
    p.profiles[0].retry_after_s_max = 0;
    assert!(acn_mockllm::Mock::with_profiles(p.clone(), 1).is_err());
    p.profiles[0].retry_after_s_max = 5;
    p.profiles[0].output_tokens_min = 9;
    assert!(acn_mockllm::Mock::with_profiles(p, 1).is_err());
}
