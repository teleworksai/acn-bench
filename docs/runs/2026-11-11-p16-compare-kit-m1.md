# POC 16, cross-target regeneration in CI at the kit's tag (2026-11-11)

**A run record for gate M1 (SPEC 095 GATE-25; SPEC 140 P16-12).** Evidence for SPEC 140's first open question, not a claim: CON-31 claims byte identity only within one build.

## Identity

| Item | Value |
|---|---|
| Tag | `kit-m1` |
| Commit | `de5e058404e27ee864f8ca9b0f8f29a4b63c60fd` |
| CI run | `43046880286` (`ci`, dispatched by hand on the tag) |
| Reference run | `tools/p16-target.sh`: `workloads/harness-smoke.toml` on `mock-auto`, scenario `scenarios/synthetic/cellular-handover.toml`, seed 1 |
| run_id | `a6dee005fd53b61a643f8a1a3465f8331bdc0607b5144ad0fd54d44a01a422f6` |
| Targets | `aarch64-apple-darwin` (macos-latest), `aarch64-unknown-linux-gnu` (ubuntu-24.04-arm), `x86_64-unknown-linux-gnu` (ubuntu-latest) |

## Result

- On each target, the `p16-target` job regenerated the reference bundle from its `run_id` on the same build, identical byte for byte (P16-6).
- Across the three targets, `p16-compare` found the bundles identical in build-neutral form (`cargo xtask p16-compare --expect 3`: `identical: true`, three records, no differences): every table and view by hash, the resources without `acn.build_hash`, and the manifest without `build`.

The reference run's manifest on `aarch64-apple-darwin` is one of the kit's reference manifests, `kit/manifests/a6dee005fd53b61a643f8a1a3465f8331bdc0607b5144ad0fd54d44a01a422f6.json`.

The comparison is the `p16-comparison` artifact of the CI run above, and each target's record is its `p16-record-<os>` artifact.
