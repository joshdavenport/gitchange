# Zero-hunk changes: whole-file hunks; mode deltas: mode hunks

A change with no line-addressable content presents a degenerate hunk, so
that **every change in the universe carries at least one hunk**
(conflicted files excepted — quarantined per ADR 0007). No change can
fall out of membership, staging, or commit for lack of a hunk. ADR 0015's
"nothing broadens a payload silently" gains its mirror: nothing narrows
one silently either. Two shapes carry the invariant:

- An empty file added (tracked or untracked) or deleted presents **one
  degenerate whole-file hunk**, extending ADR 0009's binary treatment.
- A mode delta (100644 ↔ 100755) presents **one stand-alone mode hunk**,
  always — beside whatever content hunks exist — independently stageable
  (issue #101, adopting the option this ADR first rejected).

Prior art converges here: git's own `add -p` promotes the mode header to a
synthetic hunk (`PROMPT_MODE_CHANGE`) offered independently of content,
magit renders an independently stageable `'(chmod)` hunk section, and
scm-record/jj — after shipping this exact bug twice (jj#2548, jj#3016) —
made "every mode/existence transition gets a selectable section" a model
invariant. The tools that refused (gitui, lazygit, GitHub Desktop)
silently drop mode changes from partial workflows or carry years-open
bugs about it.

## The mode hunk (issue #101)

- Each diff side's mode delta pairs across the two diffs as content hunks
  do: at most one mode hunk per file, `○` when only the worktree side
  carries the delta, `●` when both sides agree, `◑` staged-stale when a
  staged flip was worktree-reverted (index-only). A diff side's delta is
  never dropped because the other side's hunks survived pairing — the
  gate that created the synthetic hunk only "when the change would
  otherwise have zero hunks" made a worktree mode flip invisible and
  undrivable whenever the content hunks lived on the index side, and made
  the mirror (staged flip, unstaged edit) misread `◑` as `○`.
- Stage writes the index entry's mode and keeps its blob; unstage
  restores HEAD's mode and keeps the blob. Commit carries it as the temp
  index entry's mode, over whatever blob the payload's content hunks
  leave there — a payload of nothing but a mode hunk commits HEAD's blob
  at the staged mode. A mode-only change's hunk *is* its mode hunk — the
  whole-file treatment it previously received collapses into the general
  rule.
- **Permission bits are the mode hunk's alone.** A whole-file hunk
  derives `●` by blob *and object kind* (ADR 0009's OID compare, plus
  the file/symlink/gitlink bits a type change moves): a chmod'd binary
  whose bytes the index holds exactly reads `●` for its content and `○`
  for its mode, rather than one `◑` that names neither.
- **No ride-along.** No stage op carries a mode as a rider: content-hunk
  staging and the binary whole-file index write preserve the index
  entry's existing mode. A rider is what silently clobbers a separately
  staged flip.
- **Boundary:** an added or deleted file carries no separate mode hunk —
  its mode is part of the add/delete whole. This line keeps scm-record's
  `FileMode::Absent` implication rules out of gitchange: a chmod between
  two existing modes participates in no dependency rule (scm-record
  PR #95's rules all guard absence).

## Identity & matching

- A whole-file hunk's identity is `WholeFile` with the blob-OID pair
  anchor (ADR 0009 reused). Empty add and empty delete are discriminated
  by the anchor's sides (HEAD-side absent / changed-side empty blob, and
  the reverse).
- A mode hunk carries a dedicated mode-change identity, matching on
  **path continuity alone** — the strength the binary inheritance tier
  has. The blob-pair anchor cannot serve it: once mode hunks coexist with
  content hunks, the file's blobs belong to the content records, and any
  edit would shift a blob anchor out from under the mode record. A mode
  re-flip at the same path revives a dormant record — intended: same
  predicament, same home.
- Records carry no mode bits. Mode is snapshot data, present for display
  and stage derivation only.

## Presentation

- The mode hunk renders as a selectable placeholder row among the file's
  hunks, positioned first (git's pseudo-hunk #0 position):
  `Mode changed (100644 → 100755)`, octal, as git prints it. It takes
  the same changelist tag, stage glyph, and dimming treatment as a text
  hunk.
- *Amended (issue #104):* a degenerate hunk's placeholder text **is** its
  header. A mode delta and a whole file address no lines, so no surface
  frames one in `@@` coordinates — the diff row and the assign popup's
  subject line included.
- Whole-file placeholders stay on the binary/conflicted channel
  (ADR 0007): `Empty file added`, `Empty file deleted`. *Amended
  (issue #104):* the channel is the wording, not the row. A file whose
  only hunk is degenerate renders that text as its whole diff body, dim
  and unselectable — there is nothing to reach. Where the hunk has
  siblings — a chmod'd binary edit — it renders as a selectable row like
  any other, because its sibling stages separately from it.
- No new glyphs: the mode hunk's row derives `●○◑` as a hunk row does,
  and the file rows counting it derive `●○◐` as file rows do — a
  chmod+edit counts the mode hunk (`0/2` where a bare edit reads `0/1`),
  so a staged flip beside an unstaged edit reads `◐`, never wholly `○`.
  Hunk-mode entry on a lone degenerate hunk — whole-file or mode — stays
  the polite no-op (ADR 0009 idiom).

## Considered options

- **Loud out-of-scope** — row states the change needs raw `git add` and
  will not appear in a gitchange commit. Rejected: the politer cousin of
  ADR 0009's rejected "invisible to gitchange"; it leaves a class of
  change gitchange cannot drive, permanently.
- **A mode-transition identity carrying `old → new` in the record** —
  rejected: recording the modes only buys refusing dormant revival on a
  different flip at the same path, and revival there is right anyway. The
  adopted identity is the path-continuity form, which carries no modes.
- **Ride-along** (this ADR's original choice: synthetic hunk only at zero
  hunks, mode carried by any index-entry write) — superseded
  (issue #101): it dropped a diff side's mode or type delta whenever the
  other side's hunks survived pairing — invisible and undrivable in all
  four corners (mode/type × forward/mirror) — and the rider clobbered
  separately staged flips.
- **Display-only surfacing** of the orphaned delta — rejected
  (issue #101): fails drivability, which is this ADR's own invariant.
- **A scoped mode hunk** (only when the delta would otherwise be
  invisible) — rejected (issue #101): precedent-free; mode as
  sometimes-a-hunk/sometimes-a-rider preserves the failure shape it
  fixes, conditionally.

## Consequences

- ADR 0003's zero-hunk exception ("`space` moves nothing and says so") is
  superseded; its staging-scopes section now reads per this ADR.
- The apply corpus pins the mode-only index write, mode staging beside
  index-only content hunks (both directions), the mode-preserving content
  and binary stages, and empty-tracked-file deletion.
- *Amended (issue #98):* `TypeChanged` (file↔symlink) presents a
  zero-hunk shape — git reports it with mode bits alone, no hunks,
  whatever the file holds — so the invariant takes it in: the same
  whole-file hunk, and the same whole-file index write, which is what
  staging a symlink swap needs anyway. Its diff placeholder reads
  `Type changed (100644 → 120000)`, since calling a symlink swap a mode
  change would misname it. The pairing rule above — no diff side's delta
  is dropped for the other side's content — covers its corner cases
  mechanically; how a type change presents and matches stays with its
  own issue (#100), which also pins those corners.
- Submodule pointer changes stay invisible (`ignore_submodules` on every
  diff) — a standing scope decision, not a zero-hunk file.
- An **embedded repository** — a nested clone or a linked worktree inside
  the tree — is one untracked *directory* delta: trailing-slash path,
  tree mode, no blob and no hunks. It is not a file change, so there is
  nothing to hash for an anchor and no index write gitchange makes; it
  stays out of the universe entirely rather than presenting a whole-file
  hunk. `git add` is the op for it. This keeps the invariant exact:
  every change the universe holds carries a hunk.
