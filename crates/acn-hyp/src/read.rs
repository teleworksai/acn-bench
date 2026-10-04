//! Bundles as the verdict reads them (HYP-20): verified (TRC-23) with their
//! views recomputed from the tables (TRC-35), and the `session`, `turn` and
//! `call` views turned into the rows the quantity formulas take (HYP-12). Nothing here judges the set; that is
//! [`crate::verdict`].

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use acn_trace::bundle::{self, Manifest};
use acn_trace::identity::Digest;
use acn_trace::schema;
use arrow_array::cast::AsArray as _;
use arrow_array::types::Int64Type;
use arrow_array::{Array as _, ArrayRef, RecordBatch};

use crate::quantities::{Call, Turn};

/// A bundle that cannot be read: the reason names the bundle.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: {message}", dir.display())]
pub struct ReadError {
    pub dir: PathBuf,
    pub message: String,
}

/// One session of a bundle: the `session` view columns the verdict reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub session_id: [u8; 8],
    /// `acn.role`: `treatment` or `control`.
    pub role: String,
    /// `acn.replicate`.
    pub replicate: i64,
}

/// A verified bundle and the rows a verdict reads from it.
#[derive(Debug, Clone, PartialEq)]
pub struct BundleData {
    /// Where it was read from, for messages; never part of a verdict.
    pub dir: PathBuf,
    pub manifest: Manifest,
    pub run_id: Digest,
    pub bundle_digest: Digest,
    pub sessions: Vec<SessionRow>,
    pub turns: Vec<Turn>,
    pub calls: Vec<Call>,
    /// The `new_input_tokens_method` values its calls carry (TRC-12).
    pub methods: BTreeSet<String>,
}

fn col<'b>(b: &'b RecordBatch, name: &str) -> Result<&'b ArrayRef, String> {
    b.column_by_name(name)
        .ok_or_else(|| format!("a view lacks the column `{name}`"))
}

fn strings(b: &RecordBatch, name: &str) -> Result<Vec<Option<String>>, String> {
    let c = col(b, name)?;
    let s = c
        .as_string_opt::<i32>()
        .ok_or_else(|| format!("`{name}` is not a string column"))?;
    Ok((0..s.len())
        .map(|i| s.is_valid(i).then(|| s.value(i).to_owned()))
        .collect())
}

fn ints(b: &RecordBatch, name: &str) -> Result<Vec<Option<i64>>, String> {
    let c = col(b, name)?;
    let a = c
        .as_primitive_opt::<Int64Type>()
        .ok_or_else(|| format!("`{name}` is not an int64 column"))?;
    Ok((0..a.len())
        .map(|i| a.is_valid(i).then(|| a.value(i)))
        .collect())
}

fn ids(b: &RecordBatch, name: &str) -> Result<Vec<[u8; 8]>, String> {
    let c = col(b, name)?;
    let a = c
        .as_fixed_size_binary_opt()
        .ok_or_else(|| format!("`{name}` is not a fixed-size binary column"))?;
    (0..a.len())
        .map(|i| <[u8; 8]>::try_from(a.value(i)).map_err(|_| format!("`{name}` is not 8 bytes")))
        .collect()
}

fn required<T>(v: Vec<Option<T>>, name: &str) -> Result<Vec<T>, String> {
    v.into_iter()
        .map(|x| x.ok_or_else(|| format!("`{name}` is null in a non-nullable column")))
        .collect()
}

fn view<'v>(
    views: &'v [(schema::View, RecordBatch)],
    name: &str,
) -> Result<&'v RecordBatch, String> {
    views
        .iter()
        .find(|(v, _)| v.name == name)
        .map(|(_, b)| b)
        .ok_or_else(|| format!("no `{name}` view"))
}

/// Verify the bundle at `dir` (TRC-23), recompute its views from its tables and
/// require them byte-identical to the files (TRC-35), and read what a verdict
/// needs from what was verified: the manifest from the bytes `bundle_digest`
/// covers, and the rows from the recomputed views, never from a second read.
pub fn read(dir: &Path) -> Result<BundleData, ReadError> {
    let fail = |message: String| ReadError {
        dir: dir.to_path_buf(),
        message,
    };
    let (verified, views) = bundle::verify_views_read(dir).map_err(|e| fail(e.to_string()))?;

    let mut sessions = Vec::new();
    {
        let b = view(&views, "session").map_err(fail)?;
        let sid = ids(b, "session_id").map_err(fail)?;
        let role = required(strings(b, "role").map_err(fail)?, "role").map_err(fail)?;
        let rep = required(ints(b, "replicate").map_err(fail)?, "replicate").map_err(fail)?;
        for ((session_id, role), replicate) in sid.into_iter().zip(role).zip(rep) {
            sessions.push(SessionRow {
                session_id,
                role,
                replicate,
            });
        }
    }
    let mut turns = Vec::new();
    {
        let b = view(&views, "turn").map_err(fail)?;
        let sid = ids(b, "session_id").map_err(fail)?;
        let outcome = required(strings(b, "outcome").map_err(fail)?, "outcome").map_err(fail)?;
        let compaction =
            required(strings(b, "compaction").map_err(fail)?, "compaction").map_err(fail)?;
        for ((session_id, outcome), compaction) in sid.into_iter().zip(outcome).zip(compaction) {
            turns.push(Turn {
                session_id,
                outcome,
                compaction,
            });
        }
    }
    let mut calls = Vec::new();
    let mut methods = BTreeSet::new();
    {
        let b = view(&views, "call").map_err(fail)?;
        let sid = ids(b, "session_id").map_err(fail)?;
        let input = ints(b, "input_tokens").map_err(fail)?;
        let read = ints(b, "cache_read_tokens").map_err(fail)?;
        let write = ints(b, "cache_write_tokens").map_err(fail)?;
        let output = ints(b, "output_tokens").map_err(fail)?;
        let ttft = ints(b, "ttft_ns").map_err(fail)?;
        let method = required(
            strings(b, "new_input_tokens_method").map_err(fail)?,
            "new_input_tokens_method",
        )
        .map_err(fail)?;
        for i in 0..sid.len() {
            calls.push(Call {
                session_id: sid[i],
                input_tokens: input[i],
                cache_read_tokens: read[i],
                cache_write_tokens: write[i],
                output_tokens: output[i],
                ttft_ns: ttft[i],
            });
            methods.insert(method[i].clone());
        }
    }
    Ok(BundleData {
        dir: dir.to_path_buf(),
        manifest: verified.manifest,
        run_id: verified.run_id,
        bundle_digest: verified.bundle_digest,
        sessions,
        turns,
        calls,
        methods,
    })
}
