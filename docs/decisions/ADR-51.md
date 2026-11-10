# ADR-51 — SPEC 040 v0.5: the request a live run sends, and a reply cut at its limit

**Status:** accepted (`spec-change`; issue #27, following ADR-29). **IDs affected:** HAR-3, HAR-21, HAR-60, P4-3, P4-9; by reference TRC-12, CON-29, P4-10.

## Context
Issue #27 lists three things between POC 4 and a live run on current models:
- the harness always sends `temperature`, and some models refuse it;
- the P4 workloads set a token limit of 256, and on models that reason, reasoning tokens count against it;
- HAR-3 counts a reply cut at its token limit as a success, so a truncated or empty reply lowers `cost_per_success`.

The maintainer decided on 2026-10-05 (ADR-29) that live request settings are the providers' typical defaults: the harness sends no sampling parameter a provider does not need. That decision left the spec text for the live tier. This ADR writes it, ahead of the live runs, so the request path can be built and tried on the mock first.

## Decision
- **HAR-60: `temperature` is optional.** A workload that states none sends none (HAR-21). No default stands in for it, so the rule that nothing is implied at load time holds for every other key.
- **HAR-60: `model_max_tokens`**, an optional table from model identifier to a token limit, replaces `max_tokens` on the agent's calls for a run of that model. It is in the workload, so it is hashed with it. `summary_max_tokens` is not affected: a summary is short on every model.
- **HAR-3: a response cut at its token limit ends its turn with `failure`**, and is read as asking for no tool whatever it carries. The rule reads the normalised `acn.call.stop_reason = max_tokens`, so it is one rule on both dialects and on the mock.
- **SPEC 100 v0.4** restates P4-3 and P4-9 item 3 in these terms, and says that changing a workload for a live run changes its `workload_hash`, so the L1 run of record is made again on the pinned files.

**Options not taken.**
- A token limit per model in SPEC 100 or in the hypothesis file's pins. The limit shapes the request, and everything that shapes the request is in the workload, under one hash.
- A separate outcome for a truncated reply. TRC-11 has four outcomes and `cost_per_success` reads `success` only; `failure` is what a checker would say of an unfinished reply.
- Removing `temperature` from the format. The mock and the existing workloads state it, and a hypothesis about sampling may want it.

## Consequences
- **No existing workload changes, and so no `workload_hash`.** The three P4 workloads and the smoke workload keep their `temperature`. Dropping it from the P4 workloads is part of pinning them for live runs (T06d), and is the maintainer's to approve with the models.
- **The code follows in a Class B PR** (`crates/acn-harness`). Until then the harness requires `temperature`, refuses `model_max_tokens` as an unknown key, and counts a truncated reply as a success.
- **Mock bundles change with that PR wherever the mock truncates.** The mock cuts a reply longer than the request's limit and says so. The smoke workload's limit of 64 is below its profile's longest reply, so some of its turns will become failures and the digests pinned in tests will change. The P4 workloads' limit of 256 equals their profiles' longest reply; whether any P4 turn changes is for that PR to measure and state. No committed loop is re-judged.
- **A truncated compaction summary is not covered.** It is not a turn's response, and the turn goes on with whatever summary came back. SPEC 040 §10 may want a question on it once a real model has been seen to do it.
- **A truncated response that carried a whole tool call is treated as carrying none.** That loses a call a provider did make. It is the conservative reading, and it keeps the outcome a function of the stop reason alone.
