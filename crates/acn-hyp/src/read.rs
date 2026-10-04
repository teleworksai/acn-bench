//! Bundles as the verdict reads them (HYP-20): verified (TRC-23), their manifest
//! parsed, and the `session`, `turn` and `call` views read into the rows the
//! quantity formulas take (HYP-12). Nothing here judges the set; that is
//! [`crate::verdict`].

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use acn_trace::bundle::{self, MANIFEST, Manifest};
use acn_trace::identity::Digest;
use acn_trace::parquet_io;
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

fn view(dir: &Path, views: &schema::Views, name: &str) -> Result<Vec<RecordBatch>, String> {
    let v = views
        .view(name)
        .ok_or_else(|| format!("views.toml has no `{name}` view"))?;
    parquet_io::read_view(dir, v).map_err(|e| e.to_string())
}

/// Verify the bundle at `dir` (TRC-23) and read what a verdict needs from it.
pub fn read(dir: &Path) -> Result<BundleData, ReadError> {
    let fail = |message: String| ReadError {
        dir: dir.to_path_buf(),
        message,
    };
    let verified = bundle::verify(dir).map_err(|e| fail(e.to_string()))?;
    let path = dir.join(MANIFEST);
    let bytes = std::fs::read(&path).map_err(|e| fail(e.to_string()))?;
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| fail(e.to_string()))?;
    let views = schema::views().map_err(|e| fail(e.to_string()))?;

    let mut sessions = Vec::new();
    for b in view(dir, &views, "session").map_err(fail)? {
        let sid = ids(&b, "session_id").map_err(fail)?;
        let role = required(strings(&b, "role").map_err(fail)?, "role").map_err(fail)?;
        let rep = required(ints(&b, "replicate").map_err(fail)?, "replicate").map_err(fail)?;
        for ((session_id, role), replicate) in sid.into_iter().zip(role).zip(rep) {
            sessions.push(SessionRow {
                session_id,
                role,
                replicate,
            });
        }
    }
    let mut turns = Vec::new();
    for b in view(dir, &views, "turn").map_err(fail)? {
        let sid = ids(&b, "session_id").map_err(fail)?;
        let outcome = required(strings(&b, "outcome").map_err(fail)?, "outcome").map_err(fail)?;
        let compaction =
            required(strings(&b, "compaction").map_err(fail)?, "compaction").map_err(fail)?;
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
    for b in view(dir, &views, "call").map_err(fail)? {
        let sid = ids(&b, "session_id").map_err(fail)?;
        let input = ints(&b, "input_tokens").map_err(fail)?;
        let read = ints(&b, "cache_read_tokens").map_err(fail)?;
        let write = ints(&b, "cache_write_tokens").map_err(fail)?;
        let output = ints(&b, "output_tokens").map_err(fail)?;
        let ttft = ints(&b, "ttft_ns").map_err(fail)?;
        let method = required(
            strings(&b, "new_input_tokens_method").map_err(fail)?,
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
        manifest,
        run_id: verified.run_id,
        bundle_digest: verified.bundle_digest,
        sessions,
        turns,
        calls,
        methods,
    })
}
