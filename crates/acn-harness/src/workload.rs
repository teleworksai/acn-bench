//! Workloads (HAR-60): what the harness does in a run, as one strict TOML file
//! whose BLAKE3 is the run's `workload_hash` (CON-27(a)).

use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::Value;

use crate::HarnessError;

/// The tool classes of TRC-13.
pub const TOOL_CLASSES: &[&str] = &[
    "file", "shell", "search", "http", "subagent", "testbed", "other",
];

/// An inclusive integer range drawn uniformly (HAR-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
    pub min: u64,
    pub max: u64,
}

/// The agent's prompt, call parameters and thresholds.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    pub system_prompt: String,
    pub temperature: f64,
    pub max_tokens: u64,
    pub stream: bool,
    pub max_calls_per_turn: u64,
    pub compact_at_tokens: u64,
    pub read_cost_threshold_tokens: u64,
    pub summary_instruction: String,
    pub summary_max_tokens: u64,
}

/// What a `subagent` tool's children do (HAR-5).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Child {
    /// The child's own system prompt, used under `per_child` (HAR-14).
    pub system_prompt: String,
    pub instruction: String,
    /// The child's tools, by name, in presentation order; none may be a subagent.
    pub tools: Vec<String>,
    pub max_calls: u64,
}

/// One simulated tool (HAR-2).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub name: String,
    pub class: String,
    pub description: String,
    /// A JSON schema, written as a TOML table.
    pub parameters: toml::Table,
    /// Absent for a `subagent` tool, whose result is its children's answers.
    pub result_bytes: Option<Range>,
    /// Absent for a `subagent` tool, whose duration is its children's.
    pub duration_ns: Option<Range>,
    /// A `subagent` tool only: how many children it spawns.
    pub width: Option<u64>,
    /// A `subagent` tool only.
    pub child: Option<Child>,
}

impl Tool {
    /// The parameters schema as JSON.
    pub fn parameters_json(&self) -> Result<Value, HarnessError> {
        serde_json::to_value(&self.parameters)
            .map_err(|e| HarnessError::Workload(format!("tool `{}`: {e}", self.name)))
    }

    #[must_use]
    pub fn is_subagent(&self) -> bool {
        self.class == "subagent"
    }
}

/// One scripted user turn.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    pub user: String,
    pub think_time_ns: Range,
    #[serde(default)]
    pub deadline_ms: Option<u64>,
    /// The checker (HAR-3): tools the turn must call to succeed.
    #[serde(default)]
    pub expect_tools: Vec<String>,
    /// Earlier tool results of the session, by ordinal, that changed (HAR-13).
    #[serde(default)]
    pub updates: Vec<u64>,
}

/// One scripted session.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    /// The tools the agent has in this task, by name, in presentation order.
    pub tools: Vec<String>,
    #[serde(rename = "turn")]
    pub turns: Vec<Turn>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    schema_version: u32,
    agent: Agent,
    #[serde(rename = "tool", default)]
    tools: Vec<Tool>,
    #[serde(rename = "task")]
    tasks: Vec<Task>,
}

/// A checked workload and its hash.
#[derive(Debug, Clone)]
pub struct Workload {
    pub agent: Agent,
    pub tools: Vec<Tool>,
    pub tasks: Vec<Task>,
    /// BLAKE3 of the file's bytes (CON-27(a)).
    pub hash: acn_trace::identity::Digest,
}

fn bad<T>(m: impl Into<String>) -> Result<T, HarnessError> {
    Err(HarnessError::Workload(m.into()))
}

fn check_range(what: &str, r: Range) -> Result<(), HarnessError> {
    if r.min > r.max {
        return bad(format!("{what}: min {} exceeds max {}", r.min, r.max));
    }
    Ok(())
}

impl Workload {
    /// Read and check a workload file.
    pub fn load(path: &std::path::Path) -> Result<Self, HarnessError> {
        let bytes = std::fs::read(path)
            .map_err(|e| HarnessError::Workload(format!("{}: {e}", path.display())))?;
        Self::parse(&bytes)
    }

    /// Parse and check a workload from its bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, HarnessError> {
        let text = std::str::from_utf8(bytes)
            .map_err(|e| HarnessError::Workload(format!("not UTF-8: {e}")))?;
        let f: File = toml::from_str(text).map_err(|e| HarnessError::Workload(e.to_string()))?;
        if f.schema_version != 1 {
            return bad(format!(
                "schema_version {} is not supported",
                f.schema_version
            ));
        }
        let a = &f.agent;
        // CON-27(c) and serde_json agree on every float in this range (ADR-17).
        if !(0.0..=2.0).contains(&a.temperature) {
            return bad("agent.temperature must lie in [0, 2]");
        }
        for (name, v) in [
            ("max_tokens", a.max_tokens),
            ("max_calls_per_turn", a.max_calls_per_turn),
            ("compact_at_tokens", a.compact_at_tokens),
            ("summary_max_tokens", a.summary_max_tokens),
        ] {
            if v == 0 {
                return bad(format!("agent.{name} must be positive"));
            }
        }
        let mut names = BTreeSet::new();
        for t in &f.tools {
            let at = format!("tool `{}`", t.name);
            if t.name.is_empty() || !names.insert(t.name.as_str()) {
                return bad(format!("{at}: a tool name is non-empty and unique"));
            }
            if !TOOL_CLASSES.contains(&t.class.as_str()) {
                return bad(format!(
                    "{at}: class `{}` is not one of {TOOL_CLASSES:?}",
                    t.class
                ));
            }
            t.parameters_json()?;
            if t.is_subagent() {
                let (Some(width), Some(child)) = (t.width, &t.child) else {
                    return bad(format!("{at}: a subagent tool states `width` and `child`"));
                };
                if t.result_bytes.is_some() || t.duration_ns.is_some() {
                    return bad(format!(
                        "{at}: a subagent tool's result and duration are its children's"
                    ));
                }
                if width == 0 || child.max_calls == 0 {
                    return bad(format!("{at}: width and child.max_calls must be positive"));
                }
            } else {
                let (Some(r), Some(d)) = (t.result_bytes, t.duration_ns) else {
                    return bad(format!("{at}: states `result_bytes` and `duration_ns`"));
                };
                check_range(&format!("{at}.result_bytes"), r)?;
                check_range(&format!("{at}.duration_ns"), d)?;
                if t.width.is_some() || t.child.is_some() {
                    return bad(format!("{at}: only a subagent tool has width and child"));
                }
            }
        }
        let tool = |n: &str| f.tools.iter().find(|t| t.name == n);
        for t in &f.tools {
            if let Some(c) = &t.child {
                for n in &c.tools {
                    match tool(n) {
                        Some(ct) if !ct.is_subagent() => {}
                        Some(_) => {
                            return bad(format!(
                                "tool `{}`: a child cannot spawn sub-agents (HAR-5)",
                                t.name
                            ));
                        }
                        None => {
                            return bad(format!("tool `{}`: child tool `{n}` is unknown", t.name));
                        }
                    }
                }
            }
        }
        if f.tasks.is_empty() {
            return bad("a workload has at least one task");
        }
        let mut ids = BTreeSet::new();
        for task in &f.tasks {
            let at = format!("task `{}`", task.id);
            if task.id.is_empty() || !ids.insert(task.id.as_str()) {
                return bad(format!("{at}: a task id is non-empty and unique"));
            }
            if task.turns.is_empty() {
                return bad(format!("{at}: has no turn"));
            }
            let mut seen = BTreeSet::new();
            for n in &task.tools {
                if tool(n).is_none() || !seen.insert(n) {
                    return bad(format!("{at}: tool `{n}` is unknown or listed twice"));
                }
            }
            for (k, turn) in task.turns.iter().enumerate() {
                check_range(&format!("{at} turn {k}: think_time_ns"), turn.think_time_ns)?;
                for n in &turn.expect_tools {
                    if !task.tools.contains(n) {
                        return bad(format!(
                            "{at} turn {k}: expected tool `{n}` is not one of the task's tools"
                        ));
                    }
                }
                if turn.deadline_ms == Some(0) {
                    return bad(format!("{at} turn {k}: a deadline is positive"));
                }
            }
        }
        Ok(Self {
            agent: f.agent,
            tools: f.tools,
            tasks: f.tasks,
            hash: acn_trace::identity::Digest::of(bytes),
        })
    }

    /// The tool named `name`.
    #[must_use]
    pub fn tool(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.name == name)
    }
}
