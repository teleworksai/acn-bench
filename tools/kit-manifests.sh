#!/usr/bin/env bash
# SPEC 140 P16-10, SPEC 095 GATE-25: make the kit's reference manifests on this
# build, into kit/manifests/<run_id>.json.
#
#   tools/kit-manifests.sh <loop report.json> [<runs dir>]
#
# The manifests are those of the sim bundles the loop report names, each made
# again on this build from the loop's own bundle under <runs dir> (default
# runs/), and of CI's reference run (tools/p16-target.sh). A bundle made again
# must agree with the loop's in build-neutral form, or this fails.
#
# Run it from the workspace root, on a clean checkout of the commit to be
# tagged: the build is `cargo build -p acn-cli --locked`, dev profile, with
# nothing untracked or changed under what build_hash covers and no RUSTFLAGS.
# The manifests then go in a commit that touches kit/ only, which keeps the
# build_hash (CON-31), and that commit is the one to tag. KIT_JOBS sets how
# many bundles are made at once (default 4).
set -euo pipefail
report=${1:?usage: tools/kit-manifests.sh <loop report.json> [<runs dir>]}
runs=${2:-runs}
jobs=${KIT_JOBS:-4}

if [ -n "${RUSTFLAGS:-}" ]; then
  echo "RUSTFLAGS is set; a reference manifest is made with none (P16-10)" >&2
  exit 1
fi
dirty=$(git status --porcelain -- crates Cargo.toml Cargo.lock rust-toolchain.toml .cargo)
if [ -n "$dirty" ]; then
  echo "not a clean checkout of what build_hash covers (P16-10):" >&2
  echo "$dirty" >&2
  exit 1
fi

cargo build -p acn-cli --locked
acn=$PWD/target/debug/acn
out=target/kit
rm -rf "$out"
mkdir -p "$out/json" kit/manifests

# The run_ids the report names, each once.
grep -o '"run_id":"[0-9a-f]\{64\}"' "$report" | sed 's/.*:"\(.*\)"/\1/' | sort -u > "$out/ids"
test -s "$out/ids"

one() {
  id=$1
  json=$out/json/$id.json
  "$acn" run --from-run-id "$id" --runs-dir "$runs" --across-builds > "$json" 2> "$out/json/$id.err" || {
    cat "$json" "$out/json/$id.err" >&2
    return 1
  }
  grep -q '"ok":true' "$json" && grep -q '"differ":\[\]' "$json" || {
    echo "$id: made again, it differs from the loop's bundle" >&2
    cat "$json" >&2
    return 1
  }
  dir=$(sed -n 's/.*"dir":"\([^"]*\)".*/\1/p' "$json")
  cp "$dir/manifest.json" "kit/manifests/$id.json"
}
export -f one
export acn out runs
xargs -P "$jobs" -n 1 bash -c 'one "$0"' < "$out/ids"

# CI's reference run (P16-12), on the same build.
tools/p16-target.sh > /dev/null
ref=$(sed -n 's/.*"run_id":"\([0-9a-f]\{64\}\)".*/\1/p' target/p16/run.json)
test -n "$ref"
cp "target/p16/runs/$ref/manifest.json" "kit/manifests/$ref.json"

made=$(ls kit/manifests | wc -l | tr -d ' ')
want=$(( $(wc -l < "$out/ids" | tr -d ' ') + 1 ))
"$acn" version
echo "kit/manifests: $made manifests, $want expected (the loop's and the reference run $ref)"
test "$made" -eq "$want"
