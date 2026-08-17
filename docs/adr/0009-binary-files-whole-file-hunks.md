# Binary files: whole-file degenerate hunks

A changed binary file produces no text hunks, so ADR 0001's per-hunk
membership has nothing to attach to. gitchange treats a changed binary file
as **one degenerate whole-file "hunk"** with a normal membership record,
flowing through the existing assignment rules unchanged: new binary change →
the active changelist, unassigned when unassigned is active, assignable
between changelists like any hunk (as one unit with any content hunks
sharing its index entry — see below). This preserves ADR 0003's visibility invariant
(every hunk that would be in a commit is visible in the TUI) and keeps
binaries first-class — no code path forks on binary-ness at the
staging/commit layer.

## Anchor & matching (amends ADR 0001 for binaries)

- **Anchor = blob-OID pair**: the HEAD-side OID and the changed-side content
  hash (computed `hash-object`-style at refresh). No verbatim content — for
  a binary, identical/not-identical is the only askable question, and a hash
  answers it.
- **Tier 1 (exact)**: changed-side OID matches the record → membership
  holds.
- **Tier 2 (overlap-inheritance analog): path continuity.** Same path,
  still binary-changed, OID differs (re-exported image) → membership holds,
  anchor updates. The whole file *is* the hunk, so any re-edit is an edit of
  your own hunk — "editing your own hunk never sheds membership" translated.
  Consequence: a live binary record only changes changelist by explicit
  move, never by drift; binary identity is stronger than text identity,
  which can shed under heavy rewrite.
- **Dormant revival: exact only** — path *and* changed-side OID must both
  match, mirroring the text rule (revival is never inheritance-based). A
  different binary change at a path with a dormant record is a fresh change
  → active changelist.

## One owner per index entry (issue #106)

One index entry can hold deltas presented as a whole-file hunk *and*
content hunks — the index carries a staged text edit while the worktree
holds a binary rewrite (reachable since the pairing fix, issue #103). A
whole-file payload commits the live entry verbatim, so two owners in one
entry would broaden a commit silently (ADR 0015). Prior art forbids the
state rather than handling it (Perforce: a file belongs to exactly one
pending changelist; scm-record/hg: the binary is one all-or-nothing
toggle — issue #106 research).

- **The whole-file hunk and the content hunks sharing its index entry are
  one assignable unit.** Explicit assignment moves them together; a hunk
  newly joining such an entry is captured into the unit's existing owner
  (including unassigned), not the active changelist.
- **The mode hunk is not part of the unit** — it stays independently
  assignable (ADR 0017); the commit severs the mode from the entry copy,
  taking the changelist's own staged flip or HEAD's mode.
- **A split can still exist** — records predating this rule, an entry
  whose content hunks already held two owners when the whole-file hunk
  arrived (it then captures normally, into the active changelist), or
  external actors. ADR 0004's foreign-content refusal backstops those:
  the split is visible and assignable apart from commit, never
  committable.

## Staging & commit (ADR 0003/0004 unchanged in mechanism)

- `space` on an unstaged binary = **whole-file index write** (plain
  `git add` semantics — no apply machinery, none of the hunk-apply edge
  cases). Unstage = index entry back to HEAD's version, or dropped for a
  newly added file. Write-through to the live index per ADR 0003.
- **`◐` is unreachable, not forbidden**: derivation counts `0/1` or `1/1`
  staged hunks; no special case.
- **`◑` staged-stale fully applies**, derived by OID compare instead of
  line compare — both flavours (index+worktree differ; index-only after
  worktree revert). `space` on `◑` sets index := worktree. ADR 0004's
  warn-and-confirm and temp-index commit apply unchanged; the temp index
  receives the staged blob whole-file.

## Presentation

- Diff panel: **one-line placeholder with sizes** —
  `Binary file changed (12.4 KB → 15.1 KB)`, with added/deleted variants
  showing the single size. Same placeholder convention as the quarantined
  conflicted-diff line (ADR 0007). No hex dumps or previews in v0.1.
- No new glyphs: file rows derive `●○` (and `◑` tint) as normal; the
  placeholder text is what says "binary".
- Hunk-mode entry on a binary selection: **polite no-op** (the established
  idiom) — one degenerate hunk, and every action already works at file
  level. No log event.

## Considered options

- **Always unassigned** — rejected: makes binaries second-class (committing
  a changelist would silently omit its binary or need a special flow) and
  breaks ADR 0001's uniform assignment rule for one file type.
- **Invisible to gitchange** — rejected: a staged binary would enter a
  commit while invisible, violating ADR 0003's visibility invariant.
- **Verbatim-content anchors** — rejected: binary content in `state.json`
  is a non-starter, and unlike text (where verbatim lines give the overlap
  tier real extents), a binary anchor can only ever answer exact-match — a
  hash suffices.
- **Hunk-mode entry showing one whole-file hunk row** — rejected: a state
  that can only waste a keypress to enter and another to leave.
- **Auto-unify a split entry at refresh** — rejected: a forced ownership
  move needs an attribution answer (whose changelist absorbs whose?) that
  ADR 0015's multi-actor rules don't give; the commit-time refusal names
  the holder and leaves the move to the user.

## Consequences

- The matcher gains a binary branch that is *simpler* than text matching:
  OID equality and path continuity, no shifting or overlap extents.
- Hashing large changed binaries at refresh has a cost; it rides under ADR
  0005's stale-interactive posture and the ticket-14 benchmark harness, no
  numeric target (per the performance-posture decision).
- `state.json` records for binaries store OIDs where text records store
  verbatim lines — a record-shape variant, not a schema fork; dormancy and
  the 14-day prune apply unchanged (ADR 0002).
