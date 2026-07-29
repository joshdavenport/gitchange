# gitchange

A lazygit-inspired TUI for organising uncommitted local changes into named
changelists that can be staged and committed independently.

## Conventions

**Git-feel CLI vocabulary**:
Where a gitchange concept has a git analog, borrow git's vocabulary for
what users type — commands, flags, args (e.g. `switch` sets the active
changelist, as `git switch` sets the current branch). Governs naming only;
internals like exit-code schemes follow their own conventions.

## Language

**Changelist**:
A named set of uncommitted changes. User-created; any number may exist;
created, renamed, and deleted at will.
_Avoid_: group, set, patch, stack

**Active changelist**:
The single changelist (marked `*`) that captures new hunks and ambiguous
edits automatically. Exactly one is active whenever any changelists exist.
_Avoid_: current, selected, default

**Unassigned**:
The pseudo-changelist holding hunks no changelist owns: the dirty tree
before any changelist exists, orphans of deleted changelists, and explicit
moves. Rendered as a warning state. `unassigned` is a reserved name — user
changelists cannot take it.
_Avoid_: unmanaged, inbox, unsorted

**All**:
The pseudo-view (not a changelist) showing every changed file grouped by
changelist, with the unassigned group last. The launch view. `all` is a
reserved name — user changelists cannot take it.
_Avoid_: overview, everything

**Hunk-level membership**:
Changelist membership is per-hunk, not per-file. A single file's hunks may
belong to different changelists; a file is "in" a changelist when it has at
least one hunk owned by it.
_Avoid_: file membership

**Membership record**:
The persisted claim that a changelist owns a hunk: file path, line
coordinates, owning changelist, and a content anchor.
_Avoid_: tag, assignment entry

**Content anchor**:
The verbatim hunk lines (plus surrounding context) stored in a membership
record — the identity evidence hunks are matched against on refresh.
_Avoid_: hunk hash, fingerprint

**Drift**:
Divergence between stored membership records and the fresh diff, caused by
working-tree edits. Resolved at refresh by matching, never tracked live.
_Avoid_: staleness, desync

**Dormant record**:
A membership record whose hunk vanished from the fresh diff (stash,
external commit, rebase). Retained; revivable only by exact content-anchor
match, never overlap inheritance. Pruned when its changelist is deleted or
after 14 days.
_Avoid_: stale, orphaned, archived

**State file**:
The single JSON sidecar (`$GIT_DIR/gitchange/state.json`, per-worktree)
holding changelists, the active marker, and membership records. Plain text,
atomic rename, lockfile-guarded, schema-versioned. Strictly local.
_Avoid_: database, metadata store

**Staged / partially staged / unstaged**:
Per-file staging states, marked `●` (fully staged), `◐` (partially staged),
`○` (unstaged), with `stagedHunks/totalHunks` counts per file. Staged-stale
hunks count toward `◐`.
_Avoid_: added, indexed

**Derived staged state**:
Staged-ness is never persisted; it is recomputed at every refresh by
matching diff(HEAD↔index) against membership records. Staging writes
through to the live git index (`space` = real apply-to-index), which is the
single source of truth. External `git add`/`git reset` are absorbed at
refresh, never errors.
_Avoid_: staged bit, staged flag

**Staged-stale**:
Per-hunk state `◑`: the index holds an overlapping-but-different version of
the hunk (staged then edited, or staged then reverted in the worktree) —
committing now would commit content that isn't what you see. `space` on a
staged-stale hunk sets index := worktree.
_Avoid_: dirty-staged, out-of-date

**Commit payload**:
What committing a changelist commits: its staged hunks as index content,
snapshotted by a synchronous refresh at confirm time. Staged-stale hunks
enter it only via warn-and-confirm; an empty payload triggers the
stage-all-and-commit offer.
_Avoid_: commit set, selection

**Refresh**:
The single recompute pass: status + diff(HEAD↔index) + diff(HEAD↔worktree)
→ membership matching → persist → one snapshot. Atomic and last-request-
wins; triggered by the debounced watcher, gitchange's own mutations,
terminal focus, or a manual key. Nothing updates incrementally between
refreshes.
_Avoid_: update, reload, sync

**Snapshot**:
The immutable result of one refresh — the only data structure the UI
reads. Panels render the last snapshot until the next swaps in whole;
mid-refresh the view is stale but interactive, never cleared.
_Avoid_: state, model, view data

**Engine**:
The core-owned runtime that watches the filesystem, runs refreshes, and
emits snapshots and degradation events over a channel. The TUI's only data
source; the CLI bypasses it and calls operations synchronously.
_Avoid_: worker, service, daemon

**Frontend**:
A thin consumer of core — the TUI and the CLI. Frontends never speak git
directly; anything a frontend needs lives behind core's interface.
_Avoid_: client, UI layer (the CLI isn't one)

**Hunk universe**:
The set of hunks gitchange displays and matches membership against: the
union of diff(HEAD↔worktree) and diff(HEAD↔index). Invariant: every hunk
that would be in a commit is visible in the TUI.
_Avoid_: worktree diff, fresh diff (alone)

**Log panel**:
The panel at the bottom of the right-hand pane collection: one
chronological stream of executed git operations and event notices, with a
pinned region of live conditions at top. One of the three presentation
channels (panel state, Log panel, modal); there is no message line or
toast.
_Avoid_: command log, console, message line, toast

**Event**:
A one-shot occurrence recorded as a log line — immutable, chronological,
scrolls away. Carries a severity: `·` info (routine, dim), `!` notice
(automatic membership decisions worth spot-checking, warning tint),
`✗` error (also modalled). Three levels, fixed; severity glyphs are their
own tokens, never the staging set.
_Avoid_: message, alert, log level (for severity)

**Condition**:
A state that currently holds (watcher degraded, git operation in
progress, detached HEAD), surfaced as a pin at the top of the Log panel.
Condition-bound and self-clearing; never manually dismissable. An event
marks the moment a condition began; the pin is the condition itself.
_Avoid_: status, flag, sticky notice

**Quarantine**:
The per-file suspension of unmerged paths during a merge/rebase/etc.:
excluded from the hunk universe, membership records frozen (no matching,
no dormancy clock), listed live in the Conflicts group (rendered first),
diff panel shows a placeholder. Everything else stays live; records
re-enter normal matching when the file resolves.
_Avoid_: freeze (global), lockdown, hiding

**Operation guard**:
Commit disabled while any git operation is in progress (merge, rebase,
cherry-pick, revert, am) — the next commit would conclude that operation
with one changelist's content. One predicate, one pin, polite no-op on
`c`. Applies to commit only; staging is never operation-guarded.
_Avoid_: merge lock, commit block
