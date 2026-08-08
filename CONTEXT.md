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
before any changelist exists, orphans of deleted changelists, and hunks
assigned to it by hand. Rendered as a warning state. `unassigned` is a
reserved name — user changelists cannot take it.
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

**Whole-file hunk**:
The degenerate single hunk a changed binary file presents: one membership
record spanning the file, anchored by a blob-OID pair instead of verbatim
lines. Follows normal assign rules; keeps membership by path continuity
while the path stays binary-changed; `◐` unreachable, `◑` derives
by OID compare; hunk-mode entry is a polite no-op.
_Avoid_: binary hunk, file-level membership (as a general mode)

**Assign**:
Placing hunks under a changelist's ownership by hand — the manual
counterpart to the active changelist's automatic capture. The target is any
changelist or `unassigned`; there is no separate un-assign operation, since
unassigned is a target like any other. Scope escalates in three steps: the
selected hunk, a file's unassigned hunks, all of a file's hunks including
those owned by other changelists.
_Avoid_: move, add (git's `add` is staging), sort, tag

**Membership record**:
The persisted claim that a changelist owns a hunk: file path, line
coordinates, owning changelist, and a content anchor.
_Avoid_: tag, assignment entry

**Content anchor**:
The verbatim hunk lines (plus three context lines each side) stored in a
membership record — the identity evidence hunks are matched against on
refresh. The width is fixed, not incidental: see ADR 0001.
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

**Baseline HEAD**:
The commit whose tree membership-record coordinates address, stamped in
the state file at each persisting refresh. gitchange's own commit
advances it while commuting its records; any other HEAD move triggers
per-path visible dormancy instead of overlap inheritance.
_Avoid_: stored OID, HEAD anchor, snapshot commit

**State file**:
The single JSON sidecar (`$GIT_DIR/gitchange/state.json`, per-worktree)
holding changelists, the active marker, and membership records. Plain text,
atomic rename, lockfile-guarded, schema-versioned. Strictly local.
_Avoid_: database, metadata store

**Staging set**:
The four tokens marking how much of a change is staged: `●` fully staged,
`◐` partially staged, `○` unstaged, `◑` staged-stale. Each names a
staged-ness, not a level — `●` means the same thing on a hunk row and on a
file row. The level decides only which are reachable: `◐` is per-file
alone (a hunk is atomic), `◑` per-hunk alone (it rolls up into `◐`). One
replaceable set, shared by both levels so they cannot spell a shared token
two ways. Severity glyphs are never part of it.
_Avoid_: staging glyphs (as a per-level set), staged icons

**Staged / partially staged / unstaged**:
A file's staged-ness: `●` all its hunks staged, `◐` some of them, `○` none,
with `stagedHunks/totalHunks` counts alongside. Staged-stale hunks count
toward `◐`. Derived from the file's hunks, never stored. `●` and `○` are
the same tokens a hunk carries; only `◐` is exclusively per-file.
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

**Stale action**:
A stage/assign op whose target hunk no longer exists verbatim in the live
tree — the snapshot aged between glance and keypress (ADR 0005's
stale-action race). Validate-at-apply fails it soft: a `StaleHunk`
advisory ("changed since the last refresh"), nothing applied, membership records
untouched, immediate refresh. Distinct from staged-stale `◑` (a per-hunk
staging state) and dormant records (a matching outcome); bare "stale"
outside these three sanctioned senses is ambiguous — say which one.
_Avoid_: race error, conflict, stale (for the other two senses)

**Change core**:
The unpadded range a diff hunk's actual `+`/`-` lines span on the diff's
new side, context skirts excluded; a pure-deletion run has no width and
counts as touching both neighbouring lines. Range-apply staging ops select
hunks by core overlap — context-padded header ranges computed against a
different diff base than the universe's would over-select.
_Avoid_: hunk body, change range

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
that would be in a commit is visible in the TUI. Rename detection is off
(ADR 0011): a rename presents as delete at the old path plus add at the
new, and membership does not follow it.
_Avoid_: worktree diff, fresh diff (alone)

**Keybar**:
The always-visible bottom bar advertising the keys live in the current
context. Honest in one direction: it never shows a key that won't act.
Which of the live keys it lists per context is editorial — the bar has
finite width and makes no claim to be complete; whether a listed key
shows follows its disabled reason. The help overlay is the static
full-keymap reference; the keybar is the live subset.
_Avoid_: status bar, options bar, hint line

**Disabled reason**:
The single answer to "why won't this key act right now" — one value per
capability, consulted by dispatch and keybar alike: the keybar hides a
disabled binding, pressing it logs the reason, an empty reason hides
silently. Covers context guards (what the selection affords), not
content no-ops, which keep their press-time explanations.
_Avoid_: guard (alone), greyed out, disabled flag

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

**Advisory**:
Core's record of an automatic decision or fail-soft outcome worth
spot-checking (`Advisory` in `matcher.rs`): auto-capture, ambiguous
overlap, dormant revival, stale hunk, HEAD-move dormancy. Carried as data
on snapshots and op results with one canonical message
(`Advisory::message`, ADR 0006); frontends add channel dressing and
assign the severity — today every advisory logs at `!` notice, but the
severity is the presentation layer's, which is why the type is not named
after one. "Notice" is the severity level only.
_Avoid_: notice (the type — that's a severity), warning

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

**Apply-correctness corpus**:
The data-driven test suite in core covering hunk-apply edge cases (no
trailing newline, CRLF, adjacent hunks, mode changes, …): each case states
base content, edit, hunk selection, and expected result. A v0.1 exit
criterion; also the certification suite any future `GitBackend` adapter
must pass.
_Avoid_: edge-case tests, apply tests (alone)

**Tripwire**:
An error mapping whose firing is the evidence a held decision waits on —
the report, not the recovery, is the variant's point. Sole instance
today: the *apply tripwire*, `ApplyFailed` mapped from both libgit2
apply calls (staging's write-through to the index, commit's payload
apply while the temp index is assembled), whose verbatim detail is the
trigger for ADR 0003's held shell-out fallback.
_Avoid_: sensor, canary

**Operation guard**:
Commit disabled while any git operation is in progress (merge, rebase,
cherry-pick, revert, am) — the next commit would conclude that operation
with one changelist's content. One predicate, one pin; the keybar hides
`c` while the guard holds and pressing it logs why. Applies to commit
only; staging is never operation-guarded.
_Avoid_: merge lock, commit block
