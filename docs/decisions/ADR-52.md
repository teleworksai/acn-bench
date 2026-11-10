# ADR-52 — How the kit's reference manifests and its second-machine record are made

**Status:** accepted (T17.3, preparation; Class A, `tools/` and `.github/workflows/ci.yml`). **IDs affected:** P16-10, P16-11, P16-12, GATE-24, GATE-25; by reference P16-5, P16-6, CON-31, GATE-23.

## Context
Gate M1 needs a kit (GATE-25) and a second machine that regenerates it (GATE-24). SPEC 140 says what a reference manifest is and what the record names. It does not say how to make about 1 550 of them, or what counts as a second machine. T17.3 has to settle both before a tag exists.

Two facts shape the answer.
- **The loop of record was made on another build.** GATE-23's loop `90f7fda2…` ran on build `60d0899c…`, in the release profile. P16-10 asks for reference manifests made on one build from a clean checkout of the tag, in the dev profile. So the loop's `sim` bundles have to be made again on the tag's build.
- **A manifest commit keeps the `build_hash`.** `build_hash` covers `crates/`, the root manifests, the toolchain file and the Cargo configuration (CON-31), not `kit/`. Manifests made at one commit can therefore be committed in the next and still be that build's.

## Decision
- **`tools/kit-manifests.sh <loop report>` makes the reference manifests.** For every `run_id` the report names it runs `acn run --from-run-id --across-builds` against the loop's own bundle, requires the new bundle to agree with it in build-neutral form (P16-5), and copies the new bundle's manifest to `kit/manifests/<run_id>.json`. It adds the manifest of CI's reference run (`tools/p16-target.sh`). It refuses to run with `RUSTFLAGS` set, or with anything changed under what `build_hash` covers.
- **The kit is made in two commits.** The script runs on a clean checkout of the commit to be tagged; its output is committed alone, touching `kit/` only; that second commit is the tag.
- **`tools/kit-regen.sh <tag> <date>` makes the second-machine record.** It checks that the checkout is the tag, regenerates every reference manifest of the machine's target from the manifest alone, requires each to be `identical` on the same build, and writes `docs/runs/<date>-p16-<target>.md`. If any regeneration is not identical it fails and writes nothing.
- **A CI runner may be the second machine.** The job `p16-kit` runs that script on `macos-latest` when the `ci` workflow is dispatched by hand on a tag, with no build cache, and uploads the record. A runner is a different machine of the same target, built from a clean checkout. The record it makes is committed under `docs/runs/` by a PR, like any other. The maintainer may run the same script on a machine of their own instead; the gate reads the record either way.
- **No `kit/inputs/` is needed yet.** Every input of the loop's bundles and of the reference run is under a search root (`workloads/`, `scenarios/`, `lab/hypotheses/`).

## Consequences
- **The reference manifests are of one target**, `aarch64-apple-darwin`, the machine that holds the loop's bundles. A Linux runner has nothing of its target to regenerate, so `p16-kit` runs on macOS only. The cross-target comparison of P16-12 stays the reference run's.
- **Making the manifests writes scratch bundles** under `runs/regen/`, as every regeneration does (P16-6). About 1 550 of them; they are never cited.
- **Any change under `crates/` after the manifests are made means making them again.** With eight jobs that is about a quarter of an hour on the development machine.
- **The tag itself is the maintainer's.** Nothing here creates or pushes one.
- **Whether a runner satisfies "a second machine" in GATE-24 is the maintainer's reading to confirm** when signing the gate (GATE-27). The record states the tag, the commit, the target and the build either way.
