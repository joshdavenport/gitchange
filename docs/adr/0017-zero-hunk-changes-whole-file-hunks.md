# Zero-hunk changes: whole-file hunks

A change with no line-addressable content — a mode-only change, an empty
file added (tracked or untracked), an empty file deleted — presents **one
degenerate whole-file hunk**, extending ADR 0009's binary treatment to the
whole class. The invariant: **every change in the universe carries at
least one hunk** (conflicted files excepted — quarantined per ADR 0007).
No change can fall out of membership, staging, or commit for lack of a
hunk. ADR 0015's "nothing broadens a payload silently" gains its mirror:
nothing narrows one silently either.

Prior art converges here: git's own `add -p` promotes the mode header to a
synthetic hunk (`PROMPT_MODE_CHANGE`), magit renders a `'(chmod)` hunk
section, and scm-record/jj — after shipping this exact bug twice
(jj#2548, jj#3016) — made "every mode/existence transition gets a
selectable section" a model invariant. The tools that refused (gitui,
lazygit, GitHub Desktop) silently drop mode changes from partial workflows
or carry years-open bugs about it.

## Identity & matching (ADR 0009 reused, no new record shape)

- Identity is `WholeFile` with the blob-OID pair anchor. Empty add and
  empty delete are discriminated by the anchor's sides (HEAD-side absent /
  changed-side empty blob, and the reverse). Membership records, the
  record discriminator, and the matcher arms are unchanged.
- For a mode-only change the two sides are equal, so exact-match is
  vacuous and matching rests on path continuity — the same strength the
  binary inheritance tier already has. Consequence: a mode re-flip at the
  same path revives a dormant record. That is the intended behaviour:
  same predicament, same home.
- Records carry no mode bits. Mode is snapshot data on the changed file,
  present for display and stage derivation only.

## Staging & ride-along

- The synthetic hunk routes through the whole-file index ops, as binaries
  do: stage writes the index entry from the worktree (content and mode),
  unstage restores HEAD's entry (or drops a new one). No apply machinery.
- Stage derivation: by OID compare per ADR 0009, except the mode-only
  case, where the OIDs are equal and derivation compares mode bits across
  HEAD, index, and worktree. `◑` staged-stale is reachable (staged mode
  flip, then worktree revert).
- The synthetic hunk exists **only when the change would otherwise have
  zero hunks**. When content hunks exist, the mode rides along: any stage
  op that writes the file's index entry carries the worktree mode. This
  promotes today's accidental `index.add_path` behaviour to a documented
  rule and extends it to partial staging. gitchange does not stage a mode
  independently of content in the same file.

## Presentation

- Diff panel: one-line placeholders on the binary/conflicted channel
  (ADR 0007): `Mode changed (100644 → 100755)`, `Empty file added`,
  `Empty file deleted`. Octal, as git prints it.
- No new glyphs: rows derive `●○` (and `◑`) normally — `○ 0/1` replaces
  the inert `0/0`. Hunk-mode entry on a lone degenerate hunk stays the
  polite no-op (ADR 0009 idiom).

## Considered options

- **Loud out-of-scope** — row states the change needs raw `git add` and
  will not appear in a gitchange commit. Rejected: the politer cousin of
  ADR 0009's rejected "invisible to gitchange"; it leaves a class of
  change gitchange cannot drive, permanently.
- **A dedicated mode-transition identity variant** (`old → new` in the
  record) — rejected: its only behavioural difference is refusing dormant
  revival on a different mode flip at the same path, and revival there is
  right anyway; it costs a new record shape and matcher arms.
- **Always-stand-alone mode hunk beside content hunks** (git `add -p`,
  magit, scm-record) — rejected for now: needs a mode-only index write op
  and the implication rules scm-record had to add (PR #95) once
  independent selection allowed contradictory states. A chmod+edit is one
  logical change in practice. Reopenable without unwinding this decision.

## Consequences

- ADR 0003's zero-hunk exception ("`space` moves nothing and says so") is
  superseded; its staging-scopes section now reads per this ADR.
- The apply-corpus cases pinning the inertness flip to assert the
  whole-file index write, and empty-tracked-file deletion — the same
  class, previously uncovered — gains cases.
- `TypeChanged` (file↔symlink) may present zero-hunk shapes and has no
  whole-file routing arm; flagged as its own issue, not covered here.
- Submodule pointer changes stay invisible (`ignore_submodules` on every
  diff) — a standing scope decision, not a zero-hunk file.
