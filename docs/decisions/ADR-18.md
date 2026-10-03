# ADR-18 — T05 lands in three PRs, and the readings T05.1 makes

**Status:** accepted (T05.1, Class C). **IDs affected:** HYP-1 to HYP-16, HYP-27; CON-7, CON-12.

## Context
SPEC 080 has several parts:
- a strict file format;
- a typed predicate language;
- a table of quantity formulas over the views;
- a verdict engine with a seeded percentile bootstrap, twin rules, slices, labels and a byte-identical `verdict.json`;
- lint, and the read-only and freeze rules.

All of it lives in `crates/acn-hyp`, which is in the frozen set, so every PR that touches it is Class C. ADR-11's slicing rule applies: split along what can be green and reviewed alone.

## Decision
T05 lands as three PRs, each Class C, each updating `env-hash.json`:

1. **T05.1 (this PR).** The file format and status (HYP-1 to HYP-9); the predicate language, its parser, its types and units, the built-ins' signatures, the guard's restrictions, the selector rules and the per-cell/slice-level rule (HYP-10 to HYP-14, their static halves); the quantity table and its rendering (HYP-12); the two existing files (HYP-16); and `acn hyp lint` (HYP-27).
2. **T05.2.** The quantity formulas over the views, including the price table behind `cost_per_success`. Then evaluation with undefined values and Kleene logic (HYP-11), slices and cells (HYP-14), the bootstrap and `verdict.json` (HYP-15), the refusals and rules of HYP-20 to HYP-24 and HYP-28, and `acn hyp verdict`.
3. **T05.3.** The read-only and `hypothesis_changed` rules (HYP-4), no relax option (HYP-25), and the freeze PR's shape (HYP-26).

### Readings T05.1 makes
- **Units.** A literal, a counter, the result of `*` or `/`, `rel_effect`, and a quantity the table does not resolve (allowed in a candidate) are compatible with any unit. Every other unit is a name compared for equality.
- **Arms.** `effect`, `rel_effect`, `ci_low` and `ci_high` count as both treatment and per-cell, which is HYP-12's "per-cell built-in". `noise_floor`'s `control` argument is not a control term in HYP-12's sense: that is a `select` choosing the control arm.
- **The guard** admits neither arithmetic nor unary minus. HYP-10 lists the constructs it may use, and they are not among them.
- **An `at` bound** names a pooled `range` or `int_range`. A bool or enum parameter is ranged over with `at … cells`.
- **Witness search (HYP-27).**
  - Each distinct quantity term is one probe slot: a bare quantity is its treatment arm, and a `select` is keyed by its arm and its fixed parameters.
  - So is each interval bound pair, kept as `low ≤ high`, each `noise_floor`, and the `replicates` counter.
  - Search is exhaustive up to 10 slots.
  - An `at` clause is evaluated in one probe cell taken to satisfy its bound.
  - A frozen file's lint failures are errors. A candidate's are warnings, which is what HYP-27 calls advisory.
- **HYP-27 in CI.** The CI requirement is met by the acceptance suite `tests/accept/hyp_lint.rs`, which `cargo test --workspace` runs. Adding a step to `tools/ci.sh` would change CON-9's list of gates.
- **The quantity table** names the six quantities `hypotheses/p4.toml` measures, with their units and documented formulas. `cost_per_success` is in the unit `cost`; its price table is T05.2's, and needs the maintainer's decision on prices.

## Consequences
- `acn hyp lint` exists, `hypotheses/p4.toml` lints clean, and `docs/generated/quantities.md` is rendered.
- Every file in `hypotheses/` and `lab/hypotheses/` is checked when it loads.
- No verdict exists until T05.2.
