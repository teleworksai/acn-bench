//! CON-31, CON-28: the binary embeds the build identity and the engine hash of the
//! code it was compiled from, and they are what the working tree says they are.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::path::Path;
use std::process::Command;

use acn_trace::env;
use acn_trace::identity::{self, BuildInfo};

fn version() -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_acn"))
        .arg("version")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// Cites: CON-28, CON-31
#[test]
fn the_embedded_engine_hash_is_the_checkouts() {
    let rec = env::compute(root()).unwrap();
    assert_eq!(version()["engine_hash"], rec.engine_hash.as_str());
}

/// Cites: CON-31, CON-27
#[test]
fn the_embedded_build_identity_is_this_trees_and_hashes_to_build_hash() {
    let v = version();
    let b: BuildInfo = serde_json::from_value(v["build"].clone()).unwrap();
    assert_eq!(v["build_hash"], b.build_hash.as_str());
    b.check().expect("build_hash is the hash of its components");
    let h = |rel: &str| identity::file_hash(&root().join(rel)).unwrap().to_hex();
    assert_eq!(b.cargo_lock, h("Cargo.lock"));
    assert_eq!(b.rust_toolchain, h("rust-toolchain.toml"));
    assert_eq!(b.cargo_config, h(".cargo/config.toml"));
    let source = env::tree_hash(&env::source_files(root()).unwrap()).unwrap();
    assert_eq!(
        b.source_hash,
        source.to_hex(),
        "a stale binary or a missed input"
    );
    assert!(!b.target.is_empty() && !b.profile.is_empty());
    assert_eq!(b.features, "", "acn-cli declares no features yet");
}
