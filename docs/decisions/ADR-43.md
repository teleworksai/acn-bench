# ADR-43 — The call regime and the KV-read bytes in the frozen schema

**Status:** accepted (PR 69: spec-change and env-change). **IDs affected:** TRC-12, TRC-33, MLM-30a.

## Context
The v1.5 patch (PR 69) adds two attributes to SPEC 010:
- `acn.call.regime`: `prefill`, `midfill` or `decode_only`, derived;
- `acn.cache.read_bytes`: the KV bytes a midfill reads.

It also adds two matching columns to the call view, `regime` and `cache_read_bytes`. TRC-20's test requires every `acn.*` name in SPEC 010 to be in the attribute inventory, which is frozen with the trace schema (CON-7). So PR 69 failed its gates until the schema caught up. The maintainer chose to add the two attributes (2026-10-08).

## Decision
- **The inventory gains two optional `chat` attributes**, both produced by `acn-harness` and `acn-gen`, both promoted:
  - `acn.call.regime`, a string over its three values;
  - `acn.cache.read_bytes`, an integer in bytes.

  The call view gains `regime` and `cache_read_bytes`, both nullable.
- **The regime is the server's view** (the maintainer's choice, 2026-10-08, after the adversarial review). With uncached = input − cache-read tokens:
  - `decode_only` when nothing is uncached;
  - `midfill` when some tokens are read and some uncached;
  - `prefill` when nothing is read.

  The patch's rule used the harness's lineage-relative new-input count (HAR-32) beside the provider's read count. That mislabels calls: a continuation on a provider without caching read as `decode_only` although the server prefilled everything, and a fan-out sibling served wholly from cache read as `midfill`. The two counts also made the three cases overlap. On one measure they cannot overlap. Nothing is derived from a missing or negative count, or from a read larger than the input.
- **One implementation, checked.** `acn_trace::ingest::derive_regime` is the rule. The harness records its result on each `chat` from the counts it records. The call view's `regime` is always the derived value. A recorded `acn.call.regime` that differs from it, or that is recorded where the counts derive none, is refused (TRC-12: derived, never reported). A trace that records none, made by an earlier harness or imported, gets the derived value.
- **`acn.cache.read_bytes` is checked where it can be.** It is never negative, never present without a cache-read count, and zero when no tokens were read. The full product (`read_tokens × kv_bytes_per_token`) can be checked only once the bundle records the KV size, when MLM-30a lands.
- **`regime` is nullable in the view.** A call that reported no usage derives none. SPEC 010's TRC-33 says so; the patch had marked only `cache_read_bytes` nullable.
- **`acn.cache.read_bytes` is listed but not yet produced.** It needs a KV size per token, which MLM-30a puts in the mock's profiles (`kv_bytes_per_token`) and which no profile has yet. Its `when` says so. It becomes non-null when MLM-30a is implemented, a later SPEC 030 task. Until then nothing emits it.

## Consequences
- **The frozen schema moves,** so `engine_hash` moves and every `run_id` with it (CON-28). Both attributes are promoted columns of `spans.parquet`, whose schema is checked exactly (TRC-25). So bundles made before this PR are unreadable by current readers (`bundle verify --views`, `hyp verdict`, `attrib`), as for any schema change. Their hashes still verify. No bundle is committed, and the run records under `docs/runs/` are JSON and Markdown.
- **The pinned `sim` bundle digests** of `tests/accept/harness_sim.rs` change, because every `chat` span now carries its regime and the call view has two more columns. Nothing else changes: `shared_tokens` is pure and is called under the same condition as before. They are re-pinned. No other test pins these bytes.
- **The pinned set of optional attributes** (`attributes_inventory.rs`) and the count of attributes on `chat` are updated.
- **This PR is Class C:** labelled `env-change`, with an adversarial review, merged by the maintainer.
