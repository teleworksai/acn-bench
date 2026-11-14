# ADR-57 — A committed digest list for the evidence files under `docs/runs/`

**Status:** accepted (issue #84, the checks ADR-47 left open; Class A, `crates/xtask`). **IDs affected:** P16-20, P16-21, LOOP-12, GATE-2, GATE-23.

## Context
P16-20 requires a cited loop's report, twins and verdicts to be committed byte for byte. `cargo xtask docs-inventory` checks what those files say against what they name (ADR-46, ADR-47), and two changes still passed it once the page was regenerated (issue #84):

- **A twin removed together with its L2 verdict.** A report does not name its twins (LOOP-12 adds them as separate files), so the tree without them is a consistent tree, and GATE-23's evidence is gone without a failing check.
- **A verdict field that no id covers, edited.** The `verdict_id` is recomputed from some of a verdict's fields; the others can change and the page does not show it.

`acn evidence verify` walks `runs/loop/` on a machine that holds the bundles (LOOP-2); it does not read `docs/runs/`, and CI holds no bundles. No command compares a committed copy with its original under `runs/`.

No reading of the files' contents closes the first case: nothing left in the tree says the twin was ever there. Something outside the files has to say which files there are.

## Decision
- **A list is committed at `docs/evidence-digests.txt`.** It has one line, `<blake3>  <path>`, for every regular file under `docs/runs/loop/` and `docs/runs/verdicts/`, sorted by path. The digest is the BLAKE3 of the file's bytes in lower-case hex; the path is relative to the repository.
- **`cargo xtask evidence-digests --check` fails unless the tree and the list agree exactly.** A file that is unlisted, a listed file that is absent, and a file whose bytes changed each fail and are named. The list must be the text `--write` would write; another order, a repeated line or a missing final newline fails. Evidence files with no list fail. A tree with no evidence files needs no list.
- **`cargo xtask docs-inventory --check` runs the same check.** It is already a step of the gate chain (CON-9), so no step is added to `tools/ci.sh`.
- **`cargo xtask docs-inventory` in write mode neither checks nor rewrites the list.** Regenerating pages is routine and must not be a way to accept a changed file. Only `evidence-digests --write` changes the list.
- **Only the files P16-20 names are accepted.** Those are `loop/<id>/report.json`, `loop/<id>/twin/<id>/twin.json` and `verdicts/<id>/verdict.json`. Any other file under the two directories is refused in both modes, with the advice to remove it. So `--write` cannot list a stray file such as `.DS_Store`, which git may ignore and CI would then report as absent.
- **The walk fails closed.** An entry that is neither a directory nor a regular file is an error, and so is a name with a control character. A symbolic link is refused and never followed: among the files, as the list itself, and as any directory between the repository root and either of them.
- **The list is not under `docs/runs/`.** The loop's plan put it at `docs/runs/digests.txt`. The gate records' check reads every file directly under `docs/runs/` as a run record and requires each 64-hex id of a gate record to be a word of one (GATE-2). A list there would contain every loop id, verdict id and digest, and would satisfy that requirement for any of them.
- **The list is byte-compared on every checkout.** `.gitattributes` marks it and the test fixture's copies `-text`, as `docs/runs/**/*.json` already is (CON-27).

## Consequences
- **The list does not stop a change; it puts it in the diff.** Anyone can run `--write`. What the check guarantees is that removing, adding or editing an evidence file cannot be done without a changed line in `docs/evidence-digests.txt` in the same PR, where a reviewer sees which file moved. That is the same guarantee `env-hash.json` gives the frozen set (CON-7), without the `env-change` label: the list is not in the frozen set and is not a CODEOWNERS path.
- **Committing a new loop is two commands.** `cargo xtask evidence-digests --write`, then `cargo xtask docs-inventory`.
- **The list and the files can be changed together by one author.** The check does not say the committed copies equal their originals under `runs/`. Nothing does today: P16-20's "identical to its copy under `runs/`" is held by the procedure that copies them, and `acn evidence verify` reads only `runs/`. With the list, a command that compares each listed digest with the file under `runs/` is small; it is not written here.
- **An empty tree passes.** `--write` with no evidence files and an existing list leaves the list empty, and `--check` accepts that, as it accepts no files and no list. Deleting every evidence file together with the list is therefore not caught by this check. On this repository it is caught elsewhere: a test requires the real tree to list at least the four files of the loop of record, and the GATE-23 test reads the twin.
- **Run records are not covered.** The `.md` files directly under `docs/runs/` are prose, read by the gate records' check, and are not in the list.
- **Issue #84 is closed by this.** Its checks 1 and 2 landed with ADR-47; reading the twin JSON in the GATE-23 test landed with the M1 record.
