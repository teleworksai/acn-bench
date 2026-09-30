//! Workspace-root helpers shared by every task.

use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// The workspace root when none is given: two levels above this crate.
pub fn default_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| manifest.to_path_buf(), Path::to_path_buf)
}

/// Read a file to a string, attaching the path to any error.
pub fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| Error::io(path, e))
}

/// Write a file, creating parent directories, attaching the path to any error.
pub fn write(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    std::fs::write(path, contents).map_err(|e| Error::io(path, e))
}

/// A path relative to `root`, with forward slashes, for stable output on every
/// platform. Lossy for display; hashing code uses [`rel_strict`].
pub fn rel(root: &Path, path: &Path) -> String {
    let p = path.strip_prefix(root).unwrap_or(path);
    p.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Like [`rel`], but refuses a path that is not valid UTF-8: two different
/// names must never be recorded as the same string.
pub fn rel_strict(root: &Path, path: &Path) -> Result<String> {
    let p = path.strip_prefix(root).unwrap_or(path);
    let mut parts = Vec::new();
    for c in p.components() {
        let s = c.as_os_str().to_str().ok_or_else(|| {
            Error::Invalid(format!(
                "path is not valid UTF-8 and cannot be recorded: {}",
                path.display()
            ))
        })?;
        parts.push(s.to_owned());
    }
    Ok(parts.join("/"))
}
