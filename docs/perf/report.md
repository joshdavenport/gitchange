# RefreshJob benchmark

- date: 2026-07-31
- host: Apple M1 Max (Darwin arm64)
- commit: 54af01c
- per case: 5 timed refreshes after 2 warmup, fresh subprocess

## files — changed files, 4 hunks each, all hunks assigned across 4 changelists

| case | scale | hunks | records | median ms | min | max | ×prev | step exp |
|---|---|---|---|---|---|---|---|---|
| files-10 | 10 | 40 | 40 | 3.27 | 3.01 | 3.61 | — | — |
| files-50 | 50 | 200 | 200 | 14.3 | 14.2 | 14.7 | 4.4× | 0.92 |
| files-250 | 250 | 1000 | 1000 | 75.8 | 73.0 | 78.1 | 5.3× | 1.04 |
| files-1000 | 1000 | 4000 | 4000 | 270 | 270 | 273 | 3.6× | 0.92 |

Shape: t ~ scale^0.97 over 10→1000 (≈ linear; least-squares over 4 points, worst step 1.04)

Contrasts:

| case | median ms | vs baseline |
|---|---|---|
| files-1000-unassigned | 266 | 0.98× of files-1000 |
| files-250-staged | 96.6 | 1.27× of files-250 |
| files-250-touched | 80.2 | 1.06× of files-250 |

## hunks — hunks per file across 25 changed files, all assigned across 4 changelists

| case | scale | hunks | records | median ms | min | max | ×prev | step exp |
|---|---|---|---|---|---|---|---|---|
| hunks-2 | 2 | 50 | 50 | 7.19 | 6.93 | 7.36 | — | — |
| hunks-8 | 8 | 200 | 200 | 8.26 | 7.58 | 8.70 | 1.1× | 0.10 |
| hunks-32 | 32 | 800 | 800 | 12.0 | 11.8 | 12.4 | 1.5× | 0.27 |
| hunks-128 | 128 | 3200 | 3200 | 29.0 | 28.8 | 29.2 | 2.4× | 0.64 |

Shape: t ~ scale^0.33 over 2→128 (sub-linear; least-squares over 4 points, worst step 0.64)

Contrasts:

| case | median ms | vs baseline |
|---|---|---|
| hunks-128-unassigned | 24.4 | 0.84× of hunks-128 |

## records — dormant membership records on top of a fixed 50-file × 4-hunk diff (200 live records)

| case | scale | hunks | records | median ms | min | max | ×prev | step exp |
|---|---|---|---|---|---|---|---|---|
| dormant-0 | 0 | 200 | 200 | 14.1 | 13.7 | 14.2 | — | — |
| dormant-1000 | 1000 | 200 | 1200 | 18.2 | 17.5 | 19.8 | 1.3× | — |
| dormant-4000 | 4000 | 200 | 4200 | 31.2 | 30.1 | 31.8 | 1.7× | 0.39 |
| dormant-16000 | 16000 | 200 | 16200 | 78.9 | 77.9 | 84.5 | 2.5× | 0.67 |

Shape: t ~ scale^0.53 over 1000→16000 (sub-linear; least-squares over 3 points, worst step 0.67)

## huge-file — single generated file rewritten in full, no changelists — the unbounded-diff-memory probe (ADR 0005 caveat)

| case | lines | content MB | median ms | min | max | ×prev | step exp | peak RSS MB |
|---|---|---|---|---|---|---|---|---|
| huge-32k | 32768 | 5.0 | 39.9 | 39.6 | 40.6 | — | — | 6.8 → 51.8 |
| huge-128k | 131072 | 20.3 | 164 | 160 | 169 | 4.1× | 1.02 | 6.7 → 217.6 |
| huge-512k | 524288 | 81.8 | 701 | 696 | 732 | 4.3× | 1.05 | 6.8 → 790.0 |

Shape: t ~ scale^1.03 over 32768→524288 (≈ linear; least-squares over 3 points, worst step 1.05)

Contrasts:

| case | median ms | vs baseline | peak RSS MB |
|---|---|---|---|
| huge-128k-staged | 292 | 1.78× of huge-128k | 7.5 → 318.5 |

## binaries — changed 1 MiB binary files, no changelists — every refresh re-hashes each one's worktree bytes (ADR 0009's stated cost)

| case | files | content MB | median ms | min | max | ×prev | step exp | peak RSS MB |
|---|---|---|---|---|---|---|---|---|
| binary-4 | 4 | 8.0 | 51.8 | 50.9 | 52.5 | — | — | 6.8 → 12.8 |
| binary-16 | 16 | 32.0 | 171 | 170 | 175 | 3.3× | 0.86 | 6.8 → 12.8 |
| binary-64 | 64 | 128.0 | 669 | 668 | 697 | 3.9× | 0.99 | 6.8 → 12.9 |
| binary-256 | 256 | 512.0 | 2874 | 2718 | 2928 | 4.3× | 1.05 | 6.9 → 14.4 |

Shape: t ~ scale^0.97 over 4→256 (≈ linear; least-squares over 4 points, worst step 1.05)

Contrasts:

| case | median ms | vs baseline | peak RSS MB |
|---|---|---|---|
| binary-8x32m | 2654 | 0.92× of binary-256 | 6.8 → 103.8 |

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
