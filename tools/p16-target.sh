#!/usr/bin/env bash
# SPEC 140 P16-12: on this target, run the reference sim workload, regenerate it
# from its run_id on the same build (the regeneration must be identical, or this
# fails), and write the bundle's build-neutral form to target/p16/neutral.json
# for the cross-target comparison. Run from the workspace root.
set -euo pipefail
# Each step's one JSON object goes to a file; on any failure, show them all.
trap 'status=$?; if [ $status -ne 0 ]; then cat target/p16/*.json 2>/dev/null || true; fi' EXIT
cargo build -p acn-cli --locked
acn=target/debug/acn
runs=target/p16/runs
rm -rf target/p16
mkdir -p target/p16
"$acn" harness run --workload workloads/harness-smoke.toml --backend mockllm \
  --model mock-auto --seed 1 --scenario scenarios/synthetic/cellular-handover.toml \
  --runs-dir "$runs" > target/p16/run.json
run_id=$(sed -n 's/.*"run_id":"\([0-9a-f]\{64\}\)".*/\1/p' target/p16/run.json)
test -n "$run_id"
"$acn" run --from-run-id "$run_id" --runs-dir "$runs" > target/p16/regen.json
"$acn" bundle neutral "$runs/$run_id" > target/p16/neutral.json
cat target/p16/regen.json
