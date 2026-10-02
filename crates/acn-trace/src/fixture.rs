//! A fixture producer: one toy agent session of one replicate — a session, two
//! turns, three model calls with stream events and one tool call — emitted through
//! the OpenTelemetry SDK with the seeded id generator and explicit times on a
//! virtual clock, exactly as a `sim` producer does. The tests of this crate and the
//! determinism acceptance suite (TRC-24) run it; it is not a workload.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use opentelemetry::trace::{
    Span as _, SpanKind, TraceContextExt as _, Tracer as _, TracerProvider as _,
};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::trace::SdkTracerProvider;

use crate::identity::{Digest, HypStatus, IdentityError, Mode};
use crate::ids::SeededIdGenerator;
use crate::model::Trace;
use crate::otel::{Collector, ConvertError, producer_resource};

/// The fixture could not run.
#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    Convert(#[from] ConvertError),
    #[error("tracer provider: {0}")]
    Sdk(String),
    #[error("seed {0} does not fit the Int64 `acn.seed` attribute (ADR-13)")]
    Seed(u64),
}

/// What the fixture session records about its run.
#[derive(Debug, Clone)]
pub struct FixtureRun {
    pub run_id: String,
    pub seed: u64,
    pub replicate: u32,
    pub engine_hash: Digest,
    pub build_hash: Digest,
}

fn at(ns: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_nanos(ns)
}

/// Run the fixture session and return what the collector gathered.
pub fn session(run: &FixtureRun) -> Result<Trace, FixtureError> {
    let Ok(seed) = i64::try_from(run.seed) else {
        return Err(FixtureError::Seed(run.seed));
    };
    let collector = Collector::new();
    let provider = SdkTracerProvider::builder()
        .with_id_generator(SeededIdGenerator::for_replicate(run.seed, run.replicate)?)
        .with_resource(producer_resource(
            "acn-harness",
            "0.1.0",
            &run.engine_hash,
            &run.build_hash,
        ))
        .with_simple_exporter(collector.exporter())
        .build();
    let tracer = provider.tracer("acn-fixture");

    let session = tracer
        .span_builder("acn.session")
        .with_kind(SpanKind::Internal)
        .with_start_time(at(0))
        .with_attributes(vec![
            KeyValue::new("acn.run_id", run.run_id.clone()),
            KeyValue::new("acn.hypothesis.id", crate::bundle::NO_HYPOTHESIS),
            KeyValue::new("acn.hypothesis.status", HypStatus::Candidate.as_str()),
            KeyValue::new("acn.backend", crate::bundle::MOCK_BACKEND),
            KeyValue::new("acn.mode", Mode::Sim.as_str()),
            KeyValue::new("acn.scenario.hash", Digest::of(b"scenario").to_hex()),
            KeyValue::new("acn.workload.hash", Digest::of(b"workload").to_hex()),
            KeyValue::new("acn.seed", seed),
            KeyValue::new("acn.replicate", i64::from(run.replicate)),
            KeyValue::new("acn.role", "treatment"),
            KeyValue::new("acn.harness.knobs", "{}"),
            KeyValue::new("acn.stall_threshold_ms", 250.0),
            KeyValue::new("acn.keep_content", false),
        ])
        .start(&tracer);
    let session_cx = Context::new().with_span(session);

    let mut t = 1_000_000u64;
    let mut call_index = 0i64;
    for turn_index in 0..2i64 {
        let turn = tracer
            .span_builder("acn.turn")
            .with_kind(SpanKind::Internal)
            .with_start_time(at(t))
            .with_attributes(vec![
                KeyValue::new("acn.turn.index", turn_index),
                KeyValue::new("acn.turn.outcome", "success"),
                KeyValue::new("acn.turn.compaction", "none"),
            ])
            .start_with_context(&tracer, &session_cx);
        let turn_cx = session_cx.with_span(turn);
        let calls = if turn_index == 0 { 2 } else { 1 };
        for c in 0..calls {
            let start = t + 10_000;
            let mut chat = tracer
                .span_builder("chat")
                .with_kind(SpanKind::Client)
                .with_start_time(at(start))
                .with_attributes(vec![
                    KeyValue::new("gen_ai.operation.name", "chat"),
                    KeyValue::new("gen_ai.provider.name", crate::bundle::MOCK_BACKEND),
                    KeyValue::new("gen_ai.request.model", "fixture"),
                    KeyValue::new("acn.call.index", call_index),
                    KeyValue::new("acn.call.input_tokens", 1000 + 200 * call_index),
                    KeyValue::new("acn.call.new_input_tokens", 200),
                    KeyValue::new("acn.call.new_input_tokens_method", "tokens"),
                    KeyValue::new("acn.call.output_tokens", 50),
                    KeyValue::new("acn.cache.read_tokens", 800 + 200 * call_index),
                    KeyValue::new("acn.cache.write_tokens", 0),
                    KeyValue::new("acn.call.ttft_ms", 12.5),
                    KeyValue::new("acn.call.wire_bytes_up", 4096),
                    KeyValue::new("acn.call.wire_bytes_down", 1024),
                    KeyValue::new("acn.call.streamed", true),
                    KeyValue::new("acn.call.retries", 0),
                ])
                .start_with_context(&tracer, &turn_cx);
            chat.add_event_with_timestamp("acn.stream.first_token", at(start + 12_500_000), vec![]);
            if c == 1 {
                chat.add_event_with_timestamp(
                    "acn.stream.stall",
                    at(start + 20_000_000),
                    vec![
                        KeyValue::new("gap_ms", 300.0),
                        KeyValue::new("tokens_before", 7),
                    ],
                );
            }
            chat.add_event_with_timestamp("acn.stream.last_token", at(start + 40_000_000), vec![]);
            chat.end_with_timestamp(at(start + 40_000_000));
            t = start + 40_000_000;
            if c == 0 && calls > 1 {
                let mut tool = tracer
                    .span_builder("execute_tool")
                    .with_kind(SpanKind::Internal)
                    .with_start_time(at(t + 1_000))
                    .with_attributes(vec![
                        KeyValue::new("gen_ai.tool.name", "read_file"),
                        KeyValue::new("acn.tool.class", "file"),
                        KeyValue::new("acn.tool.result_bytes", 2048),
                        KeyValue::new("acn.tool.placement", "local"),
                        KeyValue::new("acn.tool.requesting_call", call_index),
                    ])
                    .start_with_context(&tracer, &turn_cx);
                tool.end_with_timestamp(at(t + 5_000_000));
                t += 5_000_000;
            }
            call_index += 1;
        }
        turn_cx.span().end_with_timestamp(at(t + 1_000));
        t += 2_000_000_000; // think time
    }
    session_cx.span().end_with_timestamp(at(t));
    provider
        .shutdown()
        .map_err(|e| FixtureError::Sdk(e.to_string()))?;
    Ok(collector.trace()?)
}
