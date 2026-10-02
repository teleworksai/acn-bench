//! CON-27(b) tree hash and CON-28 workspace root, environment hashes and the
//! preflight a run passes before it may write a bundle.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::path::Path;

use acn_trace::env::{self, EnvError, RunHypothesis};
use acn_trace::identity::{Digest, HypStatus};

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

/// A source tree: two crates, one with a build directory that must not count.
fn source_tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    write(r, "Cargo.toml", b"[workspace]\n");
    write(r, "crates/a/src/lib.rs", b"//! a\n");
    write(r, "crates/b/Cargo.toml", b"[package]\n");
    write(r, "crates/b/target/debug/junk", b"build output");
    write(r, "crates/b/.gitkeep", b"");
    write(r, "README.md", b"outside the source tree");
    dir
}

/// Cites: CON-27, CON-31
#[test]
fn the_source_tree_hash_matches_its_known_answer() {
    let dir = source_tree();
    let files = env::source_files(dir.path()).unwrap();
    assert_eq!(
        files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        ["Cargo.toml", "crates/a/src/lib.rs", "crates/b/Cargo.toml"],
        "a crate's target directory and an empty .gitkeep are left out"
    );
    assert_eq!(
        env::tree_hash(&files).unwrap().to_hex(),
        "2d7f96b759eb81d656417e496aa14b782046aea312df6a19e616ec470240024d"
    );
}

/// Cites: CON-27
#[test]
fn the_tree_walk_fails_closed() {
    let dir = source_tree();
    write(dir.path(), "crates/a/.DS_Store", b"x");
    assert!(
        env::source_files(dir.path()).is_err(),
        ".DS_Store is an error"
    );

    let dir = source_tree();
    write(dir.path(), "crates/a/.gitkeep", b"not empty");
    let files = env::source_files(dir.path()).unwrap();
    assert!(
        files.iter().any(|f| f.path == "crates/a/.gitkeep"),
        "a placeholder with content is content"
    );

    #[cfg(unix)]
    {
        let dir = source_tree();
        std::os::unix::fs::symlink("lib.rs", dir.path().join("crates/a/src/alias.rs")).unwrap();
        assert!(
            env::source_files(dir.path()).is_err(),
            "a symlink is an error"
        );
    }

    let dir = tempfile::tempdir().unwrap();
    assert!(
        env::source_files(dir.path()).is_err(),
        "no crates/ is an error"
    );
    // Only a crate's own target directory is skipped, not one deeper down.
    let dir = source_tree();
    write(dir.path(), "crates/a/src/target/mod.rs", b"//! code");
    assert!(
        env::source_files(dir.path())
            .unwrap()
            .iter()
            .any(|f| f.path == "crates/a/src/target/mod.rs")
    );
}

/// Cites: CON-27
#[test]
fn a_tree_hash_requires_sorted_unique_paths() {
    let a = env::FileHash {
        path: "b".into(),
        blake3: Digest::of(b"").to_hex(),
    };
    let b = env::FileHash {
        path: "a".into(),
        blake3: Digest::of(b"").to_hex(),
    };
    assert!(env::tree_hash(&[a.clone(), b]).is_err());
    assert!(env::tree_hash(&[a.clone(), a]).is_err());
}

/// A workspace root with a frozen set and a matching record.
fn frozen_root() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    for base in env::FROZEN_SET {
        std::fs::create_dir_all(r.join(base)).unwrap();
    }
    write(r, "hypotheses/p0.toml", b"[poc]\nid = \"p0\"\n");
    write(r, "crates/acn-hyp/src/lib.rs", b"//! hyp\n");
    let rec = env::compute(r).unwrap();
    std::fs::write(
        r.join(env::RECORD_FILE),
        serde_json::to_string_pretty(&rec).unwrap(),
    )
    .unwrap();
    dir
}

fn engine_of(root: &Path) -> Digest {
    Digest::from_hex(&env::compute(root).unwrap().engine_hash).unwrap()
}

/// Cites: CON-28
#[test]
fn the_root_is_the_nearest_ancestor_holding_the_record() {
    let dir = frozen_root();
    let deep = dir.path().join("a/b/c");
    std::fs::create_dir_all(&deep).unwrap();
    assert_eq!(env::find_root(&deep).unwrap(), dir.path());
    let other = tempfile::tempdir().unwrap();
    assert_eq!(env::find_root(other.path()), None);
}

/// Cites: CON-28
#[test]
fn engine_hash_covers_only_the_frozen_crates() {
    let dir = frozen_root();
    let before = env::compute(dir.path()).unwrap();
    write(dir.path(), "hypotheses/p1.toml", b"[poc]\nid = \"p1\"\n");
    let after = env::compute(dir.path()).unwrap();
    assert_ne!(
        before.env_hash, after.env_hash,
        "freezing a hypothesis moves env_hash"
    );
    assert_eq!(
        before.engine_hash, after.engine_hash,
        "and leaves every run of every other POC bound to the same engine"
    );
}

/// Cites: CON-28
#[test]
fn a_consistent_checkout_and_binary_may_run() {
    let dir = frozen_root();
    let engine = engine_of(dir.path());
    let pf = env::preflight(dir.path(), engine, RunHypothesis::Status(HypStatus::Frozen)).unwrap();
    assert_eq!(pf.root(), Some(dir.path()));
    assert_eq!(pf.engine_hash(), engine);
}

/// Cites: CON-28
#[test]
fn a_drifted_frozen_set_refuses_to_start() {
    let dir = frozen_root();
    let engine = engine_of(dir.path());
    write(
        dir.path(),
        "hypotheses/p0.toml",
        b"[poc]\nid = \"edited\"\n",
    );
    let err = env::preflight(dir.path(), engine, RunHypothesis::None).unwrap_err();
    assert!(matches!(err, EnvError::Refused(_)), "{err}");
}

/// Cites: CON-28
#[test]
fn an_inconsistent_record_refuses_to_start() {
    let dir = frozen_root();
    let engine = engine_of(dir.path());
    let mut rec = env::read_record(dir.path()).unwrap().unwrap();
    rec.engine_hash = Digest::of(b"forged").to_hex();
    std::fs::write(
        dir.path().join(env::RECORD_FILE),
        serde_json::to_string(&rec).unwrap(),
    )
    .unwrap();
    let err = env::preflight(dir.path(), engine, RunHypothesis::None).unwrap_err();
    assert!(matches!(err, EnvError::Refused(_)), "{err}");
}

/// Cites: CON-28, CON-31
#[test]
fn a_binary_built_from_other_frozen_code_refuses_to_start() {
    let dir = frozen_root();
    let err = env::preflight(
        dir.path(),
        Digest::of(b"another engine"),
        RunHypothesis::Status(HypStatus::Candidate),
    )
    .unwrap_err();
    assert!(matches!(err, EnvError::Refused(_)), "{err}");
}

/// Cites: CON-28
#[test]
fn without_a_root_only_unfrozen_runs_start_and_use_the_embedded_engine() {
    let dir = tempfile::tempdir().unwrap();
    let embedded = Digest::of(b"embedded");
    for h in [
        RunHypothesis::None,
        RunHypothesis::Status(HypStatus::Candidate),
    ] {
        let pf = env::preflight(dir.path(), embedded, h).unwrap();
        assert_eq!(pf.root(), None);
        assert_eq!(pf.engine_hash(), embedded);
    }
    let err = env::preflight(
        dir.path(),
        embedded,
        RunHypothesis::Status(HypStatus::Frozen),
    )
    .unwrap_err();
    assert!(matches!(err, EnvError::Refused(_)), "{err}");
}

/// Cites: CON-28
#[test]
fn this_repository_records_both_hashes_consistently() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let rec = env::read_record(&root).unwrap().expect("env-hash.json");
    assert_eq!(env::record_hash(&rec.files).to_hex(), rec.env_hash);
    assert_eq!(env::engine_hash_of(&rec.files).to_hex(), rec.engine_hash);
}
