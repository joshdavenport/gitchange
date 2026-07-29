# Staging: write-through to the live index, staged state derived

gitchange confirms the brief's section-4 default — staging is a
per-changelist step before commit (`space` stages a subset; `c` commits the
changelist's staged hunks) — and implements it **on the real git index**.
Staging a hunk performs a genuine apply-to-index on the live index
(git2 `ApplyOptions::hunk_callback`, CLI shell-out fallback per the backend
recommendation); unstaging reverse-applies. gitchange persists **no staged
bit**: staged state is derived at every refresh by matching
diff(HEAD↔index) against membership records with the tier-1 exact
content-anchor match from ADR 0001. The index is the single source of truth
for staged-ness; `state.json` remains membership-only (no change to ADR
0002's schema).

## Hunk universe (amends ADR 0001)

The set of hunks gitchange displays and matches membership against is the
**union of diff(HEAD↔worktree) and diff(HEAD↔index)** — not the worktree
diff alone. The invariant this buys: **every hunk that would be included in
a commit is visible in the TUI.**

Per-hunk staged state is three-valued:

| source                         | state                              |
| ------------------------------ | ---------------------------------- |
| worktree diff only             | `○` unstaged                       |
| both diffs, content equal      | `●` staged                         |
| both diffs, content differs    | `◑` staged-stale                   |
| index diff only                | `◑` staged-stale (reverted in worktree) |

`space` on `◑` sets index := worktree — re-staging an edited hunk,
discarding an index-only one. Per-file markers stay `●◐○`; staged-stale
counts toward `◐`.

## External staging is absorbed, never an error

Pre-existing staged changes at launch, external `git add`, external
`git reset` — all simply derive at the next refresh. Staged hunks owned by
a record derive that record's staged state; unowned staged hunks follow ADR
0001's assignment rules (active changelist + notify on ambiguity, unassigned
when no changelists exist). There is no error or confirmation flow for
externally staged state.

## Considered options

- **Virtual staging (staged bit in `state.json`, live index untouched until
  commit)** — rejected: gitchange's staging would be invisible to git
  tooling and to pre-commit hooks (`git diff --cached` would lie), external
  staging becomes an unrepresentable error state needing a
  confirm/unstage-everything flow, and the stored bit can drift from
  reality. gitchange is a layer on top of git; where a git mechanic exists
  (the index), use it.
- **Write-through plus a stored staged bit** — rejected: reintroduces the
  drift/disagreement bug class that derived-only eliminates, for no benefit
  beyond remembering intent the index already records.
- **Binary staged state, auto-restage on edit** — rejected: gitchange would
  rewrite the user's index unasked, and "stage a checkpoint, keep
  experimenting" (the classic `git add` workflow) becomes impossible.
- **Binary staged state, edit unstages** — rejected: an external `git add`
  followed by one edit would silently evaporate from the index.
- **Worktree-only hunk universe (ADR 0001 unamended)** — rejected: an
  index-only hunk (staged, then worktree reverted) would go dormant and
  invisible while remaining committable, violating the visibility
  invariant.

## Consequences

- Refresh diffs twice (HEAD↔worktree and HEAD↔index) and matches both
  against records; the matcher stays a pure function, now of (records,
  worktree diff, index diff).
- Unstaging one hunk while the same file has other staged hunks requires
  reverse-apply on the index blob — well-trodden (`git add -p`, lazygit);
  apply edge cases fall back to CLI shell-out.
- Commit mechanics (decided separately): write-through makes
  "unstage other changelists' hunks → commit the real index → restage"
  the natural default, so hooks see the truth; temp-tree synthesis becomes
  the fallback, not the default.
- `git status` in another terminal shows the union of all changelists'
  staged hunks — honest, though not separated by changelist.
- The `◑` glyph joins the replaceable-glyph set; presentation of the
  "reverted in worktree" flavour lands with the error/notification
  presentation work.
