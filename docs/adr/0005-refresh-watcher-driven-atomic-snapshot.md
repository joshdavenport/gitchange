# Refresh: watcher-driven atomic snapshot recompute

gitchange refreshes by running **one atomic RefreshJob** — status +
diff(HEAD↔index) + diff(HEAD↔worktree) → the ADR 0001 matcher → persist
records → a single `RefreshComplete` notification carrying an immutable
**snapshot** — triggered by a debounced filesystem watcher. The UI only
ever swaps whole snapshots. This **departs from gitui's staged per-kind
notifications** (status, diff, and derived data arriving independently):
gitchange's panels are grouped by changelist, so status without membership
is unrenderable — partial arrival would paint a file list whose grouping is
momentarily wrong, then reshuffle. The matcher needs the full hunk universe
as input anyway (ADR 0001), so staging the jobs buys no meaningful first
paint.

## Detection

- **Watcher**: `notify` + debouncer, **~500ms** debounce (vs gitui's 2s —
  safe because last-request-wins job slots already collapse bursts; the
  debounce only sets how stale a save-then-glance feels). Recursive watch
  of the worktree root **including `.git`** — external `git add`/`reset`/
  `commit`/`checkout` must be absorbed (ADR 0003) and arrive via
  `index`/`HEAD` events.
- **Self-loop filter**: the forwarder drops events whose paths are under
  `$GIT_DIR/gitchange/` or the temp index file (ADR 0004), so our own
  state writes and commits don't re-trigger refresh. Belt-and-braces: the
  state file is not rewritten when records are unchanged.
- **Supplements**: gitchange's own mutations (stage, move, commit) fire the
  RefreshJob directly on completion — never wait on the watcher; terminal
  `FocusGained` triggers a refresh; a manual refresh key exists. No
  periodic git polling in the healthy path — the ticker drives only the
  spinner and the indicator threshold.
- **Degraded mode**: if the watcher fails to initialise or dies, notify
  once ("watcher unavailable — falling back to polling") and refresh on a
  ~5s tick. Focus/key/own-mutation refresh are unaffected. **The mode is
  not terminal**: each tick re-attempts the subscription before running
  its refresh, so a transient cause (watch-limit exhaustion, an atomic
  save replacing the watched root) self-heals within one interval and
  ends the condition. The re-attempt rides the tick rather than a timer
  of its own, and is not backed off — a failing `watch()` is the cheap
  half of a tick that already pays for a full recompute.

## Pipeline & threading

- The RefreshJob lives in an `AsyncSingleJob` **last-request-wins** slot on
  a small thread pool; a watcher burst collapses to one recompute. Results
  return over a crossbeam channel into the **sync main-loop `Select`**
  (input / engine / watcher / tick channels) — tokio ratified out; the
  design ports 1:1 (`tokio::select!` + `spawn_blocking`) if network
  features ever justify it.
- Mutations (stage, move-hunk, commit) are separate **one-shot jobs** that
  trigger a refresh on completion. The refresh-before-commit guard
  (ADR 0004) is unchanged.

## Mid-refresh UI

- Panels keep rendering the last snapshot, **fully interactive**; nothing
  clears or flickers — the only visual change is the atomic swap.
- **Deferred indicator**: no spinner for refreshes under ~500ms (the common
  case); if a tick arrives while the job is still in flight past the
  threshold, an indicator shows until `RefreshComplete`.
- **Selection survives the swap identity-first**: files re-found by path,
  hunks by membership-record match (the matcher output already computed);
  if gone, fall to the nearest sibling by position — never reset to top.
- **Stale-action race**: stage/move act optimistically on the snapshot and
  **validate at apply** against the live tree. A hunk that no longer
  applies cleanly fails soft — notification + immediate refresh, nothing
  half-applied. Commit keeps its stricter refresh-gate (ADR 0004).

## Read-only refresh (amends the pipeline above; issue #51)

A refresh is **persisting** or **read-only**. The pipeline above is the
persisting form: matcher output becomes records in the state file, and the
decisions that refresh makes — capture, ambiguous-overlap routing,
dormancy — surface as advisories on its output, exactly once, because each
decision became a record. The read-only form runs the same recompute —
status, both diffs, matcher against the records as they stand — but
decides nothing: it writes no records, stamps no baseline HEAD, and emits
no advisories. Emitting none is a filter, not a vacancy: the recompute
still produces advisories — revival, ambiguous overlap, and guard dormancy
fire with capture off — and the read-only form discards them as previews
of decisions only a persisting refresh commits and delivers; its return
type carries no advisories, so no frontend can leak them. Ownership in its
snapshot is what the records say: record-derived ownership (overlap
inheritance, dormant revival) shows; context-derived ownership (capture,
entry-unit join) never previews — a recordless hunk reports as unassigned.

Who runs which: the engine's refreshes persist, as do the refreshes inside
every mutating op (assign, stage, commit) — the op's receipt carries the
advisories its refresh produced. Every read-only CLI invocation whose
answer reads the change universe (`status`, `diff`; with or without
`--json`) refreshes read-only: a glance at the tree never moves
membership. The bare `changelist` listing reads the state file alone — the
names and the marker derive from no diff — so it runs neither form,
reaching the same guarantee by doing less; it therefore answers wherever
the state file is readable, including a repository whose universe cannot
be built. The CLI reaches the persisting form only
by name: `gitchange refresh` is the manual refresh key's CLI form — one
deliberate persisting refresh whose receipt carries its advisories. Deferred capture is the accepted cost:
with no TUI running, a new hunk stays unassigned until the next mutation's
persisting refresh captures it, so that mutation's receipt can advise
decisions unrelated to its target.

There is no advisory journal. An advisory rides the output of the refresh
that made the decision, once, to the one actor who triggered it: the facts
survive as records, the narrative does not. A retrospective advisory
surface (an `advisories` command, stored history) is rejected — one shared
cursor recreates the consumed-by-whoever-reads-first hazard, per-reader
cursors give gitchange reader identity, and the data serves nobody:
everything durable an actor needs is readable as records, and everything
urgent is a refusal.

## Considered options

- **Staged per-kind jobs (gitui's shape)** — rejected: changelist grouping
  makes partial arrival visibly wrong (reshuffle mid-refresh), and the
  matcher consumes the full diff anyway, so the latency win is illusory.
- **Polling-only detection** — rejected: burns CPU on idle repos, always
  stale up to the interval; kept only as the watcher-failure fallback.
- **Focus/key-only refresh** — rejected: external edits while gitchange is
  visible (editor in another pane) go unseen; contradicts ADR 0003's
  absorb-external-changes posture.
- **2s debounce (copy gitui)** — rejected: last-request-wins already
  provides burst safety; 2s of lag between save and panel update is the
  jank this decision exists to avoid.
- **Refresh-gating every mutation like commit** — rejected: adds latency
  to the highest-frequency keys for a rare race that apply-time validation
  handles cleanly.
- **Locking/dimming panels or dropping input mid-refresh** — rejected: with
  a 500ms debounce the in-flight window is hit constantly while editing.
- **Lazy per-file diff detail** — deferred, not adopted: speculative
  complexity while full-diff cost is unproven; revisit under the
  large-repo performance question if measurements demand it.

## Consequences

- Every refresh is a **full recompute** (status, both diffs, matcher).
  Acceptable at ordinary repo scale; large-repo/large-diff behaviour is an
  explicitly open question (own ticket) — the deferred indicator is the
  v0.1 mitigation, lazy diffing the known escape hatch.
- The matcher being a pure function of (records, fresh diff) is what makes
  last-request-wins safe: dropping a stale job loses nothing.
- The snapshot is the single UI-facing data structure: panels never read
  engine state directly, which keeps draw code trivially consistent and
  the engine crate ratatui-free.
- Watch scope including `.git` means noisy repos (gc, fetch writing refs)
  can trigger spurious refreshes; they collapse via debounce +
  last-request-wins and produce identical snapshots — visible cost is nil,
  CPU cost is bounded.
