//! The seeded id generator of TRC-27: in `sim` mode, trace and span ids come from
//! the sub-stream `trace.ids` of the replicate a session belongs to, or of the run
//! for the `acn.scenario` root span (CON-30(b)). A trace id is the next 16 bytes of
//! the stream and a span id the next 8, in the order the SDK asks for them; an
//! all-zero draw, which OTel reserves as invalid, is skipped.

use std::sync::Mutex;

use opentelemetry::trace::{SpanId, TraceId};
use opentelemetry_sdk::trace::IdGenerator;
use rand_chacha::ChaCha20Rng;
use rand_core::Rng as _;

use crate::identity::{self, IdentityError};

/// The sub-stream name of TRC-27.
pub const STREAM: &str = "trace.ids";

/// A seeded `IdGenerator` (TRC-27, CON-5(b′)).
#[derive(Debug)]
pub struct SeededIdGenerator {
    rng: Mutex<ChaCha20Rng>,
}

impl SeededIdGenerator {
    /// The generator for the sessions of replicate `i` of the run with `seed`.
    pub fn for_replicate(seed: u64, i: u32) -> Result<Self, IdentityError> {
        Self::from_stream_seed(identity::replicate_seed(seed, i)?)
    }

    /// The generator for what belongs to the run as a whole: the `acn.scenario`
    /// root span.
    pub fn for_run(seed: u64) -> Result<Self, IdentityError> {
        Self::from_stream_seed(seed)
    }

    fn from_stream_seed(s: u64) -> Result<Self, IdentityError> {
        Ok(Self {
            rng: Mutex::new(identity::substream_rng(s, STREAM)?),
        })
    }

    fn draw<const N: usize>(&self) -> [u8; N] {
        // A poisoned lock means a producer panicked mid-draw; the stream state is
        // still a valid ChaCha state, and the run is failing anyway.
        let mut rng = match self.rng.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        loop {
            let mut bytes = [0u8; N];
            rng.fill_bytes(&mut bytes);
            if bytes.iter().any(|b| *b != 0) {
                return bytes;
            }
        }
    }
}

impl IdGenerator for SeededIdGenerator {
    fn new_trace_id(&self) -> TraceId {
        TraceId::from_bytes(self.draw::<16>())
    }

    fn new_span_id(&self) -> SpanId {
        SpanId::from_bytes(self.draw::<8>())
    }
}

/// One [`SeededIdGenerator`] shared by several tracer providers, so that the
/// `acn-emu` resource's link spans and the harness's spans of a replicate draw
/// from one stream, in program order (SPEC 020 EMU-36). A sim run is
/// single-threaded, so that order is the order the spans are started.
#[derive(Debug, Clone)]
pub struct SharedIdGenerator(pub std::sync::Arc<SeededIdGenerator>);

impl IdGenerator for SharedIdGenerator {
    fn new_trace_id(&self) -> TraceId {
        self.0.new_trace_id()
    }

    fn new_span_id(&self) -> SpanId {
        self.0.new_span_id()
    }
}
