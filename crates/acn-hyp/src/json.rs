//! The canonical JSON of `verdict.json` (HYP-15): UTF-8, keys sorted, no
//! insignificant whitespace, numbers in the text form of CON-27(c) (a float as
//! `ryu` writes it, never serde_json's formatter, ADR-13), an undefined value as
//! `null`, and one trailing newline.

use std::collections::BTreeMap;

use acn_trace::identity::float_text;

/// A JSON value whose rendering is fixed.
#[derive(Debug, Clone, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    Int(i64),
    /// Rendered with `float_text`; a non-finite value renders as `null`.
    Float(f64),
    Str(String),
    Arr(Vec<J>),
    /// Sorted by key, bytewise.
    Obj(BTreeMap<String, J>),
}

impl J {
    /// An object from `(key, value)` pairs.
    #[must_use]
    pub fn obj<K: Into<String>>(pairs: impl IntoIterator<Item = (K, J)>) -> Self {
        Self::Obj(pairs.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    #[must_use]
    pub fn str(s: impl Into<String>) -> Self {
        Self::Str(s.into())
    }

    /// A number, or `null` when undefined.
    #[must_use]
    pub fn num(v: Option<f64>) -> Self {
        v.map_or(Self::Null, Self::Float)
    }

    /// A count.
    #[must_use]
    pub fn count(n: usize) -> Self {
        i64::try_from(n).map_or(Self::Null, Self::Int)
    }

    /// The canonical text, with its trailing newline.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out.push('\n');
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Self::Int(i) => out.push_str(&i.to_string()),
            Self::Float(f) => match float_text(*f) {
                Ok(t) => out.push_str(&t),
                Err(_) => out.push_str("null"),
            },
            Self::Str(s) => string(s, out),
            Self::Arr(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    x.write(out);
                }
                out.push(']');
            }
            Self::Obj(m) => {
                out.push('{');
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    string(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

/// A JSON string: `"` and `\` escaped, control characters as `\u00XX` (or their
/// short forms), everything else as UTF-8.
fn string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
}
