//! Static checks on the workspace itself: layout (CON-6), toolchain and
//! edition (CON-2), lint posture (CON-19, CON-5), the gate chain (CON-9) and
//! dependency independence (CON-20).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::fs;

use common::repo_root;

const CRATES: &[&str] = &[
    "acn-trace",
    "acn-emu",
    "acn-mockllm",
    "acn-harness",
    "acn-gen",
    "acn-replay",
    "acn-ctl",
    "acn-hyp",
    "acn-attrib",
    "acn-cli",
    "xtask",
];

const DIRS: &[&str] = &[
    "specs",
    "hypotheses",
    "scenarios/synthetic",
    "scenarios/measured",
    "tests/accept",
    "tools",
    "docs/decisions",
    "docs/gates",
    "docs/generated",
    "docs/lab",
];

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn toml_of(rel: &str) -> toml::Value {
    toml::from_str(&read(rel)).unwrap_or_else(|e| panic!("parse {rel}: {e}"))
}

fn crate_manifests() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = CRATES
        .iter()
        .map(|c| (format!("crates/{c}"), format!("crates/{c}/Cargo.toml")))
        .collect();
    v.push((
        "tests/accept".to_owned(),
        "tests/accept/Cargo.toml".to_owned(),
    ));
    v
}

/// Cites: CON-6
#[test]
fn layout_matches_the_constitution() {
    let root = repo_root();
    for d in DIRS {
        assert!(root.join(d).is_dir(), "missing directory {d}");
    }
    for c in CRATES {
        assert!(
            root.join("crates").join(c).join("Cargo.toml").is_file(),
            "missing crate {c}"
        );
    }
    let gitignore = read(".gitignore");
    assert!(
        gitignore
            .lines()
            .any(|l| l.trim() == "/runs/" || l.trim() == "runs/"),
        "runs/ must be gitignored"
    );
}

/// Cites: CON-6
#[test]
fn workspace_members_are_exactly_the_layout() {
    let ws = toml_of("Cargo.toml");
    let mut members: Vec<String> = ws["workspace"]["members"]
        .as_array()
        .expect("members")
        .iter()
        .map(|m| m.as_str().expect("str").to_owned())
        .collect();
    members.sort();
    let mut expected: Vec<String> = CRATES.iter().map(|c| format!("crates/{c}")).collect();
    expected.push("tests/accept".to_owned());
    expected.sort();
    assert_eq!(members, expected);
    let accept = toml_of("tests/accept/Cargo.toml");
    assert_eq!(accept["package"]["name"].as_str(), Some("acn-accept"));
}

/// Cites: CON-2
#[test]
fn toolchain_is_pinned_stable_and_edition_2024() {
    let tc = toml_of("rust-toolchain.toml");
    let channel = tc["toolchain"]["channel"].as_str().expect("channel");
    let parts: Vec<&str> = channel.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "channel must be a pinned X.Y.Z, got {channel}"
    );
    assert!(
        parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())),
        "{channel}"
    );

    let ws = toml_of("Cargo.toml");
    assert_eq!(ws["workspace"]["package"]["edition"].as_str(), Some("2024"));
    let msrv = ws["workspace"]["package"]["rust-version"]
        .as_str()
        .expect("rust-version");
    assert!(
        channel.starts_with(msrv),
        "MSRV {msrv} must equal the pinned channel {channel}"
    );
    for (_, manifest) in crate_manifests() {
        let m = toml_of(&manifest);
        assert_eq!(
            m["package"]["edition"]["workspace"].as_bool(),
            Some(true),
            "{manifest}: edition must inherit from the workspace"
        );
    }
}

/// Cites: CON-19
#[test]
fn every_crate_forbids_unsafe_and_inherits_the_lint_posture() {
    let ws = toml_of("Cargo.toml");
    let lints = &ws["workspace"]["lints"];
    assert_eq!(lints["rust"]["unsafe_code"].as_str(), Some("forbid"));
    for lint in ["unwrap_used", "expect_used", "panic"] {
        assert_eq!(
            lints["clippy"][lint].as_str(),
            Some("deny"),
            "clippy::{lint} must be denied"
        );
    }
    for (dir, manifest) in crate_manifests() {
        let m = toml_of(&manifest);
        assert_eq!(
            m["lints"]["workspace"].as_bool(),
            Some(true),
            "{manifest}: [lints] workspace = true"
        );
        let root = repo_root().join(&dir);
        let mut roots = vec![root.join("src/lib.rs"), root.join("src/main.rs")];
        roots.retain(|p| p.is_file());
        assert!(!roots.is_empty(), "{dir}: no src/lib.rs or src/main.rs");
        for r in roots {
            let src = fs::read_to_string(&r).expect("read");
            assert!(
                src.contains("#![forbid(unsafe_code)]"),
                "{} must carry #![forbid(unsafe_code)]",
                r.display()
            );
        }
    }
}

/// Cites: CON-5
#[test]
fn clippy_disallows_ambient_time_and_randomness() {
    // Clause (b) of CON-5: the lint configuration that bans ambient clocks and RNGs.
    let cfg = toml_of("clippy.toml");
    let paths: Vec<&str> = cfg["disallowed-methods"]
        .as_array()
        .expect("disallowed-methods")
        .iter()
        .map(|e| e["path"].as_str().expect("path"))
        .collect();
    for p in [
        "std::time::Instant::now",
        "std::time::SystemTime::now",
        "tokio::time::Instant::now",
        "rand::thread_rng",
        "std::thread::sleep",
    ] {
        assert!(paths.contains(&p), "clippy.toml must disallow {p}");
    }
    let ws = toml_of("Cargo.toml");
    assert_eq!(
        ws["workspace"]["lints"]["clippy"]["disallowed_methods"].as_str(),
        Some("deny")
    );
}

/// Cites: CON-9
#[test]
fn ci_script_chains_the_gates_in_order() {
    let script = read("tools/ci.sh");
    let joined: String = script
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .map(|l| l.trim_end_matches('\\').trim())
        .collect::<Vec<_>>()
        .join(" ");
    let steps: Vec<&str> = joined
        .split("&&")
        .filter_map(|s| s.find("cargo").map(|i| s[i..].trim()))
        .collect();
    assert_eq!(
        steps,
        vec![
            "cargo fmt --all --check",
            "cargo clippy --workspace --all-targets --all-features -- -D warnings",
            "cargo test --workspace",
            "cargo xtask trace-check",
            "cargo xtask docs-inventory --check",
            "cargo xtask env-hash --check",
            "cargo deny check",
        ]
    );
    assert!(script.contains("set -euo pipefail"));
    let alias = read(".cargo/config.toml");
    assert!(
        alias.contains("xtask"),
        ".cargo/config.toml must alias `cargo xtask`"
    );
}

/// Whether a crate name belongs to a robotics or dataflow framework (CON-20).
fn is_banned_crate(name: &str) -> bool {
    let n = name.to_ascii_lowercase().replace('_', "-");
    [
        "dora",
        "aisle",
        "genesis",
        "rclrs",
        "r2r",
        "rosrust",
        "roslibrust",
    ]
    .contains(&n.as_str())
        || [
            "dora-",
            "ros2-",
            "ros2",
            "rosrust-",
            "rclrs-",
            "r2r-",
            "roslibrust-",
            "ros-",
        ]
        .iter()
        .any(|p| n.starts_with(p))
}

/// Every dependency table of a manifest: the three top-level ones and those
/// under `[target.'cfg(..)'.*]`. Yields (key, real package name).
fn dependency_names(m: &toml::Value) -> Vec<(String, String)> {
    const TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    let mut tables: Vec<&toml::value::Table> = TABLES
        .iter()
        .filter_map(|t| m.get(*t).and_then(toml::Value::as_table))
        .collect();
    if let Some(targets) = m.get("target").and_then(toml::Value::as_table) {
        for target in targets.values() {
            tables.extend(
                TABLES
                    .iter()
                    .filter_map(|t| target.get(*t).and_then(toml::Value::as_table)),
            );
        }
    }
    tables
        .into_iter()
        .flat_map(|t| t.iter())
        .map(|(key, spec)| {
            // `alias = { package = "real-name" }` hides the real crate behind the key.
            let package = spec
                .get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(key);
            (key.clone(), package.to_owned())
        })
        .collect()
}

/// Cites: CON-20
#[test]
fn no_substrate_crate_requires_a_robotics_or_dataflow_framework() {
    let mut manifests: Vec<String> = crate_manifests().into_iter().map(|(_, m)| m).collect();
    manifests.push("Cargo.toml".to_owned()); // [workspace.dependencies], inherited by `workspace = true`
    for manifest in manifests {
        let m = toml_of(&manifest);
        let mut names = dependency_names(&m);
        if let Some(ws) = m.get("workspace") {
            names.extend(dependency_names(ws));
        }
        for (key, package) in names {
            assert!(
                !is_banned_crate(&key) && !is_banned_crate(&package),
                "{manifest}: `{key}` (package `{package}`) is a banned required dependency (CON-20)"
            );
        }
    }
}

/// Cites: CON-20
#[test]
fn the_framework_matcher_catches_renames_and_targets_and_spares_lookalikes() {
    for banned in [
        "dora-node-api",
        "dora_core",
        "ros2-client",
        "ros2_client",
        "r2r",
        "rclrs",
        "rosrust_msg",
        "aisle",
    ] {
        assert!(is_banned_crate(banned), "{banned} must be banned");
    }
    for fine in ["rosetta", "across", "doras", "r2r2", "serde", "tokio"] {
        assert!(!is_banned_crate(fine), "{fine} must not be banned");
    }
    let m: toml::Value = toml::from_str(
        "[dependencies]\nbus = { package = \"dora-node-api\", version = \"0.3\" }\n\n[target.'cfg(unix)'.dependencies]\nros2-client = \"0.7\"\n",
    )
    .expect("toml");
    let names = dependency_names(&m);
    assert!(
        names
            .iter()
            .any(|(k, p)| k == "bus" && p == "dora-node-api"),
        "{names:?}"
    );
    assert!(names.iter().any(|(k, _)| k == "ros2-client"), "{names:?}");
}

/// Cites: CON-23
#[test]
fn lab_is_outside_the_workspace_and_outside_the_determinism_lints() {
    let ws = toml_of("Cargo.toml");
    let exclude: Vec<&str> = ws["workspace"]["exclude"]
        .as_array()
        .expect("workspace.exclude")
        .iter()
        .map(|e| e.as_str().expect("str"))
        .collect();
    assert!(
        exclude.contains(&"lab"),
        "lab/ must be excluded or cargo rejects every lab crate"
    );
    // Clippy uses the nearest clippy.toml walking up from the crate, so lab/ needs
    // its own or the CON-5 bans (Instant::now, …) would fail the lab gate.
    let lab_cfg = toml_of("lab/clippy.toml");
    assert!(
        lab_cfg.get("disallowed-methods").is_none() && lab_cfg.get("disallowed-types").is_none(),
        "lab/clippy.toml must not carry the substrate bans"
    );
    let gitignore = read(".gitignore");
    assert!(
        gitignore.lines().any(|l| l.trim() == "target/"),
        "lab crates build into their own target/; ignore it at any depth"
    );
}

// ---- policy files (pre-landing review of PR 3). Every case below was a mutation
// ---- that the earlier line-matching tests let through. The lint and deny lists
// ---- are compared whole, so an entry cannot be dropped or exempted quietly.

fn paths_of(cfg: &toml::Value, key: &str) -> Vec<String> {
    let mut v: Vec<String> = cfg[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key}"))
        .iter()
        .map(|e| {
            let t = e.as_table().expect("entry is a table");
            for k in t.keys() {
                assert!(
                    ["path", "reason", "allow-invalid"].contains(&k.as_str()),
                    "clippy.toml {key}: unexpected key `{k}`"
                );
            }
            e["path"].as_str().expect("path").to_owned()
        })
        .collect();
    v.sort();
    v
}

fn sorted(list: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = list.iter().map(|s| (*s).to_owned()).collect();
    v.sort();
    v
}

/// Cites: CON-5
#[test]
fn clippy_bans_exactly_the_listed_entropy_and_clock_sources_and_unordered_maps() {
    let cfg = toml_of("clippy.toml");
    assert_eq!(
        paths_of(&cfg, "disallowed-methods"),
        sorted(&[
            "std::time::Instant::now",
            "std::time::Instant::elapsed",
            "std::time::SystemTime::now",
            "std::time::SystemTime::elapsed",
            "tokio::time::Instant::now",
            "chrono::Utc::now",
            "chrono::Local::now",
            "time::OffsetDateTime::now_utc",
            "std::thread::sleep",
            "rand::thread_rng",
            "rand::rng",
            "rand::random",
            "fastrand::Rng::new",
            "fastrand::u32",
            "fastrand::u64",
            "fastrand::usize",
            "fastrand::f64",
            "fastrand::bool",
            "fastrand::shuffle",
            "getrandom::fill",
            "getrandom::getrandom",
            "getrandom::u32",
            "getrandom::u64",
            "ahash::RandomState::new",
            "uuid::Uuid::new_v4",
            "std::collections::hash_map::RandomState::new",
        ]),
        "clippy.toml disallowed-methods (ADR-8); the list only grows"
    );
    assert_eq!(
        paths_of(&cfg, "disallowed-types"),
        sorted(&[
            "std::collections::HashMap",
            "std::collections::HashSet",
            "hashbrown::HashMap",
            "hashbrown::HashSet",
            "ahash::AHashMap",
            "ahash::AHashSet",
            "std::hash::RandomState",
            "rand::rngs::OsRng",
            "rand::rngs::ThreadRng",
        ]),
        "clippy.toml disallowed-types (ADR-8); the list only grows"
    );
    // `allow-invalid` stops clippy from reporting a path that does not resolve, so
    // it is confined to paths that exist in only some versions of their crate.
    let mut lenient: Vec<String> = Vec::new();
    for key in ["disallowed-methods", "disallowed-types"] {
        for e in cfg[key].as_array().expect("array") {
            if e.get("allow-invalid").is_some() {
                lenient.push(e["path"].as_str().expect("path").to_owned());
            }
        }
    }
    lenient.sort();
    assert_eq!(
        lenient,
        sorted(&[
            "rand::rng",
            "getrandom::fill",
            "getrandom::getrandom",
            "getrandom::u32",
            "getrandom::u64"
        ])
    );
    let mut keys: Vec<&str> = cfg
        .as_table()
        .expect("table")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "allow-expect-in-tests",
            "allow-panic-in-tests",
            "allow-unwrap-in-tests",
            "disallowed-methods",
            "disallowed-types",
        ]
    );
}

/// The lint levels the workspace hands to every member, compared whole: `warn`
/// instead of `deny`, or a missing line, is a different posture.
///
/// Cites: CON-5, CON-19
#[test]
fn the_workspace_lint_table_is_exactly_the_agreed_posture() {
    let ws = toml_of("Cargo.toml");
    let expected: toml::Value = toml::from_str(
        r#"
[rust]
unsafe_code = "forbid"

[clippy]
unwrap_used = "deny"
expect_used = "deny"
panic = "deny"
disallowed_methods = "deny"
disallowed_types = "deny"
"#,
    )
    .expect("toml");
    assert_eq!(ws["workspace"]["lints"], expected);
    // `[patch]` and `[replace]` swap the source of a dependency for every member,
    // the checker included, and a path source is invisible to `cargo deny` sources.
    let top: Vec<&str> = ws
        .as_table()
        .expect("table")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(top, ["workspace"], "root manifest: only [workspace]");
}

/// Files that would replace the root lint configuration without touching it:
/// clippy reads the nearest `clippy.toml` above a crate and prefers `.clippy.toml`.
///
/// Cites: CON-5
#[test]
fn nothing_shadows_the_root_clippy_configuration_or_the_cargo_configuration() {
    let root = repo_root();
    let mut clippy_files = Vec::new();
    let mut cargo_dirs = Vec::new();
    let walk = walkdir::WalkDir::new(&root).into_iter().filter_entry(|e| {
        let name = e.file_name().to_string_lossy();
        !(e.file_type().is_dir() && [".git", "target", ".claude"].contains(&name.as_ref()))
    });
    for entry in walk {
        let entry = entry.expect("walk");
        let rel = entry
            .path()
            .strip_prefix(&root)
            .expect("under root")
            .to_string_lossy()
            .replace('\\', "/");
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "clippy.toml" || name == ".clippy.toml" {
            clippy_files.push(rel);
        } else if name == ".cargo" {
            cargo_dirs.push(rel);
        }
    }
    clippy_files.sort();
    assert_eq!(
        clippy_files,
        ["clippy.toml", "lab/clippy.toml"],
        "a nearer clippy.toml, or any .clippy.toml, switches the CON-5 bans off for the crates below it"
    );
    assert_eq!(cargo_dirs, [".cargo"]);
    let mut files: Vec<String> = fs::read_dir(root.join(".cargo"))
        .expect(".cargo")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, ["config.toml"]);
    // An alias can replace `cargo deny`; `[build] rustflags = ["--cap-lints", "allow"]`
    // silences every lint including forbid(unsafe_code); `[env] CLIPPY_CONF_DIR`
    // points clippy at another file. The configuration is the one alias, nothing else.
    let expected: toml::Value =
        toml::from_str("[alias]\nxtask = \"run --quiet --package xtask --\"\n").expect("toml");
    assert_eq!(toml_of(".cargo/config.toml"), expected);
}

/// Cites: CON-5
#[test]
fn only_the_clock_and_rng_modules_may_switch_a_determinism_lint_off() {
    // Written in two halves so that this file does not match itself.
    let needle = concat!("clippy::", "disallowed_");
    let mut offenders = Vec::new();
    for top in ["crates", "tests"] {
        for entry in walkdir::WalkDir::new(repo_root().join(top))
            .into_iter()
            .filter_entry(|e| e.file_name() != "target")
        {
            let entry = entry.expect("walk");
            if entry.path().extension().is_none_or(|x| x != "rs") {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(repo_root())
                .expect("under root")
                .to_string_lossy()
                .replace('\\', "/");
            let exempt = rel.starts_with("crates/acn-emu/src/clock")
                || rel.starts_with("crates/acn-emu/src/rng");
            let text = fs::read_to_string(entry.path()).expect("read");
            if !exempt && text.contains(needle) {
                offenders.push(rel);
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "`deny` can be overridden by an allow attribute; only acn_emu::clock and acn_emu::rng may (ADR-8): {offenders:?}"
    );
}

/// Cites: CON-20, CON-5
#[test]
fn deny_toml_is_exactly_the_agreed_policy() {
    // Compared whole: a `wrappers` exemption on a ban, `[graph] exclude`, `skip`,
    // `allow-git` or `allow-org` would each pass a test that only looked for names.
    // cargo-deny applies the bans to the whole default-feature graph; that part is
    // cargo-deny's behaviour and is exercised by `cargo deny check` in the gate chain.
    let expected: toml::Value = toml::from_str(
        r#"
[licenses]
allow = ["Apache-2.0", "MIT", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Unicode-3.0", "Zlib", "MPL-2.0"]

[advisories]
yanked = "deny"

[bans]
multiple-versions = "warn"
deny = [
  { crate = "dora-core", reason = "CON-20" },
  { crate = "dora-node-api", reason = "CON-20" },
  { crate = "dora-operator-api", reason = "CON-20" },
  { crate = "dora-daemon", reason = "CON-20" },
  { crate = "dora-runtime", reason = "CON-20" },
  { crate = "rclrs", reason = "CON-20" },
  { crate = "r2r", reason = "CON-20" },
  { crate = "rosrust", reason = "CON-20" },
  { crate = "ros2-client", reason = "CON-20" },
  { crate = "roslibrust", reason = "CON-20" },
  { crate = "safe_drive", reason = "CON-20" },
  { crate = "dora-message", reason = "CON-20" },
  { crate = "dora-coordinator", reason = "CON-20" },
  { crate = "dora-cli", reason = "CON-20" },
  { crate = "dora-ros2-bridge", reason = "CON-20" },
  { crate = "dora-arrow-convert", reason = "CON-20" },
  { crate = "aisle", reason = "CON-20" },
  { crate = "genesis", reason = "CON-20" },
  { crate = "fastrand", reason = "CON-5", wrappers = ["tempfile"] },
]

[sources]
unknown-registry = "deny"
unknown-git = "deny"
"#,
    )
    .expect("toml");
    assert_eq!(toml_of("deny.toml"), expected);
}

/// A path dependency is compiled into the substrate without being a workspace
/// member: clippy does not lint it, `lab/clippy.toml` would shield it, and
/// `cargo deny` sees no source for it. The lockfile lists every package the
/// workspace resolves, and a package without a `source` is a path package.
///
/// Cites: CON-23, CON-19
#[test]
fn every_path_package_in_the_lockfile_is_a_workspace_member() {
    let lock = toml_of("Cargo.lock");
    let mut local: Vec<String> = lock["package"]
        .as_array()
        .expect("package")
        .iter()
        .filter(|p| p.get("source").is_none())
        .map(|p| p["name"].as_str().expect("name").to_owned())
        .collect();
    local.sort();
    let mut members: Vec<String> = crate_manifests()
        .iter()
        .map(|(_, manifest)| {
            toml_of(manifest)["package"]["name"]
                .as_str()
                .expect("package.name")
                .to_owned()
        })
        .collect();
    members.sort();
    assert_eq!(
        local, members,
        "a substrate crate depends on a path outside the workspace (a lab crate, a vendored copy or a [patch])"
    );
}

// ---- workflows: parsed as YAML and compared whole. The workflows are enforcement
// ---- points, so any change to one is made twice, the second time in this file,
// ---- which CODEOWNERS covers. Action SHAs are checked for shape and then masked,
// ---- so that a Dependabot bump does not need an edit here.

use yaml_rust2::{Yaml, YamlLoader};

fn parse_yaml(text: &str, what: &str) -> Yaml {
    // The loader rejects duplicate mapping keys, so `if:` cannot be given twice.
    let mut docs = YamlLoader::load_from_str(text).unwrap_or_else(|e| panic!("{what}: {e}"));
    assert_eq!(docs.len(), 1, "{what}: exactly one YAML document");
    docs.remove(0)
}

/// `owner/repo[/path]@<40 hex>` becomes `owner/repo[/path]@PINNED`; anything else
/// (a tag, a branch, a local or docker action) fails.
fn mask_pin(value: &Yaml, what: &str) -> Yaml {
    let s = value
        .as_str()
        .unwrap_or_else(|| panic!("{what}: `uses` must be a string"));
    let (name, sha) = s
        .split_once('@')
        .unwrap_or_else(|| panic!("{what}: `{s}` has no ref"));
    assert!(
        sha.len() == 40 && sha.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "{what}: `{s}` must be pinned to a full commit SHA"
    );
    assert!(
        name.split('/').count() >= 2
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
            && !name.starts_with('.'),
        "{what}: `{s}` is not an owner/repo action"
    );
    Yaml::String(format!("{name}@PINNED"))
}

fn mask_pins(node: Yaml, what: &str) -> Yaml {
    match node {
        Yaml::Hash(h) => Yaml::Hash(
            h.into_iter()
                .map(|(k, v)| {
                    let v = if k.as_str() == Some("uses") {
                        mask_pin(&v, what)
                    } else {
                        mask_pins(v, what)
                    };
                    (k, v)
                })
                .collect(),
        ),
        Yaml::Array(a) => Yaml::Array(a.into_iter().map(|n| mask_pins(n, what)).collect()),
        other => other,
    }
}

fn workflow(rel: &str) -> Yaml {
    mask_pins(parse_yaml(&read(rel), rel), rel)
}

const CI_YML: &str = r#"
name: ci
on:
  pull_request:
  push:
    branches: [main]
  schedule:
    - cron: "17 7 * * *"
  workflow_dispatch:
permissions:
  contents: read
concurrency:
  group: ci-${{ github.event_name }}-${{ github.event.pull_request.number || github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}
jobs:
  gates:
    strategy:
      fail-fast: false
      matrix:
        os: [macos-latest, ubuntu-latest, ubuntu-24.04-arm]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@PINNED
        with:
          persist-credentials: false
      - run: rustup toolchain install
      - uses: Swatinem/rust-cache@PINNED
      - uses: taiki-e/install-action@PINNED
        with:
          tool: cargo-deny@0.20.2
          fallback: none
      - run: tools/ci.sh
  lab:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@PINNED
        with:
          persist-credentials: false
      - run: rustup toolchain install
      - name: fmt and clippy on every lab crate
        run: |
          set -euo pipefail
          for manifest in lab/*/Cargo.toml; do
            echo "::group::${manifest}"
            cargo fmt --manifest-path "${manifest}" --check
            cargo clippy --manifest-path "${manifest}" --all-targets -- -D warnings
            echo "::endgroup::"
          done
  nightly:
    if: github.event_name == 'schedule' || github.event_name == 'workflow_dispatch'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@PINNED
        with:
          persist-credentials: false
      - run: rustup toolchain install
      - uses: Swatinem/rust-cache@PINNED
      - run: cargo test --workspace
  netem:
    if: github.event_name == 'schedule' && vars.NETEM_ENABLED == 'true'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@PINNED
        with:
          persist-credentials: false
      - run: rustup toolchain install
      - run: sudo -E env "PATH=$PATH" cargo test --workspace --features netem -- --ignored
"#;

const PR_CHECK_YML: &str = r#"
name: pr-check
on:
  pull_request:
    types: [opened, synchronize, reopened, edited, labeled, unlabeled]
permissions:
  contents: read
  pull-requests: read
concurrency:
  group: pr-check-${{ github.event.pull_request.number }}
  cancel-in-progress: true
jobs:
  pr-check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@PINNED
        with:
          fetch-depth: 0
          persist-credentials: false
      - run: rustup toolchain install
      - uses: Swatinem/rust-cache@PINNED
      - name: cargo xtask pr-check
        env:
          GH_TOKEN: ${{ github.token }}
          REPO: ${{ github.repository }}
          PR_NUMBER: ${{ github.event.pull_request.number }}
        run: |
          set -euo pipefail
          gh api "repos/${REPO}/pulls/${PR_NUMBER}" > "${RUNNER_TEMP}/pr.json"
          base="$(jq -er '.base.ref' "${RUNNER_TEMP}/pr.json")"
          labels="$(jq -ec '[.labels[].name]' "${RUNNER_TEMP}/pr.json")"
          cargo xtask pr-check --base "refs/remotes/origin/${base}" --labels-json "${labels}"
"#;

fn keys(node: &Yaml) -> Vec<&str> {
    node.as_hash()
        .expect("a mapping")
        .keys()
        .map(|k| k.as_str().expect("string key"))
        .collect()
}

/// Every `run:` line of a job, in order.
fn commands(job: &Yaml) -> Vec<&str> {
    job["steps"]
        .as_vec()
        .expect("steps")
        .iter()
        .filter_map(|s| s["run"].as_str())
        .collect()
}

/// Cites: CON-9
#[test]
fn the_workflows_are_exactly_the_reviewed_ones() {
    assert_eq!(
        workflow(".github/workflows/ci.yml"),
        parse_yaml(CI_YML, "CI_YML")
    );
    assert_eq!(
        workflow(".github/workflows/pr-check.yml"),
        parse_yaml(PR_CHECK_YML, "PR_CHECK_YML")
    );
    // A second workflow can report a check under a required name (`gates (…)`,
    // `pr-check`), and GitHub takes the newest run of that name.
    let mut files: Vec<String> = fs::read_dir(repo_root().join(".github/workflows"))
        .expect("workflows")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, ["ci.yml", "pr-check.yml"]);
    assert_eq!(
        parse_yaml(&read(".github/dependabot.yml"), "dependabot.yml"),
        parse_yaml(
            "version: 2\nupdates:\n  - package-ecosystem: github-actions\n    directory: /\n    schedule:\n      interval: weekly\n    commit-message:\n      prefix: \"ci(deps)\"\n",
            "expected dependabot.yml"
        )
    );
}

/// Cites: CON-9
#[test]
fn the_required_jobs_cannot_be_skipped_or_allowed_to_fail() {
    // A skipped job reports success to a required check, and `continue-on-error`
    // turns a failure into one. Neither key may appear on a required job or on any
    // of its steps, however it is spelled: the keys here are parsed, not matched.
    let ci = workflow(".github/workflows/ci.yml");
    let pr = workflow(".github/workflows/pr-check.yml");
    for (job, name) in [
        (&ci["jobs"]["gates"], "gates"),
        (&ci["jobs"]["lab"], "lab"),
        (&pr["jobs"]["pr-check"], "pr-check"),
    ] {
        for banned in ["if", "continue-on-error", "timeout-minutes"] {
            assert!(!keys(job).contains(&banned), "{name}: `{banned}`");
        }
        for step in job["steps"].as_vec().expect("steps") {
            for k in keys(step) {
                assert!(
                    ["uses", "with", "run", "name", "env"].contains(&k),
                    "{name}: step key `{k}` (if, continue-on-error, shell and working-directory change what a step proves)"
                );
            }
        }
    }
    // Triggers: no path or branch filter on pull requests, so the checks always report.
    assert_eq!(ci["on"]["pull_request"], Yaml::Null);
    assert_eq!(
        keys(&ci["on"]),
        ["pull_request", "push", "schedule", "workflow_dispatch"]
    );
    assert_eq!(keys(&pr["on"]), ["pull_request"]);
    assert_eq!(keys(&pr["on"]["pull_request"]), ["types"]);
    assert_eq!(
        commands(&ci["jobs"]["gates"]),
        ["rustup toolchain install", "tools/ci.sh"]
    );
}

/// Cites: CON-1, CON-2
#[test]
fn ci_matrix_is_macos_arm64_linux_x86_64_and_linux_aarch64() {
    let ci = workflow(".github/workflows/ci.yml");
    let gates = &ci["jobs"]["gates"];
    // Compared whole: an `exclude:` or `include:` beside `os:` would change the set.
    assert_eq!(
        gates["strategy"],
        parse_yaml(
            "fail-fast: false\nmatrix:\n  os: [macos-latest, ubuntu-latest, ubuntu-24.04-arm]\n",
            "strategy"
        )
    );
    assert_eq!(gates["runs-on"].as_str(), Some("${{ matrix.os }}"));
    // CON-2: the toolchain pinned in rust-toolchain.toml, installed in every job,
    // and no step that selects another one.
    for (name, job) in ci["jobs"].as_hash().expect("jobs") {
        let cmds = commands(job);
        assert_eq!(
            cmds.first().copied(),
            Some("rustup toolchain install"),
            "{name:?}"
        );
        for c in cmds {
            assert!(
                !c.contains("rustup default") && !c.contains("rustup override"),
                "{name:?}: `{c}`"
            );
        }
    }
}

/// Cites: CON-14, CON-7
#[test]
fn pr_check_reads_live_labels_and_reruns_on_label_and_base_changes() {
    let wf = workflow(".github/workflows/pr-check.yml");
    let types: Vec<&str> = wf["on"]["pull_request"]["types"]
        .as_vec()
        .expect("types")
        .iter()
        .map(|t| t.as_str().expect("str"))
        .collect();
    for t in [
        "opened",
        "synchronize",
        "reopened",
        "edited",
        "labeled",
        "unlabeled",
    ] {
        assert!(
            types.contains(&t),
            "pull_request types need `{t}`: {types:?}"
        );
    }
    let job = &wf["jobs"]["pr-check"];
    let steps = job["steps"].as_vec().expect("steps");
    assert_eq!(steps[0]["with"]["fetch-depth"].as_i64(), Some(0));
    let check = steps.last().expect("a last step");
    let script = check["run"].as_str().expect("run");
    // Labels and base come from the API at run time: a re-run replays the old event.
    assert!(script.contains("gh api \"repos/${REPO}/pulls/${PR_NUMBER}\""));
    assert!(script.contains(
        "cargo xtask pr-check --base \"refs/remotes/origin/${base}\" --labels-json \"${labels}\""
    ));
    // No `${{ }}` inside the script: event text reaches the shell as data only.
    assert!(!script.contains("${{"), "script injection: {script}");
    assert_eq!(keys(&check["env"]), ["GH_TOKEN", "REPO", "PR_NUMBER"]);
    assert_eq!(wf["permissions"]["pull-requests"].as_str(), Some("read"));
}

/// Cites: CON-12
#[test]
fn ci_runs_the_whole_workspace_every_night() {
    let ci = workflow(".github/workflows/ci.yml");
    let crons: Vec<&str> = ci["on"]["schedule"]
        .as_vec()
        .expect("schedule")
        .iter()
        .map(|s| s["cron"].as_str().expect("cron"))
        .collect();
    assert_eq!(crons.len(), 1);
    let fields: Vec<&str> = crons[0].split(' ').collect();
    assert_eq!(fields.len(), 5, "{crons:?}");
    assert!(
        fields[0].parse::<u8>().is_ok_and(|m| m < 60)
            && fields[1].parse::<u8>().is_ok_and(|h| h < 24)
            && fields[2..] == ["*", "*", "*"],
        "daily, at a fixed minute and hour: {crons:?}"
    );
    let nightly = &ci["jobs"]["nightly"];
    assert_eq!(
        nightly["if"].as_str(),
        Some("github.event_name == 'schedule' || github.event_name == 'workflow_dispatch'")
    );
    assert_eq!(
        commands(nightly),
        ["rustup toolchain install", "cargo test --workspace"]
    );
}

/// Cites: CON-23
#[test]
fn ci_runs_the_lab_gates_on_every_lab_crate() {
    let ci = workflow(".github/workflows/ci.yml");
    // Compared whole: a `continue` or an `if false` inside the loop is a different script.
    assert_eq!(
        commands(&ci["jobs"]["lab"]),
        [
            "rustup toolchain install",
            "set -euo pipefail\nfor manifest in lab/*/Cargo.toml; do\n  echo \"::group::${manifest}\"\n  cargo fmt --manifest-path \"${manifest}\" --check\n  cargo clippy --manifest-path \"${manifest}\" --all-targets -- -D warnings\n  echo \"::endgroup::\"\ndone\n"
        ]
    );
}

fn cargo_workspace_root(manifest: &std::path::Path) -> String {
    let out = std::process::Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(manifest)
        .output()
        .expect("cargo metadata");
    assert!(
        out.status.success(),
        "a lab crate must resolve without the substrate workspace: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    meta["workspace_root"]
        .as_str()
        .expect("workspace_root")
        .to_owned()
}

/// Cites: CON-23
#[test]
fn every_lab_crate_is_its_own_workspace_root() {
    let mut seen = 0;
    for entry in fs::read_dir(repo_root().join("lab")).expect("lab") {
        let dir = entry.expect("entry").path();
        let manifest = dir.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        seen += 1;
        let parsed: toml::Value =
            toml::from_str(&fs::read_to_string(&manifest).expect("read")).expect("toml");
        assert!(
            parsed
                .get("workspace")
                .and_then(toml::Value::as_table)
                .is_some_and(toml::map::Map::is_empty),
            "{}: keep the empty [workspace] table (ADR-7)",
            manifest.display()
        );
        let ws_root = std::path::PathBuf::from(cargo_workspace_root(&manifest));
        assert_eq!(
            ws_root.canonicalize().expect("canonical"),
            dir.canonicalize().expect("canonical")
        );
    }
    assert!(seen >= 1, "lab/_template must exist");

    // The table on its own, without the root manifest's `exclude`: a copy of the
    // template inside some other workspace still resolves to itself.
    let outer = tempfile::tempdir().expect("tempdir");
    fs::write(
        outer.path().join("Cargo.toml"),
        "[workspace]\nresolver = \"3\"\nmembers = []\n",
    )
    .expect("write");
    let copy = outer.path().join("spike");
    fs::create_dir_all(copy.join("src")).expect("mkdir");
    for f in ["Cargo.toml", "src/main.rs"] {
        fs::copy(repo_root().join("lab/_template").join(f), copy.join(f)).expect("copy");
    }
    let ws_root = std::path::PathBuf::from(cargo_workspace_root(&copy.join("Cargo.toml")));
    assert_eq!(
        ws_root.canonicalize().expect("canonical"),
        copy.canonicalize().expect("canonical")
    );

    // The substrate manifest must not list any lab crate (what `cargo new` would do).
    let ws = toml_of("Cargo.toml");
    for m in ws["workspace"]["members"].as_array().expect("members") {
        let m = m.as_str().expect("str").trim_start_matches("./");
        assert!(!m.starts_with("lab"), "{m}");
    }
    // CON-19 is not among the rules CON-23 lifts.
    assert!(read("lab/_template/src/main.rs").contains("#![forbid(unsafe_code)]"));
}

// ---- contribution templates and docs (docs PR). The templates are parsed, not
// ---- searched: a needle inside a comment, or front matter GitHub would not read,
// ---- passed the first version of these tests.

/// The Markdown headings of a document, as written.
fn headings(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.starts_with('#')).collect()
}

/// Cites: CON-10, CON-11, CON-16
#[test]
fn the_pr_template_asks_for_requirement_ids_class_and_labels() {
    let t = read(".github/pull_request_template.md");
    // Agents open PRs here and read this file. A comment is invisible in the
    // rendered page, so it is the one place an instruction could hide.
    assert!(!t.contains("<!--"), "no HTML comments in the PR template");
    assert_eq!(
        headings(&t),
        [
            "## What and why",
            "## Requirement IDs (CON-11)",
            "## Risk class (CON-10)",
            "## Interpretations (CON-15)",
            "## Gates",
            "## Review (CON-16)",
        ]
    );
    assert!(
        t.lines()
            .next()
            .is_some_and(|l| l.contains("`type(scope): subject`")),
        "the first line states the title convention (CON-11)"
    );
    assert!(t.contains("One spec concern per PR (CON-11)"));
    assert!(
        t.contains("| ID | What this PR does for it | Cited by (test) |"),
        "the requirement-ID table (CON-11: the description MUST list the IDs)"
    );
    // The class boxes and the two labels sit on checkbox lines, not in prose.
    let boxes: Vec<&str> = t.lines().filter(|l| l.starts_with("- [ ] ")).collect();
    for class in ["**A**", "**B**", "**C**"] {
        assert_eq!(
            boxes.iter().filter(|l| l.contains(class)).count(),
            1,
            "one checkbox for class {class}"
        );
    }
    for (label, id) in [("`env-change`", "CON-7"), ("`spec-change`", "CON-14")] {
        assert!(
            boxes.iter().any(|l| l.contains(label) && l.contains(id)),
            "a checkbox names {label} with {id}"
        );
    }
    assert!(boxes.iter().any(|l| l.contains("`tools/ci.sh` green")));
    assert!(boxes.iter().any(|l| l.contains("enforcement point")));
    // CON-16: exactly one of three review outcomes is recorded, so that "merged
    // without review" is a statement someone made and not an empty section.
    let review: Vec<&&str> = boxes
        .iter()
        .filter(|l| l.contains("Solo-maintainer mode") || l.contains("second owner"))
        .collect();
    assert_eq!(review.len(), 3, "three review outcomes: {review:?}");
    assert!(review[0].contains("merged without independent review"));
    assert!(review[1].contains("reviewed by a separate session"));
    assert!(review[2].contains("approved by someone other than the author"));
    assert!(t.contains("Tick exactly one."));
}

/// Cites: CON-13
#[test]
fn a_spec_conflict_issue_template_exists_with_the_mandated_title() {
    let t = read(".github/ISSUE_TEMPLATE/spec-conflict.md");
    assert!(
        !t.contains("<!--"),
        "no HTML comments in the issue template"
    );
    // GitHub reads the template only if the file opens with a closed front-matter
    // block that has `name` and `about`.
    let rest = t
        .strip_prefix("---\n")
        .expect("the file starts with a front-matter fence");
    let (front, body) = rest
        .split_once("\n---\n")
        .expect("the front matter is closed");
    let fm = parse_yaml(front, "spec-conflict front matter");
    assert_eq!(keys(&fm), ["name", "about", "title", "labels"]);
    for k in ["name", "about"] {
        assert!(fm[k].as_str().is_some_and(|v| !v.trim().is_empty()), "{k}");
    }
    assert_eq!(fm["title"].as_str(), Some("spec-conflict: <ids>"));
    assert_eq!(fm["labels"].as_str(), Some("spec-conflict"));
    for field in [
        "**Requirement IDs:**",
        "**What the spec says**",
        "**What the test asserts**",
        "**Work stopped at**",
    ] {
        assert!(body.contains(field), "the body asks for {field}");
    }
}

/// The same install command is written in three documents and pinned in CI. They
/// disagreed once (pinned, unpinned, absent), and an unpinned local cargo-deny can
/// pass where CI fails.
///
/// Cites: CON-9
#[test]
fn the_docs_install_the_cargo_deny_version_that_ci_pins() {
    let ci = workflow(".github/workflows/ci.yml");
    let tool = ci["jobs"]["gates"]["steps"]
        .as_vec()
        .expect("steps")
        .iter()
        .find_map(|s| s["with"]["tool"].as_str())
        .expect("the install step");
    let version = tool
        .strip_prefix("cargo-deny@")
        .expect("cargo-deny@<version>");
    let command = format!("cargo install cargo-deny --locked --version {version}");
    for doc in ["README.md", "CONTRIBUTING.md", "GETTING-STARTED.md"] {
        assert!(read(doc).contains(&command), "{doc} must say `{command}`");
    }
    // And the crates the README names are the workspace's crates.
    let readme = read("README.md");
    for c in CRATES {
        assert!(readme.contains(c), "README layout omits {c}");
    }
}
