//! CON-31: compute `build_hash` and its components, and the `engine_hash` of the
//! frozen code being compiled (CON-28), and embed them in the binary. Reruns when
//! anything under `crates/` or any hashed build input changes.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use acn_trace::env;
use acn_trace::identity::{self, BuildParts, Digest};

fn main() {
    if let Err(e) = run() {
        println!("cargo::error=acn-cli build identity (CON-31): {e}");
    }
}

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

fn hash(root: &Path, rel: &str) -> Result<Digest, String> {
    identity::file_hash(&root.join(rel)).map_err(|e| e.to_string())
}

/// `acn-cli`'s enabled features as spelled in its manifest, `default` excluded,
/// sorted and comma-joined. Cargo reports a feature as `CARGO_FEATURE_<NAME>`,
/// upper-cased with `-` as `_`; the manifest has the spelling.
fn features(manifest: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(manifest).map_err(|e| e.to_string())?;
    let doc: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
    let mut on: Vec<String> = doc
        .get("features")
        .and_then(toml::Value::as_table)
        .map(|t| {
            t.keys()
                .filter(|k| *k != "default")
                .filter(|k| {
                    std::env::var_os(format!(
                        "CARGO_FEATURE_{}",
                        k.to_uppercase().replace('-', "_")
                    ))
                    .is_some()
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    on.sort();
    Ok(on.join(","))
}

fn run() -> Result<(), String> {
    let manifest_dir = PathBuf::from(var("CARGO_MANIFEST_DIR"));
    let Some(root) = manifest_dir.parent().and_then(Path::parent) else {
        return Err("acn-cli is not at crates/acn-cli under the workspace root".into());
    };
    for p in [
        "crates",
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        ".cargo/config.toml",
    ] {
        println!("cargo::rerun-if-changed={}", root.join(p).display());
    }
    println!("cargo::rerun-if-env-changed=CARGO_ENCODED_RUSTFLAGS");

    let source = env::source_files(root).map_err(|e| e.to_string())?;
    let target = var("TARGET");
    let profile = var("PROFILE");
    let features = features(&manifest_dir.join("Cargo.toml"))?;
    let rustflags = var("CARGO_ENCODED_RUSTFLAGS");
    let info = BuildParts {
        cargo_lock: hash(root, "Cargo.lock")?,
        rust_toolchain: hash(root, "rust-toolchain.toml")?,
        cargo_config: hash(root, ".cargo/config.toml")?,
        source_hash: env::tree_hash(&source).map_err(|e| e.to_string())?,
        target: &target,
        profile: &profile,
        features: &features,
        rustflags: &rustflags,
    }
    .info()
    .map_err(|e| e.to_string())?;
    let engine = env::compute(root).map_err(|e| e.to_string())?.engine_hash;

    // `rustc-env` values cannot hold the 0x1f separators of encoded rustflags, so
    // every string is embedded hex-encoded and decoded at run time.
    let hexs = |s: &str| s.bytes().map(|b| format!("{b:02x}")).collect::<String>();
    for (k, v) in [
        ("ACN_BUILD_HASH", info.build_hash.clone()),
        ("ACN_BUILD_CARGO_LOCK", info.cargo_lock.clone()),
        ("ACN_BUILD_RUST_TOOLCHAIN", info.rust_toolchain.clone()),
        ("ACN_BUILD_CARGO_CONFIG", info.cargo_config.clone()),
        ("ACN_BUILD_SOURCE_HASH", info.source_hash.clone()),
        ("ACN_BUILD_TARGET_HEX", hexs(&info.target)),
        ("ACN_BUILD_PROFILE_HEX", hexs(&info.profile)),
        ("ACN_BUILD_FEATURES_HEX", hexs(&info.features)),
        ("ACN_BUILD_RUSTFLAGS_HEX", hexs(&info.rustflags)),
        ("ACN_ENGINE_HASH", engine),
    ] {
        println!("cargo::rustc-env={k}={v}");
    }
    Ok(())
}
