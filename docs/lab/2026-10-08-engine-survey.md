# Lab note — engine survey, vLLM v0.30 and SGLang v0.5.21 (2026-10-08)

Track: lab (CON-23). No crate; a reading of the two engines' shipped code and roadmaps as
of 8 October 2026, done for ACN report v1.6. **Nothing here is a measurement.** The only
numbers are the engines' own published ones, labelled as such, and none may be cited
(CON-24). Companion page: "SGLang versus vLLM" (report v1.6, §17.2).

## Question

Which engine-side facts change what acn-bench must record, which provider rows POC 4 needs,
and whether the hint interface the report calls I-1 now exists in shipped form.

## What was read

- vLLM v0.30.0 (22 Sep 2026) and its disaggregated-serving guide (29 Sep 2026); the Q2 2026
  roadmap (issue #39749); the OpenTelemetry example.
- SGLang v0.5.21 (1 Oct 2026); roadmaps #22949 (Q2), #21846 (distributed KV cache for agentic
  workloads), #21703 (PD disaggregation); RFCs #27574 (Programmatic KV Cache for Agentic
  Workloads) and #24656 (Agent-Aware KV Cache Phase 1); the production request-tracing and
  router documentation.

## What was learned

**Both engines satisfy TRC-18 today, and neither emits the bytes side.** vLLM exports OTLP
spans through a documented example and accepts a propagated `traceparent`; SGLang's
`--enable-trace --otlp-traces-endpoint` gives a request → thread → slice span tree, with
links across its ZMQ hand-offs, levels switchable at runtime, and the router hop included.
Neither emits, per request, the KV bytes fetched from a lower tier, the tier that hit, or the
restore time. So `acn.cache.read_bytes` (proposed in patch 0002, SPEC 010 TRC-12) cannot come
from engine spans in the live twin; it has to come from the harness's own accounting or from
the cache provider's logs (LMCache, Mooncake Store, HiCache) until engine instrumentation
lands. SGLang's RL users filed the same complaint on #27574: aggregate D2H/H2D counters cannot
show which transferred bytes were reused. Proposed as SPEC 010 §10 open question 4 (spec-change
PR, not this one).

**Two lifecycle primitives are now shipped and belong in the blob model (p19).**

| Primitive | Side | Shipped form | Default / unit |
|---|---|---|---|
| lease | producer: how long the computing node holds a blob for a consumer told where it is | vLLM `kv_lease_duration` on the disaggregated path | 30 s |
| retention TTL | consumer: ask that a prefix survive ordinary eviction | SGLang `KvHintEnvelope { retention: [{prefix_tokens, ttl_seconds}] }` → Pin = Mooncake L3 lease; OpenAI `prompt_cache_retention`; Anthropic `cache_control`; Dynamo maps the two API fields onto SGLang's | seconds; API tiers 5 min / 1 h |
| load-failure policy | consumer: what to do when the producer's blocks are gone | vLLM `kv_load_failure_policy = fail \| recompute` | `fail` |

`p19-blob-lifecycle` (patch 0002) should carry `lease_s` and `retention_ttl_s` as varied
parameters once it is reconciled with this note; the rebuild-from-text option of report §4.3 is
`recompute` here.

**The hint interface I-1 has a shipped engine form, in SGLang only.** Verbs: Pin (implemented),
Retain (eviction priority, building on the priority radix strategy), Prefetch and Demote
(deferred until L3 restore time is measured). Principle stated in the RFC: the orchestrator
owns policy, the engine executes, every hint may be rejected. Of the report's five upward
fields (§5.5), the pin request and the session declaration (`session_id`, blocks tagged by
session) have a shipped form; expected duration, fan-out notice and batch schedule do not; of
the three downward fields, none is returned. vLLM carries no session or hint field natively and
expects them from llm-d, Dynamo or LMCache. **That makes three vocabularies for one quantity**
(SGLang verbs; Dynamo's hint set; the API-level retention fields), which is G4 restated and the
reason for candidate `p20-hint-equivalence`.

The single published measurement of the mechanism, SGLang's Pin POC on MiniMax M2.7 (H100):
an unretained cold worker recomputed 10,032 tokens; a retained worker restored 10,016 from L3
and computed 16. *Engine's own number; exploratory here.*

**A correctness hazard for any cross-site KV reuse.** vLLM's guide documents that when a
chat template drops the previous turn's reasoning tokens, prefill and decode hold blocks for
different prefixes, the mismatch is not detected, and the output is wrong. Nothing in either
engine checks that two parties agree on the producing prefix. A KV blob that crosses an
administrative boundary needs the content hash of its producing prefix carried with it; for
acn-bench, POC 11's prefix-block hashes (SPEC 010 §10 Q1) are the natural carrier.

**Engine-internal form of the decision/movement split (report §4.7).** vLLM: thin engine,
prefix identity is a content hash of fixed-size blocks, every tiering and transfer decision goes
through the KV-connector interface (NIXL, LMCache, Mooncake, FlexKV, AMD MoRI-IO; a
multi-connector chains them), routing and retention are external (llm-d, Dynamo,
production-stack). SGLang: radix tree inside the engine, HiCache L2/L3 native with a direct-L3
mode, session-aware tree, hint API. Both sit on Mooncake TE or NIXL as the movement layer. For
acn-bench the engine is therefore a provider axis, not the subject.

**Midfill paths.** SGLang's delta KV transfer (decode reports its cached prefix, prefill sends
only the uncached suffix) and vLLM's `bidirectional_kv_xfer` are the engines' midfill
mechanisms. Both are intra-cluster; neither has a cross-site mode. That is the empty cell H-6
describes, and it is what POC 7 and POC 8 measure around.

**A placeable component.** vLLM's render tier (CPU-only chat templating, tokenization and
tool-call parsing; tokens-in, tokens-out engine boundary; the guide reports ~15 ms CPU for a
9K-token prompt and ~73 req/s per core) is the first engine piece an operator could host at a
regional or access-edge site. The tokens boundary is also where POC 11's delta encoding would
sit if it ever moved off the client.

## Consequences for the substrate (proposed, not done here)

1. **POC 4 provider rows.** `hypotheses/p4.toml` is frozen (M0 closed); its `provider` enum
   should gain `sglang-hicache-l3` and `vllm-offload-connector` beside the `tensormesh` row
   proposed in patch 0002. Bundle this into the Class C env-change that GATE-14 / T06d will
   need anyway when real providers come in; do not open a separate env-change for it.
2. **SPEC 010 §10 open question 4** (spec-change): the source of `acn.cache.read_bytes` in the
   live twin while engines emit no per-request KV-movement attributes.
3. **p19** gains `lease_s` and `retention_ttl_s`; **p20** enters as a candidate with this note.
   Its falsifier is per provider slice (hints have an effect at all); the equivalence judgement
   is a comparison *across* slices, which the predicate language of SPEC 080 does not express
   (HYP-12). A cross-slice comparator is a HYP spec-change to propose only if p20 graduates.
   `restore_bytes`, `recomputed_tokens` and `hint_rejections` are not yet in the quantity
   table (HYP-7 warning, acceptable for a candidate); they need the engine or provider-log
   source of item 2 before they can be defined.
4. **PLAN §11**: the GPU-node prerequisite is satisfiable by either engine; p20 adds a router
   with a hint API (sgl-router, or Dynamo in front of vLLM) as a prerequisite of its own.

## Graduate / park / drop

- `p20-hint-equivalence`: **park** until one GPU node and a router with hints exist (PLAN §11);
  graduate as POC 17 after a first run shows any measurable effect of hints.
- Provider rows for p4: **graduate** with the GATE-14 env-change.
- Open question 4: **graduate** as a one-line spec-change.
