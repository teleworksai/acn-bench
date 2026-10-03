//! The six cache-discipline knobs (HAR-10), their domains and defaults.

use std::collections::BTreeMap;

use acn_trace::identity::Value;

use crate::HarnessError;

/// Where `cache_control` breakpoints go (HAR-16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    None,
    SystemOnly,
    SystemAndTools,
    RollingTail,
}

/// How a changed earlier tool result reaches the model (HAR-13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backfill {
    TailRestate,
    MidPrefix,
}

/// What a sub-agent's first context is (HAR-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fanout {
    ForkFromPrefix,
    PerChild,
}

/// When the main lineage compacts (HAR-15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compaction {
    WindowFull,
    ReadCostThreshold,
}

impl Compaction {
    /// The `acn.turn.compaction` value (TRC-11).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WindowFull => "window_full",
            Self::ReadCostThreshold => "read_cost_threshold",
        }
    }
}

/// The knob map in force for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Knobs {
    pub timestamp_in_system_prompt: bool,
    pub tool_order_stable: bool,
    pub backfill_mode: Backfill,
    pub fanout_prompting: Fanout,
    pub compaction_trigger: Compaction,
    pub cache_breakpoint_placement: Placement,
}

/// The domain of a varied parameter (HYP-6).
#[derive(Debug, Clone, PartialEq)]
pub enum Domain {
    Bool,
    Enum(Vec<String>),
    Range { min: f64, max: f64 },
    IntRange { min: i64, max: i64 },
}

impl Domain {
    /// A `[varies]` entry of a hypothesis file. An unknown or missing `kind`,
    /// or a bound or value list of the wrong type, is an error, never a string.
    pub fn parse(name: &str, v: &toml::Value) -> Result<Self, HarnessError> {
        let bad = |m: &str| HarnessError::Config(format!("[varies].{name}: {m}"));
        let kind = v.get("kind").and_then(toml::Value::as_str);
        let float = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_float().or_else(|| x.as_integer().map(|i| i as f64)))
                .ok_or_else(|| bad(&format!("`{k}` is not a number")))
        };
        let integer = |k: &str| {
            v.get(k)
                .and_then(toml::Value::as_integer)
                .ok_or_else(|| bad(&format!("`{k}` is not an integer")))
        };
        match kind {
            Some("bool") => Ok(Self::Bool),
            Some("enum") => {
                let values = v
                    .get("values")
                    .and_then(toml::Value::as_array)
                    .ok_or_else(|| bad("`values` is missing"))?;
                values
                    .iter()
                    .map(|x| {
                        x.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| bad("a value is not a string"))
                    })
                    .collect::<Result<_, _>>()
                    .map(Self::Enum)
            }
            Some("range") => Ok(Self::Range {
                min: float("min")?,
                max: float("max")?,
            }),
            Some("int_range") => Ok(Self::IntRange {
                min: integer("min")?,
                max: integer("max")?,
            }),
            Some(k) => Err(bad(&format!(
                "kind `{k}` is not bool, enum, range or int_range"
            ))),
            None => Err(bad("`kind` is missing")),
        }
    }

    /// `text` as a value of this domain (CON-27(c): a range value is a float),
    /// or an error if it lies outside it.
    pub fn value(&self, name: &str, text: &str) -> Result<Value, HarnessError> {
        let bad = || {
            HarnessError::Knob(format!(
                "`{name}` = `{text}` is outside its domain {self:?}"
            ))
        };
        Ok(match self {
            Self::Bool => Value::Bool(text.parse().map_err(|_| bad())?),
            Self::Enum(values) if values.iter().any(|v| v == text) => Value::Str(text.to_owned()),
            Self::Enum(_) => return Err(bad()),
            Self::Range { min, max } => {
                let f: f64 = text.parse().map_err(|_| bad())?;
                if !(f.is_finite() && *min <= f && f <= *max) {
                    return Err(bad());
                }
                Value::Float(f)
            }
            Self::IntRange { min, max } => {
                let i: i64 = text.parse().map_err(|_| bad())?;
                if !(*min <= i && i <= *max) {
                    return Err(bad());
                }
                Value::Int(i)
            }
        })
    }
}

/// The knob names, sorted.
pub const NAMES: &[&str] = &[
    "backfill_mode",
    "cache_breakpoint_placement",
    "compaction_trigger",
    "fanout_prompting",
    "timestamp_in_system_prompt",
    "tool_order_stable",
];

impl Default for Knobs {
    /// The shipped configuration: the control of `hypotheses/p4.toml` (HAR-10).
    fn default() -> Self {
        Self {
            timestamp_in_system_prompt: true,
            tool_order_stable: true,
            backfill_mode: Backfill::MidPrefix,
            fanout_prompting: Fanout::PerChild,
            compaction_trigger: Compaction::WindowFull,
            cache_breakpoint_placement: Placement::SystemOnly,
        }
    }
}

fn enum_value<'a, T: Copy>(
    knob: &str,
    v: &Value,
    table: &'a [(&'a str, T)],
) -> Result<T, HarnessError> {
    if let Value::Str(s) = v
        && let Some((_, t)) = table.iter().find(|(n, _)| n == s)
    {
        return Ok(*t);
    }
    let allowed: Vec<&str> = table.iter().map(|(n, _)| *n).collect();
    Err(HarnessError::Knob(format!(
        "`{knob}` takes one of {allowed:?}, not {}",
        v.to_text().unwrap_or_else(|_| "an unwritable value".into())
    )))
}

fn bool_value(knob: &str, v: &Value) -> Result<bool, HarnessError> {
    match v {
        Value::Bool(b) => Ok(*b),
        _ => Err(HarnessError::Knob(format!("`{knob}` is a bool"))),
    }
}

impl Knobs {
    /// Whether `name` is a knob.
    #[must_use]
    pub fn is_knob(name: &str) -> bool {
        NAMES.contains(&name)
    }

    /// A knob's domain (HAR-10), or `None` for a name that is not a knob.
    #[must_use]
    pub fn domain(name: &str) -> Option<Domain> {
        let e = |v: &[&str]| Some(Domain::Enum(v.iter().map(|s| (*s).to_owned()).collect()));
        match name {
            "timestamp_in_system_prompt" | "tool_order_stable" => Some(Domain::Bool),
            "backfill_mode" => e(&["tail_restate", "mid_prefix"]),
            "fanout_prompting" => e(&["fork_from_prefix", "per_child"]),
            "compaction_trigger" => e(&["window_full", "read_cost_threshold"]),
            "cache_breakpoint_placement" => {
                e(&["none", "system_only", "system_and_tools", "rolling_tail"])
            }
            _ => None,
        }
    }

    /// The knob map of a run: each knob from its `vary.<knob>` value when given,
    /// the default otherwise (HAR-10). Names that are not knobs are ignored here.
    pub fn from_vary(vary: &BTreeMap<String, Value>) -> Result<Self, HarnessError> {
        let mut k = Self::default();
        for (name, v) in vary {
            match name.as_str() {
                "timestamp_in_system_prompt" => k.timestamp_in_system_prompt = bool_value(name, v)?,
                "tool_order_stable" => k.tool_order_stable = bool_value(name, v)?,
                "backfill_mode" => {
                    k.backfill_mode = enum_value(
                        name,
                        v,
                        &[
                            ("tail_restate", Backfill::TailRestate),
                            ("mid_prefix", Backfill::MidPrefix),
                        ],
                    )?;
                }
                "fanout_prompting" => {
                    k.fanout_prompting = enum_value(
                        name,
                        v,
                        &[
                            ("fork_from_prefix", Fanout::ForkFromPrefix),
                            ("per_child", Fanout::PerChild),
                        ],
                    )?;
                }
                "compaction_trigger" => {
                    k.compaction_trigger = enum_value(
                        name,
                        v,
                        &[
                            ("window_full", Compaction::WindowFull),
                            ("read_cost_threshold", Compaction::ReadCostThreshold),
                        ],
                    )?;
                }
                "cache_breakpoint_placement" => {
                    k.cache_breakpoint_placement = enum_value(
                        name,
                        v,
                        &[
                            ("none", Placement::None),
                            ("system_only", Placement::SystemOnly),
                            ("system_and_tools", Placement::SystemAndTools),
                            ("rolling_tail", Placement::RollingTail),
                        ],
                    )?;
                }
                _ => {}
            }
        }
        Ok(k)
    }

    /// The `acn.harness.knobs` value: all six keys, sorted, no whitespace (HAR-10).
    #[must_use]
    pub fn to_json(&self) -> String {
        let s = |v: &str| format!("\"{v}\"");
        let fields = [
            (
                "backfill_mode",
                s(match self.backfill_mode {
                    Backfill::TailRestate => "tail_restate",
                    Backfill::MidPrefix => "mid_prefix",
                }),
            ),
            (
                "cache_breakpoint_placement",
                s(match self.cache_breakpoint_placement {
                    Placement::None => "none",
                    Placement::SystemOnly => "system_only",
                    Placement::SystemAndTools => "system_and_tools",
                    Placement::RollingTail => "rolling_tail",
                }),
            ),
            ("compaction_trigger", s(self.compaction_trigger.as_str())),
            (
                "fanout_prompting",
                s(match self.fanout_prompting {
                    Fanout::ForkFromPrefix => "fork_from_prefix",
                    Fanout::PerChild => "per_child",
                }),
            ),
            (
                "timestamp_in_system_prompt",
                self.timestamp_in_system_prompt.to_string(),
            ),
            ("tool_order_stable", self.tool_order_stable.to_string()),
        ];
        let body: Vec<String> = fields.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect();
        format!("{{{}}}", body.join(","))
    }
}
