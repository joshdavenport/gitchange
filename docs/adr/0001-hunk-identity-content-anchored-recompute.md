# Hunk identity: content-anchored records, recomputed at refresh

gitchange must keep per-hunk changelist membership stable while the working
tree is edited by anything — editors, scripts, git itself — including while
gitchange isn't running. We persist a **membership record** per owned hunk
(path, old/new line coordinates, owning changelist, and a verbatim content
anchor: the hunk's added/removed lines plus ~3 context lines each side,
stored as plain text — amended by ADR 0002, which supersedes this ADR's
original "compressed") and re-derive membership on every refresh by matching the fresh
diff against stored records. There is no resident tracking layer: the matcher
is a pure function of (stored records, fresh diff), and records mutate only
via a completed refresh or an explicit user action.

## Matching

Two tiers, in order:

1. **Exact content-anchor match** — catches untouched hunks, including ones
   that moved position.
2. **Overlap inheritance** — remaining records are shifted by preceding
   matched hunks' line deltas (git-absorb-style commutation); a new hunk
   overlapping exactly one changelist's shifted region inherits its
   membership. Splits inherit the parent's changelist. Editing your own hunk
   never sheds membership.

## Assignment rules

- New hunk, no overlapping record → the **active changelist** (uniform rule:
  applies both to hunks appearing live and to hunks discovered at launch
  after offline editing).
- New hunk overlapping records from **two or more** changelists → the
  **active changelist + a notification** (IntelliJ's rule; presentation is
  part of the error/notification presentation work, not decided here).
- No changelists exist (so no active target) → **unassigned**. Unassigned
  therefore holds: the pre-changelist dirty tree, orphans of deleted
  changelists, and explicit moves.

## Considered options

- **IntelliJ-style baseline range tracking** — rejected: its guarantees come
  from observing every keystroke; we don't own the editor, and offline edits
  would be invisible. Adapting it to refresh-time keeps its snapshot storage
  cost without its precision.
- **Changelists as hidden commits (jj/stgit family)** — rejected: dissolves
  hunk identity but makes reconciling external `git add`/`git commit` much
  harder, requires continuous history rewriting on private refs, and
  re-implements a slice of jj.
- **Per-file membership + on-demand attribution** — rejected: weakens the
  confirmed per-hunk requirement to best-effort.
- **Hash-only content anchors** — rejected in favour of verbatim lines: a
  hash can only answer identical/not-identical; verbatim content gives the
  overlap tier real extents, makes stored state human-debuggable, and allows
  a future fuzzy tier without schema migration.

## Consequences

- The matcher is the hardest code in the app, but it is deterministic and
  table-testable: same records + same diff → same membership, no UI or
  watcher needed in tests.
- Heavy rewrites (reflow, large deletes) will still shed membership — by
  design the failure mode is visible (active + notify, or unassigned), never
  a silent wrong-list assignment.
- This ADR fixes the record *shape*; where records physically live (custom
  refs vs sidecar file) is the persistence decision, made separately.
- Refresh scheduling, debounce, and threading are out of scope here; the
  contract they must honour is the purity rule above.
