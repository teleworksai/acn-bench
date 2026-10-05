//! The loop runner's writes (LOOP-11, LOOP-14, HYP-4): the report, under the
//! workspace's own `runs/`, and the fresh directory a regeneration runs into.
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

/// Write a file that must not exist, never leaving a partial one: a sibling
/// temporary file linked into place, which fails if the target exists.
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), LoopError> {
    use std::io::Write as _;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(".{name}.partial"));
    let _ = std::fs::remove_file(&tmp);
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| io_err(&tmp, e))?;
        f.write_all(bytes).map_err(|e| io_err(&tmp, e))?;
        f.sync_all().map_err(|e| io_err(&tmp, e))?;
        std::fs::hard_link(&tmp, path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                LoopError {
                    code: Code::LoopExists,
                    message: format!(
                        "{} exists; a loop report is never overwritten (LOOP-11)",
                        path.display()
                    ),
                }
            } else {
                io_err(path, e)
            }
        })
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Write `report.json`, then `report.md`, under `runs/<prefix…>/loop/<loop_id>/`
/// (LOOP-11); neither is ever overwritten. Returns the path of `report.json`.
pub(crate) fn write_report(
    runs: &Path,
    prefix: &[&str],
    loop_id: &Digest,
    json: &str,
    md: &str,
) -> Result<PathBuf, LoopError> {
    let id = loop_id.to_hex();
    let mut parts: Vec<&str> = prefix.to_vec();
    parts.extend(["loop", id.as_str()]);
    let dir = dir_under(runs, &parts)?;
    let path = dir.join(REPORT_JSON);
    if std::fs::symlink_metadata(&path).is_ok()
        || std::fs::symlink_metadata(dir.join(REPORT_MD)).is_ok()
    {
        return err(
            Code::LoopExists,
            format!(
                "{} exists; a loop report is never overwritten (LOOP-11)",
                dir.display()
            ),
        );
    }
    write_new(&path, json.as_bytes())?;
    write_new(&dir.join(REPORT_MD), md.as_bytes())?;
    Ok(path)
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
