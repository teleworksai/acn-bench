//! TRC-1, TRC-25: the bridge from the OpenTelemetry SDK refuses what it cannot
//! store faithfully, and never hands back a trace with spans missing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::time::{Duration, UNIX_EPOCH};

use acn_trace::identity::Digest;
use acn_trace::otel::{Collector, producer_resource};
use opentelemetry::trace::{Span as _, Tracer as _, TracerProvider as _};
use opentelemetry::{Array, KeyValue, Value};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;

fn provider(c: &Collector, resource: Resource) -> SdkTracerProvider {
    SdkTracerProvider::builder()
        .with_resource(resource)
        .with_simple_exporter(c.exporter())
        .build()
}

fn resource(name: &str) -> Resource {
    producer_resource(name, "0.1.0", &Digest::of(b"e"), &Digest::of(b"b"))
}

fn span(p: &SdkTracerProvider, name: &'static str, attrs: Vec<KeyValue>) {
    let t = p.tracer("t");
    let mut s = t
        .span_builder(name)
        .with_start_time(UNIX_EPOCH)
        .with_attributes(attrs)
        .start(&t);
    s.end_with_timestamp(UNIX_EPOCH + Duration::from_nanos(10));
}

/// Cites: TRC-1, TRC-25
#[test]
fn a_rejected_resource_fails_the_trace_instead_of_dropping_its_spans() {
    let c = Collector::new();
    let good = provider(&c, resource("good"));
    let bad = provider(
        &c,
        Resource::builder_empty()
            .with_attribute(KeyValue::new("x", Value::Array(Array::I64(vec![1, 2]))))
            .build(),
    );
    span(&good, "g", vec![]);
    span(&bad, "b1", vec![]);
    span(&bad, "b2", vec![]);
    good.shutdown().unwrap();
    let _ = bad.shutdown();
    let err = c.trace().unwrap_err().to_string();
    assert!(err.contains("lost"), "{err}");
}

/// Cites: TRC-25
#[test]
fn values_the_profile_cannot_store_are_refused() {
    for attrs in [
        vec![KeyValue::new("k", Value::Array(Array::I64(vec![1])))],
        vec![KeyValue::new("acn.call.ttft_ms", f64::NAN)],
        vec![KeyValue::new("acn.call.ttft_ms", f64::INFINITY)],
    ] {
        let c = Collector::new();
        let p = provider(&c, resource("p"));
        span(&p, "s", attrs);
        p.shutdown().unwrap();
        assert!(c.trace().is_err());
    }
}

/// Cites: TRC-25
#[test]
fn resource_ids_do_not_depend_on_which_provider_exported_first() {
    let run = |first: &str, second: &str| {
        let c = Collector::new();
        let a = provider(&c, resource(first));
        let b = provider(&c, resource(second));
        span(&a, "x", vec![]);
        span(&b, "y", vec![]);
        a.shutdown().unwrap();
        b.shutdown().unwrap();
        let t = c.trace().unwrap();
        let names: Vec<(String, i32)> = t
            .spans
            .iter()
            .map(|s| {
                let r = t
                    .resources
                    .iter()
                    .find(|r| r.resource_id == s.resource_id)
                    .unwrap();
                (format!("{:?}", r.attrs["service.name"]), s.resource_id)
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        (t.resources, names)
    };
    assert_eq!(run("acn-gen", "acn-harness"), run("acn-harness", "acn-gen"));
}

/// Cites: TRC-25
#[test]
fn negative_zero_is_stored_as_zero() {
    let c = Collector::new();
    let p = provider(&c, resource("p"));
    span(&p, "s", vec![KeyValue::new("acn.call.ttft_ms", -0.0)]);
    p.shutdown().unwrap();
    let t = c.trace().unwrap();
    let v = &t.spans[0].attrs["acn.call.ttft_ms"];
    assert!(matches!(v, acn_trace::model::AttrValue::Float(f) if f.to_bits() == 0.0f64.to_bits()));
}
