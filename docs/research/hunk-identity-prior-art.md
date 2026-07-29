# Hunk identity & drift — prior art

Research for wayfinder-brief §6 ("Hunk identity & drift"). Question: a hunk has
no stable git identity — as the working tree changes, hunk boundaries move,
merge, and split. How do existing tools keep per-hunk (or per-patch) membership
stable across edits?

All claims below were checked against primary sources (source code / official
docs), cited inline. Researched 2026-07-29.

---

## 1. IntelliJ IDEA changelists — live range tracking with markers

**Sources:**
- [`platform/vcs-impl/src/com/intellij/openapi/vcs/ex/PartialLocalLineStatusTracker.kt`](https://github.com/JetBrains/intellij-community/blob/master/platform/vcs-impl/src/com/intellij/openapi/vcs/ex/PartialLocalLineStatusTracker.kt) (JetBrains/intellij-community)
- [`platform/vcs-impl/src/com/intellij/openapi/vcs/impl/LineStatusTrackerManager.kt`](https://github.com/JetBrains/intellij-community/blob/master/platform/vcs-impl/src/com/intellij/openapi/vcs/impl/LineStatusTrackerManager.kt)

**Identity.** No hunk ID at all. A `DocumentTracker.Block` (a contiguous
changed region vs the VCS baseline) carries a mutable
`ChangeListBlockData.marker: ChangeListMarker?` — the changelist ID stamped
directly on the range. The public `LocalRange` exposes `changelistId: String`
and `excludedFromCommit: RangeExclusionState` (the latter is per-range partial
staging state).

**Survival across edits.** IntelliJ owns the editor, so it never has to
*re-identify* a hunk — it *watches* every document mutation and updates ranges
incrementally:
- On range refresh/split, all resulting blocks inherit the parent's marker and
  exclusion state (`onRangeRefreshed`: `block.marker = marker` for each `after`
  block).
- On pure shifts, `after.marker = before.marker`.
- Trackers are installed when an editor opens or when a document in a
  non-default changelist is modified (`LineStatusTrackerManager`:
  `requestTrackerFor` / `installTracker`, `MyDocumentListener.documentChanged`).

**Ambiguity.** `mergeRanges(block1, block2, merged)` only merges blocks when
`block1.marker == block2.marker && block1.excludedFromCommit ==
block2.excludedFromCommit`; otherwise it returns `false` and the tracker keeps
the ranges separate. When a *new* edit spans ranges of multiple changelists,
the merged range gets `after.marker = defaultMarker` (the active changelist)
and the user is warned via `lstManager.notifyInactiveRangesDamaged(virtualFile)`.
So ambiguity resolves to: reassign to active changelist + notify.

**Persistence / cost.** Range state is serialized as
`RangeState(range, changelistId, excludedFromCommit)`; `FullState` additionally
snapshots `vcsContent` and `currentContent` — i.e. IntelliJ stores *whole
document snapshots* so it can rebuild trackers exactly. Persisted via
`PartialLineStatusTrackerManagerState.saveCurrentState / restoreState` (IDE
restart survival), plus `fileStatesAwaitingRefresh` for trackers released
before content loads. Cost: a resident change-tracking layer per open document,
document snapshots on disk, and — critically — **edits made while the IDE is
not running are invisible to it**; the model depends on observing every
keystroke.

---

## 2. jj (Jujutsu) — changes are commits; identity is a change ID

**Sources:**
- [Working copy docs](https://docs.jj-vcs.dev/latest/working-copy/)
- [Glossary](https://docs.jj-vcs.dev/latest/glossary/)
- [CLI reference: `jj absorb`, `jj split`](https://docs.jj-vcs.dev/latest/cli-reference/#jj-absorb)

**Identity.** jj sidesteps hunk identity entirely by making every tracked unit
a commit. "A change is a commit as it evolves over time. Changes themselves
don't exist as an object in the data model; only the change ID does"
(glossary). "Rewriting a commit results in a new commit, and thus a new commit
ID, but the change ID generally remains the same." The working copy itself is
a commit: "There is one working-copy commit per workspace."

**Survival across edits.** Every `jj` command first snapshots the working copy
("Snapshot the working copy (which gets recorded as an operation)") — working
tree edits are auto-amended into the working-copy commit, which keeps its
change ID. There is no per-hunk membership state to drift: content lives in
tree objects, and identity lives one level up (the change).

**Splitting / moving hunks.** `jj split --interactive` lets the user choose
hunks via a diff editor — a **one-time interactive operation**, not persisted
membership. `jj absorb` "splits changes in the source revision and moves each
change to the closest mutable ancestor where the corresponding lines were
modified last" — i.e. attribution by line-history/annotation, recomputed at
invocation time.

**Ambiguity.** `jj absorb`: "If the destination revision cannot be determined
unambiguously, the change will be left in the source revision." Ambiguous →
do nothing, leave in place.

**Cost.** Requires owning the repo model (an operation log, auto-snapshot on
every command, working-copy-as-commit). Concurrent hunks *within one
working-copy* aren't separately grouped — jj's answer to "N groups of
uncommitted changes" is "make N commits and `jj edit`/`jj squash` between
them", which changes the user model rather than tracking hunks.

---

## 3. stgit — patch stack; drift absorbed by explicit refresh

**Source:** [`stg-refresh` man page](https://stacked-git.github.io/man/stg-refresh/) (stacked-git.github.io)

**Identity.** A patch is a *named* ref to a real commit; the name is stable,
the commit is rewritten freely. Same shape as jj's change ID, but human-named
and stack-ordered.

**Survival across edits.** Working-tree drift is **not tracked at all** —
it's unowned until the user runs `stg refresh`, which "include[s] the latest
work tree and index changes in the current patch" by rewriting that patch's
commit. Refreshing a non-top patch (`--patch=<name>`) "first creates a new
temporary patch with your updates, and then merges that patch into the patch
you asked to have refreshed" — attribution into the stack is done by
commit-level merge machinery, not hunk matching.

**Ambiguity.** If the merge into a non-top patch conflicts, "the temporary
patch will be left for you to take care of, for example with `stg squash`".
`--conflicts=allow|disallow` controls whether conflicted pushes are permitted.
`--index` refreshes from staged content only; mixed staged/unstaged state
produces a warning unless `--force`.

**Cost.** Zero background tracking, zero heuristics — but the user must
manually say *which patch owns new edits*, and non-top refresh can conflict.
Everything is ordinary git objects (commits + a stack log), so undo is cheap
(the man page notes refresh is "logged" and undoable in two steps).

---

## 4. git-absorb — attribution by commutation, computed at invocation

**Sources:**
- [`src/commute.rs`](https://github.com/tummychow/git-absorb/blob/master/src/commute.rs) (tummychow/git-absorb)
- [README](https://github.com/tummychow/git-absorb/blob/master/README.md)

**Identity.** None persisted. A hunk is two `Block`s (`added`, `removed`),
each lines + start position, parsed fresh from the staged diff.

**Attribution.** "For each hunk in the index, `git absorb` will check if that
hunk commutes with the last commit, then the one before that, etc." Two hunks
commute (`commute()`) when they don't overlap, adjusting offsets as they swap;
the first commit a hunk *fails* to commute past is "the right parent commit
for this change" (`commute_diff_before()` walks the whole sequence). Overlap →
`None` (no commutation), with one special case: hunks that are both pure-adds
or pure-removes of an identical repeated line (`uniform()`) commute anyway.

**Ambiguity.** "If the hunk commutes with all commits in the range, it means
we have not found a suitable parent commit for this change; a warning is
displayed, and this hunk remains uncommitted in the index." Default search
depth: last 10 commits (`--base` to override).

**Cost.** Pure function of (diff, commit range) at invocation time — no state,
no watcher. The trade: it only ever *classifies*; it can't remember a
classification the content doesn't imply, and it punts on anything ambiguous.

---

## 5. gitui — hunk addressed by hash of its header, matched against a fresh diff

**Source:** [`asyncgit/src/sync/hunks.rs`](https://github.com/gitui-org/gitui/blob/master/asyncgit/src/sync/hunks.rs) (gitui-org/gitui)

**Identity.** Transient. A hunk is addressed by a `u64` hash of its
`HunkHeader` (`hash(&HunkHeader::from(hunk))` — i.e. the old/new start+count
coordinates), passed around as `hunk_hash`.

**Survival.** None intended: `stage_hunk`/`unstage_hunk` recompute the diff
(`get_diff_raw()`) and locate the hunk by comparing header hashes in a
`hunk_callback` / `find_hunk_index()` over `diff.foreach()`. The hash is only
expected to survive between "render diff" and "user presses stage".

**Ambiguity / staleness.** If nothing matches: `stage_hunk` silently applies
zero hunks; `unstage_hunk`/`reset_hunk` return
`Err(Error::Generic("hunk not found"))`. So a stale handle is either a no-op
or an error — never a guess.

**Cost.** Trivial. But instructive negatively: a header-coordinate hash breaks
the moment *any earlier hunk in the file* changes line counts, so it is
unusable as persistent identity.

---

## 6. lazygit — no identity at all; positional selection over a re-parsed diff

**Sources (jesseduffield/lazygit):**
- [`pkg/commands/patch/patch.go`](https://github.com/jesseduffield/lazygit/blob/master/pkg/commands/patch/patch.go)
- [`pkg/commands/patch/transform.go`](https://github.com/jesseduffield/lazygit/blob/master/pkg/commands/patch/transform.go)
- [`pkg/commands/git_commands/patch.go`](https://github.com/jesseduffield/lazygit/blob/master/pkg/commands/git_commands/patch.go)

**Identity.** None. A `Patch` is `header []string` + `hunks []*Hunk`, parsed
fresh; lines are addressed by integer index into the flattened
`Lines()` view (`HunkContainingLine(idx)` etc.). No UUID, no hash.

**Staging mechanics.** `Transform(TransformOpts{IncludedLineIndices, Reverse,
FileNameOverride})` rebuilds a minimal valid patch from selected indices:
unselected deletions become context lines, unselected additions are dropped,
and `transformHunkHeader` recomputes the hunk offsets. The result is written
to a temp file and applied via `git apply` with conditional `--cached` /
`--index` / `--3way` / `--reverse` (`ApplyPatch` → `applyPatchFile`;
`ApplyCustomPatch` uses `--index --3way [--reverse]`).

**Survival / ambiguity.** Not applicable — selection lives only for the
duration of one keypress; every refresh re-diffs and the UI reselects by
position. Ambiguity cannot arise because nothing persists.

**Cost.** Zero. Relevant to gitchange mainly as the **commit-mechanics**
precedent (synthesize partial patch → `git apply --cached`), not as an
identity mechanism.

---

## Summary table

| Tool | Identity of a tracked change | Survives edits by | On ambiguity | Cost |
|---|---|---|---|---|
| IntelliJ | changelist marker on a live line range | observing every document edit; split/shift inherit marker | merged cross-changelist edit → active changelist + user notification | resident tracker, content snapshots persisted, blind to offline edits |
| jj | change ID (stable) over rewritten commits | auto-snapshot amends working-copy commit | `jj absorb`: leave in source | owns the whole repo model; groups = commits, not hunks |
| stgit | patch name → rewritten commit | explicit `stg refresh` merge into patch | temporary patch left for user | no tracking; manual attribution; merge conflicts possible |
| git-absorb | none (computed) | n/a — commutation classifies at invocation | warn + leave staged | stateless; classification only |
| gitui | hash of hunk header (transient) | re-diff + hash match per action | no-op or "hunk not found" error | trivial; hash breaks on any upstream line-count change |
| lazygit | none | n/a — positional, per-keypress | n/a | trivial; good precedent for apply mechanics |

Two families emerge: **watch-and-update** (IntelliJ — identity is maintained,
never recovered) and **recompute-and-classify** (git-absorb, jj absorb —
identity is derived from content/history each time). jj/stgit dissolve the
problem by promoting the unit of membership to a commit with a stable name.

---

## Candidate approaches for gitchange

Not a decision — trade-off inventory for a later human call. All assume the
planned FS watcher (brief §6 "Watching / refresh") exists.

### A. Recompute + content-anchored matching (gitui/absorb family, made persistent)

Persist per-hunk membership records in a `.git` sidecar: file path, hunk
old/new line coordinates, plus a *content anchor* (the hunk's added/removed
lines and some context, or a hash thereof). On every refresh: re-diff, then
match new hunks to stored records — exact content match first, then
overlap/commutation-style positional reasoning (shift records by preceding
hunks' line deltas, as `git-absorb`'s `commute()` does). New unmatched hunks →
active changelist (or unassigned); a new hunk overlapping records from two
changelists → explicit ambiguity rule (unassigned + flag, mirroring IntelliJ's
notify or absorb's warn-and-skip).

- **Pros:** no background process beyond the watcher; works when edits happen
  outside the tool (editor, scripts); storage is small; degradation is
  legible (worst case a hunk falls to unassigned, never silently to the wrong
  list).
- **Cons:** heuristic — heavy edits (reflow, big deletes) will shed
  membership; matcher is the hardest code in the app; needs care to stay
  deterministic. Don't hash header coordinates alone (gitui shows why).

### B. IntelliJ-style range tracking against a stored baseline

Store per file the baseline blob (HEAD content at track time) and ranges with
changelist markers, like `RangeState`/`FullState`. On refresh, diff *previous
snapshot → current content* (not baseline → current) and map ranges through
that edit script: shifts preserve markers, splits inherit, cross-changelist
merges go to active + warn — IntelliJ's exact rules, applied per-refresh
instead of per-keystroke.

- **Pros:** closest to the confirmed IntelliJ UX; principled rules for
  split/merge/shift already proven; snapshots make behavior reproducible.
- **Cons:** must store content snapshots per tracked file (IntelliJ stores
  full `vcsContent`+`currentContent`); a large edit between refreshes
  collapses to one opaque range spanning many owners (same failure as A, but
  with more machinery); most complex option.

### C. Materialize changelists as real git objects (jj/stgit family)

Each changelist is a hidden commit (or stack of commits) on a private ref;
membership = content of that commit. New working-tree drift is periodically
absorbed into the *active* changelist's commit (jj-style auto-snapshot);
moving a hunk = rewriting two commits (commutation / `stg refresh --patch`
style merge); identity = the ref/change-ID, which git keeps stable for free.

- **Pros:** hunk-identity problem disappears — git's object model carries it;
  persistence and branch-switch behavior (brief §6 "Persistence model") come
  along free; commit mechanics become trivial (the commit already exists).
- **Cons:** working tree must remain the merge of all changelists — moving
  hunks between commits can conflict (stgit's temporary-patch failure mode);
  continuous history rewriting on private refs; reconciling external `git
  add`/`git commit` is harder; effectively re-implements a slice of jj.

### D. No persistent hunk identity — per-file membership + attribution on demand

Persist membership only per *file* (cheap, stable identity). Per-hunk
assignment is recomputed each refresh: hunks in a multi-changelist file are
attributed by an absorb-style classifier against each changelist's last-known
patch, and anything ambiguous is presented as unassigned for one-keypress
re-filing (the `m` flow already exists).

- **Pros:** simplest honest option; never lies about identity it doesn't
  have; ships fast and the UX already has an "unassigned" pressure valve.
- **Cons:** weakens the confirmed per-hunk requirement (brief §2) to
  "per-hunk, best-effort"; frequent re-filing in hot files would be
  irritating; likely only acceptable as a v1 stepping stone toward A or B.

**Orthogonal note:** whichever is chosen, lazygit's `Transform` →
`git apply --cached` pipeline is the proven mechanic for the *commit* side
(brief §6 "Commit mechanics"), independent of how identity is stored.
