//! The synthetic agent conversation, and the stand-in model both servers share.
//!
//! Everything is derived from the turn index, so client and server agree on the
//! expected output without exchanging it and every arm can verify what it received.

use std::time::Duration;

pub const SYSTEM: u8 = 0;
pub const USER: u8 = 1;
pub const ASSISTANT: u8 = 2;

const WORDS: [&str; 16] = [
    "link", "turn", "agent", "prefix", "token", "stream", "cache", "route", "delta", "tool",
    "plan", "gap", "probe", "edge", "burst", "trace",
];

#[derive(Clone, Debug)]
pub struct Workload {
    pub turns: u32,
    /// System prompt plus tool definitions, sent on every baseline request.
    pub system_bytes: usize,
    /// New user message or tool result per turn.
    pub user_bytes: usize,
    pub out_tokens: usize,
    /// Server time before the first token. Identical in every arm: a warm provider
    /// prefix cache is assumed, so re-sending the prefix costs bytes, not prefill.
    pub prefill: Duration,
    pub token_interval: Duration,
}

impl Workload {
    pub fn system(&self) -> String {
        text(0x5157, self.system_bytes)
    }

    pub fn user(&self, turn: u32) -> String {
        text(0x1000 + u64::from(turn), self.user_bytes)
    }

    pub fn tokens(&self, turn: u32) -> Vec<String> {
        let mut rng = XorShift::new(0x2000 + u64::from(turn));
        (0..self.out_tokens)
            .map(|_| format!("{} ", rng.word()))
            .collect()
    }

    pub fn output(&self, turn: u32) -> String {
        self.tokens(turn).concat()
    }
}

/// One record of the canonical context the turn transport hashes and deltas over:
/// `role u8 | len u32 le | bytes`.
pub fn record(role: u8, body: &[u8]) -> Vec<u8> {
    let mut rec = Vec::with_capacity(5 + body.len());
    rec.push(role);
    rec.extend_from_slice(&(body.len() as u32).to_le_bytes());
    rec.extend_from_slice(body);
    rec
}

/// What one turn cost, as seen by the client.
#[derive(Clone, Debug, Default)]
pub struct TurnStats {
    pub ttft: Duration,
    pub total: Duration,
    /// Times the client had to act on a lost path (restart, resume or migrate).
    pub recoveries: u32,
    /// Times the server refused the client's base, or the turn, and the client had to
    /// open it again with the full context.
    pub fallbacks: u32,
    /// Output bytes the client received and then had to throw away.
    pub discarded: usize,
    pub output: String,
}

fn text(seed: u64, bytes: usize) -> String {
    let mut rng = XorShift::new(seed);
    let mut out = String::with_capacity(bytes + 8);
    while out.len() < bytes {
        out.push_str(rng.word());
        out.push(' ');
    }
    out.truncate(bytes);
    out
}

struct XorShift(u64);

impl XorShift {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn word(&mut self) -> &'static str {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        WORDS[(self.0 >> 32) as usize % WORDS.len()]
    }
}
