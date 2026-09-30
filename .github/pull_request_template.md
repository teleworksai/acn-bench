*Title: conventional commit, `type(scope): subject`. It becomes the squash-merge subject (CON-11). Delete this line.*

## What and why

## Requirement IDs (CON-11)

| ID | What this PR does for it | Cited by (test) |
|---|---|---|
|  |  |  |

IDs added to `trace-scope.toml`:

## Risk class (CON-10)

A PR that touches more than one class is handled at the highest. One spec concern per PR (CON-11).

- [ ] **A** — docs, tests, tools, xtask, scenarios/synthetic
- [ ] **B** — run-path crate: affected acceptance suites run (`cargo test -p acn-accept --test <poc>`)
- [ ] **C** — frozen set: updated `env-hash.json`; after the M0 gate also label `env-change`, adversarial review done and linked, **human-merged** (CON-7)
- [ ] touches `specs/` or removes an entry from `trace-scope.toml`: label `spec-change`, rationale and IDs added / changed / retired stated above (CON-14)
- [ ] touches an enforcement point (workflows, `clippy.toml`, `deny.toml`, `.cargo/`, root manifest or lockfile, `tools/ci.sh`, `crates/xtask/`, CODEOWNERS): said so above, and the copy in `crates/xtask/tests/workspace.rs` updated

## Interpretations (CON-15)

ADRs written:

## Gates

- [ ] `docs/generated/` regenerated (`cargo xtask docs-inventory`)
- [ ] `tools/ci.sh` green locally (documentation-only changes may run `fmt`, `docs-inventory --check` and `trace-check` only, CON-9)
- [ ] For POC tasks (from T05 on): bundle `run_id` and `acn hyp verdict` output attached

## Review (CON-16)

Tick exactly one. The mode is read from `.github/CODEOWNERS` on the base branch.

- [ ] Solo-maintainer mode, merged without independent review (for a Class C or a risky Class B change, say why above)
- [ ] Solo-maintainer mode, reviewed by a separate session (adversarial prompt for Class C, cross-review otherwise): link
- [ ] CODEOWNERS names a second owner: approved by someone other than the author

Findings go in PR comments; a reviewer does not push. After the M0 gate a Class C change needs the adversarial review of CON-7 in either mode.
