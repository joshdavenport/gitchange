# Benchmark harness

`cargo xbench` (release-mode alias for `cargo xtask bench`) times core's
real `Repo::refresh()` — the whole RefreshJob minus the watcher: status +
both diffs + matcher + persist — over synthetic repos generated at
graduated scales, and reports **scaling shape per dimension**, not just
wall-clock. A v0.1 exit criterion (issue #29, resolved in issue #16): all
four ADR 0005 mitigations (lazy per-file diff detail, capped diff
context, skipping huge files, incremental matching) stay
measurement-gated behind this harness's numbers, and the gate ticket
(#36) commits a run's results as the v0.1 exit record.

## Commands

```sh
cargo xbench                      # full matrix → markdown report on stdout
cargo xbench > report.md          # progress goes to stderr, so this records a run
cargo xbench --out docs/perf      # also write report.md + results.csv
cargo xbench --quick              # truncated scales: validates the harness, not a measurement
cargo xbench --dimension records  # one dimension: files | hunks | records | huge-file
cargo xbench --iterations 9       # timed refreshes per case (default 5, or 3 under --quick)
```

`cargo xtask bench` (debug) refuses to run without `--allow-debug`:
debug-build numbers would mislead.

## Dimensions

Varied one at a time, so each table reads as that dimension's shape:

| Dimension | Varies | Held fixed |
| --- | --- | --- |
| `files` | changed files 10 → 1000 | 4 hunks/file, all assigned across 4 changelists |
| `hunks` | hunks per file 2 → 128 | 25 changed files, all assigned |
| `records` | dormant records 0 → 16000 | 50-file × 4-hunk diff, 200 live records |
| `huge-file` | single file 32k → 512k lines, fully rewritten | no changelists — probes raw diff cost |

Contrast cases (excluded from the shape fit, reported as ratios) split
the costs #36 needs to attribute: `*-unassigned` twins run the same diff
with no changelists at all — the matcher does no record work — so
`assigned − unassigned` ≈ matcher cost, and the twin itself ≈ diff cost.
`files-250-staged` stages half its files, exercising the index-diff side
of the hunk universe. `files-250-touched` re-edits one file before every
timed refresh, so each iteration is a genuinely-changed refresh —
re-match plus state persist inside the timer — where the graduated rows
measure the steady state (unchanged tree, no-rewrite path).
`huge-128k-staged` stages one huge rewrite and lays a second over it,
putting the huge content through **both** diffs at once.

The huge-file cases also report **peak RSS** (before first refresh →
after all refreshes): both diffs load in full and nothing bounds that
today (the ADR 0005 caveat this dimension exists to probe). Each case
runs in its own subprocess so the high-water mark is per-case.

## Mechanics

- Repos are built in temp dirs by the sandbox `Sandbox` builder (pinned
  git config, deterministic content) and discarded after the run.
- Membership records are seeded through core's real ops, never
  hand-assembled (same rule as the sandboxes): edits are auto-captured
  by refreshing with the target changelist active; dormant records come
  from capture-then-revert, the stash/revert reality dormancy exists for.
- Each case verifies its built shape (file/hunk/record counts) before
  timing and fails loudly on mismatch — a benchmark of the wrong shape
  must not report plausible numbers.
- Graduated rows measure the steady state: records are unchanged, so
  the no-rewrite rule (ADR 0005) means no state-file write is included.
  State-file *load* and parse are included — they're part of every real
  refresh. The save-triggered event (edit → refresh with re-match and
  persist) is measured by the `files-250-touched` contrast.

## Interpreting a run

The headline per dimension is the log-log exponent (`t ~ scale^k`):
absolute numbers on the dev machine are optimistic (issue #16), but
shape is machine-independent enough to drive mitigation decisions.
Un-gating any ADR 0005 mitigation should cite a run: which dimension
misbehaves, and whether diffing or matching dominates (the contrasts
answer that).
