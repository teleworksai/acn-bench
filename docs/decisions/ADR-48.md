# ADR-48 — A bundle is built in a staging directory and moved into place

**Status:** accepted (issue #20; Class B, `crates/acn-trace/src/bundle.rs`). **IDs affected:** CON-29, TRC-22, TRC-23, HAR-23, CTL-13.

## Context
`Bundle::create` made `runs/<run_id>/` at the start of a run and wrote into it. The harness removes that directory when a run returns an error (HAR-23), but a process that is killed, or a machine that loses power, returns nothing. The directory stayed, with no `manifest.json`, and from then on:
- the harness refused that `run_id` ("exists; a bundle is never replaced", CON-29);
- `acn loop run` stopped every loop that needed that cell with `bundle_incomplete`.

The review of PR 19 reproduced it. It costs most where a run is long or paid for: a live twin, and later a real provider.

Issue #20 proposed a staging directory and left three points open: where `logs/` lives during a run, what `verify` does with a staging directory, and who removes old ones.

## Decision
- **A bundle is built in `runs/.<run_id>.partial.<k>/`**, beside its final place, where `k` is the first index this writer can create. `finish` writes the tables, then the manifest, last (TRC-23), and then renames the directory to `runs/<run_id>/` (TRC-22). Nothing is at `runs/<run_id>/` before that, so a writer that dies claims no `run_id`.
- **Anything already at `runs/<run_id>/` refuses the run**, at the start and again at `finish`: a directory, an empty one included, a file or a link (CON-29). The check at `finish` is made again when the system refuses the move, so a bundle that lands in between is reported as a refusal, whatever error code the platform gives.
- **A writer that gives up removes its own staging directory**: on an error, on a panic that unwinds, and when the run's future is dropped. Only a writer that is killed leaves one.
- **`logs/` is in the staging directory** from the start, as before, and moves with it.
- **`bundle::verify` is unchanged and keeps refusing a staging directory.** An unfinished one has no manifest. One that is complete but for the rename is refused because its name is not its `run_id` (TRC-22).
- **Nothing removes another writer's staging directory.** It may belong to a writer that is still running, and this code has no clock to judge its age by (CON-5). Those a killed writer left are the user's to remove. One `run_id` may have 1024 before a writer refuses and says so.

## Consequences
- **A bundle's bytes do not change.** The same run gives the same `bundle_digest` as before, and after a dead writer as without one. The path a run writes to during the run is the only difference, and no file in a bundle records it.
- **`Bundle::dir()` is the staging directory until `finish`.** `Written::dir` is `runs/<run_id>/`. The harness's cleanup of a failed run (HAR-23) still names that directory; the writer has already removed it by then.
- **Two writers of one `run_id` no longer collide at the start.** The first to finish has the bundle; the second is refused at `finish`. Before, the second was refused before it ran.
- **`bundle_incomplete` can now only come from a directory made by an older build**, or by hand. The check in `crates/acn-hyp` and its message stay as they are.
- **A killed run leaves a hidden directory behind.** Tools that list `runs/` see names starting with a dot. The twin's check of its live directory (LOOP-12) names any entry that is not one of its bundles; a twin starts in a fresh live directory, so a staging directory from an earlier, dead twin is not in it.
- **The control plane's own cleanups** (`crates/acn-ctl`) look at `runs/<run_id>/` only. They still remove a directory there that does not verify, which now means one from an older build. A run that panics under the control plane unwinds, so its staging directory goes with it; a retry does not add another.
- **Durability is as before.** Every file is synced; no directory is. After a power loss a finished bundle may be back under its staging name. That is safe for CON-29, and the run has to be made again.
