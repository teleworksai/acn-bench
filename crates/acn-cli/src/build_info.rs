//! The build identity this binary was compiled with (CON-31) and the `engine_hash`
//! of the frozen code it compiled (CON-28), embedded by `build.rs`.

use acn_trace::identity::{BuildInfo, Digest, IdentityError};

fn unhex(s: &str) -> Result<String, IdentityError> {
    let bytes: Option<Vec<u8>> = (0..s.len())
        .step_by(2)
        .map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect();
    bytes
        .and_then(|b| String::from_utf8(b).ok())
        .ok_or_else(|| {
            IdentityError::Invalid(format!("embedded build string `{s}` is not hex UTF-8"))
        })
}

/// The embedded build identity, checked: its `build_hash` must be the hash of its
/// components.
pub fn build_info() -> Result<BuildInfo, IdentityError> {
    let info = BuildInfo {
        build_hash: env!("ACN_BUILD_HASH").to_owned(),
        cargo_lock: env!("ACN_BUILD_CARGO_LOCK").to_owned(),
        rust_toolchain: env!("ACN_BUILD_RUST_TOOLCHAIN").to_owned(),
        cargo_config: env!("ACN_BUILD_CARGO_CONFIG").to_owned(),
        source_hash: env!("ACN_BUILD_SOURCE_HASH").to_owned(),
        target: unhex(env!("ACN_BUILD_TARGET_HEX"))?,
        profile: unhex(env!("ACN_BUILD_PROFILE_HEX"))?,
        features: unhex(env!("ACN_BUILD_FEATURES_HEX"))?,
        rustflags: unhex(env!("ACN_BUILD_RUSTFLAGS_HEX"))?,
    };
    info.check()?;
    Ok(info)
}

/// The embedded `engine_hash` (CON-28).
pub fn engine_hash() -> Result<Digest, IdentityError> {
    Digest::from_hex(env!("ACN_ENGINE_HASH"))
}
