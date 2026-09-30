//! Lab spike (CON-23): a turn-native QUIC transport against HTTP/1.1 + SSE, over an
//! impaired loopback link. Exploratory; nothing printed here may be cited (CON-24).
//!
//! Arms: `sse` is the status quo. `quic` and `quic-migrate` are ablations that carry
//! the full prefix over a QUIC stream, recovering from a broken path by restarting
//! the turn or by QUIC connection migration. `turn` is the prototype: prefix as a
//! delta, declared deadline, turn-level resume on a new connection. `turn-probe` adds
//! a resume on the same connection when the output goes silent.
//!
//! A full run is the arm x gap matrix, a conversation whose session the server evicts,
//! a sweep of blackout lengths, and two deadline checks. `--arm` or `--gap` narrows the
//! matrix and skips the rest.
#![forbid(unsafe_code)] // CON-19 still binds lab crates: CON-23 exempts CON-4 to CON-18 only

mod link;
mod sse;
mod turn;
mod workload;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use serde::Serialize;
use tokio::task::JoinSet;
use tokio::time::Instant;

use link::{GapKind, Link, LinkCfg, Snapshot};
use sse::{SseClient, SseServer};
use turn::{ClientCfg, IDLE_TIMEOUT, Recovery, Tuning, TurnClient, TurnServer};
use workload::{TurnStats, Workload};

#[derive(Parser, Clone, Debug, Serialize)]
struct Args {
    #[arg(long, default_value_t = 6)]
    turns: u32,
    /// System prompt plus tool definitions, bytes.
    #[arg(long, default_value_t = 16_384)]
    system_bytes: usize,
    /// New user message or tool result per turn, bytes.
    #[arg(long, default_value_t = 2_048)]
    user_bytes: usize,
    #[arg(long, default_value_t = 100)]
    out_tokens: usize,
    #[arg(long, default_value_t = 20)]
    token_ms: u64,
    #[arg(long, default_value_t = 150)]
    prefill_ms: u64,
    #[arg(long, default_value_t = 40)]
    rtt_ms: u64,
    #[arg(long, default_value_t = 10.0)]
    up_mbps: f64,
    #[arg(long, default_value_t = 50.0)]
    down_mbps: f64,
    /// Length of the gap in the matrix.
    #[arg(long, default_value_t = 2_000)]
    gap_ms: u64,
    /// Turn the gap lands in.
    #[arg(long, default_value_t = 3)]
    gap_turn: u32,
    /// How far into that turn the gap opens.
    #[arg(long, default_value_t = 1_000)]
    gap_at_ms: u64,
    /// Blackout lengths for the sweep. Recovery after a blackout moves in steps, and the
    /// silence probe's cost depends on where in its interval the gap ends, so one gap
    /// length says little.
    #[arg(
        long,
        value_delimiter = ',',
        default_value = "1000,1500,2000,2100,2500,3000,4000"
    )]
    sweep_gaps_ms: Vec<u64>,
    /// Deadline declared on every turn of the QUIC arms. Twice this is the hang timeout
    /// for a turn of any arm.
    #[arg(long, default_value_t = 30_000)]
    deadline_ms: u64,
    #[arg(long, default_value_t = 3)]
    reps: u32,
    /// Silence, once output flows, after which `turn-probe` resumes on a new stream.
    #[arg(long, default_value_t = 250)]
    silence_ms: u64,
    /// QUIC client keep-alive; 0 is quinn's default, none.
    #[arg(long, default_value_t = 0)]
    keep_alive_ms: u64,
    /// Switch off quinn's path MTU discovery (its probes are uplink bytes).
    #[arg(long)]
    no_mtud: bool,
    /// Run only this arm, and skip the eviction run, the sweep and the deadline checks.
    #[arg(long, value_enum)]
    arm: Option<Arm>,
    /// Run only this gap kind, and skip the same.
    #[arg(long, value_enum)]
    gap: Option<GapKind>,
    /// Print the raw results as one JSON object instead of tables.
    #[arg(long)]
    json: bool,
}

impl Args {
    fn workload(&self) -> Workload {
        Workload {
            turns: self.turns,
            system_bytes: self.system_bytes,
            user_bytes: self.user_bytes,
            out_tokens: self.out_tokens,
            prefill: Duration::from_millis(self.prefill_ms),
            token_interval: Duration::from_millis(self.token_ms),
        }
    }

    fn link(&self) -> LinkCfg {
        LinkCfg {
            one_way: Duration::from_millis(self.rtt_ms) / 2,
            up_bps: (self.up_mbps * 1e6) as u64,
            down_bps: (self.down_mbps * 1e6) as u64,
        }
    }

    fn tuning(&self) -> Tuning {
        Tuning {
            mtud: !self.no_mtud,
            keep_alive: (self.keep_alive_ms > 0).then(|| Duration::from_millis(self.keep_alive_ms)),
        }
    }

    /// Server time for a whole turn, without the network.
    fn generation(&self) -> Duration {
        Duration::from_millis(self.prefill_ms + self.token_ms * self.out_tokens as u64)
    }

    fn validate(&self) -> Result<()> {
        if self.gap_turn >= self.turns {
            bail!("--gap-turn must be below --turns");
        }
        let link = self.link();
        if link.up_bps == 0 || link.down_bps == 0 {
            bail!("--up-mbps and --down-mbps must be above zero");
        }
        let longest = self
            .sweep_gaps_ms
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
            .max(self.gap_ms);
        if Duration::from_millis(longest) + Duration::from_secs(2) > IDLE_TIMEOUT {
            bail!(
                "a {longest} ms gap is too close to the {} s QUIC idle timeout: connections would die in a blackout",
                IDLE_TIMEOUT.as_secs()
            );
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
enum Arm {
    Sse,
    Quic,
    QuicMigrate,
    Turn,
    TurnProbe,
}

const ARMS: [Arm; 5] = [
    Arm::Sse,
    Arm::Quic,
    Arm::QuicMigrate,
    Arm::Turn,
    Arm::TurnProbe,
];
const SWEEP_ARMS: [Arm; 3] = [Arm::Sse, Arm::Turn, Arm::TurnProbe];
const GAPS: [GapKind; 3] = [GapKind::None, GapKind::Blackout, GapKind::Break];
const SESSION: u64 = 1;

/// The name clap and the tables use for an arm or a gap kind.
fn name<T: ValueEnum>(value: &T) -> String {
    value
        .to_possible_value()
        .map(|v| v.get_name().to_owned())
        .unwrap_or_default()
}

/// Server, impaired link and client for one arm.
enum Rig {
    Sse {
        _server: SseServer,
        client: SseClient,
    },
    Quic {
        server: TurnServer,
        client: Box<TurnClient>,
    },
}

impl Rig {
    async fn build(
        args: &Args,
        workload: Arc<Workload>,
        arm: Arm,
        deadline: Duration,
    ) -> Result<(Rig, Arc<Link>)> {
        let system = workload.system();
        let (delta, recovery) = match arm {
            Arm::Sse => {
                let server = SseServer::start(workload).await?;
                let link = Arc::new(Link::tcp(server.addr, args.link()).await?);
                let client = SseClient::new(link.addr, link.state(), system);
                let rig = Rig::Sse {
                    _server: server,
                    client,
                };
                return Ok((rig, link));
            }
            Arm::Quic => (false, Recovery::Restart),
            Arm::QuicMigrate => (false, Recovery::Migrate),
            Arm::Turn | Arm::TurnProbe => (true, Recovery::Resume),
        };
        let cfg = ClientCfg {
            delta,
            recovery,
            deadline,
            silence: (arm == Arm::TurnProbe).then(|| Duration::from_millis(args.silence_ms)),
            tuning: args.tuning(),
        };
        let server = TurnServer::start(workload, args.tuning())?;
        let link = Arc::new(Link::udp(server.addr, args.link()).await?);
        let client = TurnClient::new(
            cfg,
            link.addr,
            server.cert.clone(),
            link.state(),
            SESSION,
            &system,
        )?;
        let client = Box::new(client);
        Ok((Rig::Quic { server, client }, link))
    }

    async fn turn(&mut self, idx: u32, user: &str) -> Result<TurnStats> {
        match self {
            Rig::Sse { client, .. } => client.turn(user).await,
            Rig::Quic { client, .. } => client.turn(idx, user).await,
        }
    }
}

/// One conversation to run.
#[derive(Clone, Copy, Debug, Serialize)]
struct Plan {
    arm: Arm,
    gap: GapKind,
    gap_ms: u64,
    /// Make the server forget the session ahead of this turn.
    evict_before: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
struct TurnRow {
    link: Snapshot,
    ttft_ms: f64,
    total_ms: f64,
    recoveries: u32,
    fallbacks: u32,
    discarded: usize,
}

#[derive(Clone, Debug, Serialize)]
struct Conversation {
    plan: Plan,
    turns: Vec<TurnRow>,
    link: Snapshot,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn open_gap(
    link: &Arc<Link>,
    kind: GapKind,
    at_ms: u64,
    len_ms: u64,
) -> tokio::task::JoinHandle<()> {
    let link = link.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(at_ms)).await;
        link.gap(kind, Duration::from_millis(len_ms)).await;
    })
}

/// One conversation on a fresh server and link. Per-turn bytes are link-ingress counts
/// between turn boundaries, so a turn's trailing ACKs land in the next row; the
/// conversation total is taken after the link has drained.
async fn converse(args: Args, plan: Plan) -> Result<Conversation> {
    let workload = Arc::new(args.workload());
    let deadline = Duration::from_millis(args.deadline_ms);
    let (mut rig, link) = Rig::build(&args, workload.clone(), plan.arm, deadline).await?;
    let mut turns = Vec::new();
    for idx in 0..workload.turns {
        if plan.evict_before == Some(idx)
            && let Rig::Quic { server, client } = &rig
        {
            server.evict(client.session);
        }
        let before = link.snapshot();
        let gap_task = (idx == args.gap_turn && plan.gap != GapKind::None)
            .then(|| open_gap(&link, plan.gap, args.gap_at_ms, plan.gap_ms));
        let stats = tokio::time::timeout(deadline * 2, rig.turn(idx, &workload.user(idx)))
            .await
            .with_context(|| format!("turn {idx}: hung"))??;
        if let Some(task) = gap_task {
            task.await?;
        }
        if stats.output != workload.output(idx) {
            bail!("turn {idx}: output differs from what the model produced");
        }
        turns.push(TurnRow {
            link: link.snapshot().since(&before),
            ttft_ms: ms(stats.ttft),
            total_ms: ms(stats.total),
            recoveries: stats.recoveries,
            fallbacks: stats.fallbacks,
            discarded: stats.discarded,
        });
    }
    tokio::time::sleep(Duration::from_millis(args.rtt_ms * 3)).await;
    Ok(Conversation {
        plan,
        turns,
        link: link.snapshot(),
    })
}

// ---------------------------------------------------------------- deadline checks

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Check<T> {
    Ran(T),
    Skipped(String),
}

#[derive(Debug, Serialize)]
struct DeadlineCheck {
    declared_ms: u64,
    client_failed_after_ms: f64,
    client_error: String,
    /// Output bytes the server had generated when the deadline cut each turn short.
    server_cut_short_at: Vec<usize>,
    full_output: usize,
    server_turns_held_after: usize,
}

/// A turn on the `turn` arm whose deadline cannot be met, optionally after an
/// undisturbed warm-up turn and with a break in the way. State is inspected 100 ms
/// after the client gives up, not at some later convenient moment.
async fn deadline_check(
    args: &Args,
    declared: Duration,
    warm_up: bool,
    gap: GapKind,
) -> Result<DeadlineCheck> {
    let workload = Arc::new(args.workload());
    let (mut rig, link) = Rig::build(args, workload.clone(), Arm::Turn, declared).await?;
    let idx = u32::from(warm_up);
    if warm_up {
        rig.turn(0, &workload.user(0)).await?;
    }
    let gap_task =
        (gap != GapKind::None).then(|| open_gap(&link, gap, args.gap_at_ms, args.gap_ms));
    let start = Instant::now();
    let outcome = rig.turn(idx, &workload.user(idx)).await;
    let failed_after = start.elapsed();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let Rig::Quic { server, .. } = &rig else {
        bail!("deadline check needs a QUIC arm");
    };
    let check = DeadlineCheck {
        declared_ms: declared.as_millis() as u64,
        client_failed_after_ms: ms(failed_after),
        client_error: match outcome {
            Ok(_) => bail!("the turn completed although its deadline could not be met"),
            Err(e) => e.to_string(),
        },
        server_cut_short_at: server.expired(),
        full_output: workload.output(idx).len(),
        server_turns_held_after: server.live_turns(),
    };
    if let Some(task) = gap_task {
        task.await?;
    }
    Ok(check)
}

#[derive(Debug, Serialize)]
struct DeadlineChecks {
    /// Deadline at half the generation time: the server has to stop generating.
    mid_generation: Check<DeadlineCheck>,
    /// Deadline inside a break, after generation has finished: the client has to stop
    /// waiting for the link, and the server has to drop the resumable state.
    inside_break: Check<DeadlineCheck>,
}

async fn deadline_checks(args: &Args) -> DeadlineChecks {
    let ran = |r: Result<DeadlineCheck>| match r {
        Ok(check) => Check::Ran(check),
        Err(e) => Check::Skipped(format!("failed: {e:#}")),
    };
    let mid = args.generation() / 2;
    let mid_generation = ran(deadline_check(args, mid, false, GapKind::None).await);

    let inside = Duration::from_millis(args.gap_at_ms + args.gap_ms * 3 / 4);
    let undisturbed = args.generation() + Duration::from_millis(args.rtt_ms * 4);
    let inside_break = if inside <= undisturbed {
        Check::Skipped(
            "no deadline fits between an undisturbed turn and the end of the gap".to_owned(),
        )
    } else {
        ran(deadline_check(args, inside, true, GapKind::Break).await)
    };
    DeadlineChecks {
        mid_generation,
        inside_break,
    }
}

// ---------------------------------------------------------------- run

#[derive(Serialize)]
struct Report {
    args: Args,
    matrix: Vec<Conversation>,
    evicted: Vec<Conversation>,
    sweep: Vec<Conversation>,
    deadline: Option<DeadlineChecks>,
    failures: Vec<String>,
}

/// Runs the plans side by side: every conversation has its own server and link, and
/// the load is timers, not CPU. A failed conversation is reported, not fatal.
async fn run_batch(args: &Args, plans: Vec<Plan>, failures: &mut Vec<String>) -> Vec<Conversation> {
    let mut set = JoinSet::new();
    for plan in plans {
        let args = args.clone();
        set.spawn(async move { (plan, converse(args, plan).await) });
    }
    let mut done = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((_, Ok(conversation))) => done.push(conversation),
            Ok((plan, Err(e))) => {
                let failure = format!(
                    "{} / {} / {} ms: {e:#}",
                    name(&plan.arm),
                    name(&plan.gap),
                    plan.gap_ms
                );
                eprintln!("FAILED {failure}");
                failures.push(failure);
            }
            Err(e) => failures.push(format!("conversation task: {e}")),
        }
    }
    done
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    args.validate()?;
    let full_run = args.arm.is_none() && args.gap.is_none();
    let plan = |arm, gap, gap_ms, evict_before| Plan {
        arm,
        gap,
        gap_ms,
        evict_before,
    };

    let mut report = Report {
        args: args.clone(),
        matrix: Vec::new(),
        evicted: Vec::new(),
        sweep: Vec::new(),
        deadline: None,
        failures: Vec::new(),
    };
    for rep in 0..args.reps {
        eprintln!("rep {}/{}", rep + 1, args.reps);
        let mut plans = Vec::new();
        for arm in ARMS
            .into_iter()
            .filter(|a| args.arm.is_none_or(|only| only == *a))
        {
            for gap in GAPS
                .into_iter()
                .filter(|g| args.gap.is_none_or(|only| only == *g))
            {
                plans.push(plan(arm, gap, args.gap_ms, None));
            }
        }
        if full_run {
            plans.push(plan(Arm::Turn, GapKind::None, 0, Some(args.gap_turn)));
        }
        let done = run_batch(&args, plans, &mut report.failures).await;
        let (evicted, matrix): (Vec<_>, Vec<_>) = done
            .into_iter()
            .partition(|c| c.plan.evict_before.is_some());
        report.matrix.extend(matrix);
        report.evicted.extend(evicted);

        if full_run {
            let plans = args
                .sweep_gaps_ms
                .iter()
                .flat_map(|gap_ms| {
                    SWEEP_ARMS.map(|arm| plan(arm, GapKind::Blackout, *gap_ms, None))
                })
                .collect();
            report
                .sweep
                .extend(run_batch(&args, plans, &mut report.failures).await);
        }
    }
    if full_run {
        report.deadline = Some(deadline_checks(&args).await);
        let fell_back = |c: &Conversation| c.turns[args.gap_turn as usize].fallbacks > 0;
        if args.gap_turn > 0 && !report.evicted.iter().all(fell_back) {
            report.failures.push(
                "an evicted session did not make the client fall back to the full prefix"
                    .to_owned(),
            );
        }
    }

    if args.json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        print_tables(&report);
    }
    if !report.failures.is_empty() {
        bail!("{} failure(s), listed above", report.failures.len());
    }
    Ok(())
}

// ---------------------------------------------------------------- tables

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    match values.len() {
        0 => f64::NAN,
        n if n % 2 == 1 => values[n / 2],
        n => (values[n / 2 - 1] + values[n / 2]) / 2.0,
    }
}

/// Median over reps of `f(conversation)` for the conversations `pick` selects.
fn over_reps(
    all: &[Conversation],
    pick: impl Fn(&Plan) -> bool,
    f: impl Fn(&Conversation) -> f64,
) -> f64 {
    median(all.iter().filter(|c| pick(&c.plan)).map(f).collect())
}

fn print_tables(report: &Report) {
    let a = &report.args;
    let gap_turn = a.gap_turn as usize;
    let keep_alive = match a.keep_alive_ms {
        0 => "none".to_owned(),
        ms => format!("{ms} ms"),
    };
    println!(
        "workload: {} turns, system {} B, +{} B user/turn, {} tokens/turn at {} ms, prefill {} ms",
        a.turns, a.system_bytes, a.user_bytes, a.out_tokens, a.token_ms, a.prefill_ms
    );
    println!(
        "link: RTT {} ms, {} Mbit/s up, {} Mbit/s down; gap {} ms at {} ms into turn {}; QUIC keep-alive {keep_alive}; median of {} reps\n",
        a.rtt_ms, a.up_mbps, a.down_mbps, a.gap_ms, a.gap_at_ms, a.gap_turn, a.reps
    );

    let matrix = &report.matrix;
    let cell = |arm: Arm, gap: GapKind, f: &dyn Fn(&Conversation) -> f64| {
        over_reps(matrix, |p| p.arm == arm && p.gap == gap, f)
    };
    let evicted = |f: &dyn Fn(&Conversation) -> f64| over_reps(&report.evicted, |_| true, f);

    println!("### Uplink bytes per turn, no gap\n");
    let names: Vec<String> = ARMS.iter().map(name).collect();
    println!(
        "| turn | {} | turn, session evicted before turn {gap_turn} |",
        names.join(" | ")
    );
    println!("|---{}|---|", "|---".repeat(names.len()));
    for i in 0..a.turns as usize {
        let cells: Vec<String> = ARMS
            .iter()
            .map(|arm| {
                format!(
                    "{:.0}",
                    cell(*arm, GapKind::None, &|c| c.turns[i].link.up_bytes as f64)
                )
            })
            .collect();
        println!(
            "| {i} | {} | {:.0} |",
            cells.join(" | "),
            evicted(&|c| c.turns[i].link.up_bytes as f64)
        );
    }
    let totals: Vec<String> = ARMS
        .iter()
        .map(|arm| {
            format!(
                "{:.0}",
                cell(*arm, GapKind::None, &|c| c.link.up_bytes as f64)
            )
        })
        .collect();
    println!(
        "| all | {} | {:.0} |\n",
        totals.join(" | "),
        evicted(&|c| c.link.up_bytes as f64)
    );

    println!("### Per arm and gap\n");
    println!(
        "| arm | gap | up B | down B | lost B (UDP only) | up pkts | down pkts | gap turn ms | other turns ms | turn 0 ms | warm TTFT ms | recoveries, gap turn | recoveries, other turns | output discarded B |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    // Turns that are neither the cold first one nor the one with the gap.
    let steady = |c: &Conversation, f: &dyn Fn(&TurnRow) -> f64| -> Vec<f64> {
        let rows = c.turns.iter().enumerate();
        rows.filter(|(i, _)| *i != 0 && *i != gap_turn)
            .map(|(_, t)| f(t))
            .collect()
    };
    for arm in ARMS {
        for gap in GAPS {
            let m = |f: &dyn Fn(&Conversation) -> f64| cell(arm, gap, f);
            let lost = match arm {
                Arm::Sse => "-".to_owned(),
                _ => format!("{:.0}", m(&|c| (c.link.up_lost + c.link.down_lost) as f64)),
            };
            println!(
                "| {} | {} | {:.0} | {:.0} | {lost} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} |",
                name(&arm),
                name(&gap),
                m(&|c| c.link.up_bytes as f64),
                m(&|c| c.link.down_bytes as f64),
                m(&|c| c.link.up_pkts as f64),
                m(&|c| c.link.down_pkts as f64),
                m(&|c| c.turns[gap_turn].total_ms),
                m(&|c| median(steady(c, &|t| t.total_ms))),
                m(&|c| c.turns[0].total_ms),
                m(&|c| median(steady(c, &|t| t.ttft_ms))),
                m(&|c| c.turns[gap_turn].recoveries as f64),
                m(&|c| {
                    let others = c.turns.iter().enumerate().filter(|(i, _)| *i != gap_turn);
                    others.map(|(_, t)| f64::from(t.recoveries)).sum()
                }),
                m(&|c| c.turns[gap_turn].discarded as f64),
            );
        }
    }

    if !report.sweep.is_empty() {
        println!("\n### Blackout length sweep: gap turn ms (silence probes sent)\n");
        let names: Vec<String> = SWEEP_ARMS.iter().map(name).collect();
        println!("| blackout ms | {} |", names.join(" | "));
        println!("|---{}|", "|---".repeat(names.len()));
        for gap_ms in &a.sweep_gaps_ms {
            let cells: Vec<String> = SWEEP_ARMS
                .iter()
                .map(|arm| {
                    let pick = |p: &Plan| p.arm == *arm && p.gap_ms == *gap_ms;
                    let total = over_reps(&report.sweep, pick, |c| c.turns[gap_turn].total_ms);
                    let probes =
                        over_reps(&report.sweep, pick, |c| c.turns[gap_turn].recoveries as f64);
                    match arm {
                        Arm::TurnProbe => format!("{total:.0} ({probes:.0})"),
                        _ => format!("{total:.0}"),
                    }
                })
                .collect();
            println!("| {gap_ms} | {} |", cells.join(" | "));
        }
    }

    if !report.evicted.is_empty() {
        println!("\n### Checks\n");
        println!(
            "- evicted session (turn arm, no gap): turn {gap_turn} took {:.0} fallback(s) to the full prefix, {:.0} B up, TTFT {:.0} ms against {:.0} ms unevicted",
            evicted(&|c| c.turns[gap_turn].fallbacks as f64),
            evicted(&|c| c.turns[gap_turn].link.up_bytes as f64),
            evicted(&|c| c.turns[gap_turn].ttft_ms),
            cell(Arm::Turn, GapKind::None, &|c| c.turns[gap_turn].ttft_ms),
        );
    }
    if let Some(deadline) = &report.deadline {
        let checks = [
            (
                "deadline at half the generation time, no gap",
                &deadline.mid_generation,
            ),
            (
                "deadline inside a break, after generation finished",
                &deadline.inside_break,
            ),
        ];
        for (what, check) in checks {
            match check {
                Check::Skipped(why) => println!("- {what}: skipped, {why}"),
                Check::Ran(d) => println!(
                    "- {what}: declared {} ms, client failed after {:.0} ms (\"{}\"); server cut generation short at {:?} of {} B and holds {} turn(s) 100 ms later",
                    d.declared_ms,
                    d.client_failed_after_ms,
                    d.client_error,
                    d.server_cut_short_at,
                    d.full_output,
                    d.server_turns_held_after
                ),
            }
        }
    }
    for failure in &report.failures {
        println!("- FAILED: {failure}");
    }
}
