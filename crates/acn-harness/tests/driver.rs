//! The driver seam (SPEC 050 GEN-20, GEN-21): another crate's driver decides a
//! run's sessions on the harness's run path, under its own producer name, with
//! the attributes SPEC 010 lists for that producer and the knobs at their
//! defaults.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_harness::HarnessError;
use acn_harness::agent::{CallState, Cx, Replicate};
use acn_harness::context::{Context, Dialect, Encoding, Msg, Sampling, ToolChoice, ToolDef};
use acn_harness::env::Env;
use acn_harness::knobs::Placement;
use acn_harness::run::{Driver, RunConfig, run_driven_blocking};
use acn_harness::workload::Workload;
use acn_trace::model::AttrValue;
use common::{fast_smoke, run_fixture, spans};

/// The agent loop under another producer's name, its knobs fixed.
struct Toy;

impl Driver for Toy {
    fn workload(&self, cfg: &RunConfig) -> Result<Workload, HarnessError> {
        Workload::load(&cfg.workload)
    }
    fn producer(&self) -> (&'static str, &'static str) {
        ("acn-gen", "0.1.0")
    }
    fn knobs_fixed(&self) -> bool {
        true
    }
    async fn replicate<E: Env>(
        &self,
        rep: &Replicate<'_, E>,
        seed: i64,
    ) -> Result<(), HarnessError> {
        for t in 0..rep.setup.workload.tasks.len() {
            rep.session(t, seed).await?;
        }
        Ok(())
    }
}

/// Cites: GEN-20, GEN-21, TRC-19
#[test]
fn a_driver_runs_on_the_run_path_under_its_own_producer() {
    let f = run_fixture(&fast_smoke(), "auto");
    let w = run_driven_blocking(&f.cfg, None, &Toy).unwrap();
    let t = common::read(&w.dir);
    // The spans' resource names the driver's producer.
    let names: Vec<&AttrValue> = t
        .resources
        .iter()
        .filter_map(|r| r.attrs.get("service.name"))
        .collect();
    assert!(
        names.contains(&&AttrValue::String("acn-gen".into())),
        "{names:?}"
    );
    assert!(!names.contains(&&AttrValue::String("acn-harness".into())));
    // Only the attributes SPEC 010 lists for it: the knob map yes, the
    // harness's own run options no.
    for s in spans(&t, "acn.session") {
        assert!(s.attrs.contains_key("acn.harness.knobs"));
        for absent in [
            "acn.harness.endpoint",
            "acn.harness.max_retries",
            "acn.harness.retry_base_ms",
            "acn.harness.request_timeout_ms",
        ] {
            assert!(!s.attrs.contains_key(absent), "{absent}");
        }
    }
    assert!(!spans(&t, "chat").is_empty());
}

/// Cites: GEN-21
#[test]
fn a_driver_with_fixed_knobs_refuses_a_knob_vary() {
    let mut f = run_fixture(&fast_smoke(), "auto");
    f.cfg
        .vary
        .insert("tool_order_stable".into(), "false".into());
    let e = run_driven_blocking(&f.cfg, None, &Toy)
        .unwrap_err()
        .to_string();
    assert!(e.contains("GEN-21"), "{e}");
    // Refused before the run has an identity: no bundle is left behind.
    assert!(
        !f.cfg.runs_dir.exists() || std::fs::read_dir(&f.cfg.runs_dir).unwrap().next().is_none()
    );
}

fn tool(name: &str) -> ToolDef {
    ToolDef {
        name: name.into(),
        description: format!("A {name} tool."),
        parameters: serde_json::json!({"type": "object", "properties": {}}),
    }
}

fn sampling() -> Sampling {
    Sampling {
        max_tokens: 16,
        temperature: 0.0,
        stream: true,
    }
}

/// Cites: GEN-11
#[test]
fn only_one_tool_is_encoded_as_gen_11_writes_it() {
    let ctx = Context {
        system: "s".into(),
        tools: vec![tool("x"), tool("y")],
        messages: vec![Msg::User { text: "u".into() }],
        tool_choice: Some(ToolChoice::Only("y".into())),
    };
    let chat = ctx.encode(
        Encoding::plain(Dialect::ChatCompletions),
        "m",
        sampling(),
        Placement::None,
    );
    assert_eq!(
        chat["tool_choice"],
        serde_json::json!({"type": "allowed_tools", "allowed_tools": {"tools": [
            {"type": "function", "function": {"name": "y"}}
        ]}})
    );
    let messages = ctx.encode(
        Encoding::plain(Dialect::Messages),
        "m",
        sampling(),
        Placement::None,
    );
    assert_eq!(
        messages["tool_choice"],
        serde_json::json!({"type": "tool", "name": "y"})
    );
    // Without tools there is no choice to make, so none is sent.
    let bare = Context {
        tools: Vec::new(),
        ..ctx
    };
    let none = bare.encode(
        Encoding::plain(Dialect::ChatCompletions),
        "m",
        sampling(),
        Placement::None,
    );
    assert!(none.get("tool_choice").is_none());
}

/// A driver that makes its own calls through the run path's `call`.
struct Caller;

impl Driver for Caller {
    fn workload(&self, cfg: &RunConfig) -> Result<Workload, HarnessError> {
        Workload::load(&cfg.workload)
    }
    fn producer(&self) -> (&'static str, &'static str) {
        ("acn-gen", "0.1.0")
    }
    async fn replicate<E: Env>(
        &self,
        rep: &Replicate<'_, E>,
        seed: i64,
    ) -> Result<(), HarnessError> {
        use opentelemetry::KeyValue;
        use opentelemetry::trace::{TraceContextExt as _, Tracer as _};
        let at = |ns: i64| std::time::UNIX_EPOCH + std::time::Duration::from_nanos(ns as u64);
        let mut attrs = rep.setup.session_attrs.clone();
        attrs.push(KeyValue::new("acn.seed", seed));
        attrs.push(KeyValue::new("acn.replicate", i64::from(rep.replicate)));
        let start = rep.env.now();
        let session = rep
            .tracer
            .span_builder("acn.session")
            .with_start_time(at(start))
            .with_attributes(attrs)
            .start(rep.tracer);
        let session_cx = Cx::new().with_span(session);
        let turn = rep
            .tracer
            .span_builder("acn.turn")
            .with_start_time(at(start))
            .with_attributes(vec![
                KeyValue::new("acn.turn.index", 0i64),
                KeyValue::new("acn.turn.outcome", "success"),
                KeyValue::new("acn.turn.compaction", "none"),
            ])
            .start_with_context(rep.tracer, &session_cx);
        let turn_cx = session_cx.with_span(turn);
        let mut st = CallState::default();
        let mut ctx = Context {
            system: rep.system("base", start),
            tools: vec![tool("x"), tool("y")],
            messages: vec![Msg::User { text: "go".into() }],
            tool_choice: Some(ToolChoice::Only("y".into())),
        };
        // The mock calls the one tool allowed (MLM-40).
        let first = rep.call(&mut st, ctx.clone(), 16, None, &turn_cx).await?;
        let reply = first.reply.expect("a reply");
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.tool_calls[0].name, "y");
        ctx.messages.push(Msg::Assistant {
            text: reply.text.clone(),
            tool_calls: reply.tool_calls.clone(),
        });
        ctx.tool_choice = Some(ToolChoice::Forbid);
        let second = rep.call(&mut st, ctx, 16, None, &turn_cx).await?;
        assert!(second.reply.expect("a reply").tool_calls.is_empty());
        assert_eq!((first.index, second.index), (0, 1));
        let end = rep.env.now();
        turn_cx.span().end_with_timestamp(at(end));
        session_cx.span().end_with_timestamp(at(end));
        Ok(())
    }
}

/// Cites: GEN-20, GEN-11
#[test]
fn a_driver_makes_its_own_calls_through_the_run_paths_call() {
    let f = run_fixture(&fast_smoke(), "auto");
    let w = run_driven_blocking(&f.cfg, None, &Caller).unwrap();
    let t = common::read(&w.dir);
    let chats = spans(&t, "chat");
    assert_eq!(chats.len(), 2);
    let mut index: Vec<i64> = chats
        .iter()
        .map(|c| common::int(c, "acn.call.index").unwrap())
        .collect();
    index.sort_unstable();
    assert_eq!(index, [0, 1]);
}
