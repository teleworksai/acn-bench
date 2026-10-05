//! The loop runner's writes (LOOP-11, LOOP-14, HYP-4): the report, under the
//! workspace's own `runs/`, written all or nothing, and the fresh directory a
//! regeneration runs into.
//! With `verdict::write`, these are the only functions of this crate that
//! create a file or a directory; xtask's `workspace.rs` checks it.

use std::path::{Path, PathBuf};

use acn_trace::identity::Digest;

use crate::loop_run::{Code, LoopError, REPORT_JSON, REPORT_MD, err, io_err};
use crate::verdict;

/// Create `dir` if it is missing, then require a real directory: never a
/// symbolic link (HYP-4).
fn make_dir(dir: &Path) -> Result<(), LoopError> {
    if let Err(e) = std::fs::create_dir(dir)
        && e.kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(io_err(dir, e));
    }
    if !std::fs::symlink_metadata(dir)
        .map_err(|e| io_err(dir, e))?
        .is_dir()
    {
        return err(
            Code::Path,
            format!(
                "{} is not a directory, or is a symbolic link (HYP-4)",
                dir.display()
            ),
        );
    }
    Ok(())
}

/// `runs/<parts…>`, each level created and checked before the next; `runs` is
/// the workspace's own (HYP-4).
fn dir_under(runs: &Path, parts: &[&str]) -> Result<PathBuf, LoopError> {
    verdict::check_runs_dir(runs).map_err(|e| LoopError {
        code: Code::Path,
        message: e.to_string(),
    })?;
    make_dir(runs)?;
    let mut dir = runs.to_path_buf();
    for p in parts {
        dir.push(p);
        make_dir(&dir)?;
    }
    Ok(dir)
}

/// Write a new file into a directory this process just created, so no other
/// writer shares its name.
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), LoopError> {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| io_err(path, e))?;
    f.write_all(bytes).map_err(|e| io_err(path, e))?;
    f.sync_all().map_err(|e| io_err(path, e))
}

/// A directory beside `final_dir` that no other writer uses:
/// `.<name>.partial.<k>`, the first `k` whose creation succeeds.
fn staging_dir(final_dir: &Path) -> Result<PathBuf, LoopError> {
    let name = final_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("dir");
    for k in 0u64.. {
        let dir = final_dir.with_file_name(format!(".{name}.partial.{k}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io_err(&dir, e)),
        }
    }
    err(Code::Internal, "no staging directory left")
}

/// Write `report.json` and `report.md` as `runs/<prefix…>/loop/<loop_id>/`,
/// all or nothing (LOOP-11): both go into a staging directory of this
/// writer's own, which is then renamed into place. A crash leaves at most a
/// staging directory, which blocks nothing; an existing report directory is
/// never replaced. Returns the path of `report.json`.
pub(crate) fn write_report(
    runs: &Path,
    prefix: &[&str],
    loop_id: &Digest,
    json: &str,
    md: &str,
) -> Result<PathBuf, LoopError> {
    let mut parts: Vec<&str> = prefix.to_vec();
    parts.push("loop");
    let parent = dir_under(runs, &parts)?;
    let dir = parent.join(loop_id.to_hex());
    let exists = || LoopError {
        code: Code::LoopExists,
        message: format!(
            "{} exists; a loop report is never overwritten (LOOP-11)",
            dir.display()
        ),
    };
    if std::fs::symlink_metadata(&dir).is_ok() {
        return Err(exists());
    }
    let staging = staging_dir(&dir)?;
    let result = write_file(&staging.join(REPORT_JSON), json.as_bytes())
        .and_then(|()| write_file(&staging.join(REPORT_MD), md.as_bytes()))
        .and_then(|()| {
            // A rename onto an existing directory could replace an empty one;
            // check again just before it (LOOP-11).
            if std::fs::symlink_metadata(&dir).is_ok() {
                return Err(exists());
            }
            std::fs::rename(&staging, &dir).map_err(|e| io_err(&dir, e))
        });
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result.map(|()| dir.join(REPORT_JSON))
}

/// A fresh `runs/regen/<loop_id>/<n>/`, `n` the smallest positive decimal not
/// yet used (LOOP-14).
pub(crate) fn fresh_regen_dir(runs: &Path, loop_id: &Digest) -> Result<PathBuf, LoopError> {
    let parent = dir_under(runs, &["regen", &loop_id.to_hex()])?;
    for n in 1u64.. {
        let dir = parent.join(n.to_string());
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io_err(&dir, e)),
        }
    }
    err(Code::Internal, "no regeneration directory left")
}
