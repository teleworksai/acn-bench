# ADR-16 — T03: a minimal clock, and the readings SPEC 030 leaves open

**Status:** accepted (T03, Class B). **IDs affected:** CON-5, MLM-1, MLM-3 to MLM-5, MLM-7, MLM-20 to MLM-23, MLM-30, MLM-31, MLM-40, MLM-41, MLM-51.

## Context
SPEC 030 makes the mock read time only through the run's `Clock` (MLM-6). CON-5(b) puts that clock in `acn_emu::clock`, whose crate is otherwise T10–T12 work. Implementing the mock also needed choices the spec does not make.

## Decision
- **A minimal `acn_emu::clock` now.** It holds a `Clock` trait with `now_ns` and `sleep_until`.
  - `SimClock` starts at 0 and moves only forward. Waiting on it advances it at once, so a sim run takes no wall time.
  - `WallClock` is monotonic from its creation, which is the run's start (TRC-26), and sleeps on tokio's timer.

  This module is the one place the process clock is read; ADR-8's self-test allows the lint exemption only here. SPEC 020 ratifies or extends the interface at T10, and `acn_emu::rng` arrives with it.
- **Draw order per request (MLM-6).**
  1. Three fault draws (429, 500, cut), always made, so that a fault rate never shifts the other draws.
  2. For a 429, its retry-after.
  3. For a text answer, its length, then one word per token.
  4. For a cut stream, the cut point.
  5. One jitter draw per output token.

  Uniform draws use exact rejection sampling.
- **The first token is at the time to first token.** MLM-30 states the TTFT formula exactly. The jitter of token 1 is drawn, which keeps the draw count fixed, but not applied; jitter moves tokens 2 onwards.
- **What a breakpoint is.** Every `cache_control` member is one breakpoint, wherever it sits, so two marked parts of one message count as two. The prefix a breakpoint marks ends where the object carrying it ends in the canonical JSON: the end of a marked content part, or, for a marked message or tool definition, the end of its element including the `\n`. The count is checked when the prompt is built, before any draw, so an injected fault never replaces the 400. The other cache models ignore breakpoints and set no limit on them.
- **Breakpoints end on whole tokens.** A breakpoint at a byte offset *e* marks the prefix of ⌊*e*/4⌋ tokens. Rounding up would pull up to three bytes of the next element into the cached prefix, so an unchanged prefix would miss whenever the next element changed. More breakpoints than `max_breakpoints` is a 400, so MLM-21's "the first ones are honoured" never applies.
- **Automatic prefix stores every increment.** A request stores each prefix of `min_cacheable_tokens + k × increment_tokens` tokens that fits, not only the longest. Otherwise a later prompt sharing a shorter prefix could not find it.
- **`cache_write_tokens` under explicit breakpoints** is, as MLM-21 says, the length of the longest prefix the request wrote. Anthropic reports only the tokens written beyond the read, so the two differ when a request both reads a shorter breakpoint and writes a longer one. This is followed literally here, and flagged for the maintainer as a possible spec erratum.
- **Block eviction:** after a request's blocks are stored, leaves are evicted, earliest last use first, ties by the lowest hash, until the store fits. The leaves evicted can include the request's own newest blocks when it alone exceeds the capacity.
- **Profiles share nothing.** A profile is a model (SPEC 030 §2), and providers never share a cache or a serving pool across models, so each profile has its own cache and its own slots in a mock instance. Tenants still never share within a profile.
- **A tool call is one output token** and is streamed as one chunk carrying the whole call. Its arguments are `{}`, so splitting them across chunks would model nothing. The token limit caps text answers only (MLM-40), so a tool call is made even under `max_tokens: 0`.
- **The token limit.** `max_completion_tokens` wins over `max_tokens`. A `null` is an unset field, as clients serialising an unset option send it; any other value that is not a non-negative integer is a 400.
- **An empty answer streams** as one chunk with `{"role": "assistant", "content": ""}`, so that it assembles to the plain response's `""` (MLM-3).
- **Request shapes.** Beyond MLM-1's JSON and model checks, a message must be an object with a role MLM-1 names and string, null or array-of-objects content, and a tool must have a string `function.name`; anything else is a 400.
- **Faults.** A 429 or 500, like every error, is answered at arrival, with no slot taken, no cache change, and a timing header of zeros (MLM-31 puts the header on every response). A cut applies only to a streamed request: its cache accounting is computed on a copy and discarded.
- **Queueing.** The request starts in the slot that frees earliest (ties to the lowest index), and that slot is busy until the request's last token.
- **Identifiers.** `id` is `chatcmpl-` and the first 24 hex digits of the BLAKE3 of the tenant, the arrival time and the prompt bytes. `created` is the arrival time in whole seconds of the run's clock: deterministic, and never a wall-clock date in `sim`.
- **Tenants.** Over HTTP, the tenant is the `Authorization` header; in process, it is a parameter.
- **Ordering (MLM-7).** `Mock::handle_batch` orders simultaneous requests by arrival, tenant, then the BLAKE3 of their prompt bytes. A request with no prompt bytes (one that gets a 400) sorts first among its equals, by the BLAKE3 of its body. Over HTTP, requests are handled in the order the server takes its lock, and the arrival time is read under that lock, so the engine never sees arrivals go backwards; which request wins the lock is left to the scheduler, as `live` is statistical by definition (CON-5(d)).
- **The server marks what the router answers.** A 404, 405 or 413 that axum produces itself gets the `x-acn-mockllm` marker with `profile=-` and an OpenAI-shaped error body (MLM-1, MLM-4); so does `GET /v1/models`. Request bodies up to 256 MiB are accepted.
- **Profiles are checked wherever they enter.** `Mock::with_profiles` runs the checks `Profiles::parse` runs, since a caller can build `Profiles` directly.
- **Profile identity stays as SPEC 030 has it** (maintainer's decision). A profile's constants move `build_hash`, not `run_id`, and the profiles file's BLAKE3 is reported by `GET /v1/models`.

## Consequences
SPEC 030 is implemented, except two clauses that bind the harness (T04): MLM-4's requirement that a harness record `acn.backend = "mockllm"` when it sees the mock's marker, and MLM-60's bundle labelling. MLM-51's rule against reusing a profile name is a review rule, not a machine check. Running the mock as a standalone `live` server (an `acn` subcommand) is left for when the harness or generator needs it.
