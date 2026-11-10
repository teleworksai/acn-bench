#!/usr/bin/env bash
# SPEC 140 P16-11, SPEC 095 GATE-24: on this machine, regenerate every reference
# manifest of the kit that is of this machine's target, require each to be
# identical on the same build, and write the record.
#
#   tools/kit-regen.sh <tag> <yyyy-mm-dd> [<record.md>]
#
# Run it from the workspace root of a clean checkout of <tag>, built as P16-10
# says: `cargo build -p acn-cli --locked`, dev profile, no RUSTFLAGS. The record
# goes to docs/runs/<yyyy-mm-dd>-p16-<target>.md unless a path is given. It
# fails, and writes no record, unless every regeneration is identical.
# KIT_JOBS sets how many run at once (default 4).
set -euo pipefail
tag=${1:?usage: tools/kit-regen.sh <tag> <yyyy-mm-dd> [<record.md>]}
day=${2:?usage: tools/kit-regen.sh <tag> <yyyy-mm-dd> [<record.md>]}
jobs=${KIT_JOBS:-4}

if [ -n "${RUSTFLAGS:-}" ]; then
  echo "RUSTFLAGS is set; the kit is built with none (P16-10)" >&2
  exit 1
fi
head=$(git rev-parse HEAD)
at=$(git rev-parse "$tag^{commit}")
if [ "$head" != "$at" ]; then
  echo "HEAD $head is not the tag $tag ($at)" >&2
  exit 1
fi
dirty=$(git status --porcelain -- crates Cargo.toml Cargo.lock rust-toolchain.toml .cargo kit)
if [ -n "$dirty" ]; then
  echo "not a clean checkout of $tag:" >&2
  echo "$dirty" >&2
  exit 1
fi

cargo build -p acn-cli --locked
acn=$PWD/target/debug/acn
target=$(rustc -vV | sed -n 's/^host: //p')
record=${3:-docs/runs/$day-p16-$target.md}
out=target/kit-regen
rm -rf "$out"
mkdir -p "$out/json" "$out/runs"

# The reference manifests of this target.
grep -l "\"target\":\"$target\"" kit/manifests/*.json | sed 's#.*/\(.*\)\.json#\1#' | sort > "$out/ids"
if [ ! -s "$out/ids" ]; then
  echo "kit/manifests holds no reference manifest of $target" >&2
  exit 1
fi

one() {
  id=$1
  json=$out/json/$id.json
  "$acn" run --from-run-id "$id" --runs-dir "$out/runs" > "$json" 2> "$out/json/$id.err" || {
    cat "$json" "$out/json/$id.err" >&2
    return 1
  }
  grep -q '"identical":true' "$json" && grep -q '"same_build":true' "$json" || {
    echo "$id: not identical on the same build" >&2
    cat "$json" >&2
    return 1
  }
}
export -f one
export acn out
xargs -P "$jobs" -n 1 bash -c 'one "$0"' < "$out/ids"

n=$(wc -l < "$out/ids" | tr -d ' ')
first=$(head -n 1 "$out/ids")
field() { sed -n "s/.*\"build\":{[^}]*\"$1\":\"\([^\"]*\)\".*/\1/p" "kit/manifests/$first.json"; }
{
  echo "# POC 16, the kit regenerated on a second machine: $target ($day)"
  echo
  echo "**A regeneration record (SPEC 140 P16-11), read by gate M1 (SPEC 095 GATE-24).** Written by \`tools/kit-regen.sh\`, which fails and writes nothing unless every regeneration is identical."
  echo
  echo "## Identity"
  echo
  echo "| Item | Value |"
  echo "|---|---|"
  echo "| Tag | \`$tag\` |"
  echo "| Commit | \`$head\` |"
  echo "| Target | \`$target\` |"
  echo "| build_hash | \`$(field build_hash)\` |"
  echo "| profile | \`$(field profile)\` |"
  echo "| features | \`$(field features)\` |"
  echo "| rustflags | \`$(field rustflags)\` |"
  echo "| source_hash | \`$(field source_hash)\` |"
  echo "| cargo_lock | \`$(field cargo_lock)\` |"
  echo "| rust_toolchain | \`$(field rust_toolchain)\` |"
  echo "| cargo_config | \`$(field cargo_config)\` |"
  echo
  echo "The build components are those of the reference manifests, which each regeneration reproduced on this machine's own build: \`acn run --from-run-id\` reports \`same_build\` only when this binary's \`build_hash\` is the manifest's (P16-5)."
  echo
  echo "## Result"
  echo
  echo "$n of $n reference manifests of \`$target\` regenerated \`identical\`, byte for byte, from the manifest alone (P16-6)."
  echo
  echo '```'
  sed 's/$/ identical/' "$out/ids"
  echo '```'
} > "$record"
"$acn" version
echo "$record: $n of $n identical"
