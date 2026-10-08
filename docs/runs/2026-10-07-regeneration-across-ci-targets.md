# POC 16, cross-target regeneration in CI (2026-10-07)

**Evidence for SPEC 140's first open question, not a claim (CON-31).** CI's first `p16-target` and `p16-compare` jobs (P16-12) regenerated the reference `sim` run on three targets, and the three agree in build-neutral form.

## Identity

| Item | Value |
|---|---|
| Commit | `080154d` (PR 67, T16.3) |
| CI run | `37722939344` (`ci`, pull request 67) |
| Reference run | `tools/p16-target.sh`: `workloads/harness-smoke.toml` on `mock-auto`, scenario `scenarios/synthetic/cellular-handover.toml`, seed 1 |
| run_id | `d625e220a8abf9dd160d1a67c271665d3cf7e4766265e511ae5e08568c2c7047` |
| Targets | `aarch64-apple-darwin` (macos-latest), `aarch64-unknown-linux-gnu` (ubuntu-24.04-arm), `x86_64-unknown-linux-gnu` (ubuntu-latest) |

## Result

- On each target, the bundle regenerated from its `run_id` on the same build was identical, byte for byte (P16-6).
- Across the three targets, the bundles were identical in build-neutral form (`cargo xtask p16-compare --expect 3`: `identical: true`, three records, no differences). The comparison covered:
  - every table and view, by hash;
  - the resources without `acn.build_hash`;
  - the manifest without `build`.

So on this run, nothing in a `sim` bundle but the record of its build depends on the target. CON-31 still claims identity only within one build. A later spec-change may claim more once releases keep showing the same (SPEC 140 §7, question 1).

The comparison is the `p16-comparison` artifact of the CI run above, and each target's record is its `p16-record-<os>` artifact.
