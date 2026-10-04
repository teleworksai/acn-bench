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
2. **T05.2** (split in two by ADR-19). The quantity formulas over the views, including the price table behind `cost_per_success`. Then evaluation with undefined values and Kleene logic (HYP-11), slices and cells (HYP-14), the bootstrap and `verdict.json` (HYP-15), the refusals and rules of HYP-20 to HYP-24 and HYP-28, and `acn hyp verdict`.
3. **T05.3.** The read-only and `hypothesis_changed` rules (HYP-4), no relax option (HYP-25), and the freeze PR's shape (HYP-26).

### Readings T05.1 makes
- **Units.** A literal, a counter, the result of `*` or `/`, `rel_effect`, and a quantity the table does not resolve (allowed in a candidate) are compatible with any unit. Every other unit is a name compared for equality.
- **Arms.** `effect`, `rel_effect`, `ci_low` and `ci_high` count as both treatment and per-cell, which is HYP-12's "per-cell built-in". `noise_floor`'s `control` argument is not a control term in HYP-12's sense: that is a `select` choosing the control arm.
- **The guard** admits neither arithmetic nor unary minus. HYP-10 lists the constructs it may use, and they are not among them.
- **`at` stands only at the top level.** It quantifies the whole predicate, so `(a < 1 at all cells) or b < 2` is refused rather than given a meaning HYP-10 does not state. The rendering of a parsed predicate prints a top-level `at` without outer parentheses, so it parses back.
- **An `at` bound** names a pooled `range` or `int_range`. A bool or enum parameter is ranged over with `at … cells`. The bound must be met by some value a run takes (one of the grid's `levels` when it has them, otherwise a value in `[min, max]`), and it may not bound a parameter a selector fixes. Otherwise the clause is vacuous.
- **Selectors are normalised.** A shorthand value and `name = value` are one spelling, sorted with the arm first. Two spellings of one term are the same term, and lint gives them one slot. A fixed value must be one a run takes. Fixes in a control select count like any other:
  - a parameter fixed anywhere may not be free in a treatment term or under a per-cell built-in;
  - with a control term present, each parameter takes one value across every term.
- **Control terms and the per-cell rule (HYP-14).** `q(fixed) - q(control)` is slice-level when the treatment terms fix every pooled parameter and nothing else is free. The control's cell is then determined by the treatment's.
- **Status and the workspace root.** A file is frozen when it lies under `hypotheses/` of the CON-28 root found from the current directory, the record in `env-hash.json` lists it with its hash, and the frozen set's current hash matches the record. Otherwise the file is a candidate, with a warning naming the reason, so a locally edited copy is a candidate (HYP-3). Status is read through accessors; nothing in the public API can mark a file frozen.
- **File checks.**
  - `[poc].spec` is `specs/<name>.md` and its file name appears as a cell of the README's catalogue.
  - `[control].config` is not empty.
  - `provider` and every `pooled = false` parameter have finite domains.
  - Integers stay `i64` throughout.
  - A tolerance is `{ abs = x }` with `x > 0`, or a plain relative value.
  - `pins.models` keys are `provider` values.
  - `id` is unique across the directory and its subdirectories. A file that cannot be read or parsed there is an error, since uniqueness cannot then be checked.
- **Parser limits.** Nesting is bounded at 64 levels and a predicate at 2048 bytes, so input is refused, never overflowed. A number must end at a token boundary, and a nonzero literal that underflows to zero is refused.
- **One evaluator.** `eval` holds the only evaluator, with undefined values and Kleene logic: a zero `noise_floor` and `rel_effect` over a zero control are undefined. Lint runs it over probe cells, and T05.2's verdict will run it over the views.
- **Witness search (HYP-27).**
  - Each distinct normalised quantity term is one probe slot, and so is each interval bound, each `noise_floor` and each counter.
  - Interval bounds are kept as `low ≤ high`, and a noise floor is never negative.
  - Search is exhaustive up to 10 slots. Past that, no search is made and the file gets a finding.
  - An `at` clause is evaluated in one probe cell, which load has shown can satisfy its bound.
  - A frozen file's lint failures are errors. A candidate's are warnings, which is what HYP-27 calls advisory.
- **Negative values (spec-conflict on HYP-27).** HYP-27's probe values are non-negative, but an effect can be negative. When the search finds no witness, lint searches again with the signed values `±1e9, ±1, ±1e-9, 0`, up to 6 slots. A witness found only there is reported as a warning naming the conflict, not as an error. This stands until issue #10 is resolved by a `spec-change`.
- **The guard is linted.** For a run with the design's replicates and providers equal to the provider count, the guard must evaluate to false. A guard that is true or undefined then would disarm the falsifier for every complete run, and that is a finding.
- **HYP-27 in CI.** The CI requirement is met by the acceptance suite `tests/accept/hyp_lint.rs`, which `cargo test --workspace` runs. Adding a step to `tools/ci.sh` would change CON-9's list of gates.
- **The quantity table** names the six quantities `hypotheses/p4.toml` measures, with their units and documented formulas. `cost_per_success` is in the unit `cost`; its price table is ADR-19's.

## Consequences
- `acn hyp lint` exists, `hypotheses/p4.toml` lints clean, and `docs/generated/quantities.md` is rendered.
- Every file in `hypotheses/` and `lab/hypotheses/` is checked when it loads.
- A falsifier that fires only on a negative effect lints with a warning until HYP-27's probe set is settled.
- No verdict exists until T05.2.
