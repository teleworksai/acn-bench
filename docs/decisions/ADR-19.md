# ADR-19 — T05.2 lands in two PRs; the price table and the readings T05.2a makes

**Status:** accepted (T05.2a, Class C). **IDs affected:** HYP-11 to HYP-15, HYP-21, HYP-22; CON-5, CON-27, CON-30. **Amends:** ADR-18's plan for T05.2.

## Context
ADR-18 planned T05.2 as one PR covering:
- the quantity formulas and the price table;
- evaluation over real data;
- the bootstrap;
- every rule of HYP-20 to HYP-24;
- `verdict.json`;
- `acn hyp verdict`.

That is two concerns of different kinds:
- **Arithmetic**, which can be checked on synthetic numbers: what a quantity is, how a slice is evaluated, and how the bootstrap draws.
- **Bundle handling and verdict policy:** which bundles are refused, how they map to slices, cells and arms, the twin rule, labels, and the output file.

ADR-11's slicing rule applies: split along what can be green and reviewed alone.

HYP-12 puts a per-provider price table behind `cost_per_success` in this frozen crate. The options, put to the maintainer with PR #9, were:
- (a) list prices per model;
- (b) relative weights per provider;
- (c) per-model prices beside `[design].pins.models`.

The maintainer started T05.2 without choosing, so CON-15 applies: this ADR takes the recommended option and records it.

## Decision

### Two PRs
1. **T05.2a (this PR).**
   - The formula of every quantity, over one replicate's view rows (HYP-12), and the price table.
   - The bootstrap: the verdict seed, sub-streams, the ranged sampler, percentile indices, the effect interval and the split-half noise floor (HYP-13, HYP-15).
   - Evaluating a falsifier over one slice's data, with undefined values and Kleene logic (HYP-11), and aggregates, selects, controls and `at` over cells (HYP-12, HYP-14).
   - The outcome HYP-21 reads, the record HYP-28 prints, and HYP-22's decision cells.
2. **T05.2b.**
   - Reading verified bundles into slice data, including the control mapping of HYP-8 and the ignored replicates.
   - The refusals of HYP-20, and the slice and file rules of HYP-21 to HYP-24 with the guard.
   - `verdict.json` (HYP-15, HYP-28) and `acn hyp verdict`.
   - The tests `verdict_semantics.rs` and the verdict-level half of `verdict_determinism.rs`.

### The price table (option b)
- **Units.** Each row gives the price of a cache read, a cache write and an output token, each a multiple of one uncached input token. `cost_per_success` is in those units.
- **Why ratios are enough.** `provider` is never pooled (CON-26), so every comparison a predicate makes is within one provider. Scaling a row scales an effect and its noise floor alike, so only the ratios can change a verdict, and ratios move far less than prices.
- **Rows.**

  | Provider | Cache read | Cache write | Output | Taken from |
  |---|---|---|---|---|
  | `anthropic` | 0.1 | 1.25 | 5 | Claude 4.x list prices with 5-minute cache writes, October 2026 |
  | `openai` | 0.1 | 1.0 | 8 | GPT-5 family list prices (automatic caching, no write surcharge), October 2026 |

  The source of each row is recorded in the table itself.
- **No row for `vllm` or `sglang`.** Self-hosted servers have no list price, so `cost_per_success` is undefined there and those slices are inconclusive until a Class C PR adds a row. Inventing a compute proxy would put a number in a frozen file that nothing supports. With `min_providers_for_verdict = 2`, POC 4 can still reach a verdict on `anthropic` and `openai`.
- **Which row applies.** The key is the bundle's `vary.provider` when the file has a `provider` parameter, and its backend otherwise.
  - In a file with a `provider` parameter, a mock bundle is priced as the provider it stands in for.
  - In a file without one, its key is `mockllm`, which has no row, so `cost_per_success` is undefined.
  - Mock bundles are labelled `mock-gated` in any case (HYP-23).
- **The cost formula.** Uncached input is `input_tokens − cache_read_tokens − cache_write_tokens`, because `input_tokens` is the total prompt (TRC-12). A negative result is undefined.
  - This reads writes as disjoint from reads, as Anthropic reports them.
  - The mock's explicit-breakpoint profile does not: MLM-21 counts the write from token 0, so it overlaps the read. Issue #12 asks for a `spec-change` to MLM-21.
  - Until then, mock and sim values of `cost_per_success` under that profile are undefined or biased. They are never cited (CON-26).

### Formulas
- **No partial totals, no dropped calls.** A formula over a replicate's calls is undefined when any call lacks a value it reads, as in the views. That makes the replicate undefined, and so the arm (HYP-11).
  - This applies to `cost_per_success`, `input_tokens_per_turn` and `cached_token_ratio`.
  - It applies to `ttft_p50_ms` and `ttft_p99_ms` too. A call that never produced a token is not dropped from the percentile: dropping it would make a treatment that times out more look faster, which is HYP-11's survivorship bias.
  - For `openai`, `vllm` and `sglang`, an unreported cache-read count is absent, not zero (TRC-21). A call that returns usage without one therefore leaves `cached_token_ratio` undefined.
- **Percentiles** are nearest-rank, at rank `ceil(p/100 · n)`.
- **Division by zero** is undefined: no turn succeeded, no turn, or no session.

### The bootstrap
- **The sampler.** The ranged-integer sampler that HYP-15 names is Lemire's multiply-shift on one 64-bit draw. It rejects a low product below `2^64 mod n`. It is implemented in this crate rather than taken from `rand`, so the frozen code fixes the draws and no new generator crate needs pinning. Its golden vector, computed independently of the crate, is in `verdict_determinism.rs` (CON-5(a)).
- **Cell keys.** A cell's key covers every `[varies]` parameter, the non-pooled ones included, because a cell is an assignment to every parameter (§1). The `noise_floor` key is the control's effective configuration. Values are in the text form of CON-27(c), so a `range` value is always a float, as in `rtt=150.0`.
- **Sharing resamples.** One evaluation keeps every set of resamples it draws. `ci_low`, `ci_high` and every `ci` level then read the same draws.
- **Levels whose bounds cross** (spec-conflict, issue #13). Below about `ci = 1e-4`, HYP-15's lower index passes the upper one, and the half-width would be negative. Load refuses such a level, and `bounds` is undefined if the indices ever cross.

### Evaluation over a slice
- **What aggregates and `at` range over.** They range over the distinct assignments of the parameters no selector fixes, taken in the HYP-14 order of the slice's cells. For a grid, the slice holds every grid cell, and a cell with no runs is undefined (HYP-21).
- **Treatment selects.** A treatment `select` reads the cell that agrees with the current assignment on the free parameters and with the select's own fixes on the others.
- **Control selects.** A control `select` applies every fix in the predicate. With a control term, each parameter has one value across all terms (ADR-18), so this gives the complete treatment configuration of HYP-12. It then reads the control arm that cell maps to.
- **The `replicates` counter.** Within a slice, a cell with no control arm counts as zero completed control replicates.
- **No cell.** A predicate with nowhere to be evaluated is undefined: no evaluated cell, no cell in an `at` range, an empty aggregate, or a slice with no control for `noise_floor`.
- **The outcome HYP-21 reads.** Kleene's rules can make a falsifier false while some operand is undefined (`false and U`, or an `at all` with one false cell and one missing). HYP-21 does not let that pass. So an evaluation:
  - visits every cell an aggregate or `at` ranges over, and evaluates both operands of every connective, comparison and arithmetic operator;
  - observes every sub-expression;
  - reports `Refuted` (true), `NotRefuted` (false with no undefined operand anywhere) or `Undefined`.
  - Only `NotRefuted` can become a `pass`.
- **The record (HYP-28).** An evaluation records every sub-expression's value, keyed by its rendering and the cell whose free parameters it was evaluated at. It also records every term read: the cell and control it resolved to, its value, the completed replicates behind it and, for an interval bound, the interval. T05.2b writes this record into `verdict.json`.
- **Decision cells (HYP-22).**
  - An aggregate's are the cells read at the contexts where it attains its value.
  - An `at` clause's are every cell in range when `at all` holds or `at any` fails (or the value is undefined), otherwise the first deciding cell.
  - Cells read outside any aggregate or `at`, such as a slice-level `select`, also decide the value, so they are decision cells too. Otherwise a predicate with neither construct would have none to twin.
- **Checked slice data.** `SliceData::new` checks the data against the hypothesis:
  - every cell assigns every parameter a value of its domain;
  - cell and control keys are unique;
  - every arm holds exactly `[design].replicates` entries;
  - every control index exists.

  It then sorts the cells into HYP-14 order. A control records whether it is a `config` or a `workload` control (HYP-8). Parameter values parse from the manifest's text: a `range` value must be written as a float. Integers compare as integers, never through a double.
- **The `replicates` counter** within a slice does not count a grid cell with no runs: that cell is not evaluated, and is already undefined.

## Consequences
- POC 4's quantity can be computed for `anthropic` and `openai`. Its verdict is still T05.2b's.
- A run with even one call that returned no usage, or no token, leaves every quantity that reads it undefined for that replicate's arm. On real providers, that makes transport errors visible as inconclusive cells instead of hiding them. SPEC 100 should say whether POC 4 retries such replicates.
- Changing a price row, a formula or the sampler is a Class C change that moves `engine_hash`.
