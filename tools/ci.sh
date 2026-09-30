#!/usr/bin/env bash
# CON-9: the gate chain. Exit non-zero on the first failure.
set -euo pipefail
cargo fmt --all --check \
&& cargo clippy --workspace --all-targets --all-features -- -D warnings \
&& cargo test --workspace \
&& cargo xtask trace-check \
&& cargo xtask docs-inventory --check \
&& cargo xtask env-hash --check \
&& cargo deny check
