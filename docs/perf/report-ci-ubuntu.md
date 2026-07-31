# RefreshJob benchmark

- date: 2026-07-31
- host: Linux x86_64
- commit: 54af01c
- per case: 5 timed refreshes after 2 warmup, fresh subprocess

## files — changed files, 4 hunks each, all hunks assigned across 4 changelists

| case | scale | hunks | records | median ms | min | max | ×prev | step exp |
|---|---|---|---|---|---|---|---|---|
| files-10 | 10 | 40 | 40 | 1.11 | 1.09 | 1.14 | — | — |
| files-50 | 50 | 200 | 200 | 5.02 | 4.95 | 5.04 | 4.5× | 0.94 |
| files-250 | 250 | 1000 | 1000 | 24.0 | 23.7 | 24.7 | 4.8× | 0.97 |
| files-1000 | 1000 | 4000 | 4000 | 95.2 | 94.4 | 97.3 | 4.0× | 0.99 |

Shape: t ~ scale^0.97 over 10→1000 (≈ linear; least-squares over 4 points, worst step 0.99)

Contrasts:

| case | median ms | vs baseline |
|---|---|---|
| files-1000-unassigned | 88.0 | 0.92× of files-1000 |
| files-250-staged | 30.2 | 1.25× of files-250 |
| files-250-touched | 27.3 | 1.14× of files-250 |

## hunks — hunks per file across 25 changed files, all assigned across 4 changelists

| case | scale | hunks | records | median ms | min | max | ×prev | step exp |
|---|---|---|---|---|---|---|---|---|
| hunks-2 | 2 | 50 | 50 | 2.17 | 2.16 | 2.20 | — | — |
| hunks-8 | 8 | 200 | 200 | 3.07 | 2.96 | 3.35 | 1.4× | 0.25 |
| hunks-32 | 32 | 800 | 800 | 7.55 | 6.94 | 8.49 | 2.5× | 0.65 |
| hunks-128 | 128 | 3200 | 3200 | 22.9 | 22.4 | 23.3 | 3.0× | 0.80 |

Shape: t ~ scale^0.57 over 2→128 (sub-linear; least-squares over 4 points, worst step 0.80)

Contrasts:

| case | median ms | vs baseline |
|---|---|---|
| hunks-128-unassigned | 15.9 | 0.70× of hunks-128 |

## records — dormant membership records on top of a fixed 50-file × 4-hunk diff (200 live records)

| case | scale | hunks | records | median ms | min | max | ×prev | step exp |
|---|---|---|---|---|---|---|---|---|
| dormant-0 | 0 | 200 | 200 | 4.80 | 4.57 | 5.01 | — | — |
| dormant-1000 | 1000 | 200 | 1200 | 7.64 | 7.35 | 7.71 | 1.6× | — |
| dormant-4000 | 4000 | 200 | 4200 | 14.8 | 14.5 | 15.8 | 1.9× | 0.48 |
| dormant-16000 | 16000 | 200 | 16200 | 47.2 | 45.3 | 47.8 | 3.2× | 0.83 |

Shape: t ~ scale^0.66 over 1000→16000 (sub-linear; least-squares over 3 points, worst step 0.83)

## huge-file — single generated file rewritten in full, no changelists — the unbounded-diff-memory probe (ADR 0005 caveat)

| case | lines | content MB | median ms | min | max | ×prev | step exp | peak RSS MB |
|---|---|---|---|---|---|---|---|---|
| huge-32k | 32768 | 5.0 | 45.5 | 44.0 | 45.5 | — | — | 4.3 → 29.2 |
| huge-128k | 131072 | 20.3 | 186 | 184 | 190 | 4.1× | 1.02 | 4.2 → 104.9 |
| huge-512k | 524288 | 81.8 | 835 | 819 | 839 | 4.5× | 1.08 | 4.3 → 397.2 |

Shape: t ~ scale^1.05 over 32768→524288 (≈ linear; least-squares over 3 points, worst step 1.08)

Contrasts:

| case | median ms | vs baseline | peak RSS MB |
|---|---|---|---|
| huge-128k-staged | 333 | 1.79× of huge-128k | 4.5 → 138.1 |

## binaries — changed 1 MiB binary files, no changelists — every refresh re-hashes each one's worktree bytes (ADR 0009's stated cost)

| case | files | content MB | median ms | min | max | ×prev | step exp | peak RSS MB |
|---|---|---|---|---|---|---|---|---|
| binary-4 | 4 | 8.0 | 42.8 | 41.7 | 43.3 | — | — | 4.3 → 7.6 |
| binary-16 | 16 | 32.0 | 173 | 169 | 175 | 4.1× | 1.01 | 4.1 → 7.5 |
| binary-64 | 64 | 128.0 | 692 | 684 | 695 | 4.0× | 1.00 | 4.0 → 7.5 |
| binary-256 | 256 | 512.0 | 2700 | 2682 | 2732 | 3.9× | 0.98 | 4.4 → 7.9 |

Shape: t ~ scale^1.00 over 4→256 (≈ linear; least-squares over 4 points, worst step 1.01)

Contrasts:

| case | median ms | vs baseline | peak RSS MB |
|---|---|---|---|
| binary-8x32m | 2596 | 0.96× of binary-256 | 4.1 → 68.5 |

## Caveats

- Synthetic repos carry a single baseline commit, so the constant
  recent-commits window (300) is under-represented; it doesn't
  affect per-dimension shape.
- Dormant records accrue on vanished paths (the stash/revert
  reality); live-path tier-1 scan cost is exercised by the hunks
  dimension instead.
- Unassigned contrasts run with no changelists at all: the matcher
  does no record work, isolating diff cost from matching cost.
- Absolute numbers are dev-machine optimistic (issue #16); the
  exponents are the decision inputs.
