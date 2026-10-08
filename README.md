# acn-bench

[![ci](https://github.com/teleworksai/acn-bench/actions/workflows/ci.yml/badge.svg)](https://github.com/teleworksai/acn-bench/actions/workflows/ci.yml)

A Rust workspace that builds the experimental substrate for **Agentic Communication Networks (ACN)** and runs its proof-of-concept catalogue as frozen, falsifiable hypotheses with mandatory controls. It implements the programme in Appendix E of the ACN technical report ([what the repo reads from it](docs/report/README.md)).

The question it exists to answer: *when the endpoint is an agent loop rather than a person, where does the network actually matter, and by how much?* Every experiment is stated so that "it does not" is a publishable result.

**Status:** milestone M1 in progress: the substrate runs on the mock, in `sim` and `live`, with attribution and regeneration from a `run_id`. Nothing here is citable yet: mock results never are (CON-26), and citable numbers wait for live providers. See [`TASKS.md`](TASKS.md).

## How it is organised

| Track | Where | Rules |
|---|---|---|
| Exploration | [`lab/`](lab/README.md) | Any Rust, any dependency. `fmt`, `clippy`, and a lab note. Nothing here is citable. |
| Substrate | `crates/`, `specs/`, `hypotheses/` | Specs with requirement IDs, tests that cite them, deterministic runs, a control for every experiment, and, after the M0 gate, a frozen set that changes only through a human-merged `env-change` PR (CON-7). |

An idea graduates from lab to substrate when someone wants to cite a number from it.

```
specs/        numbered specs; every requirement has an ID (CON-5, TRC-24, …)
hypotheses/   one TOML per POC: hypothesis, variables, control, falsifier   [frozen after M0]
scenarios/    synthetic link scenarios; measured impairment traces          [measured: frozen after M0]
crates/       acn-trace · acn-emu · acn-mockllm · acn-harness · acn-gen · acn-replay
              acn-ctl · acn-hyp · acn-attrib · acn-cli · xtask
tests/accept/ one acceptance suite per POC
lab/          spikes and candidate hypotheses
docs/         decisions (ADRs), gates, generated inventory, lab notes, report pointer
```

Start with [`PLAN.md`](PLAN.md) for the design and [`specs/000-constitution.md`](specs/000-constitution.md) for the invariants. To contribute, read [`CONTRIBUTING.md`](CONTRIBUTING.md). [`GETTING-STARTED.md`](GETTING-STARTED.md) is the owner's record of how the repository was created and configured; you do not need it to build.

## Build and check

```bash
git clone https://github.com/teleworksai/acn-bench && cd acn-bench
rustup toolchain install                              # the toolchain pinned in rust-toolchain.toml
cargo install cargo-deny --locked --version 0.20.2    # the last gate needs it; same version as CI
tools/ci.sh   # fmt, clippy -D warnings, tests, trace-check, docs-inventory, env-hash, cargo-deny
```

`cargo xtask trace-check` fails when a requirement listed in `trace-scope.toml` has no citing test, when a test cites an ID that does not exist, or when a document under `docs/`, `specs/`, `.github/` or the root names an ID its spec does not define (`docs/lab/` and `docs/generated/` are exempt; ADR-3). The state of every requirement, including the many that nothing checks yet, is in [`docs/generated/requirements.md`](docs/generated/requirements.md).

## Quickstart: a bundle, regenerated from its run_id

From the workspace root, with `target/debug` on your `PATH`, these lines build `acn`, run one `sim` bundle on the mock and regenerate it from its `run_id` alone, byte for byte (SPEC 140). Write the `run_id` the second line prints in place of `$RUN_ID`. A test runs exactly these lines (P16-30).

```sh quickstart
cargo build -p acn-cli --locked
acn harness run --workload workloads/harness-smoke.toml --backend mockllm --model mock-auto --seed 1 --runs-dir target/quickstart/runs
acn run --from-run-id $RUN_ID --runs-dir target/quickstart/runs
```

The last line prints `"identical":true`: the regenerated bundle is the same bytes as the first. On another machine of the same target, at the same tag, the same holds (P16-11). A run is never written twice (CON-29), so remove `target/quickstart/` before running the lines again.

## Rules worth knowing before you read the code

These are requirements of the constitution, not descriptions of code that exists. Most of the code they bind arrives with later tasks.

- **Determinism is a feature (CON-5).** Clocks and randomness must be injected; in `sim` mode the same inputs must produce a byte-identical bundle.
- **Sim has a live twin (CON-25).** A simulated number may be cited only alongside a live run of the same scenario and the recorded divergence.
- **Mock is not a result (CON-26).** Bundles produced on the mock inference server gate the suite; they are never cited.
- **Negative results are reported, not re-hypothesised (CON-17).** Hypothesis files are frozen before the run that tests them.

What is machine-checked today, and what still depends on repository settings and on the maintainer, is listed in [ADR-6](docs/decisions/ADR-6.md).

## Contributing and licence

See [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`SECURITY.md`](SECURITY.md). Agents follow [`CLAUDE.md`](CLAUDE.md). The code is licensed under [Apache-2.0](LICENSE). The technical report is not part of this repository and is not covered by that licence.
