# ADR-28 — T06b4: the mock's explicit-breakpoint cache writes partition the prompt

**Status:** accepted (T06b4; `spec-change` to SPEC 030 v0.3, and Class B in `acn-mockllm`). **IDs affected:** MLM-2, MLM-21; TRC-21, HYP-12; P4-7, P4-12. **Closes:** issue #12.

## Context
POC 4's first complete run of record on the mock (loop `f3afab22…`, budget 1 548, 13 minutes on the thinned trajectory of ADR-27) gave:
- **`openai`** (`mock-auto`): `pass`.
- **`anthropic`** (`mock-explicit`): `inconclusive`. Its `cost_per_success` was undefined in all 20 replicates of every `rolling_tail` cell, and of every `system_and_tools` cell on `retrieval`.

The cause is issue #12.
- MLM-21 counted a cache write as the whole prefix written, from token 0, so it overlapped the read.
- A breakpoint that moves along a growing conversation reads the previous marked prefix and writes a longer one.
- Read plus write then exceeded the prompt. The uncached input that `cost_per_success` prices (input − read − write, HYP-12) went negative, which makes the quantity undefined.

## Decision
- **The fix (MLM-21, SPEC 030 v0.3).** On `explicit_breakpoints`, `cache_write_tokens` is the longest prefix written less the read, or zero. This is Anthropic's accounting: `cache_creation_input_tokens` beside `cache_read_input_tokens`, so the uncached, read and written tokens partition the prompt, as TRC-21's mapping and HYP-12's formula already assume.
- **Why the mock changes, not the formula.** A provider-dependent cost formula would put mock behaviour into the frozen verdict engine.
- **No other cache model changes.** `automatic_prefix` writes nothing, and `block_granular` was never affected.

## Consequences
- **Superseded run.** The run of record `f3afab22…` is superseded and is redone on this build (P4-7).
- **Bundle bytes.** Sim bundles on `mock-explicit` change wherever a request reads one marked prefix and writes a longer one.
- **ADR-16.** Its reading that a write is "the length of the longest prefix the request wrote", flagged there as a possible spec erratum, is superseded by this ADR.
- **Tests.** The one hand-computed case in `cache_models.rs` changes from (16, 32) to (16, 16). New tests check the partition over a rolling breakpoint exactly, and the zero-write case: a new breakpoint inside the prefix read is stored and costs nothing.
- **A second gap, not fixed here.** Writing the rolling test showed that MLM-21 reads only prefixes the current request marks. Anthropic also looks back to earlier block boundaries, so each `rolling_tail` request on the mock reads only the system prefix. Issue #32 proposes a lookback parameter, and SPEC 100 P4-12 now lists the gap among the differences the mock deliverable states.
