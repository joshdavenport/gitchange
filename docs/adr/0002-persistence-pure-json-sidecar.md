# Persistence: pure JSON sidecar in the per-worktree git dir

Changelist metadata (the membership records of ADR 0001, plus changelist
names and the active marker) lives in a single pretty-printed JSON file at
`$GIT_DIR/gitchange/state.json`, located via
`git rev-parse --git-path gitchange`. No git objects are written and no
anchor ref is needed — records contain only text. Writes are atomic
write-then-rename guarded by a git-convention lockfile
(`state.json.lock`); a process finding the lock held fails fast with a
clear error rather than waiting. The lockfile records the holder's PID, so
that error separates a live holder — retry — from a leaked lock, the only
case where removing the file is sound advice; an unreadable PID is assumed
live. The file carries a schema version field.

Anchors are stored as **plain text, not compressed** — this supersedes ADR
0001's "compressed" wording. Uncommitted-change metadata is small, and
plain text preserves the human-debuggable property ADR 0001 used to justify
verbatim anchors in the first place: `cat` is the debugger.

## Scope semantics

- **Branch switches: changelists are global to the working tree.** Git
  carries dirty hunks across `git switch` untouched, so changelists ride
  along; after a switch the user sees the same changelists and the next
  refresh re-matches as usual. Records store no branch.
- **Worktrees: per-worktree sets.** Each linked worktree has its own
  independent changelist state, for free via `--git-path` resolution
  (`gitchange` is not on git's common-paths list, so it resolves to
  `.git/worktrees/<id>/gitchange/` in linked worktrees). Forced by the
  semantics: changelists partition *this* working tree's dirty hunks.
- **Strictly local for v0.1.** No sync, export, or push/pull of changelist
  state. Persistence sits behind a trait, leaving room for a refs-based
  backend as a fresh effort if cross-machine sync ever matters.

## Record lifecycle for vanished hunks

gc and clone are non-events: there are no objects to prune, and a fresh
clone has no dirty tree, so empty state is correct. The live case is hunks
vanishing from the diff via legitimate git operations (stash, external
commit, rebase):

- An unmatched record becomes **dormant**: retained in the state file,
  revivable only via the tier-1 exact content-anchor match — never overlap
  inheritance, so stale records cannot mis-claim unrelated future edits.
  `git stash` → `git stash pop` therefore round-trips membership cleanly.
- gitchange-initiated commits remove their own records explicitly (the
  tool knows those hunks were committed).
- All of a deleted changelist's records are pruned, live and dormant
  (ADR 0016); dormant records also prune after 14 days.

## Considered options

- **Custom refs, stgit-style (`refs/gitchange/*` → state commit)** —
  rejected for v0.1: records mutate on every completed refresh (ADR 0001),
  so a refs backend synthesizes commit+tree+blob on a hot path (odb churn),
  and schema iteration goes through git plumbing. Its advantages (atomic
  CAS, opt-in sharing, git-native introspection) don't pay for that here;
  the trait keeps the door open.
- **Sidecar + anchor ref hybrid (jj-style)** — rejected: the anchor ref
  only earns its keep if records reference git objects, and ADR 0001's
  anchors are verbatim text. Speculative complexity; can be added later
  without migration.
- **git notes / literal stash mechanics / working-tree file** — rejected in
  the ticket 02 survey (wrong shape; reflog decay; ignore-rule fragility).
- **SQLite** — rejected: real locking for free, but a binary blob in
  `.git`, a dependency, and query machinery for a read-all/write-all
  dataset.
- **Compressed anchors / per-branch changelists / shared-across-worktrees
  sets** — rejected as noted above.

## Consequences

- The persistence layer is trivially testable: a temp dir and a JSON file,
  no git objects involved.
- State is invisible to git tooling by design; introspection is `cat
  .git/gitchange/state.json` or a future `gitchange` subcommand, never
  `git cat-file`.
- Concurrency is fail-fast, not queued: the lock primitive refuses
  immediately, and a lock is never taken from a live holder. Frontends
  may retry briefly on contention — the engine does — but no writer
  waits in line.
- The 14-day dormant prune bounds state-file growth; stashes older than
  that lose membership on pop — a visible failure (auto-capture to active
  + notification per ADR 0001), never a silent wrong-list assignment.
