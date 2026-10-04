# ADR-20 — The readings T05.2b makes: from bundles to `verdict.json`

**Status:** accepted (T05.2b, Class C). **IDs affected:** HYP-6, HYP-8, HYP-11, HYP-12, HYP-15, HYP-20 to HYP-24, HYP-28; TRC-23, CON-8.

## Context
T05.2b is the second half of T05.2 (ADR-19). It turns verified bundles into the slice data that T05.2a evaluates, and applies:
- the refusals of HYP-20;
- the slice rules of HYP-21;
- the twin rule of HYP-22;
- the labels of HYP-23;
- the file-level rules of HYP-24;
- the output of HYP-15 and HYP-28.

It also adds `acn hyp verdict`. Several of these rules leave room for reading, and this ADR records each reading.

## Decision

### Reading a bundle
- `read` runs `bundle::verify` (TRC-23), not `--views`. The hashes then bind the view files to the manifest, and the verdict reads the `session`, `turn` and `call` views directly.
- A replicate index of an arm is **completed** in a bundle when the bundle holds at least one session of that arm with that index. Every turn carries an outcome (`acn.turn.outcome` is required), so nothing more is needed.
- A replicate's quantities are computed over all of its sessions, their turns and their calls.

### What a bundle is
- **Its cell.** A bundle's cell is its `vary.<name>` values (HYP-6).
  - Every `[varies]` parameter must be present and must parse as a value of its domain. A `range` value must be written as a float (CON-27(c)).
  - A missing parameter, an extra one, or a value outside the domain is a refusal.
- **Its arms** come from `params.arms`. A session whose role is not among them is a refusal.
- **A config control bundle** is keyed by its own `vary` values, its effective configuration (HYP-8). A treatment cell maps to it by applying `[control].config` to the cell.
- **A workload control** is keyed by the parameters it inherits, plus the non-pooled ones.
  - The inherited parameters are `[control].inherits`, or by default every `range` and `int_range` parameter.
  - HYP-8 also counts any enum parameter the generator mode declares that it reads. The engine cannot know that, so such a parameter must be listed in `inherits`.

### Refusals (HYP-20, HYP-21)
- **What is refused.** Everything HYP-20 lists, plus:
  - an empty set;
  - the parameter cases above;
  - netem bundles beside sim bundles;
  - live bundles beside netem bundles.
- **Duplicate coverage.** It is checked per (slice, cell, arm, mode, replicate index), over indices below `[design].replicates`. One cell's replicates may come from several bundles, as long as no index appears twice.
- **Scenario and workload.** Within one mode, all bundles of one arm and configuration share their scenario and workload hashes. A treatment shares both with the control it maps to (only the scenario, for a workload control). A sim arm shares both with its live twin.
- **Pins.** The model pin is looked up by `vary.provider`, or by the backend when the file has no `provider` parameter.
- **The frozen seed.** It is checked for a frozen file only: every bundle must carry the file's derived seed (HYP-9).

### Slices and cells (HYP-21, HYP-24)
- **Slices.** The declared slices are every assignment of the non-pooled parameters. A slice's key is empty when the file declares exactly one slice.
- **Cells.** A `grid` slice's cells are every grid cell. Any other design's cells are the cells its treatment bundles ran, in the mode the quantities come from.
- **Evaluated cells.** A cell is evaluated when a treatment bundle for it exists in that mode, even if none of its replicates completed.
- **`control_missing`** applies when the file has no control, or when an evaluated cell has no control bundle.
- **Reasons are listed whenever they apply.** A slice can therefore `fail` and still list `grid_cell_missing`: a falsifier that is true refutes despite missing cells (HYP-11).
- **`undefined_value`** refers to every sub-expression that evaluated to undefined.

### The twin rule (HYP-22)
- **Which arms.** For each quantity with a tolerance, an arm's divergence compares sim and live over the replicate indices both modes completed. With no such index it is undefined.
- **Which cells.** The cells measured are those with a live arm.
- **The effect divergence** uses the indices completed in all four arms. A relative tolerance divides by the sim control's mean over those indices.
- **`twin_failed`** is a reason only when `twin_required` is true. Otherwise the divergences are reported and decide nothing.
- **Twinned cells.** A decision cell is twinned when both of its arms have live bundles with at least `replicates / 2` indices paired with sim.
- **Labels.** `sim-only` and `partially-twinned` apply per slice when the quantities come from sim. The file's labels are the union of its slices' labels.

### The file level (HYP-24)
- **Provider status.** A provider value is `not_run` when no slice with that value has a bundle. It is reported (`pass` or `fail`) when every one of its slices is conclusive, and `inconclusive` otherwise.
- **The guard's `replicates`** is the minimum over the slices of reported providers, or over every slice when the file has no `provider` parameter. With no such slice it is undefined, so the guard gives `inconclusive`.
- **`slice_inconclusive`.** In a file with a `provider` parameter, it counts only slices that have bundles. In a file without one, it counts every slice.

### `verdict.json` (HYP-15, HYP-28)
- **Top-level keys:**
  - `verdict_id`, `verdict`, `reasons`, `labels`;
  - `hypothesis {id, status, hash}`, `expected {outcome, note}`;
  - `providers`: every declared value with its status, or `null` when the file has no `provider` parameter;
  - `bundles` (`run_id`, `bundle_digest`, `mode`, ascending);
  - `ignored_replicates` (per `run_id`);
  - `twin_only_cells`, `engine_hash`, `build_hashes`;
  - `replicates`: the guard's counter;
  - `slices`.
- **Each slice** carries:
  - its key and parameters, `verdict`, `reasons`, `labels`;
  - the falsifier's value and `outcome`;
  - its `replicates`;
  - `cells` in HYP-14 order;
  - `controls`;
  - `decision_cells`;
  - every evaluated sub-expression (`values`) and every term read (`readings`).
- **Each cell** carries:
  - its treatment arm, with its completed, incomplete and undefined replicates and its mean of every quantity;
  - its control's key;
  - the effect of every primary quantity with its 95% interval (CON-18), drawn from the same sub-stream the falsifier's interval would use;
  - whether it is a decision cell and whether it is twinned;
  - its divergences.
- **Format.** The canonical writer renders floats with `float_text` (CON-27(c)), never with serde_json's formatter.

### The command
- `acn hyp verdict --hypothesis <file> <bundle>… [--runs-dir runs]` writes `runs/verdicts/<verdict_id>/verdict.json`.
- It prints `ok`, `verdict_id`, `verdict_path`, `run_ids` and `verdict`, the object of the file (CON-8). That object is reparsed for printing, so only the file is canonical.
- A refusal, an unreadable bundle, or an existing verdict directory gives `ok: false`.

## Consequences
- **POC 4 in sim.** The mock is deterministic, so on sim bundles the control replicates of a cell agree exactly. `noise_floor` is then zero, which is undefined (HYP-13), and POC 4's predicate is `inconclusive` in sim, whatever the knobs do. The acceptance suite therefore uses a fixed threshold. Before POC 4 relies on L1 sim runs, SPEC 100 should decide one of:
  - make the mock's timing and caching vary per replicate;
  - judge POC 4 on live bundles only.
- **Results in place.** Every requirement of SPEC 080 but HYP-4, HYP-25 and HYP-26 is now implemented. Those three are T05.3's.
