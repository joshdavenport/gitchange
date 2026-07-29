# Presentation: three channels, a Log panel, and git-state guards

gitchange surfaces everything demanding user attention through **exactly
three channels**, in escalating weight: **persistent panel state** (styling
on rows that are *in* a state), the **Log panel** (a new panel at the
bottom of the right-hand pane collection, lazygit Command Log-style), and
**modal dialogs** (decisions and hard failures). There is **no message
line and no toast overlay** — a bottom line fights the keybar for width
and either wastes a row or shifts layout; the Log panel pays neither cost
and buys history for free.

## The Log panel

- **One chronological stream, two record kinds**: executed git operations
  (the lazygit-style transparency echo — gitchange's index surgery earns
  trust by showing its work) and event notices.
- **Three severities**, each with its own tokenised glyph, distinct from
  the staging set (`○●◑` mean staging states and nothing else):
  `·` **info** (dim — command echo, routine events), `!` **notice**
  (warning tint — automatic membership decisions the user should be able
  to spot-check), `✗` **error** (also modalled; logged for the record).
- **Pinned region at top, for conditions**: the log body records
  **events** (things that happened — immutable, scroll away); pins surface
  **conditions** (states that currently hold). A pin appears when its
  condition starts and **self-clears** when it ends; pins are never
  manually dismissable — a dismissable pin stops being trustworthy and
  needs dismissed-state bookkeeping. Known pins: watcher degraded
  ("watcher unavailable — polling"), git operation in progress
  ("rebase in progress — 2 conflicted"), detached HEAD.
- **No unseen-notice indicator**: the log offers no resolution path, so an
  attention cue on the panel would be a nag. ADR 0001's mandated
  auto-capture notification is satisfied by the notice line itself — which
  holds because the Log panel is permanently on screen. A future layout
  that collapses the panel must revisit this.

## Severity mapping

- **notice**: ambiguous-hunk auto-capture (ADR 0001), dormant-record
  revival ("restored 3 hunks to api-refactor") — automatic membership
  decisions. Hunks that return *changed* don't revive (exact-match only)
  and auto-capture instead, which emits its own notice — the failure mode
  needs no extra machinery.
- **info**: going-dormant, 14-day prune, soft no-ops (`c` on the all
  view, `space` on a conflicted file), watcher recovery.
- **error**: anything that produced an error modal.

## The error-modal contract

One pattern for every hard failure (hook rejection, apply failure,
lockfile contention, backend errors): title names the operation ("Commit
failed"), body is the **detail verbatim and scrollable** — hook stderr is
the user's own tooling talking to them; truncating it would be hostile.
Internal errors show our message, never a `git2` debug string (ADR 0006).
Dismiss with `esc`/`enter`; no inline actions in v0.1 — resolution
happens in the world, not the dialog. Every modal is also logged at `✗`
so the record survives dismissal.

**One exception: hook rejection returns to the commit dialog with the
message intact.** Losing a composed message to a linter complaint is
rage-inducing; retry is `enter` again, or `ctrl+n` for `--no-verify`,
both already in the dialog (ticket 10). Lockfile contention stays a plain
modal ("another gitchange holds the lock") — no retry loop in v0.1.

## Conflicts: quarantine, not participation

Refresh runs during merges whether planned for or not — the watcher fires
on every conflict-resolution save — so the choice is only what the
matcher and panels do with unmerged paths. gitchange **acknowledges
merges, guards its own state, and points the user elsewhere**:

- **Unmerged paths are quarantined per-file**: excluded from the hunk
  universe, listed live in a **Conflicts group rendered first** in the
  file panels (error-tinted, own tokenised glyph), regardless of what
  membership records claim. Everything else stays fully live and
  interactive; the Conflicts group shrinks in real time as files are
  resolved.
- **Their membership records freeze** — no matching, no dormancy clock.
  Letting them go dormant would strand the user's sorting: dormant
  revival is exact-match only, and post-resolution content rarely matches
  pre-merge content. Frozen records re-enter **normal** matching (exact
  *then overlap*) when the file leaves the unmerged state — overlap
  inheritance survives merge-shifted hunks. A conflict is a suspension,
  not a disappearance.
- **No conflict diff rendering**: selecting a conflicted file shows a
  one-line placeholder ("conflicted — resolve outside gitchange").
  Combined-diff formats and three index stages are a workflow gitchange
  doesn't serve. `space` on conflicted content politely refuses (info).

## Git-state guards

- **Commit is globally guarded while any git operation is in progress** —
  merge, rebase (both flavours), cherry-pick, revert, `git am`. Each
  means "the next commit concludes this operation"; our shelled-out
  `git commit` would conclude it with one changelist's content (e.g. mint
  a merge commit whose tree is a partial payload). One predicate (git
  status exposes the state), one pin naming the operation, `c` is a no-op
  with an info line ("merge in progress — conclude or abort it first").
- **Staging is never guarded beyond unmerged paths** — its worst case in
  every state is "git refuses", which lands in the existing error path.
- **Detached HEAD: pin, no guard.** Committing detached is legitimate;
  the pin notes the commit belongs to no branch. Blocking would be
  nannying — git itself allows it and warns on switch-away.
- **Unborn branch (fresh `git init`): commit allowed** — it's the initial
  commit. Implementation diffs against the empty tree instead of HEAD.

## Considered options

- **Transient bottom message line (gitui-style)** — rejected: the keybar
  owns the bottom row; a second line is either an always-empty gap or a
  layout shift, and "latest wins" evicts unread messages when one refresh
  emits several notices.
- **Toast overlays** — rejected: competes with the message line for the
  same job; one channel per weight, no overlap.
- **Notices-only Log panel (no command echo)** — rejected: the git-op
  echo earns trust exactly where gitchange does its scariest work, and
  one stream avoids a fourth channel.
- **Unseen-notice tint on the Log panel border** — rejected: no
  resolution path means it's a nag, not an affordance.
- **Manually dismissable pins** — rejected: an indicator you can dismiss
  while the condition persists is untrustworthy, and it drags dismissal
  state into the engine.
- **Modal for auto-capture** — rejected: refresh-triggered modals
  interrupting typing is the failure mode that makes tools feel hostile.
- **Rendering conflict diffs (even read-only)** — rejected: combined
  diffs and index stages are a can of worms for a workflow explicitly
  out of scope.
- **Letting conflicted files' records go dormant** — rejected: silent
  data loss of the user's sorting (exact-match revival won't fire on
  post-merge content).
- **Blocking commit on detached HEAD** — rejected: legitimate git use;
  a pin informs without nannying.

## Consequences

- The Log panel is a new layout element the TUI prototype doesn't show —
  prototyped separately (pins, severities, conflicts group) so
  implementing agents aren't inventing layout.
- The engine's event vocabulary gains condition-started/condition-ended
  alongside one-shot events (ADR 0006's degradation events generalise to
  conditions); the TUI renders the live condition set as pins.
- Severity is fixed at three levels; new event classes must map onto
  info/notice/error rather than grow the scale.
- The commit dialog must be restorable after a failed commit (message
  preserved) — commit flow keeps pre-confirm state until success.
