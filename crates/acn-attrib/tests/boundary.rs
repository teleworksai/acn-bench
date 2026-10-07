//! SPEC 090 ATR-22: what `acn-attrib` may depend on, and which way.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeSet;
use std::path::Path;

fn deps(manifest: &Path) -> BTreeSet<String> {
    let text = std::fs::read_to_string(manifest).unwrap();
    let t: toml::Table = toml::from_str(&text).unwrap();
    t.get("dependencies")
        .and_then(|d| d.as_table())
        .map(|d| d.keys().cloned().collect())
        .unwrap_or_default()
}

/// Cites: ATR-22
#[test]
fn attribution_depends_on_the_trace_crate_alone_and_the_verdict_on_it() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let own = deps(&here.join("Cargo.toml"));
    let workspace: BTreeSet<&str> = own
        .iter()
        .map(String::as_str)
        .filter(|d| d.starts_with("acn-"))
        .collect();
    assert_eq!(workspace, BTreeSet::from(["acn-trace"]), "{own:?}");
    // Nothing that draws or reaches the network, and no build script.
    assert!(!own.contains("plotters"), "{own:?}");
    assert!(!here.join("build.rs").exists());
    // The verdict depends on attribution, never the other way.
    let hyp = deps(&here.join("../acn-hyp/Cargo.toml"));
    assert!(hyp.contains("acn-attrib"), "{hyp:?}");
}
