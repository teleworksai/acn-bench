# ADR-49 — The evidence page shows each layer, and marks a candidate's loops

**Status:** accepted (issue #83; Class A, `crates/xtask/src/evidence.rs`). **IDs affected:** P16-21, LOOP-30, LOOP-12, HYP-3, HYP-22, HYP-24, GATE-23.

## Context
The review of PR 82 found four things about `docs/evidence/p4.md` (issue #83), and ADR-46 recorded two of them as consequences to fix later:
1. The provider table was L1's, with no layer named. On the loop of record it showed two providers passing, right after the L2 divergence table, while all four are inconclusive at L2.
2. Slice-level reasons were not shown, so `twin_failed`, which decides the L2 statuses, was nowhere on the page.
3. A candidate and the frozen hypothesis of the same id share one page, titled as the frozen hypothesis's citation target.
4. `within` sat inside a JSON blob in each divergence row, and about 6 150 control-effect rows came before the sections a gate reads.

The issue offered two ways to handle the third: key pages by hypothesis path or hash, or give candidate loops their own marking on the shared page.

## Decision
- **One provider table per layer** (HYP-24), each under its layer and its verdict's id, in the order L1, then each twin's L2.
- **A "Slices" table lists every slice of each layer's verdict** with the slice's own verdict and reasons. Each reason is followed by how many things it refers to, which for `twin_failed` is the number of cells (HYP-22).
- **Pages stay keyed by hypothesis id.** P16-21 names the page `docs/evidence/<hypothesis-id>.md`, and a page per path or hash would be a spec change. On the shared page:
  - a loop has the file in its heading, as "(candidate `<path>`)", unless its report says `frozen` and the file is under `hypotheses/` (HYP-3). A file there that its report calls a candidate, one not yet in the freeze record, is marked too. ADR-47's check of a candidate's bytes still goes by location alone, since `env-hash` covers that directory;
  - a path is printed in a code span as written, and a report whose path has any character but letters, digits, `/`, `.`, `-` and `_` is refused;
  - every loop has a "File" row;
  - the page says, when it holds such a loop, that it is not a loop of the frozen hypothesis;
  - the frozen hypothesis's loops come first.
- **The divergence table has a column per figure**, each written as committed, and a "Within" column. A line above it gives how many twinned quantities are within their tolerance and names GATE-23.
- **The control effects come last** in a loop's section, after the chain. They are not in P16-21's list of what the page shows; they stay because CON-18 asks for the control to be visible.

## Consequences
- **ADR-46's two renderer consequences are met.** Its text describes the page as it was on the day it was written and is left as it is.
- **The page is no shorter.** The control effects are still one row per cell and quantity. Collapsing them is left open.
- **GATE-23's row in the M1 record can cite the summary line and the Slices table** beside ADR-46, so the divergence figures and the `twin_failed` reasons are both on the page it cites. The test of GATE-23's substance still reads the twin JSON, not the page (issue #84).
- **A page per candidate remains possible** as a change to P16-21, if a second candidate under one id makes the shared page hard to read.
