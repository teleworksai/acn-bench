//! The ACN trace schema (SPEC 010 §3–§7). This module is in the frozen set
//! (CON-7): every change is a Class C `env-change` PR and moves `env_hash` and
//! `engine_hash` (CON-28).
//!
//! The schema is data: `SEMCONV_VERSION` (TRC-2), `acn_attributes.toml` (TRC-20,
//! TRC-21, the option defaults of CON-29) and `views.toml` (TRC-37), embedded in the
//! binary and parsed strictly. A file that does not parse, or that breaks one of the
//! rules below, is an error with the offending name; nothing is partially honoured.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

/// The pinned semantic-conventions version (TRC-2), as written in the file.
const SEMCONV_VERSION_FILE: &str = include_str!("SEMCONV_VERSION");
const ATTRIBUTES_TOML: &str = include_str!("acn_attributes.toml");
const VIEWS_TOML: &str = include_str!("views.toml");

/// A schema file that cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("{file}: {message}")]
    Parse { file: &'static str, message: String },
    #[error("{file}: {message}")]
    Invalid { file: &'static str, message: String },
}

type Result<T> = std::result::Result<T, SchemaError>;

const ATTRS: &str = "acn_attributes.toml";
const VIEWS: &str = "views.toml";

fn bad<T>(file: &'static str, message: String) -> Result<T> {
    Err(SchemaError::Invalid { file, message })
}

/// The value type of an attribute: the members of the OTLP attribute union that
/// the Parquet layout stores (TRC-25).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueType {
    String,
    Int,
    Float,
    Bool,
    Bytes,
}

/// The OTLP span kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpanKind {
    Internal,
    Client,
    Server,
    Producer,
    Consumer,
}

impl SpanKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Client => "client",
            Self::Server => "server",
            Self::Producer => "producer",
            Self::Consumer => "consumer",
        }
    }
}

/// A span the profile defines (SPEC 010 §3).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Span {
    pub name: String,
    /// The kinds a producer may give it.
    pub kind: Vec<SpanKind>,
    /// `root`, or the declared spans it may be a child of.
    pub parents: Vec<String>,
    pub producers: Vec<String>,
    pub requirement: String,
}

/// A field of a span event. Event fields are not `acn.`-prefixed.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventField {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ValueType,
    pub unit: String,
    /// The closed set of values, for a string field that has one.
    #[serde(default)]
    pub values: Vec<String>,
}

/// A span event the profile defines.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub name: String,
    pub on: String,
    pub producers: Vec<String>,
    pub requirement: String,
    #[serde(default)]
    pub fields: Vec<EventField>,
}

/// One `acn.*` attribute (TRC-20).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attribute {
    pub name: String,
    /// Span names, `resource`, or `*` for any span.
    pub on: Vec<String>,
    #[serde(rename = "type")]
    pub ty: ValueType,
    pub unit: String,
    pub producers: Vec<String>,
    pub required: bool,
    /// For an optional attribute, the condition under which it is present. It is
    /// absent, never zero or empty, otherwise (SPEC 010 §3).
    #[serde(default)]
    pub when: Option<String>,
    /// Whether `spans.parquet` carries a typed, nullable column for it (TRC-25).
    pub promoted: bool,
    /// The closed set of values, for a string attribute that has one.
    #[serde(default)]
    pub values: Vec<String>,
    pub requirement: String,
    #[serde(default)]
    pub doc: String,
}

impl Attribute {
    /// The name of the promoted Parquet column: dots become underscores.
    #[must_use]
    pub fn column_name(&self) -> String {
        self.name.replace('.', "_")
    }
}

/// A run option and its default (CON-29).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunOption {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ValueType,
    /// In the text form of CON-27(c); the loader rejects any other spelling.
    pub default: String,
    /// The session attribute that records the value in force.
    pub attribute: String,
    #[serde(default)]
    pub doc: String,
}

/// What an absent provider field means (TRC-12: a producer never invents a count).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AbsentMeans {
    /// The provider always reports the field, so a response without it had none.
    Zero,
    /// The provider may simply not report it: the value is unknown.
    Absent,
}

impl AbsentMeans {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zero => "zero",
            Self::Absent => "absent",
        }
    }
}

/// How one provider's response fields map to `acn.*` (TRC-21). The rules are stated
/// in the header of the provider section of `acn_attributes.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    pub name: String,
    /// Dotted paths that are summed to give the total prompt length (TRC-12).
    pub input_tokens: Vec<String>,
    /// The provider's own prompt count, one of `input_tokens`: without it there is no usage.
    pub input_tokens_base: String,
    pub output_tokens: String,
    pub cache_read: String,
    /// What a missing cache-read field means once usage is present.
    pub cache_read_absent: AbsentMeans,
    /// Absent when the provider has no cache-write count; the value is then 0.
    #[serde(default)]
    pub cache_write: Option<String>,
    pub stop_reason_field: String,
    /// Provider value to normalised value; anything else is `other`.
    pub stop_reason: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InventoryFile {
    schema_version: u32,
    #[serde(default)]
    span: Vec<Span>,
    #[serde(default)]
    event: Vec<Event>,
    #[serde(default)]
    attribute: Vec<Attribute>,
    #[serde(default)]
    option: Vec<RunOption>,
    #[serde(default)]
    provider: Vec<Provider>,
}

/// The parsed and validated inventory.
#[derive(Debug, Clone)]
pub struct Inventory {
    semconv_version: String,
    spans: Vec<Span>,
    events: Vec<Event>,
    attributes: Vec<Attribute>,
    options: Vec<RunOption>,
    providers: Vec<Provider>,
}

fn is_name(s: &str, prefix: &str) -> bool {
    s.starts_with(prefix)
        && !s.ends_with('.')
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_')
}

fn is_requirement(s: &str) -> bool {
    match s.split_once('-') {
        Some((prefix, number)) => {
            (2..=5).contains(&prefix.len())
                && prefix.chars().all(|c| c.is_ascii_uppercase())
                && !number.is_empty()
                && number.chars().all(|c| c.is_ascii_digit())
        }
        None => false,
    }
}

/// The unit and type a name's suffix implies (TRC-20: units follow OTel conventions).
fn suffix_rule(name: &str) -> Option<(&'static str, &'static str, ValueType)> {
    if name.ends_with("_ms") {
        Some(("_ms", "ms", ValueType::Float))
    } else if name.ends_with("_ns") {
        Some(("_ns", "ns", ValueType::Int))
    } else if name.ends_with("_tokens") {
        Some(("_tokens", "tokens", ValueType::Int))
    } else if name.ends_with("_kbps") {
        Some(("_kbps", "kbps", ValueType::Float))
    } else if name.ends_with("bytes")
        || name.ends_with("_bytes_up")
        || name.ends_with("_bytes_down")
    {
        Some(("bytes", "bytes", ValueType::Int))
    } else {
        None
    }
}

/// The first entry of `items` that occurs twice.
fn first_duplicate(items: &[String]) -> Option<&str> {
    let mut seen = BTreeSet::new();
    items.iter().map(String::as_str).find(|i| !seen.insert(*i))
}

/// Whether `text` is the one spelling CON-27(c) gives a value of type `ty`.
fn is_canonical(ty: ValueType, text: &str) -> bool {
    match ty {
        ValueType::Bool => text == "true" || text == "false",
        ValueType::Int => text.parse::<i64>().is_ok_and(|v| v.to_string() == text),
        // serde_json writes an f64 as `ryu` does, which is the reference form.
        ValueType::Float => text
            .parse::<f64>()
            .is_ok_and(|v| v.is_finite() && serde_json::to_string(&v).is_ok_and(|s| s == text)),
        ValueType::String => true,
        ValueType::Bytes => false,
    }
}

/// Normalised values only the harness may assign: the client or the link cut the call.
const HARNESS_ONLY_STOPS: &[&str] = &["client_abort", "transport_error"];

/// `strict` also rejects a unit on a name without a suffix. Attribute names are ours
/// to choose, so they are strict; event field names come from the spec (`tokens_before`),
/// so for them only the suffix-implies-unit direction applies.
fn check_unit(what: &str, name: &str, unit: &str, ty: ValueType, strict: bool) -> Result<()> {
    match suffix_rule(name) {
        Some((suffix, u, t)) if unit != u || ty != t => bad(
            ATTRS,
            format!("{what}: the `{suffix}` suffix requires unit `{u}` and type {t:?}"),
        ),
        None if strict && !unit.is_empty() => bad(
            ATTRS,
            format!(
                "{what}: a unit needs the matching suffix in the name (TRC-20); found unit `{unit}`"
            ),
        ),
        _ if ty == ValueType::Bool && !unit.is_empty() => {
            bad(ATTRS, format!("{what}: a boolean carries no unit"))
        }
        _ => Ok(()),
    }
}

fn check_values(what: &str, values: &[String], ty: ValueType) -> Result<()> {
    if !values.is_empty() && ty != ValueType::String {
        return bad(ATTRS, format!("{what}: `values` is for strings"));
    }
    match first_duplicate(values) {
        Some(d) => bad(ATTRS, format!("{what}: duplicate value `{d}`")),
        None => Ok(()),
    }
}

fn check_requirement(what: &str, id: &str) -> Result<()> {
    if is_requirement(id) {
        Ok(())
    } else {
        bad(
            ATTRS,
            format!("{what}: `{id}` is not a requirement ID such as TRC-12"),
        )
    }
}

fn check_spans(file: &InventoryFile, seen: &mut BTreeSet<String>) -> Result<()> {
    for s in &file.span {
        let what = format!("span `{}`", s.name);
        if !seen.insert(s.name.clone()) {
            return bad(ATTRS, format!("duplicate name `{}`", s.name));
        }
        if s.producers.is_empty() {
            return bad(ATTRS, format!("{what} has no producer"));
        }
        if s.kind.is_empty() {
            return bad(ATTRS, format!("{what} has no kind"));
        }
        if s.parents.is_empty() {
            return bad(ATTRS, format!("{what} has no parent"));
        }
        for parent in &s.parents {
            if parent != "root" && !file.span.iter().any(|x| &x.name == parent) {
                return bad(
                    ATTRS,
                    format!("{what} names the parent `{parent}`, which is not a declared span"),
                );
            }
        }
        check_requirement(&what, &s.requirement)?;
    }
    Ok(())
}

fn check_events(file: &InventoryFile, seen: &mut BTreeSet<String>) -> Result<()> {
    for e in &file.event {
        let what = format!("event `{}`", e.name);
        if !is_name(&e.name, "acn.") {
            return bad(ATTRS, format!("{what} must be a lowercase `acn.` name"));
        }
        if !seen.insert(e.name.clone()) {
            return bad(ATTRS, format!("duplicate name `{}`", e.name));
        }
        if !file.span.iter().any(|s| s.name == e.on) {
            return bad(
                ATTRS,
                format!("{what} sits on `{}`, which is not a declared span", e.on),
            );
        }
        if e.producers.is_empty() {
            return bad(ATTRS, format!("{what} has no producer"));
        }
        check_requirement(&what, &e.requirement)?;
        for f in &e.fields {
            let field = format!("{what} field `{}`", f.name);
            check_unit(&field, &f.name, &f.unit, f.ty, false)?;
            check_values(&field, &f.values, f.ty)?;
        }
    }
    Ok(())
}

fn check_attributes(file: &InventoryFile, seen: &mut BTreeSet<String>) -> Result<()> {
    let mut columns = BTreeSet::new();
    for a in &file.attribute {
        let n = a.name.as_str();
        let what = format!("`{n}`");
        if !is_name(n, "acn.") {
            return bad(
                ATTRS,
                format!("attribute {what} must be a lowercase `acn.` name (TRC-3)"),
            );
        }
        if !seen.insert(a.name.clone()) {
            return bad(ATTRS, format!("duplicate name {what}"));
        }
        if a.on.is_empty() {
            return bad(
                ATTRS,
                format!("{what} is not attached to any span or resource"),
            );
        }
        if let Some(d) = first_duplicate(&a.on) {
            return bad(ATTRS, format!("{what}: duplicate target `{d}`"));
        }
        for on in &a.on {
            if on != "resource" && on != "*" && !file.span.iter().any(|s| &s.name == on) {
                return bad(
                    ATTRS,
                    format!(
                        "{what} sits on `{on}`, which is not a declared span, `resource` or `*`"
                    ),
                );
            }
        }
        if a.producers.is_empty() {
            return bad(ATTRS, format!("{what} has no producer"));
        }
        check_unit(&what, n, &a.unit, a.ty, true)?;
        check_values(&what, &a.values, a.ty)?;
        match (a.required, &a.when) {
            (false, None) => {
                return bad(
                    ATTRS,
                    format!("{what} is optional and must say `when` it is present"),
                );
            }
            (true, Some(_)) => {
                return bad(
                    ATTRS,
                    format!("{what} is required, so `when` does not apply"),
                );
            }
            _ => {}
        }
        if a.promoted {
            if a.ty == ValueType::Bytes || a.on.iter().any(|o| o == "resource") {
                return bad(
                    ATTRS,
                    format!("{what} cannot be promoted: a promoted column is a scalar on a span"),
                );
            }
            if !columns.insert(a.column_name()) {
                return bad(
                    ATTRS,
                    format!("two attributes promote to the column `{}`", a.column_name()),
                );
            }
        }
        check_requirement(&what, &a.requirement)?;
    }
    Ok(())
}

fn check_options(file: &InventoryFile) -> Result<()> {
    let mut seen = BTreeSet::new();
    for o in &file.option {
        let what = format!("option `{}`", o.name);
        if !is_name(&o.name, "opt.") {
            return bad(
                ATTRS,
                format!("{what} must be a lowercase `opt.` name (CON-29)"),
            );
        }
        if !seen.insert(o.name.as_str()) {
            return bad(ATTRS, format!("duplicate {what}"));
        }
        let Some(attr) = file.attribute.iter().find(|a| a.name == o.attribute) else {
            return bad(
                ATTRS,
                format!("{what} records into `{}`, which is not listed", o.attribute),
            );
        };
        if attr.ty != o.ty {
            return bad(
                ATTRS,
                format!("{what} and `{}` differ in type", o.attribute),
            );
        }
        if attr.on != ["acn.session"] || !attr.required {
            return bad(
                ATTRS,
                format!(
                    "{what} must record into a required attribute on `acn.session` alone, not `{}`",
                    o.attribute
                ),
            );
        }
        if file
            .option
            .iter()
            .filter(|x| x.attribute == o.attribute)
            .count()
            > 1
        {
            return bad(
                ATTRS,
                format!(
                    "`{}` records more than one option; each attribute records one option",
                    o.attribute
                ),
            );
        }
        if !is_canonical(o.ty, &o.default) {
            return bad(
                ATTRS,
                format!(
                    "{what}: the default `{}` is not the CON-27(c) spelling of a {:?}; it enters `params_hash`, so there is exactly one",
                    o.default, o.ty
                ),
            );
        }
    }
    Ok(())
}

fn check_providers(file: &InventoryFile) -> Result<()> {
    let stop_values: Vec<&str> = file
        .attribute
        .iter()
        .find(|a| a.name == "acn.call.stop_reason")
        .map(|a| a.values.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    for p in &file.provider {
        let what = format!("provider `{}`", p.name);
        if !seen.insert(p.name.as_str()) {
            return bad(ATTRS, format!("duplicate {what}"));
        }
        if p.input_tokens.is_empty() {
            return bad(
                ATTRS,
                format!("{what} maps no field to the input token total (TRC-12)"),
            );
        }
        if !p.input_tokens.contains(&p.input_tokens_base) {
            return bad(
                ATTRS,
                format!("{what}: `input_tokens_base` must be one of its `input_tokens` fields"),
            );
        }
        if let Some(d) = first_duplicate(&p.input_tokens) {
            return bad(
                ATTRS,
                format!("{what}: duplicate input field `{d}` would be counted twice"),
            );
        }
        for v in p.stop_reason.values() {
            if !stop_values.contains(&v.as_str()) {
                return bad(
                    ATTRS,
                    format!(
                        "{what} maps a stop value to `{v}`, which `acn.call.stop_reason` does not list"
                    ),
                );
            }
            if HARNESS_ONLY_STOPS.contains(&v.as_str()) {
                return bad(
                    ATTRS,
                    format!(
                        "{what} maps a stop value to `{v}`: only the harness observes that the client or the link cut a call (TRC-12)"
                    ),
                );
            }
        }
    }
    Ok(())
}

impl Inventory {
    /// Parse and validate an inventory. `semconv_version` is the pin it reports.
    pub fn parse(text: &str, semconv_version: &str) -> Result<Self> {
        let file: InventoryFile = toml::from_str(text).map_err(|e| SchemaError::Parse {
            file: ATTRS,
            message: e.to_string(),
        })?;
        if file.schema_version != 1 {
            return bad(
                ATTRS,
                format!("schema_version {} is not supported", file.schema_version),
            );
        }
        let mut seen = BTreeSet::new();
        check_spans(&file, &mut seen)?;
        check_events(&file, &mut seen)?;
        check_attributes(&file, &mut seen)?;
        check_options(&file)?;
        check_providers(&file)?;
        Ok(Self {
            semconv_version: semconv_version.trim().to_owned(),
            spans: file.span,
            events: file.event,
            attributes: file.attribute,
            options: file.option,
            providers: file.provider,
        })
    }

    #[must_use]
    pub fn semconv_version(&self) -> &str {
        &self.semconv_version
    }
    #[must_use]
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }
    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }
    #[must_use]
    pub fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }
    #[must_use]
    pub fn options(&self) -> &[RunOption] {
        &self.options
    }
    #[must_use]
    pub fn providers(&self) -> &[Provider] {
        &self.providers
    }
    #[must_use]
    pub fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|a| a.name == name)
    }
    #[must_use]
    pub fn option(&self, name: &str) -> Option<&RunOption> {
        self.options.iter().find(|o| o.name == name)
    }
    #[must_use]
    pub fn provider(&self, name: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.name == name)
    }
    /// The attributes that have a typed column in `spans.parquet` (TRC-25), in file
    /// order, which is the column order (stated in the header of the data file).
    pub fn promoted(&self) -> impl Iterator<Item = &Attribute> {
        self.attributes.iter().filter(|a| a.promoted)
    }
    /// Every `acn.` name the inventory defines: spans, events and attributes.
    pub fn all_names(&self) -> impl Iterator<Item = &str> {
        self.spans
            .iter()
            .map(|s| s.name.as_str())
            .chain(self.events.iter().map(|e| e.name.as_str()))
            .chain(self.attributes.iter().map(|a| a.name.as_str()))
            .filter(|n| n.starts_with("acn."))
    }
}

/// The inventory embedded in this build.
pub fn inventory() -> Result<Inventory> {
    Inventory::parse(ATTRIBUTES_TOML, SEMCONV_VERSION_FILE)
}

/// The Parquet writer settings (TRC-25). They are fixed here, in the frozen module,
/// because each one changes the bytes of every bundle: a change moves `engine_hash`.
/// The writer sets every one of them explicitly rather than inheriting a library
/// default, so that a dependency bump cannot change them silently (`build_hash`
/// records the library version, CON-31).
pub mod parquet {
    /// Compression codec and level: zstd, level 3.
    pub const ZSTD_LEVEL: i32 = 3;
    /// Rows per row group.
    pub const ROW_GROUP_ROWS: usize = 65_536;
    /// Column statistics are written, at page level.
    pub const PAGE_STATISTICS: bool = true;
    /// Dictionary encoding is enabled for every column.
    pub const DICTIONARY: bool = true;
    /// Target size of a data page, in bytes.
    pub const DATA_PAGE_BYTES: usize = 1024 * 1024;
    /// Target size of a dictionary page, in bytes.
    pub const DICTIONARY_PAGE_BYTES: usize = 1024 * 1024;
    /// Parquet format writer version: `1.0` pages.
    pub const WRITER_VERSION_1_0: bool = true;
    /// The `created_by` field of the file footer. Fixed, so that it names the
    /// format's owner and never a time or a host; the library version is recorded
    /// in `build_hash` instead.
    pub const CREATED_BY: &str = "acn-bench acn-trace (SPEC 010)";
    /// The union members of the `attrs` map value, in order, by OTLP value type.
    /// Parquet has no union type, so the value is a struct of these nullable
    /// members with exactly one set (ADR-13).
    pub const ATTR_MEMBERS: &[&str] = &["string", "int", "float", "bool", "bytes"];
}

/// One column of a derived view (TRC-37).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub unit: String,
    pub nullable: bool,
    #[serde(default)]
    pub source: String,
}

/// One derived view.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub name: String,
    pub file: String,
    pub row_per: String,
    pub requirement: String,
    #[serde(rename = "column", default)]
    pub columns: Vec<Column>,
}

impl View {
    #[must_use]
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|c| c.name == name)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewsFile {
    schema_version: u32,
    #[serde(default)]
    view: Vec<View>,
}

/// The column types a view may use.
pub const VIEW_TYPES: &[&str] = &[
    "utf8",
    "int64",
    "float64",
    "bool",
    "fixed_size_binary(8)",
    "map<utf8,int64>",
];

/// Fragments of a `source` that would mean message content (TRC-42).
const CONTENT_SOURCES: &[&str] = &[
    "messages",
    "gen_ai.input",
    "gen_ai.output",
    "gen_ai.system_instructions",
];

/// The parsed and validated view schema.
#[derive(Debug, Clone)]
pub struct Views(Vec<View>);

impl Views {
    /// Parse and validate the file by itself. Use [`Views::parse_checked`] to also
    /// check it against an inventory.
    pub fn parse(text: &str) -> Result<Self> {
        let file: ViewsFile = toml::from_str(text).map_err(|e| SchemaError::Parse {
            file: VIEWS,
            message: e.to_string(),
        })?;
        if file.schema_version != 1 {
            return bad(
                VIEWS,
                format!("schema_version {} is not supported", file.schema_version),
            );
        }
        let mut names = BTreeSet::new();
        for v in &file.view {
            if !names.insert(v.name.as_str()) {
                return bad(VIEWS, format!("duplicate view `{}`", v.name));
            }
            let expected = format!("views/{}.parquet", v.name);
            if v.file != expected {
                return bad(
                    VIEWS,
                    format!(
                        "view `{}` must be written to `{expected}` (TRC-22), not `{}`",
                        v.name, v.file
                    ),
                );
            }
            if v.columns.is_empty() {
                return bad(VIEWS, format!("view `{}` has no columns", v.name));
            }
            if !is_requirement(&v.requirement) {
                return bad(
                    VIEWS,
                    format!(
                        "view `{}`: `{}` is not a requirement ID",
                        v.name, v.requirement
                    ),
                );
            }
            let mut cols = BTreeSet::new();
            for c in &v.columns {
                if !cols.insert(c.name.as_str()) {
                    return bad(VIEWS, format!("duplicate column `{}.{}`", v.name, c.name));
                }
                if !VIEW_TYPES.contains(&c.ty.as_str()) {
                    return bad(
                        VIEWS,
                        format!(
                            "`{}.{}`: `{}` is not a view column type",
                            v.name, c.name, c.ty
                        ),
                    );
                }
                if c.name.ends_with("_ns") && (c.ty != "int64" || c.unit != "ns") {
                    return bad(
                        VIEWS,
                        format!(
                            "`{}.{}`: a `_ns` column is int64 nanoseconds",
                            v.name, c.name
                        ),
                    );
                }
            }
        }
        Ok(Self(file.view))
    }

    /// Parse, then check every column against `inv` (TRC-37): a `row_per` that is a
    /// declared span; no message content (TRC-42); and, where `source` (up to any `;`
    /// note) is exactly an attribute name, a column that is nullable exactly when the attribute is
    /// optional and whose type can hold it.
    pub fn parse_checked(text: &str, inv: &Inventory) -> Result<Self> {
        let views = Self::parse(text)?;
        for v in &views.0 {
            if !inv.spans().iter().any(|s| s.name == v.row_per) {
                return bad(
                    VIEWS,
                    format!(
                        "view `{}` has one row per `{}`, which is not a declared span",
                        v.name, v.row_per
                    ),
                );
            }
            for c in &v.columns {
                let at = format!("`{}.{}`", v.name, c.name);
                if CONTENT_SOURCES.iter().any(|w| c.source.contains(w)) {
                    return bad(
                        VIEWS,
                        format!(
                            "{at}: message content MUST never be promoted into a view (TRC-42)"
                        ),
                    );
                }
                // The attribute a column copies is the part of `source` before any
                // `;` note ("acn.tool.requesting_call; never inferred from timestamps").
                let key = c.source.split(';').next().unwrap_or("").trim();
                let Some(a) = inv.attribute(key) else {
                    if key.starts_with("acn.") && !key.contains(' ') {
                        return bad(
                            VIEWS,
                            format!("{at} reads `{key}`, which the inventory does not list"),
                        );
                    }
                    continue;
                };
                if c.nullable == a.required {
                    return bad(
                        VIEWS,
                        format!(
                            "{at}: nullable must be {} because `{}` is {}",
                            !a.required,
                            a.name,
                            if a.required { "required" } else { "optional" }
                        ),
                    );
                }
                let fits = match a.ty {
                    ValueType::String => c.ty == "utf8",
                    ValueType::Int => c.ty == "int64",
                    // A `_ms` float becomes an integer-nanosecond column (file header).
                    ValueType::Float => {
                        c.ty == "float64"
                            || (a.name.ends_with("_ms")
                                && c.name.ends_with("_ns")
                                && c.ty == "int64")
                    }
                    ValueType::Bool => c.ty == "bool",
                    ValueType::Bytes => false,
                };
                if !fits {
                    return bad(
                        VIEWS,
                        format!(
                            "{at}: the type `{}` cannot hold `{}`, which is a {:?}",
                            c.ty, a.name, a.ty
                        ),
                    );
                }
            }
        }
        Ok(views)
    }

    pub fn iter(&self) -> impl Iterator<Item = &View> {
        self.0.iter()
    }

    #[must_use]
    pub fn view(&self, name: &str) -> Option<&View> {
        self.0.iter().find(|v| v.name == name)
    }
}

/// The view schema embedded in this build, checked against the embedded inventory.
pub fn views() -> Result<Views> {
    Views::parse_checked(VIEWS_TOML, &inventory()?)
}
