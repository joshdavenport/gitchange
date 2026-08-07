# Staging: write-through to the live index, staged state derived

gitchange confirms the brief's section-4 default — staging is a
per-changelist step before commit (`space` stages a subset; `c` commits the
changelist's staged hunks) — and implements it **on the real git index**.
Staging a hunk performs a genuine apply-to-index on the live index
(git2 `ApplyOptions::hunk_callback`, with a CLI shell-out fallback per the
backend recommendation — re-scoped to a conditional mitigation below);
unstaging reverse-applies. gitchange persists **no staged
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

## The shell-out apply fallback is conditional, not scheduled work

This ADR named a CLI shell-out fallback for "apply edge cases" as
belt-and-braces against libgit2's apply being less battle-hardened than
git's own `apply.c`. **It is a contingency gated on evidence, not a
deliverable** — v0.1 ships without it deliberately, and the trigger has
never fired.

Why the failure class it guards against has not appeared: gitchange never
applies a *foreign* patch. `stage_worktree_range` computes
diff(index↔worktree) from the live repo and immediately applies a subset
of its own hunks back to the very index it was computed against;
`unstage_head_range` mirrors that with diff(HEAD↔index), reversed. The
dominant real cause of `git apply` failure — preimage mismatch, fuzz,
context or whitespace drift — is structurally unreachable. libgit2 also
computes every postimage before touching the index, so a refusal is
already all-or-nothing, which is the guarantee this ADR wanted from the
fallback. The apply corpus (ADR 0008's exit criterion, and this
mitigation's certification suite) covers the seeded edge list — no
trailing newline, CRLF, blank-line-only hunks, adjacent hunks, mode
changes, create/delete, empty-file edges — and is green on Linux, macOS
and Windows.

Fixed now so the mitigation is one document away rather than one
archaeology session away:

- **The trigger exists**: `Error::ApplyFailed { path, detail, site }` —
  the apply tripwire — mapped from the two libgit2 apply calls alone:
  staging's apply-to-index, and commit's payload apply against HEAD's
  tree while the temp index is assembled (ADR 0004). Never mapped from
  the diff computation or index plumbing around them; an environmental
  failure striking inside an apply call (a broken odb) still maps, and
  the trigger condition below is the filter. It is a **hard** error
  carrying libgit2's message verbatim into the ADR 0007 modal, not a
  soft advisory: an unapplyable hunk is an unexpected limitation whose
  full text *is* the evidence this mitigation waits on, whereas
  advisories cover expected races (a hunk that moved under the user).
  The staging site's message names `git add -p` as the workaround — a
  direct route to the same end state, which gitchange then absorbs like
  any external staging. The commit site's message offers no workaround
  and states ADR 0004's abort guarantee (nothing was committed)
  instead: no direct route to a partial commit exists, and `git commit`
  would commit every changelist's staged hunks at once.
- **Its shape, if triggered**: a fallback *inside* an adapter's apply
  methods — build the patch text for the selected hunks and pipe it to
  git's own apply: `git apply --cached` at the staging site, the same
  with `GIT_INDEX_FILE` pointed at the temp index at the commit site.
  **Not a second `GitBackend` implementor.** Only
  2 of the seam's methods apply anything; a second full adapter would
  re-implement diffs, HEAD, log and operation state in shell-out for no
  reason. This corrects ADR 0006 and ADR 0008, which both read the
  fallback as a second adapter — consequently **ADR 0008's
  "parameterize the whole suite across both adapters" clause does not
  fire**; only the apply corpus would parameterize.
- **The trigger condition, precisely**: an `ApplyFailed` reproducible in
  a real repository where git's own apply succeeds on the same hunk
  selection against the same base (`git apply --cached`; at the commit
  site with `GIT_INDEX_FILE` pointed at the temp index). Absent that,
  building the fallback is net negative — it
  replaces libgit2's postimage computation with hand-written unified
  diff text (headers, post-filter line counts, `\ No newline at end of
  file`), a *new* failure surface with an unknown error rate, in front
  of a backstop with zero observed failures.

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
  apply edge cases would fall back to CLI shell-out, conditionally (above).
- Commit mechanics (decided separately): write-through makes
  "unstage other changelists' hunks → commit the real index → restage"
  the natural default, so hooks see the truth; temp-tree synthesis becomes
  the fallback, not the default.
- `git status` in another terminal shows the union of all changelists'
  staged hunks — honest, though not separated by changelist.
- The `◑` glyph joins the replaceable-glyph set; presentation of the
  "reverted in worktree" flavour lands with the error/notification
  presentation work.
