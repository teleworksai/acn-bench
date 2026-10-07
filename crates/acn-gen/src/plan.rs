//! Plans (SPEC 050 GEN-3, GEN-4): every draw of a session and of each of its
//! turns, from sub-streams named by what they decide, made before the turn's
//! first call so that nothing depends on when a response arrives.

use acn_trace::identity::substream_rng;
use rand_chacha::ChaCha20Rng;

use crate::GenError;
use crate::sheet::{SUBAGENT, Sheet};

/// One tool call of a chain: its class, how long it runs and how many tokens
/// its result has. A `subagent` call's result is its sub-agents' answers
/// (GEN-12), so its duration and size are not drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolStep {
    pub class: &'static str,
    pub duration_ns: u64,
    pub result_tokens: u64,
}

/// One lineage's chain (GEN-11): its user text, its tool calls, then its
/// answer's output cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    pub user_tokens: u64,
    pub tools: Vec<ToolStep>,
    pub answer_tokens: u64,
}

/// A turn's plan (GEN-4): the main chain and, when it fans out, one chain
/// per sub-agent in index order (GEN-12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnPlan {
    pub main: Chain,
    pub children: Vec<Chain>,
}

/// A session's own draws (GEN-3, GEN-10): its start after the replicate's,
/// its turn count, and the think time before each turn after the first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPlan {
    pub start_ns: u64,
    pub turns: u64,
    pub think_ns: Vec<u64>,
}

fn stream(seed: u64, name: &str) -> Result<ChaCha20Rng, GenError> {
    substream_rng(seed, name).map_err(|e| GenError::Internal(e.to_string()))
}

/// Session `k` of the replicate whose seed is `replicate_seed`, from
/// `gen.session.<k>`.
pub fn session(sheet: &Sheet, replicate_seed: u64, k: u64) -> Result<SessionPlan, GenError> {
    let mut rng = stream(replicate_seed, &format!("gen.session.{k}"))?;
    let start_ns = sheet.session_start_ns.draw(&mut rng);
    let turns = sheet.turns_per_session.draw(&mut rng);
    let think_ns = (1..turns)
        .map(|_| sheet.think_time_ns.draw(&mut rng))
        .collect();
    Ok(SessionPlan {
        start_ns,
        turns,
        think_ns,
    })
}

/// One chain's draws, in GEN-4's order: user text and length, the width
/// when `fans` (else none), each tool call's class, duration and result size,
/// then the answer cap. Returns the chain and its width.
fn chain(sheet: &Sheet, rng: &mut ChaCha20Rng, fans: bool) -> (Chain, u64) {
    let user_tokens = sheet.user_tokens.draw(rng);
    let length = sheet.chain_length.draw(rng);
    let width = if fans {
        sheet.fanout_width.draw(rng)
    } else {
        0
    };
    let mut tools = Vec::new();
    for i in 0..length {
        if i == 0 && width > 0 {
            tools.push(ToolStep {
                class: SUBAGENT,
                duration_ns: 0,
                result_tokens: 0,
            });
            continue;
        }
        let at = usize::try_from(sheet.tool_class.draw(rng)).unwrap_or(0);
        let class = sheet.classes.get(at).copied().unwrap_or("other");
        let duration_ns = sheet.tool_duration_ns.get(class).map_or(0, |d| d.draw(rng));
        let result_tokens = sheet
            .tool_result_tokens
            .get(class)
            .map_or(0, |d| d.draw(rng));
        tools.push(ToolStep {
            class,
            duration_ns,
            result_tokens,
        });
    }
    let answer_tokens = sheet.answer_tokens.draw(rng);
    (
        Chain {
            user_tokens,
            tools,
            answer_tokens,
        },
        width,
    )
}

/// Turn `t` of session `k`, drawn whole from `gen.plan.<k>.<t>` (GEN-4).
pub fn turn(sheet: &Sheet, replicate_seed: u64, k: u64, t: u64) -> Result<TurnPlan, GenError> {
    let mut rng = stream(replicate_seed, &format!("gen.plan.{k}.{t}"))?;
    let (main, width) = chain(sheet, &mut rng, true);
    let children = (0..width)
        .map(|_| chain(sheet, &mut rng, false).0)
        .collect();
    Ok(TurnPlan { main, children })
}

/// The stream of lineage `c`'s text in turn `t` of session `k` (GEN-3): 0
/// for the main lineage, 1 + *i* for sub-agent *i*.
pub fn text_stream(replicate_seed: u64, k: u64, t: u64, c: u64) -> Result<ChaCha20Rng, GenError> {
    stream(replicate_seed, &format!("gen.text.{k}.{t}.{c}"))
}

/// The stream of the system message every session shares (GEN-13).
pub fn system_stream(replicate_seed: u64) -> Result<ChaCha20Rng, GenError> {
    stream(replicate_seed, "gen.text.system")
}
