# Commit mechanics: temp index via GIT_INDEX_FILE, live index untouched

Committing a changelist builds a **temporary index** — HEAD's tree plus only
this changelist's staged hunks, applied from diff(HEAD↔index) filtered by
membership — and shells out to `git commit` with `GIT_INDEX_FILE` pointing
at it. Hooks run natively and see the commit's true content via
`git diff --cached`; the live index is never modified, so every failure
(hook rejection, apply failure, aborted message) discards a temp file and
changes nothing. This **supersedes ADR 0003's predicted default** (the
real-index unstage/commit/restage shuffle). Shell-out is forced regardless
of path choice: git2's commit API does not run hooks.

Post-commit consistency is free: after HEAD moves, diff(new HEAD↔live index)
is exactly the other changelists' staged hunks, so derived staged state
(ADR 0003) stays correct with no restoration step.

## What `c` commits

- The changelist's **staged hunks — index content** is the payload.
- **Staged-stale `◑` hunks in the payload → warn-and-confirm**, with an
  **align option**: set index := worktree for the changelist's stale hunks
  (bulk application of ADR 0003's `space`-on-`◑` semantic) before
  committing. Never silent, never blocking.
- **Zero staged hunks → offer stage-all-and-commit** ("nothing staged —
  stage all N hunks and commit?"). One commit mechanism, always from staged
  hunks; `c` never grows a second meaning. *Amended (issue #90):* the
  offer stages the changelist's `○` hunks only — a `◑` hunk is itself payload, so an empty
  payload means the changelist holds nothing else. This is narrower than
  `space` on a changelist (ADR 0003), which stages `○` and `◑` alike.
- **Unassigned is committable** like any changelist — no ceremony forced on
  an unsorted tree; the warning styling means "unsorted", not "broken".
  `c` on the `all` view is a no-op with a message.

## Guards

- **Freshness: refresh-before-commit, re-confirm on drift.** Confirming
  triggers a synchronous refresh; commit proceeds only if the changelist's
  staged payload is unchanged, otherwise the flow returns to the confirm
  step with a notice. The confirm dialog's contents are a guarantee, not a
  hope.
- **`--no-verify` is supported**; how the commit UI exposes it is the
  commit-flow UI's decision (ticket 10).
- Known limitation, documented not solved: hooks that inspect the
  **worktree** directly (linters on files, not `--cached` content) see
  other changelists' edits too — inherent to any partial-commit tool
  sharing one worktree, same as `git add -p`.

## Aftermath

- Membership records for **fully-consumed** hunks (index == worktree at
  commit time) are removed explicitly (per ADR 0002).
- Records for **`◑` hunks committed as-is are retained**: the commit leaves
  a residual worktree diff (worktree vs the committed checkpoint), and at
  the next refresh ADR 0001's overlap-inheritance tier re-attaches it to
  the same changelist. No new rules; unmatched retained records go dormant
  per ADR 0002. **Amended by ADR 0012**: re-attachment holds only because
  `commit()` rewrites retained `◑` records against the new HEAD
  (coordinates and anchor) as part of its commutation shift; across
  external HEAD moves, retained records are exact-revival-only.
- The worktree is never touched; a full refresh follows the commit; files
  with no remaining owned hunks leave their lists naturally.
- **An emptied changelist is kept** until explicitly deleted — deletion
  stays a deliberate act; no post-commit dialog.

## Amend

**In for v0.1.** Same temp-index path with `--amend`: message reused for
editing, hooks run, `--no-verify` applies, identical guards and
bookkeeping. Rewriting pushed commits is standard git territory, not ours
to guard. Keybinding/UI is ticket 10's.

## Considered options

- **Real-index shuffle (ADR 0003's predicted default)** — reverse-apply
  other changelists' staged hunks, commit the live index, re-apply after.
  Rejected: a hook rejection, crash, or failed re-apply mid-dance damages
  the user's other staged work, requiring index snapshot/restore and crash
  recovery — an entire failure-window problem class the temp index deletes.
  Its only advantage (mid-commit `git status` in another terminal shows the
  literal commit state) doesn't pay for that.
- **git2 commit-tree, no hooks** — rejected: pre-commit/commit-msg hooks
  are table stakes for the target user's repos.
- **Block commit on `◑` hunks** — rejected: fights the
  stage-a-checkpoint-keep-experimenting workflow ADR 0003 explicitly
  preserved.
- **Silent auto-stage on zero-staged `c` / hard block** — rejected for the
  offer: silent makes `c` mean two things invisibly; blocking adds friction
  to the most common starting state.
- **Remove all committed records including `◑`** — rejected: the residual
  diff would auto-capture to the *active* changelist (ADR 0001), silently
  moving a continuation of the committed work to the wrong list.
- **Auto-delete / prompt-to-delete emptied changelists** — rejected:
  auto-delete silently loses the name and forces the active marker to move;
  a prompt adds a dialog to a flow that already carries message entry and
  possible `◑` confirmation.

## Consequences

- Commit is implementable as: write HEAD's tree into a temp index
  (read-tree), apply the payload hunks `--cached` against it, run
  `git commit` (shell-out, `GIT_INDEX_FILE` env, message via `-F`,
  optional `--no-verify`/`--amend`), delete the temp file. Apply failures
  map to `ApplyFailed` — the commit site of ADR 0003's apply tripwire —
  and abort cleanly before any commit exists.
- GPG signing, commit templates, and prepare-commit-msg/commit-msg hooks
  work for free via inherited git config.
- Hooks that rewrite files (formatters) leave worktree edits that surface
  as new hunks at next refresh and auto-capture per ADR 0001 — visible,
  never silent.
- Message-entry UX, `◑`-confirm presentation, and hook-failure display
  belong to the commit-flow UI (ticket 10) and the error/notification
  presentation work.
