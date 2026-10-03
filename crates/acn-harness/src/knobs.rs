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
