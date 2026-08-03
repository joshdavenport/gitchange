# gitchange — implementation brief (prototype handoff)

Input for breaking implementation into digestible chunks. Captures decisions
from the TUI prototype session (2026-07-29) that cannot be inferred from the
prototype file alone. The prototype (`tui-prototype.html`) is the visual
source of truth; this doc is the semantic one. Where they conflict, this doc
wins.

## 1. What this is

**gitchange** is a lazygit-inspired TUI for managing *changelists*: named
groups of uncommitted local changes (per IntelliJ's changelist feature) that
can be organised, staged, and committed independently. Target user is an
existing lazygit user; UX familiarity with lazygit's panel-stack model is a
design requirement, not a nice-to-have.

## 2. Domain model & vocabulary

- **Changelist** — a named set of uncommitted changes. User-created, any
  number, create/delete/rename at will.
- **Active changelist** — exactly one changelist is active at a time
  (marked `*`). New/incoming changes conceptually associate with it.
  Toggled with `a`.
- **Unassigned** — pseudo-changelist holding changes not yet sorted into any
  changelist. Rendered as a warning (`!`, orange). *Naming not final* —
  "unassigned" beat "unmanaged"; "inbox"/"unsorted" were floated and never
  ruled out.
- **All** — pseudo-view (not a changelist) showing every changed file
  grouped by changelist, unassigned group last. This is the launch view.
- **Hunk-level membership** — changelist membership is per-hunk, not just
  per-file (confirmed requirement; IntelliJ supports this but hides it).
  A single file's hunks may belong to different changelists. A file "in" a
  changelist means it has ≥1 hunk owned by that changelist.
- **Staged / unstaged** — per-file markers: `●` fully staged, `◐` partially
  staged, `○` unstaged, with `stagedHunks/totalHunks` counts shown per file.

## 3. Validated UX decisions (and why)

- **Layout: lazygit panel stack** — left column of numbered panels
  ([1] Status, [2] Changelists, [3] Files, [4] Commits), [0] Diff dominating
  the right. Won because of the drill-down interaction (`2` → select
  changelist → `3` → operate on files) and lazygit muscle-memory.
  Rejected: IntelliJ-style single tree panel; Miller columns (but its
  per-hunk tag display was salvaged — see below).
- **Panel 2 ordering (top→bottom):** `all`, then user changelists, then
  `unassigned` pinned last.
- **Launch state:** `all` selected — it forms the overview.
- **Hunk tags in the diff:** every hunk header carries a tag naming its
  owning changelist in the `all` view. In a drilled-in changelist view,
  hunks owned by *other* changelists are still shown but dimmed (~45%
  opacity) with a dim tag; the changelist's own hunks render untagged at
  full strength.
- **Hunk mode:** `enter` on a file focuses the diff panel for per-hunk work.
  Contextual keybar replaces the main one.
- **Move flow:** `m` opens a centered "Move to changelist" popup listing
  changelists (active one annotated), with a "+ create new changelist…"
  escape hatch at the bottom. `enter` move, `esc` cancel.
  _Superseded by ticket 41: the operation is **assign**, the popup is
  "Assign to changelist", and the keys are `a`/`A`/`ctrl+a` (ADR 0013).
  The popup shape below is otherwise unchanged._
- **Keybindings (settled this session; the assign keys were later
  replaced — see ADR 0013 and ticket 41, which win over this list):**
  - `0-4` focus panel · `j/k` move within panel/hunks
  - `n` new changelist · `d` delete · `r` rename · `a` set active
  - `m` move file/hunk to changelist
  - `space` stage file (files panel) / stage hunk (hunk mode)
  - `enter` (files panel) enter hunk mode
  - `enter` (hunk mode) add **all** of the file's hunks to a changelist
  - `shift+enter` (hunk mode) add **selected** hunk to a changelist
  - `c` commit changelist · `esc` back · `?` keybindings
- **Panel title carries context**, e.g. Diff title
  `print.css (2/4 staged · 1 hunk elsewhere)` — staged counts in a
  drilled-in view refer to the *changelist's own* hunks, not the file's
  total (confirmed reading).
- There is no theming so colour scheme should not be opinionated, but lean
  on sensible defaults. Token use should be employed for ease of maintenance.
- Glyph choices (`●◐○`, `*`, `!`, `≡`) are decided, but should also be easily
  replaceable.

## 4. Prototype assumptions — embedded but not explicitly confirmed

Treat these as defaults to challenge, not requirements:

- Staging is a per-changelist step *before* commit (stage subset → `c`
  commits the changelist's staged changes), rather than `c` committing the
  whole changelist atomically.
- Commits panel and Status panel content mirror lazygit; nothing
  changelist-specific was designed for them.

## 5. Deliberately deferred (not designed yet)

To be derived by an agent from existing TUI elements and/or a later
prototype session — consistency with section 3 patterns is the requirement:

- Rename / new-changelist text-input flows
- Commit flow UI (message entry, what `c` shows before committing)
- Delete-changelist confirmation (and where its files go — unassigned?)
- Empty states, error states, conflict presentation

## 6. Open technical questions (likely the spine of the breakdown)

The prototype answered none of these; each is probably its own chunk:

- **Persistence model** — where changelist metadata lives (git refs? notes?
  `.git/` sidecar file? stash-like objects?). Must survive branch switches
  sanely; behaviour across branches needs defining.
- **Hunk identity & drift** — a "hunk" has no stable git identity. As the
  working tree changes, hunk boundaries move/merge/split. How does
  membership track edits? (This is the hardest problem; IntelliJ does it
  with its own local-history tracking.)
- **Interaction with the git index** — there is one index but N changelists
  with independent staged state. Per-changelist staging likely means
  building the index at commit time (`git apply --cached` of owned hunks or
  temp-index via `GIT_INDEX_FILE`), not mirroring live index state.
  External `git add` run outside the tool needs a defined reconciliation.
- **Commit mechanics** — committing one changelist while others' changes
  remain in the working tree; partial-file commits require synthesizing
  blobs from selected hunks (lazygit/`git add -p` precedent).
- **Watching / refresh** — detecting working-tree changes, re-diffing, and
  re-associating hunks without UI jank.
- **TUI stack** — undecided. Prototype is deliberately stack-agnostic HTML.
  (An OpenTUI skill is available locally if that route is chosen; Go +
  bubbletea/gocui would be the lazygit-native choice.)

## 7. References

- Prototype: `tui-prototype.html` (open in browser; `?variant=A|B|C|D`
  cycles launch overview / drill-down / hunk mode / move flow)
- Round history: git log — round 1 (three layouts), round 2 (winner +
  states), round 3 (hunk semantics)
- `renderDiff(hunks, { context, selected })` in the prototype is a first
  sketch of the diff-rendering contract: `context` = drilled changelist or
  `'all'`, drives tag/dim logic
