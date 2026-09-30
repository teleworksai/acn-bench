# lab/ — the exploration track (CON-23)

Anything goes here, in Rust. Spikes, throwaway simulators, wrappers around a partner's
dora/ROS stack, candidate hypotheses (`lab/hypotheses/*.toml`), half-ideas.

Gates: `cargo fmt` and `cargo clippy` on your crate, and a lab note in
`docs/lab/<yyyy-mm-dd>-<slug>.md` (question · what was tried · what was learned ·
graduate / park / drop). No specs, no IDs, no controls, no frozen anything.

Nothing here may be cited as a result. When you want to cite a number, graduate it (CON-24).

## Starting a spike

```bash
cp -R lab/_template lab/<slug>      # do NOT use `cargo new` here: it edits the root manifest
rm -rf lab/<slug>/target             # in case the template was ever built
$EDITOR lab/<slug>/Cargo.toml        # rename the package; keep the empty [workspace] table
cargo fmt    --manifest-path lab/<slug>/Cargo.toml --check
cargo clippy --manifest-path lab/<slug>/Cargo.toml --all-targets -- -D warnings
```

Each lab crate is its own Cargo workspace root (the empty `[workspace]` table), with its own
`Cargo.lock` and `target/`. `lab/clippy.toml` switches off the substrate's determinism bans, so
`Instant::now()` and friends are fine here. CI runs the two gates above on every `lab/*/Cargo.toml`.
