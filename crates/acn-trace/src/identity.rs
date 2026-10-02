//! Run identity: the canonical encodings, hashes and seeds of CON-27, CON-29 and
//! CON-30. Every preimage is built by [`Preimage`], which writes the context string
//! bare and zero-terminated, digests raw, integers little-endian of their stated
//! width and strings length-prefixed, so that no two derivations can collide.
//!
//! A change to any encoding here changes every `run_id` and every seed: it is a
//! Class C change and takes a new context version (`/v2`, CON-27(d)). The
//! known-answer vectors in `tests/identity.rs` pin the current ones.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng as _;

/// An identity value that cannot be encoded.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("{0}")]
    Invalid(String),
    #[error("{path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
}

type Result<T> = std::result::Result<T, IdentityError>;

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(IdentityError::Invalid(message.into()))
}

/// A 32-byte BLAKE3 digest, printed as lowercase hex (CON-27).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest(pub [u8; 32]);

impl Digest {
    /// The digest a run with no hypothesis uses for `hypothesis_hash` (CON-27(a)).
    pub const ZERO: Self = Self([0; 32]);

    /// Plain BLAKE3 of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// Lowercase hex.
    #[must_use]
    pub fn to_hex(&self) -> String {
        blake3::Hash::from_bytes(self.0).to_hex().to_string()
    }

    /// Parse 64 lowercase hex characters; any other spelling is an error, so that a
    /// digest has one text form.
    pub fn from_hex(text: &str) -> Result<Self> {
        let lower = text.len() == 64
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !lower {
            return invalid(format!(
                "`{text}` is not a digest: expected 64 lowercase hex characters"
            ));
        }
        match blake3::Hash::from_hex(text) {
            Ok(h) => Ok(Self(*h.as_bytes())),
            Err(e) => invalid(format!("`{text}` is not a digest: {e}")),
        }
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Digest({})", self.to_hex())
    }
}

/// The BLAKE3 of one file's bytes as read from the working tree, never through git
/// (CON-27(a)). Streamed, so a large measured trace cannot exhaust memory.
pub fn file_hash(path: &Path) -> Result<Digest> {
    let io = |source| IdentityError::Io {
        path: path.display().to_string(),
        source,
    };
    let file = std::fs::File::open(path).map_err(io)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(file).map_err(io)?;
    Ok(Digest(*hasher.finalize().as_bytes()))
}

/// A preimage under construction (CON-27).
#[derive(Debug, Clone)]
pub struct Preimage {
    bytes: Vec<u8>,
}

impl Preimage {
    /// Start a preimage with its context string, written bare and terminated by one
    /// zero byte. A context is printable ASCII without NUL, by construction of the
    /// call sites; anything else is an error rather than a silent collision risk.
    pub fn new(context: &str) -> Result<Self> {
        if context.is_empty() || !context.bytes().all(|b| (0x20..0x7f).contains(&b)) {
            return invalid(format!(
                "context string `{context}` must be non-empty printable ASCII"
            ));
        }
        let mut bytes = context.as_bytes().to_vec();
        bytes.push(0);
        Ok(Self { bytes })
    }

    /// A digest, as its raw 32 bytes.
    #[must_use]
    pub fn digest(mut self, d: &Digest) -> Self {
        self.bytes.extend_from_slice(&d.0);
        self
    }

    /// An unsigned 64-bit integer, little-endian.
    #[must_use]
    pub fn u64(mut self, v: u64) -> Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// An unsigned 32-bit integer, little-endian.
    #[must_use]
    pub fn u32(mut self, v: u32) -> Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// One byte.
    #[must_use]
    pub fn u8(mut self, v: u8) -> Self {
        self.bytes.push(v);
        self
    }

    /// A string: its length as an unsigned 32-bit little-endian integer, then its
    /// UTF-8 bytes.
    pub fn str(mut self, s: &str) -> Result<Self> {
        let Ok(len) = u32::try_from(s.len()) else {
            return invalid("a string in a preimage must be shorter than 4 GiB");
        };
        self.bytes.extend_from_slice(&len.to_le_bytes());
        self.bytes.extend_from_slice(s.as_bytes());
        Ok(self)
    }

    /// The bytes written so far (for known-answer tests).
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The BLAKE3 of the preimage.
    #[must_use]
    pub fn finish(&self) -> Digest {
        Digest::of(&self.bytes)
    }
}

/// A value written as text (CON-27(c)).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
}

impl Value {
    /// The text form of CON-27(c): a decimal integer; a float as `ryu` writes it
    /// (the form `serde_json` emits), negative zero as `0.0`, NaN and infinities
    /// rejected; `true`/`false`; a string as itself.
    pub fn to_text(&self) -> Result<String> {
        match self {
            Self::Int(v) => Ok(v.to_string()),
            Self::Float(v) => float_text(*v),
            Self::Bool(v) => Ok(v.to_string()),
            Self::Str(s) => Ok(s.clone()),
        }
    }
}

/// A float in the text form of CON-27(c).
pub fn float_text(v: f64) -> Result<String> {
    if !v.is_finite() {
        return invalid(format!(
            "{v} cannot be written: NaN and infinities are rejected (CON-27(c))"
        ));
    }
    let v = if v == 0.0 { 0.0 } else { v };
    let mut buf = ryu::Buffer::new();
    Ok(buf.format_finite(v).to_owned())
}

/// The execution mode, as the one byte of CON-29.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    Sim,
    Live,
    Netem,
}

impl Mode {
    /// The byte that enters `run_id`.
    #[must_use]
    pub fn byte(self) -> u8 {
        match self {
            Self::Sim => 0,
            Self::Live => 1,
            Self::Netem => 2,
        }
    }

    /// The `acn.mode` and manifest spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sim => "sim",
            Self::Live => "live",
            Self::Netem => "netem",
        }
    }

    /// Parse the spelling of [`Mode::as_str`].
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "sim" => Ok(Self::Sim),
            "live" => Ok(Self::Live),
            "netem" => Ok(Self::Netem),
            other => invalid(format!(
                "mode `{other}` is not one of `sim`, `live`, `netem`"
            )),
        }
    }
}

/// A hypothesis status (HYP-3), as it enters `params.hyp_status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HypStatus {
    Frozen,
    Candidate,
}

impl HypStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Frozen => "frozen",
            Self::Candidate => "candidate",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "frozen" => Ok(Self::Frozen),
            "candidate" => Ok(Self::Candidate),
            other => invalid(format!(
                "hypothesis status `{other}` is not `frozen` or `candidate`"
            )),
        }
    }
}

/// The arms a session can belong to (TRC-10 `acn.role`, without `researcher`).
const ARMS: &[&str] = &["control", "treatment"];

/// The run parameters of CON-29, before they become pairs.
#[derive(Debug, Clone)]
pub struct RunParams {
    /// The `acn.backend` value (TRC-10).
    pub backend: String,
    /// The model requested from the backend; for `mockllm`, the mock profile name.
    pub model: String,
    pub hyp_status: HypStatus,
    /// The arms the run executes; written sorted and comma-joined.
    pub arms: Vec<String>,
    pub replicates: u32,
    /// One entry per `[varies]` parameter of the cell, keyed without the `vary.` prefix.
    pub vary: BTreeMap<String, Value>,
    /// Run options in force, keyed with their `opt.` prefix. An option that equals
    /// its default is dropped; an option not listed in the inventory is an error.
    pub opts: BTreeMap<String, Value>,
}

/// A run option as the inventory declares it: its name, type and default text.
#[derive(Debug, Clone, Copy)]
pub struct OptionDecl<'a> {
    pub name: &'a str,
    pub ty: crate::schema::ValueType,
    pub default: &'a str,
}

impl RunParams {
    /// The key/value pairs of CON-29, keyed in bytewise order. `options` is the list
    /// of declared run options; the inventory supplies it (`acn_attributes.toml`), so
    /// that adding an option changes no existing `run_id`.
    pub fn pairs(&self, options: &[OptionDecl<'_>]) -> Result<BTreeMap<String, String>> {
        let mut pairs = BTreeMap::new();
        for (key, value) in [("backend", &self.backend), ("model", &self.model)] {
            if value.is_empty() {
                return invalid(format!("run parameter `{key}` must not be empty"));
            }
            pairs.insert(key.to_owned(), value.clone());
        }
        pairs.insert("hyp_status".to_owned(), self.hyp_status.as_str().to_owned());

        let mut arms: Vec<&str> = self.arms.iter().map(String::as_str).collect();
        arms.sort_unstable();
        if arms.is_empty() {
            return invalid("a run executes at least one arm");
        }
        if arms.windows(2).any(|w| w[0] == w[1]) {
            return invalid(format!("arms {arms:?} name an arm twice"));
        }
        if let Some(bad) = arms.iter().find(|a| !ARMS.contains(a)) {
            return invalid(format!("arm `{bad}` is not one of {ARMS:?}"));
        }
        pairs.insert("arms".to_owned(), arms.join(","));
        if self.replicates == 0 {
            return invalid("a run has at least one replicate");
        }
        pairs.insert("replicates".to_owned(), self.replicates.to_string());

        for (name, value) in &self.vary {
            if !is_param_name(name) {
                return invalid(format!(
                    "`{name}` is not a parameter name (lowercase ASCII, digits, `_`)"
                ));
            }
            pairs.insert(format!("vary.{name}"), value.to_text()?);
        }
        for (name, value) in &self.opts {
            let Some(decl) = options.iter().find(|o| o.name == name) else {
                return invalid(format!(
                    "`{name}` is not a run option listed in acn_attributes.toml (CON-29)"
                ));
            };
            let text = typed_text(decl, value)?;
            if text != decl.default {
                pairs.insert(name.clone(), text);
            }
        }
        Ok(pairs)
    }
}

/// A run option's value as text, checked against its declared type. An integer
/// given for a float option is the float of that value, as for a `range` parameter.
fn typed_text(decl: &OptionDecl<'_>, value: &Value) -> Result<String> {
    use crate::schema::ValueType as T;
    let text = match (decl.ty, value) {
        (T::Float, Value::Float(v)) => float_text(*v)?,
        #[allow(clippy::cast_precision_loss)] // an option written as an integer literal
        (T::Float, Value::Int(v)) => float_text(*v as f64)?,
        (T::Int, Value::Int(v)) => v.to_string(),
        (T::Bool, Value::Bool(v)) => v.to_string(),
        (T::String, Value::Str(v)) => v.clone(),
        (ty, v) => {
            return invalid(format!(
                "run option `{}` is declared {ty:?}, not {v:?}",
                decl.name
            ));
        }
    };
    Ok(text)
}

fn is_param_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// `params_hash = blake3("acn-bench/params/v1\0" ‖ pairs)`: for each pair in bytewise
/// key order, the key as a string and then the value as a string, with no count.
pub fn params_hash(pairs: &BTreeMap<String, String>) -> Result<Digest> {
    let mut p = Preimage::new("acn-bench/params/v1")?;
    for (k, v) in pairs {
        p = p.str(k)?.str(v)?;
    }
    Ok(p.finish())
}

/// The inputs of `run_id` (CON-29).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunIdentity {
    pub seed: u64,
    pub scenario_hash: Digest,
    pub workload_hash: Digest,
    pub hypothesis_hash: Digest,
    pub engine_hash: Digest,
    pub mode: Mode,
    pub params_hash: Digest,
}

impl RunIdentity {
    /// `run_id = blake3("acn-bench/run_id/v1\0" ‖ seed ‖ scenario_hash ‖ workload_hash ‖
    /// hypothesis_hash ‖ engine_hash ‖ mode ‖ params_hash)`.
    pub fn run_id(&self) -> Result<Digest> {
        Ok(Preimage::new("acn-bench/run_id/v1")?
            .u64(self.seed)
            .digest(&self.scenario_hash)
            .digest(&self.workload_hash)
            .digest(&self.hypothesis_hash)
            .digest(&self.engine_hash)
            .u8(self.mode.byte())
            .digest(&self.params_hash)
            .finish())
    }
}

/// A derived seed: the first 8 bytes of `d`, little-endian, with the most significant
/// bit cleared, so that it fits a signed 64-bit field (CON-30).
#[must_use]
pub fn derived_seed(d: &Digest) -> u64 {
    let mut first = [0u8; 8];
    first.copy_from_slice(&d.0[..8]);
    u64::from_le_bytes(first) & !(1u64 << 63)
}

/// `replicate_seed(i)`, the derived seed of
/// `blake3("acn-bench/replicate/v1\0" ‖ seed ‖ i)` (CON-30(a)).
pub fn replicate_seed(seed: u64, i: u32) -> Result<u64> {
    Ok(derived_seed(
        &Preimage::new("acn-bench/replicate/v1")?
            .u64(seed)
            .u32(i)
            .finish(),
    ))
}

/// The 32 bytes that seed the component sub-stream `name` under `s`:
/// `blake3("acn-bench/rng/v1\0" ‖ s ‖ name)` (CON-30(b)). `s` is the replicate seed
/// for anything that belongs to a replicate and the run seed for anything that
/// belongs to the run. `name` is ASCII.
pub fn substream_seed(s: u64, name: &str) -> Result<[u8; 32]> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_graphic()) {
        return invalid(format!(
            "sub-stream name `{name}` must be non-empty printable ASCII without spaces"
        ));
    }
    Ok(Preimage::new("acn-bench/rng/v1")?
        .u64(s)
        .str(name)?
        .finish()
        .0)
}

/// The generator of the component sub-stream `name` under `s` (CON-5(a), CON-30(b)).
pub fn substream_rng(s: u64, name: &str) -> Result<ChaCha20Rng> {
    Ok(ChaCha20Rng::from_seed(substream_seed(s, name)?))
}

/// The run options the inventory declares, for [`RunParams::pairs`].
#[must_use]
pub fn options(inv: &crate::schema::Inventory) -> Vec<OptionDecl<'_>> {
    inv.options()
        .iter()
        .map(|o| OptionDecl {
            name: &o.name,
            ty: o.ty,
            default: &o.default,
        })
        .collect()
}

/// Build identity (CON-31): the components, each a digest or a string, and the
/// `build_hash` over them. Computed by `acn-cli`'s build script and embedded in the
/// binary; every manifest carries all of it, so that a third party can check
/// `source_hash` against a tagged tree.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildInfo {
    pub build_hash: String,
    /// BLAKE3 of `Cargo.lock`.
    pub cargo_lock: String,
    /// BLAKE3 of `rust-toolchain.toml`.
    pub rust_toolchain: String,
    /// BLAKE3 of `.cargo/config.toml`.
    pub cargo_config: String,
    /// The tree hash of CON-27(b) over the source (`env::source_files`).
    pub source_hash: String,
    pub target: String,
    pub profile: String,
    /// `acn-cli`'s enabled features as spelled in its manifest, `default` excluded,
    /// sorted and comma-joined.
    pub features: String,
    /// `CARGO_ENCODED_RUSTFLAGS`, empty when unset.
    pub rustflags: String,
}

/// The components of [`BuildInfo`] before the hash is taken.
#[derive(Debug, Clone, Copy)]
pub struct BuildParts<'a> {
    pub cargo_lock: Digest,
    pub rust_toolchain: Digest,
    pub cargo_config: Digest,
    pub source_hash: Digest,
    pub target: &'a str,
    pub profile: &'a str,
    pub features: &'a str,
    pub rustflags: &'a str,
}

impl BuildParts<'_> {
    /// `build_hash = blake3("acn-bench/build/v1\0" ‖ blake3(Cargo.lock) ‖
    /// blake3(rust-toolchain.toml) ‖ blake3(.cargo/config.toml) ‖ source_hash ‖ target ‖
    /// profile ‖ features ‖ rustflags)`, the last four as strings.
    pub fn build_hash(&self) -> Result<Digest> {
        Ok(Preimage::new("acn-bench/build/v1")?
            .digest(&self.cargo_lock)
            .digest(&self.rust_toolchain)
            .digest(&self.cargo_config)
            .digest(&self.source_hash)
            .str(self.target)?
            .str(self.profile)?
            .str(self.features)?
            .str(self.rustflags)?
            .finish())
    }

    /// The full record.
    pub fn info(&self) -> Result<BuildInfo> {
        Ok(BuildInfo {
            build_hash: self.build_hash()?.to_hex(),
            cargo_lock: self.cargo_lock.to_hex(),
            rust_toolchain: self.rust_toolchain.to_hex(),
            cargo_config: self.cargo_config.to_hex(),
            source_hash: self.source_hash.to_hex(),
            target: self.target.to_owned(),
            profile: self.profile.to_owned(),
            features: self.features.to_owned(),
            rustflags: self.rustflags.to_owned(),
        })
    }
}

impl BuildInfo {
    /// Recompute `build_hash` from the components and compare.
    pub fn check(&self) -> Result<Digest> {
        let parts = BuildParts {
            cargo_lock: Digest::from_hex(&self.cargo_lock)?,
            rust_toolchain: Digest::from_hex(&self.rust_toolchain)?,
            cargo_config: Digest::from_hex(&self.cargo_config)?,
            source_hash: Digest::from_hex(&self.source_hash)?,
            target: &self.target,
            profile: &self.profile,
            features: &self.features,
            rustflags: &self.rustflags,
        };
        let h = parts.build_hash()?;
        if h.to_hex() != self.build_hash {
            return invalid(format!(
                "build_hash {} is not the hash of its components ({h}) (CON-31)",
                self.build_hash
            ));
        }
        Ok(h)
    }
}
