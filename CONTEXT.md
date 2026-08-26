# gitchange

A lazygit-inspired TUI for organising uncommitted local changes into named
changelists that can be staged and committed independently.

## Conventions

**Git-feel CLI vocabulary**:
Where a gitchange concept has a git analog, borrow git's vocabulary for
what users type — commands, flags, args (e.g. `switch` sets the active
changelist, as `git switch` sets the current branch). Governs naming only;
internals like exit-code schemes follow their own conventions. Short
flags follow the borrow: a flag whose git spelling has a short takes it
wholesale with the borrowed grammar; a short for a gitchange-native flag
must earn its place through user demand or observed agent mistakes.

**Op parity**:
Every core op the TUI can invoke has a CLI form; the design question
for a CLI surface is always addressing — how it names a hunk, a
changelist, a scope — never whether the op is exposed. Interaction
patterns are exempt: the TUI's three-step scope escalation is
ergonomics, not an op, and the CLI's native form for a scope is an
explicit set of arguments. Parity is measured against core's ops, not
TUI reachability: a frontend may exceed the other's reach where core
already makes the write — parity forbids gaps, never surplus.

## Language

**Changelist**:
A named set of uncommitted changes. User-created; any number may exist;
created, renamed, and deleted at will.
_Avoid_: group, set, patch, stack

**Active changelist**:
The single target (marked `*`) that captures new hunks and ambiguous
edits automatically. Exactly one of {the changelists, **unassigned**} is
active. Unassigned active is capture-off: capture and ambiguous-edit
routing flow there instead (ADR 0015). Only `switch` moves the marker —
creating a changelist never does, and deleting the active one leaves
unassigned active.
_Avoid_: current, selected, default

**Changelist roster**:
Every changelist in user order plus which one holds the marker — the
changelist set on its own, carrying nothing from the change universe.
What the CLI's bare `changelist` listing reads, and the only question
answerable without a refresh of either form; a **snapshot** carries the
same facts alongside the universe.
_Avoid_: list, registry, index

**Unassigned**:
The pseudo-changelist holding every hunk with no membership record —
that absence is the one membership test (ADR 0016). Holds the dirty
tree before any changelist exists and, while it is the **active
changelist** (capture off), whatever capture routes there; with a
changelist active, hunks released to unassigned are recaptured on the
next refresh. Rendered as a warning state. A switch target like any
changelist. `unassigned` is a reserved name — user changelists cannot
take it.
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

**File row**:
One row of a file listing — the TUI's Files panel, or either face of the
CLI's `status`: a (group, path) pair, where the group is a changelist,
unassigned, or Conflicts. Not a file — the same path under two changelists
is two rows and two independent targets. A Files panel row's marker and
counts read only the hunks its group owns in the file (issue #97), which is
why two rows of one path routinely disagree there; a `status` row's stage
and counts are whole-file facts, identical on every row of one path, since
per-group narrowing belongs to the panel where `space` acts on the row and
group-scoped hunk detail is `diff --json`'s (#143). A Conflicts row owns no
hunks at all, so `space` and assign politely refuse on it (ADR 0007).
_Avoid_: file entry (that's the type), cell, file (when the row is meant)

**Whole-file hunk**:
The degenerate hunk presented by a change with no line-addressable
content: a changed binary file (ADR 0009), an empty file added or an
empty file deleted (ADR 0017). Usually a file's only hunk, though a mode
hunk sits beside it when the file was chmod'd too. One membership record
spanning the file, anchored by a blob-OID pair instead of verbatim
lines. Follows normal assign rules; keeps membership by path continuity;
`◐` unreachable, `◑` derives by OID and object-kind compare —
permission bits belong to the mode hunk. Hunk-mode entry is a polite
no-op where it is the file's only hunk. Assigns as one index-entry unit
with the content hunks sharing that entry (ADR 0009).
_Avoid_: binary hunk, file-level membership (as a general mode)

**Index-entry unit**:
A whole-file hunk and the content hunks beside it, which assign as one
unit because they share one index entry and a whole-file payload commits
that entry whole (ADR 0009, issue #106). Membership ignores staging, and
the mode hunk is never in it. A hunk landing in a unit that already has an
owner joins that owner rather than the active changelist — capture
redirected, never turned on, so capture-off still claims nothing. Its
**holders** are the owners of the hunks the entry actually holds,
unassigned counting as one; more than one holder is the split ADR 0004's
commit refusal names and blocks.
_Avoid_: entry group, binary unit, holder (of a hunk — a hunk has an owner)

**Mode hunk**:
The stand-alone hunk a mode delta (100644 ↔ 100755) presents — always,
beside whatever content hunks exist, independently stageable (ADR 0017,
issue #101). Pairs across the two diffs and derives `○●◑` by mode-bit
compare; stage writes the index entry's mode keeping its blob; identity
matches on path continuity alone. No stage op carries a mode as a rider,
and an added or deleted file carries no mode hunk — its mode is part of
the add/delete whole. Renders first among the file's hunks as a
selectable placeholder row: `Mode changed (100644 → 100755)`.
_Avoid_: chmod hunk, mode rider, ride-along

**Zero-hunk change**:
A change git reports with no text hunks: a mode-only change, an empty
file added, an empty file deleted. Presents one degenerate hunk — a mode
hunk for the mode-only case, a whole-file hunk otherwise (ADR 0017) —
every non-conflicted change in the universe carries at least one hunk,
so nothing falls out of membership, staging, or commit.
_Avoid_: hunkless change, invisible change

**Assign**:
Placing hunks under a changelist's ownership by hand — the manual
counterpart to the active changelist's automatic capture. The target is
any changelist or `unassigned`; as a target, `unassigned` means release
(ADR 0016): the hunks' records are deleted, and unless capture is off
the next refresh captures them into the active changelist. In the TUI,
scope escalates in three steps: the selected hunk, a file's unassigned
hunks, all of a file's hunks including those owned by other changelists.
The CLI states its scope instead (§Op parity): a path is a **membership
sweep** and the third step is the named `--take-owned`, so taking another
changelist's hunks is always said out loud.
_Avoid_: move, add (git's `add` is staging), sort, tag

**Foreign**:
Belonging to a holder or actor other than the one in scope — an
adjective, never a condition of its own. Foreign *content* is another
holder's staged content sharing an index entry with a commit payload,
which the commit refuses (ADR 0004); a foreign *head* is a HEAD that is
not the named changelist's own recorded commit, the CLI amend guard's
condition (#51); the diff view dims foreign hunks — hunks the viewed
changelist does not own. One meaning across all three; coin no "foreign"
for a condition that is not about another actor's work.
_Avoid_: alien; external (an external op is one made outside gitchange,
whoever made it — foreign is about who, not where)

**Membership record**:
The persisted claim that a changelist owns a hunk: file path, line
coordinates, owning changelist, and a content anchor.
_Avoid_: tag, assignment entry

**Content anchor**:
The verbatim hunk lines (plus three context lines each side) stored in a
membership record — the identity evidence hunks are matched against on
refresh. The width is fixed, not incidental: see ADR 0001.
_Avoid_: hunk hash, fingerprint

**Hunk ID**:
The snapshot-scoped address frontends use to name one hunk — a hash of
the file path plus content anchor (a degenerate hunk hashes its kind in
place of the anchor, keeping a file's mode and whole-file hunks
distinct), printed with an `h` sigil so it reads as neither a commit
nor a blob OID. An address, not identity: identity
stays with membership records and matching (ADR 0001), so an ID from an
aged snapshot fails loud as not-found. Identical hunks in a file share a
base ID, told apart by an ordinal suffix (`/0`, `/1`). Composed with its
path as `<path>:<id>[/<n>]` — the address every hunk-addressing verb
speaks.
_Avoid_: hunk hash (that names the anchor mistake), hunk ref, durable ID

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
holding changelists, the active marker, membership records, and the last
gitchange commit record (`{ oid, changelist }`, the amend guard's
reference — ADR 0004). Plain text, atomic rename, lockfile-guarded,
schema-versioned. Strictly local.
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
toward `◐`. Derived from hunks, never stored — a **file row** from the
hunks its group owns in the file (issue #97), file-level surfaces from all
of the file's hunks. `●` and `○` are the same tokens a hunk carries; only
`◐` is exclusively per-file.
_Avoid_: added, indexed

**Derived staged state**:
Staged-ness is never persisted; it is recomputed at every refresh by
matching diff(HEAD↔index) against membership records. Staging writes
through to the live git index (`space` = real apply-to-index), which is the
single source of truth. External `git add`/`git reset` are absorbed at
refresh, never errors.
_Avoid_: staged bit, staged flag

**Stage toggle**:
`space`, at whatever scope the focused panel selects: the selected hunk in
hunk mode, the selected **file row**'s owned hunks in the Files panel,
every hunk of the selected changelist (or of `unassigned`) in the
Changelists panel. One key and one decide-by-current-state rule,
hunk-granular at every scope — anything `○` or `◑` stages, and only a
fully `●` selection unstages. gitchange has no
whole-file stage: `git add` is that op, and is absorbed at refresh. `all`
is a view, so it is not a staging target.
_Avoid_: stage key and unstage key (as two TUI keys — a presentation
rule about keybindings, not ops; the CLI's explicit-direction `add` and
`unstage` verbs are separate ops by design), whole-file stage

**Sweep**:
An op over every hunk in a scope rather than one named hunk. A **staging
sweep**'s scope is a whole changelist (or `unassigned`), or that changelist
narrowed to some of its **file rows**, and it is always ownership-scoped —
it moves only hunks the named changelist owns, which is why the staging
verbs need no cross-ownership override. A **membership sweep**'s scope is a
path: `assign <path>` takes that path's whole universe whoever owns it, so
the taking is guarded rather than scoped — default-on, and the override is
the named `--take-owned` (ADR 0015). Direction is the caller's, not the
current state's: the stage direction takes `○` and `◑`, the unstage
direction takes `●` only — and names each `◑` it kept on the receipt,
with both ways to move it, so the residue is never silent. A scope already
in the target state is satisfied rather than refused, and a
**stale action** discovered at apply fails soft per hunk with the skips
counted on the receipt. Both frontends sweep — the **stage toggle** at
Files-panel and Changelists-panel scope, `add`/`unstage` at the same two
plus a multi-row argument list. An addressed hunk is the other thing, and
the contrast is load-bearing: an address has already decided which hunk
moves, so it moves ungated by the direction's filter — which is how an
addressed `◑` unstages where a sweep keeps it.
_Avoid_: batch, stage-all (that is the commit flow's own offer — it takes
`○` only, so it is a narrower op, not a sweep); and "sweep" for record
pruning — ADR 0016's unknown-name prune is not one

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
snapshotted by a synchronous refresh — the TUI's at confirm time, the
CLI's inside the one invocation, which is also the snapshot its payload
guards read; the amend guard alone reads a fact no snapshot carries, the
state file's last-commit record.
Staged-stale hunks never enter it silently, and an empty payload is
never filled silently; each frontend answers both in its own register
(ADR 0015): the TUI warns and confirms, and offers stage-all-and-commit,
while the CLI refuses and names the override (`--allow-staged-stale`) or
the op (`add <changelist>`).
_Avoid_: commit set, selection

**Refresh**:
The single recompute pass: status + diff(HEAD↔index) + diff(HEAD↔worktree)
→ membership matching → persist → one snapshot. Persisting (the engine's,
the one inside each mutating op, and the CLI's `refresh`, which asks for
one by name) or read-only (a read whose answer
reads the change universe): a read-only refresh writes nothing, decides
nothing, and advises nothing — a recordless hunk reports as unassigned
(ADR 0005). Atomic and last-request-wins; triggered by the debounced
watcher, gitchange's own mutations, terminal focus, the TUI's manual key,
or the CLI's `refresh`. Nothing updates incrementally between refreshes.
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
spot-checking (`Advisory` in `matcher.rs`): auto-capture, entry-unit
capture, ambiguous overlap, dormant revival, stale hunk, HEAD-move
dormancy, active-changelist delete, kept staged-stale. Carried as data
beside a persisting refresh's snapshot and on op results — never on the
snapshot itself, which is how the read-only form's filter is structural
(ADR 0005) — with one canonical message
(`Advisory::message`, ADR 0006); frontends add channel dressing and
assign the severity — today every advisory logs at `!` notice, but the
severity is the presentation layer's, which is why the type is not named
after one. "Notice" is the severity level only.
_Avoid_: notice (the type — that's a severity), warning

**Receipt**:
A mutating CLI command's one-time output: the op's echo on stdout, each
advisory as a `notice:` line on stderr (#51). The sole delivery of the
decisions the op made — its persisting refresh's, or, for a bare state
write, the ones the write itself produced — nothing replays a discarded
receipt; the records keep the facts, the receipt carries the narrative.
Not a parsing contract: agents act on exit codes and re-read state
through the JSON reads.
_Avoid_: response, report, output (for this surface)

**Escalation ladder**:
The agent workflow spine: `assign <path> --to <changelist>` → refusal
names the owner → retry with `--containing <line>` → refusal lists
candidate hunk IDs or names the owner → scoped `diff --json`, or
escalate to the human. Each refusal's text is the instruction for the
next rung; the happy path is one command and zero reads.
_Avoid_: retry loop, fallback chain, error recovery

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

**Sandbox**:
A named, persistent fake repo under `.sandbox/<scenario>/` holding one
state worth eyeballing in the TUI, reached only through core's real ops. A
development artifact, never asserted against: the *scenario* — the code
that builds it — is the definition, and the repo on disk is only its
latest output, so a rebuild is the way back to a known state. Distinct
from a **fixture** (`RepoFixture`), the throwaway repo a test builds for
itself.
_Avoid_: fixture, playground, demo repo

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

**Records guard**:
Deleting a changelist that holds any **membership record**, live or
dormant, is refused rather than done quietly: deletion prunes the records
(ADR 0016) and leaves the hunks recordless, and released hunks do not
rest — the next persisting **refresh** claims them, so one actor's work
can land under another's name. An empty changelist deletes ungated. The
guard has one named override per surface: the CLI's `-f`/`-D`, the TUI's
delete-confirm (ADR 0015's parity). Live and dormant records are counted
apart because they carry different stakes: live records hold hunks the
deletion releases, dormant ones hold a **revival** it ends. The CLI's
refusal and its forced-release notice name the counts and the claim
mechanism and never a destination (a **forecast** an intervening `switch`
would contradict); the TUI's confirm does name the destination, which it
can, being answered and acted on in one moment (ADR 0016).
_Avoid_: delete confirm, safety check
