# Keymap: one binding record, shared disabled-reasons, dispatch stays a match

The keymap was encoded in four hand-maintained places — the dispatch
match, the capability guards inside it, the keybar's per-panel hints, and
the help overlay's flat list — with nothing linking them, so any of the
four could silently drift from the others (issue #53). We decided:

- **One binding record per action** (the *binding core*): identity, key
  spelling(s), capability link, help-register label. Dispatch arms match
  against the core's key values instead of spelling keys inline; both
  display surfaces render spellings from it; core order is help order.
  Multi-key bindings are one record with N spellings (bar shows the
  primary, help shows all). Combined hints (`a/A/ctrl+a`, the panel
  digits) are derived from core records, never hand-concatenated. Modal
  keys stay with their modals, outside the core.
- **Capability guards become disabled-reason predicates** — one value per
  capability answering "why won't this key act right now", consulted by
  dispatch (which logs the reason, ADR 0007's channel) and by the keybar
  (which hides the binding). An empty reason hides silently. This covers
  *context* guards only — what the selection affords: the
  changelist-ops scope, the assign context, commit's operation guard and
  All-view case, and *(issue #90)* `space`'s staging scope, whose panels
  without one hide silently and whose own All-view case reads like
  commit's. *Content* no-ops (hunkless `enter`, empty assign payload,
  file-less `space`, a changelist with nothing to stage) keep their
  press-time behaviour inside the helpers; predicates must stay cheap
  per-frame reads.
- **Dispatch stays a hand-written match.** The core carries no handler
  functions.
- **Bar arms stay editorial.** Which bindings a panel advertises, in what
  order, with what contextual short label, is a hand-picked decision per
  panel — only spellings and liveness are derived. The help overlay is a
  static global cheatsheet, built at runtime (so its glyphs can theme)
  and ignoring live state.

## Why: prior art says the drift is real and locates it precisely

Surveyed gitui and lazygit before deciding (both faced exactly this;
lazygit v0.20 had our exact architecture).

**gitui** (per-component `event()`/`commands()`, nothing linking them)
shipped and re-shipped precisely the bugs this structure invites, over
six years: hints rendering a *different binding's key* than dispatch
matched (`stash_open`, `log_find`, `quit` — three live in HEAD today),
keys live but unadvertised, hints advertised but dead (one open and
maintainer-confirmed since 2023). Two findings steer this ADR: every
long-lived silent bug was a wrong key *spelling* whose display string
named a different binding identity than dispatch matched — killed by a
shared identity that both sides must reference; and where a guard was a
named `&self` method called by both `event()` and `commands()`
(`can_commit`) it never drifted, while every inlined restatement did.

**lazygit** converged, monotonically over seven years and never
backwards, on: one `Binding` record per action; `GetDisabledReason` as a
single closure the dispatcher checks before the handler *and* both
display surfaces consult to render (bar hides, menu strikes through,
pressing toasts the reason); one binding carrying N keys rather than N
bindings deduped later; guards still buried inside handlers treated as
defects to migrate out, because they are invisible to the UI. Their bar
asserts exact strings in integration tests.

We also found the bug class live here: with *All* or *Unassigned*
selected, the bar advertised `d`/`r`/`s` while `scoped_changelist()`
returned `None` and the keys died silently — the same shape as gitui's
oldest open issue. The shared predicate fixes it; the disabled-rendering
policy (hide / log / static help) makes the fix uniform instead of
gitui's three uncoordinated answers.

## Considered options

- **Narrow dedup only** (issue #53's option a) — banks the mechanical
  wins but leaves the dispatch↔display axes open; rejected once the
  capability drift was recognised as the more serious failure.
- **Full table-driven dispatch** (option c: bindings carry handlers) —
  rejected. The drift lives in the display half; both surveyed projects
  demonstrate the *record* must be shared, not the dispatch organisation.
  lazygit still holds unmigrated bindings ten years on and it costs them
  nothing; handler-carrying tables cost enum-or-boxed-closure plumbing
  that only amortises across hundreds of bindings. We have ~16.
- **Pure resolve/apply split** (dispatch queryable by the bar) — the
  structural guarantee is real, but it is option (c)'s shape without its
  table, and it forces every content no-op to be hoisted into the pure
  half at once. The predicates already hoist the two guards that matter.
- **Advertised↔dispatched property test instead of structure** — gitui's
  missing tests, worth writing there; here the core + shared predicates
  make both directions structural, so the test would mostly re-verify
  the compiler. Snapshot tests of exact bar strings per state (lazygit's
  approach) cover what stays editorial.

## Consequences

- The keybar is honest in one direction by construction — ADR 0013's
  principle extended from "never advertise what the terminal can't send"
  to "never advertise what the current context won't act on".
  *Amended (issue #86):* one direction, not both — which of the live
  keys an arm lists stays editorial, because the bar has finite width
  and makes no claim to be complete.
- Behaviour change: `c` disappears from the bar during a git operation
  (the pin still names the operation) and on the All view; `d`/`r`/`s`
  disappear when the scoped row is *All*/*Unassigned*, and pressing them
  logs why instead of dying silently. CONTEXT.md's *Operation guard*
  entry gains this clause.
- The issue #45 hand-written keybar `if` (blurred hunk selection)
  dissolves: the assign group is editorially listed in every arm where
  it can be live, and the predicate hides it elsewhere.
- The two `→` literals in the help overlay become themeable, closing the
  last token-literal carve-out from the ADR 0006 sweep.
- Adding a binding touches: one core record, one dispatch arm, an
  editorial mention per advertising panel. Spelling and liveness cannot
  drift; a forgotten editorial mention is caught by the bar snapshots.
- The keybar remains derived per frame; anything expensive is barred
  from predicates (lazygit's twice-repeated warning).
