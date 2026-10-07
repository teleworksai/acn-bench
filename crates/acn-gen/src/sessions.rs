//! The generator's sessions (SPEC 050 GEN-10 to GEN-15) as a driver of the
//! harness's run path (GEN-20): every call goes through the harness's
//! `Replicate::call`, so retries, usage, `chat` spans and link spans are the
//! harness's own; this module decides what each call sends.

use std::cell::Cell;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use acn_harness::HarnessError;
use acn_harness::agent::{CallState, Called, Cx, Replicate};
use acn_harness::context::{Context, Msg, ToolCall, ToolChoice, ToolDef};
use acn_harness::env::{Env, join_all};
use acn_harness::run::{Driver, RunConfig};
use acn_harness::wire::Reply;
use acn_harness::workload::{Agent, Workload};
use acn_trace::identity::Digest;
use opentelemetry::KeyValue;
use opentelemetry::trace::{Span as _, SpanKind, TraceContextExt as _, Tracer as _};

use crate::plan::{self, Chain};
use crate::sheet::{SUBAGENT, Sheet};
use crate::text::words;

/// A span time from run-clock nanoseconds.
fn at(ns: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_nanos(u64::try_from(ns).unwrap_or(0))
}

fn int(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// A duration in milliseconds, for an attribute.
fn ms(ns: i64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let v = ns as f64 / 1_000_000.0;
    v
}

fn tool_name(class: &str) -> String {
    format!("gen_{class}")
}

/// The generator as a driver: a sheet and its hash, and what it made.
pub struct GenDriver {
    sheet: Sheet,
    hash: Digest,
    sessions: Cell<u64>,
    calls: Cell<u64>,
}

impl GenDriver {
    /// The driver of `sheet`, whose file's BLAKE3 is `hash` (GEN-21).
    #[must_use]
    pub fn new(sheet: Sheet, hash: Digest) -> Self {
        Self {
            sheet,
            hash,
            sessions: Cell::new(0),
            calls: Cell::new(0),
        }
    }

    /// Sessions started and calls made so far, over every replicate.
    #[must_use]
    pub fn counts(&self) -> (u64, u64) {
        (self.sessions.get(), self.calls.get())
    }

    fn count_call(&self) {
        self.calls.set(self.calls.get() + 1);
    }

    /// The tools every call carries (GEN-11), in TRC-13's order. Each
    /// description starts with the replicate's isolation marker, as HAR-42
    /// requires of every tool definition (ADR-38).
    fn tools(&self, marker: &str, with_subagent: bool) -> Vec<ToolDef> {
        self.sheet
            .tool_classes()
            .into_iter()
            .filter(|c| with_subagent || *c != SUBAGENT)
            .map(|c| ToolDef {
                name: tool_name(c),
                description: format!("{marker} A {c} tool."),
                parameters: serde_json_object(),
            })
            .collect()
    }
}

fn serde_json_object() -> serde_json::Value {
    serde_json::json!({ "type": "object", "properties": {} })
}

fn assistant(r: &Reply) -> Msg {
    Msg::Assistant {
        text: r.text.clone(),
        tool_calls: r.tool_calls.clone(),
    }
}

/// What a session's turns share: the system words and the tools (GEN-11,
/// GEN-13), and its run seed.
struct Shared<'a> {
    system: &'a str,
    tools: &'a [ToolDef],
    child_tools: &'a [ToolDef],
    seed: i64,
}

/// One lineage of a session: its messages, its call state and the ordinal of
/// its next main-lineage tool result (HAR-13).
struct Lineage {
    messages: Vec<Msg>,
    state: CallState,
    ordinal: u64,
}

impl Driver for GenDriver {
    fn workload(&self, _cfg: &RunConfig) -> Result<Workload, HarnessError> {
        // The sheet stands for the workload: its hash enters `run_id`, and
        // its calls stream at temperature 0 (ADR-38). The generator builds
        // its own prompts, tools and tasks, so the agent's are empty.
        Ok(Workload {
            agent: Agent {
                system_prompt: String::new(),
                temperature: 0.0,
                max_tokens: 0,
                stream: true,
                max_calls_per_turn: 0,
                compact_at_tokens: self.sheet.compact_at_tokens,
                read_cost_threshold_tokens: 0,
                summary_instruction: String::new(),
                summary_max_tokens: self.sheet.summary_max_tokens,
            },
            tools: Vec::new(),
            tasks: Vec::new(),
            hash: self.hash,
        })
    }

    fn producer(&self) -> (&'static str, &'static str) {
        ("acn-gen", env!("CARGO_PKG_VERSION"))
    }

    fn knobs_fixed(&self) -> bool {
        true
    }

    async fn replicate<E: Env>(
        &self,
        rep: &Replicate<'_, E>,
        seed: i64,
    ) -> Result<(), HarnessError> {
        let origin = rep.env.now();
        let mut system_rng = plan::system_stream(rep.seed).map_err(gen_err)?;
        let system = words(&mut system_rng, self.sheet.system_tokens);
        let tools = self.tools(&rep.marker, true);
        let child_tools = self.tools(&rep.marker, false);
        let shared = Shared {
            system: &system,
            tools: &tools,
            child_tools: &child_tools,
            seed,
        };
        // GEN-10: every session at once, each from its own start.
        let sessions = (0..self.sheet.sessions)
            .map(|k| self.session(rep, &shared, k, origin))
            .collect();
        for r in join_all(sessions).await {
            r?;
        }
        Ok(())
    }
}

fn gen_err(e: crate::GenError) -> HarnessError {
    HarnessError::Internal(e.to_string())
}

/// How a chain ended (GEN-15).
enum Ended {
    /// Every call got a response; the last answer's text.
    Answered(Reply),
    /// A call failed after its retries (HAR-24).
    Aborted,
}

impl GenDriver {
    /// Session `k` (GEN-10).
    async fn session<E: Env>(
        &self,
        rep: &Replicate<'_, E>,
        shared: &Shared<'_>,
        k: u64,
        origin: i64,
    ) -> Result<(), HarnessError> {
        let sp = plan::session(&self.sheet, rep.seed, k).map_err(gen_err)?;
        let start = origin.saturating_add(int(sp.start_ns));
        rep.env.sleep_until(start).await;
        self.sessions.set(self.sessions.get() + 1);
        let mut attrs = rep.setup.session_attrs.clone();
        attrs.push(KeyValue::new("acn.seed", shared.seed));
        attrs.push(KeyValue::new("acn.replicate", i64::from(rep.replicate)));
        let span = rep
            .tracer
            .span_builder("acn.session")
            .with_kind(SpanKind::Internal)
            .with_start_time(at(start))
            .with_attributes(attrs)
            .start(rep.tracer);
        let session_cx = Cx::new().with_span(span);
        let mut lin = Lineage {
            messages: Vec::new(),
            state: CallState::default(),
            ordinal: 0,
        };
        for t in 0..sp.turns {
            if let Some(think) = t
                .checked_sub(1)
                .and_then(|i| usize::try_from(i).ok())
                .and_then(|i| sp.think_ns.get(i))
            {
                let now = rep.env.now();
                rep.env.sleep_until(now.saturating_add(int(*think))).await;
            }
            self.turn(rep, shared, k, t, &mut lin, &session_cx).await?;
        }
        session_cx.span().end_with_timestamp(at(rep.env.now()));
        Ok(())
    }

    /// The context of a call on the main lineage or a sub-agent's.
    fn context(
        rep_system: String,
        tools: &[ToolDef],
        messages: &[Msg],
        choice: ToolChoice,
    ) -> Context {
        Context {
            system: rep_system,
            tools: tools.to_vec(),
            messages: messages.to_vec(),
            tool_choice: Some(choice),
        }
    }

    /// Turn `t` of session `k` (GEN-11 to GEN-15).
    #[allow(clippy::too_many_lines)]
    async fn turn<E: Env>(
        &self,
        rep: &Replicate<'_, E>,
        shared: &Shared<'_>,
        k: u64,
        t: u64,
        lin: &mut Lineage,
        session_cx: &Cx,
    ) -> Result<(), HarnessError> {
        let p = plan::turn(&self.sheet, rep.seed, k, t).map_err(gen_err)?;
        let mut text = plan::text_stream(rep.seed, k, t, 0).map_err(gen_err)?;
        let start = rep.env.now();
        let span = rep
            .tracer
            .span_builder("acn.turn")
            .with_kind(SpanKind::Internal)
            .with_start_time(at(start))
            .start_with_context(rep.tracer, session_cx);
        let cx = session_cx.with_span(span);
        lin.state.call_index = 0;
        let mut turn_first = lin.messages.len();
        lin.messages.push(Msg::User {
            text: words(&mut text, p.main.user_tokens),
        });
        let system = rep.system(shared.system, start);
        let mut compacted = false;
        let mut useful = None;
        let mut aborted = false;
        // The tool calls, then the answer (GEN-11).
        let steps: Vec<Option<&plan::ToolStep>> = p
            .main
            .tools
            .iter()
            .map(Some)
            .chain(std::iter::once(None))
            .collect();
        for step in steps {
            // GEN-14: HAR-4's compaction, HAR-15's `window_full` trigger.
            let probe = Self::context(
                system.clone(),
                shared.tools,
                &lin.messages,
                ToolChoice::Forbid,
            );
            let window = (probe.canonical_bytes().len() as u64).div_ceil(4);
            if self.sheet.compact_at_tokens > 0 && window >= self.sheet.compact_at_tokens {
                let mut cctx = probe;
                cctx.messages.push(Msg::User {
                    text: words(&mut text, self.sheet.summary_instruction_tokens),
                });
                let c = rep
                    .call(
                        &mut lin.state,
                        cctx,
                        self.sheet.summary_max_tokens,
                        None,
                        &cx,
                    )
                    .await?;
                self.count_call();
                let Some(r) = c.reply else {
                    aborted = true;
                    break;
                };
                let summary = if r.tool_calls.is_empty() {
                    r.text.unwrap_or_default()
                } else {
                    String::new()
                };
                let current: Vec<Msg> = lin.messages.get(turn_first..).unwrap_or_default().to_vec();
                lin.messages = vec![Msg::User {
                    text: format!("Summary of earlier conversation:\n{summary}"),
                }];
                lin.messages.extend(current);
                turn_first = 1;
                lin.state.last_uncached = None;
                compacted = true;
            }
            let Some(step) = step else {
                // The answer.
                let ctx = Self::context(
                    system.clone(),
                    shared.tools,
                    &lin.messages,
                    ToolChoice::Forbid,
                );
                let c = rep
                    .call(&mut lin.state, ctx, p.main.answer_tokens, None, &cx)
                    .await?;
                self.count_call();
                match c.reply {
                    Some(r) => {
                        useful = r.token_times.first().map(|f| f - start);
                        lin.messages.push(assistant(&r));
                    }
                    None => aborted = true,
                }
                break;
            };
            let ctx = Self::context(
                system.clone(),
                shared.tools,
                &lin.messages,
                ToolChoice::Only(tool_name(step.class)),
            );
            let c = rep
                .call(&mut lin.state, ctx, p.main.answer_tokens, None, &cx)
                .await?;
            self.count_call();
            let Some(reply) = c.reply.clone() else {
                aborted = true;
                break;
            };
            lin.messages.push(assistant(&reply));
            // A text reply where a tool call was asked for has no tool to run
            // (ADR-38).
            let Some(tc) = reply.tool_calls.first().cloned() else {
                continue;
            };
            let (content, failed) = if step.class == SUBAGENT {
                self.fan_out(rep, shared, &system, k, t, &p.children, &c, &tc, &cx)
                    .await?
            } else {
                (
                    self.run_tool(rep, &tc, step, &mut text, c.index, &cx).await,
                    false,
                )
            };
            // The tool call always gets its result, the sub-agents' partial
            // answers when one of them failed, so no later call carries a
            // tool call without one (ADR-38).
            lin.messages.push(Msg::ToolResult {
                call_id: tc.id.clone(),
                tool: tc.name.clone(),
                ordinal: Some(lin.ordinal),
                content,
            });
            lin.ordinal += 1;
            if failed {
                aborted = true;
                break;
            }
        }
        let outcome = if aborted { "aborted" } else { "success" };
        let span = cx.span();
        span.set_attribute(KeyValue::new("acn.turn.index", int(t)));
        span.set_attribute(KeyValue::new("acn.turn.outcome", outcome));
        span.set_attribute(KeyValue::new(
            "acn.turn.compaction",
            if compacted { "window_full" } else { "none" },
        ));
        if !aborted && let Some(u) = useful {
            span.set_attribute(KeyValue::new("acn.turn.first_useful_result_ms", ms(u)));
        }
        span.end_with_timestamp(at(rep.env.now()));
        Ok(())
    }

    /// One tool of a drawn class: wait its duration, return its text, record
    /// its span (GEN-11, TRC-13).
    async fn run_tool<E: Env>(
        &self,
        rep: &Replicate<'_, E>,
        tc: &ToolCall,
        step: &plan::ToolStep,
        text: &mut rand_chacha::ChaCha20Rng,
        requesting: i64,
        parent: &Cx,
    ) -> String {
        let start = rep.env.now();
        rep.env
            .sleep_until(start.saturating_add(int(step.duration_ns)))
            .await;
        let content = words(text, step.result_tokens);
        let mut span = rep
            .tracer
            .span_builder("execute_tool")
            .with_kind(SpanKind::Internal)
            .with_start_time(at(start))
            .with_attributes(vec![
                KeyValue::new("gen_ai.tool.name", tc.name.clone()),
                KeyValue::new("gen_ai.tool.call.id", tc.id.clone()),
                KeyValue::new("acn.tool.class", step.class),
                KeyValue::new("acn.tool.result_bytes", int(content.len() as u64)),
                KeyValue::new("acn.tool.placement", "local"),
                KeyValue::new("acn.tool.requesting_call", requesting),
            ])
            .start_with_context(rep.tracer, parent);
        span.end_with_timestamp(at(rep.env.now()));
        content
    }

    /// GEN-12: the turn's sub-agents, spawned by its first tool call as HAR-5
    /// spawns them; their answers, joined in index order, are its result.
    /// The result, and whether a sub-agent's call failed after its retries.
    /// Sub-agents share the turn's system prompt, as the harness's do.
    #[allow(clippy::too_many_arguments)]
    async fn fan_out<E: Env>(
        &self,
        rep: &Replicate<'_, E>,
        shared: &Shared<'_>,
        system: &str,
        k: u64,
        t: u64,
        children: &[Chain],
        spawning: &Called,
        tc: &ToolCall,
        turn_cx: &Cx,
    ) -> Result<(String, bool), HarnessError> {
        // Every sub-agent's first context and its comparison bytes, before
        // any span opens, so an error leaves no span open.
        let mut firsts = Vec::with_capacity(children.len());
        for (i, chain) in children.iter().enumerate() {
            let c = (i as u64).saturating_add(1);
            let mut text = plan::text_stream(rep.seed, k, t, c).map_err(gen_err)?;
            let messages = vec![Msg::User {
                text: words(&mut text, chain.user_tokens),
            }];
            let first = Self::context(
                system.to_owned(),
                shared.child_tools,
                &messages,
                ToolChoice::Forbid,
            );
            let child_bytes = rep.compared(&first)?;
            // As the harness counts it: by the spawning call's tokens when it
            // reported them, else by whole 4-byte tokens (TRC-14).
            let basis = spawning
                .input_tokens
                .unwrap_or_else(|| (spawning.compared.len() as u64).div_ceil(4));
            let shared_tokens = rep.shared_tokens(
                &spawning.compared,
                &child_bytes,
                (basis, spawning.compared.len()),
            );
            firsts.push((chain, text, messages, shared_tokens));
        }
        let start = rep.env.now();
        let tool_span = rep
            .tracer
            .span_builder("execute_tool")
            .with_kind(SpanKind::Internal)
            .with_start_time(at(start))
            .with_attributes(vec![
                KeyValue::new("gen_ai.tool.name", tc.name.clone()),
                KeyValue::new("gen_ai.tool.call.id", tc.id.clone()),
                KeyValue::new("acn.tool.class", SUBAGENT),
                KeyValue::new("acn.tool.placement", "local"),
                KeyValue::new("acn.tool.requesting_call", spawning.index),
            ])
            .start_with_context(rep.tracer, turn_cx);
        let tool_cx = turn_cx.with_span(tool_span);
        let width = children.len() as u64;
        let mut runs = Vec::with_capacity(firsts.len());
        for (chain, text, messages, shared_tokens) in firsts {
            let span = rep
                .tracer
                .span_builder("invoke_agent")
                .with_kind(SpanKind::Internal)
                .with_start_time(at(start))
                .with_attributes(vec![
                    KeyValue::new("gen_ai.operation.name", "invoke_agent"),
                    KeyValue::new("acn.fanout.parent_turn", int(t)),
                    KeyValue::new("acn.fanout.parent_call", spawning.index),
                    KeyValue::new("acn.fanout.depth", 1i64),
                    KeyValue::new("acn.fanout.width", int(width)),
                    KeyValue::new("acn.fanout.shared_prefix_tokens", int(shared_tokens)),
                ])
                .start_with_context(rep.tracer, &tool_cx);
            // TRC-12: a sub-agent's first call is compared with the spawning
            // context.
            let state = CallState {
                prev: Some(spawning.compared.clone()),
                ..CallState::default()
            };
            runs.push(self.sub_agent(
                rep,
                shared,
                system.to_owned(),
                chain,
                messages,
                state,
                text,
                tool_cx.with_span(span),
            ));
        }
        let mut answers = Vec::with_capacity(runs.len());
        let mut failed = false;
        for a in join_all(runs).await {
            match a? {
                Ended::Answered(r) => answers.push(r.text.unwrap_or_default()),
                Ended::Aborted => failed = true,
            }
        }
        let result = answers.join("\n");
        let span = tool_cx.span();
        span.set_attribute(KeyValue::new(
            "acn.tool.result_bytes",
            int(result.len() as u64),
        ));
        span.end_with_timestamp(at(rep.env.now()));
        Ok((result, failed))
    }

    /// One sub-agent's chain (GEN-12): its tool calls, then its answer, on
    /// the tools without `subagent`.
    #[allow(clippy::too_many_arguments)]
    async fn sub_agent<E: Env>(
        &self,
        rep: &Replicate<'_, E>,
        shared: &Shared<'_>,
        system: String,
        chain: &Chain,
        mut messages: Vec<Msg>,
        mut state: CallState,
        mut text: rand_chacha::ChaCha20Rng,
        cx: Cx,
    ) -> Result<Ended, HarnessError> {
        let mut ended = Ended::Aborted;
        for step in chain.tools.iter().map(Some).chain(std::iter::once(None)) {
            let choice = step.map_or(ToolChoice::Forbid, |s| ToolChoice::Only(tool_name(s.class)));
            let ctx = Self::context(system.clone(), shared.child_tools, &messages, choice);
            let c = rep
                .call(&mut state, ctx, chain.answer_tokens, None, &cx)
                .await?;
            self.count_call();
            let Some(reply) = c.reply.clone() else {
                ended = Ended::Aborted;
                break;
            };
            messages.push(assistant(&reply));
            let Some(step) = step else {
                ended = Ended::Answered(reply);
                break;
            };
            let Some(tc) = reply.tool_calls.first().cloned() else {
                continue;
            };
            let content = self.run_tool(rep, &tc, step, &mut text, c.index, &cx).await;
            messages.push(Msg::ToolResult {
                call_id: tc.id.clone(),
                tool: tc.name.clone(),
                ordinal: None,
                content,
            });
        }
        cx.span().end_with_timestamp(at(rep.env.now()));
        Ok(ended)
    }
}
