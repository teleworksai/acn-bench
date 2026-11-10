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
# Ignored files count too: source_hash reads crates/ without asking git (CON-31).
dirty=$(git status --porcelain --ignored -- crates Cargo.toml Cargo.lock rust-toolchain.toml .cargo | grep -v '^!! crates/[^/]*/target/$' || true)
if [ -n "$dirty" ]; then
  echo "not a clean checkout of what build_hash covers (P16-10):" >&2
  echo "$dirty" >&2
  exit 1
fi

cargo build -p acn-cli --locked
acn=$PWD/target/debug/acn
# The build this binary records is the one P16-10 describes: dev profile, and no
# flags from any source (RUSTFLAGS, a Cargo configuration, a target table).
"$acn" version > target/kit-version.json
grep -q '"profile":"debug"' target/kit-version.json && grep -q '"rustflags":""' target/kit-version.json || {
  echo "this build is not the dev profile with no rustflags (P16-10):" >&2
  cat target/kit-version.json >&2
  exit 1
}
out=target/kit
rm -rf "$out"
# Staged: kit/manifests/ is replaced only when every manifest is made.
mkdir -p "$out/json" "$out/manifests"

# The run_ids the report names, each once.
grep -o '"run_id":"[0-9a-f]\{64\}"' "$report" | sed 's/.*:"\(.*\)"/\1/' | sort -u > "$out/ids"
test -s "$out/ids"

one() {
  id=$1
  json=$out/json/$id.json
  # The loop's own bundle, never a reference manifest left from an earlier run
  # (P16-2 falls back to kit/manifests/ when the bundle is missing).
  if [ ! -f "$runs/$id/manifest.json" ]; then
    echo "$runs/$id is not a bundle; the loop's bundles are needed" >&2
    return 1
  fi
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
  cp "$dir/manifest.json" "$out/manifests/$id.json"
}
export -f one
export acn out runs
xargs -P "$jobs" -n 1 bash -c 'one "$0"' < "$out/ids"

# CI's reference run (P16-12), on the same build.
tools/p16-target.sh > /dev/null
ref=$(sed -n 's/.*"run_id":"\([0-9a-f]\{64\}\)".*/\1/p' target/p16/run.json)
test -n "$ref"
cp "target/p16/runs/$ref/manifest.json" "$out/manifests/$ref.json"

made=$(ls "$out/manifests" | wc -l | tr -d ' ')
want=$(( $(wc -l < "$out/ids" | tr -d ' ') + 1 ))
if [ "$made" -ne "$want" ]; then
  echo "$made manifests made, $want expected; kit/manifests/ is left as it was" >&2
  exit 1
fi
rm -rf kit/manifests
mkdir -p kit
mv "$out/manifests" kit/manifests
cat target/kit-version.json
echo "kit/manifests: $made manifests (the loop's $(( want - 1 )) and the reference run $ref)"
