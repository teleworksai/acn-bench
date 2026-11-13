# ADR-56 — The live POC 4 campaign: what is proposed, in what order, and what it waits for

**Status:** proposed (2026-11-12; nothing here is decided until the maintainer accepts it; no provider has been called, and API work is on hold by the maintainer's word). **IDs affected:** GATE-14; P4-9, P4-10, P4-11; HAR-60; HYP-9, HYP-20, HYP-23; LOOP-4; CON-26. Follows ADR-29, ADR-42, ADR-51, ADR-54.

## Context
GATE-14, POC 4 on at least two real providers, is due at M2 after two deferrals (ADR-42). What a campaign would spend was estimated in `docs/lab/2026-11-10-p4-pilot.md`, from the mock's token counts and list prices read on 2026-11-10. Since then SPEC 080 v0.3 is in the verdict crate (PR 101), so a file pinned to real models can still be run on the mock, and a workload may leave `temperature` out (PR 94).

Two facts are new on 2026-11-12:
- **The maintainer holds a $200 plan with each of Anthropic and OpenAI.** Both vendors' documentation, read that day, describes monthly API credits bundled with plans of that tier, used through an ordinary API key of a linked organisation: no rollover, and requests stop when the credit is spent. The amounts ($200 a month each, by the documentation's tables) and the eligibility of these two accounts were not checked against the accounts themselves.
- **A subscription login cannot stand in for an API key.** Neither vendor permits or supports raw API calls with it, and their agent CLIs build the request themselves, which removes what POC 4 controls.

## Decision (proposed)
**1. Tier.** The campaign runs on the cheapest current model of each provider's family, `claude-haiku-5-5` and `gpt-6-luna`, as the pilot note proposed. A mid-priced tier is a separate decision, taken after this one has a verdict.

**2. Order.** Each step is its own PR or run, and none starts before the one above it is accepted.

| step | what | class | calls a provider |
|---|---|---|---|
| a | The request each model accepts (SPEC 100 §7 question 3): `temperature` left out of the P4 workloads; `max_tokens` per pinned model; thinking off where the model allows it, which needs a harness option (SPEC 040) | `spec-change`, then Class B | no |
| b | The L1 run of record made again on the changed workloads, and P4-10's estimate read from it | run record | no |
| c | A pilot: 27 bundles a provider at 5 replicates, by `acn harness run`, as the pilot note describes; lab, never cited | lab | yes |
| d | `[design].pins` in `hypotheses/p4.toml`: the workload hashes of step b, the scenario hash the runs use, and the two models | Class C, `env-change` | no |
| e | The frozen grid on two providers: 387 bundles a provider at 20 replicates, then `acn hyp verdict` per P4-9 | runs | yes |

**3. Spend.** By the mock's counts the grid is about $76 on `claude-haiku-5-5` and $53 to $62 on `gpt-6-luna`; the pilot note's guess for a real run is 2.2 to 3.8 times that while the token limit stays at 256, and more if it is raised. The proposed ceilings are $25 for the pilot and, for the grid, the month's bundled credit of each provider and nothing beyond it. On Anthropic the guessed range ($170 to $290) straddles $200, so the grid may take two months' credit, or the ceiling is raised by the maintainer once step b and the pilot have replaced the guess. The estimate that binds is P4-10's, from step b.

**4. No `acn loop promote` for this campaign.** POC 4 declares `twin_required = false`, which waives LOOP-4's L3 check, and P4-9 asks only for real-provider bundles judged by `acn hyp verdict`. The bundles of steps c and e are made by `acn harness run`, one per cell, arm and provider, by a driver in `lab/` or `tools/`. `acn loop promote` stays with T30, where SPEC 085 puts its format. If the maintainer wants the L3 object for GATE-14's record, that is a SPEC 085 `spec-change` first.

## What it waits for
- **The maintainer lifting the hold**, with keys in the environment (HAR-22) and a ceiling.
- **OpenAI bills cache writes on the proposed model, and the mock does not model it.** OpenAI's prompt-caching guide, read on 2026-11-12, gives for "GPT-5.6 and later" a cache-write rate of 1.25 times the uncached input rate at the default 30-minute lifetime (2.5 times at 12 hours, in the Responses API), as a total rate and not a surcharge, and "no additional cache-write charge" for GPT-5.5 and earlier models. This settles the pilot note's finding 5 for `gpt-6-luna`: a write is billed. `mock-auto` records no cache write, so on that model the mock leaves out a cost term that depends on the knobs, and P4-10's estimate from an L1 run on `mock-auto` is low by it (the pilot note's upper figure, $62, is the bound). Three ways out, for the maintainer: pin an OpenAI model with no write charge (a `gpt-5.5` model, if its cached-token counters and price suit); or give `mock-auto` a write charge (a SPEC 030 change) before step b; or accept the term as a stated difference between L1 and the live run. The guide's remaining third was read only in summary.
- **Whether OpenAI's bundled credit covers Chat Completions**, which the harness speaks. The credit's page, read on 2026-11-12, says it covers "paid API Platform usage billed to the linked organization, including" the Responses API and others, adds "Other APIs are available through API Platform", and excludes only Codex usage through API keys. It never names Chat Completions. The list reads as open, so coverage is likely and not confirmed; one request on the account settles it. A Responses dialect in the harness (a SPEC 040 `spec-change`) is needed only if that request is refused, or if the 12-hour lifetime is wanted. The same page says a request beyond the credit fails with HTTP 429 and `credit_balance_exhausted`, which the harness would retry as a rate limit unless told otherwise.
- **Truncation** (SPEC 040 §10 question 6, issue #27): what a reply cut at its limit is, and the limit per model.
- **Thinking blocks on Anthropic** (pilot note, finding 8): whether the harness would send them back, and the option to turn thinking off.
- **Rate limits** of an organisation funded by bundled credit, which neither vendor's page states. The grid is about 250 000 calls a provider.

## Consequences
- Steps a and b are the only parts that can go ahead while the hold stands. Step a's first half is the maintainer's to decide: leaving `temperature` out is one decision for every provider, and it changes the workloads' hashes.
- A campaign on the cheapest tier answers GATE-14 as written. It does not show that the effect holds on a larger model.
- Nothing in this ADR changes a spec, a frozen file or a run.
