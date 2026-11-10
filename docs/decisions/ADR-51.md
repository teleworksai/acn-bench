# ADR-51 — SPEC 040 v0.5: a workload may leave `temperature` out; the rest of issue #27 stays open

**Status:** accepted (`spec-change`; issue #27, following ADR-29). **IDs affected:** HAR-21, HAR-60, P4-9; SPEC 040 §10 question 6; by reference HAR-3, HAR-4, HAR-14, TRC-11, TRC-12, CON-29, P4-3, P4-10, GEN-15.

## Context
Issue #27 lists three things between POC 4 and a live run on current models:
- the harness always sends `temperature`, and some models refuse it;
- the P4 workloads set a token limit of 256, and on models that reason, reasoning tokens count against it;
- HAR-3 counts a reply cut at its token limit as a success, so a truncated or empty reply lowers `cost_per_success`.

The maintainer decided the first on 2026-10-05 (ADR-29): live request settings are the providers' typical defaults, and the harness sends no sampling parameter a provider does not need. The issue proposed answers to the other two, and the maintainer has not decided them.

A first draft of this amendment wrote all three into SPEC 040. Its adversarial review found the second and third not implementable as written: four blocking gaps, each a choice that changes results. This ADR records what the amendment settles and what it leaves.

## Decision
- **HAR-60: `temperature` is optional.** A workload that states none sends none (HAR-21). No default stands in for it, so the rule that nothing is implied at load time still holds.
- **SPEC 100 v0.4** restates P4-9 item 3: leaving `temperature` out is now possible, it is one decision for all providers because the workloads are shared, and the other two points are open.
- **The cut reply and the token limit are an open question**, SPEC 040 §10 question 6, with what each proposal has to say before it can be written as a requirement.

## What the review found, kept here so it is not found twice
**A reply cut at its limit as a `failure`.**
- A sub-agent has no outcome of its own, so "ends its turn with `failure`" says nothing for a child's cut reply, and the two readings give different `cost_per_success` on the fan-out workload.
- A tool call cut mid-argument does not reach the rule today. On the `messages` dialect the stream fails to assemble; on `chat_completions` a call without a name is malformed. Either way the turn ends `aborted` with stop reason `other`.
- The spec does not say what the cut assistant message becomes in the context. Kept with its tool calls and no results, it makes the next request invalid on both dialects.
- A cut compaction summary replaces the context and the turn can still be a `success`.
- Precedence against `timeout` and `max_calls_per_turn` is not stated; the code checks the deadline first.
- Other unfinished stops (`content_filter`, a refusal) would still be successes.
- HAR-3 and TRC-11 say the outcome is the workload checker's and never the harness's; a truncation rule is a harness rule, and the text has to make room for it.
- SPEC 050's generator records `success` the same way and caps answers by design.

**A token limit per model.**
- Reasoning tokens count against `summary_max_tokens` too, so a limit that leaves the summary alone does not cover the models it exists for.
- It needs a lower bound that P4-3's check sees, a rule for a key that matches no model, and a stated TOML form for identifiers with `.`, `:` or `/`.
- CON-29 asks that in `live` mode sampling parameters and token limits be `opt.` parameters, which enter `run_id`. The workload carries them today, under `workload_hash`. Either the limit moves, or CON-29's text is brought in line; that is a constitution question.

**On the mock.** A reply of exactly the limit is not cut. The P4 profiles draw 32 to 256 tokens against a limit of 256, and a tool call is never capped, so a truncation rule would change no P4 turn. The smoke workload's limit is 64 and about 85% of its text answers are longer, so its outcomes, the digests pinned in `tests/accept/harness_sim.rs`, the POC 16 reference run and `tests/accept/attrib.rs` would change.

## Consequences
- **No existing workload changes**, and so no `workload_hash` or `run_id`. The mock never reads `temperature`, so a workload that leaves it out gets the same replies; only the request bytes differ.
- **The code follows in a Class B PR** in `crates/acn-harness` and `crates/acn-gen`, which builds its agent settings in memory. Until then the harness refuses a workload with no `temperature`.
- **Dropping `temperature` from the P4 workloads is still the maintainer's**, with the models and pins (T06d). It changes the workloads' hashes, so the L1 run of record is made again. On a provider it lets replicates differ more, which raises the `noise_floor` the falsifier compares the largest effect against, and so makes a `pass` harder.
- **The first live trial (the lab pilot) should record how often replies are cut and by how many tokens**, per model, before question 6 is settled.
- **Two documents still say the request settings land with the live tier**: the deferred table of `docs/gates/M0.md` and T06 in `TASKS.md`. Both stay true for the open part.
