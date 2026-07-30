# HEAD moves: own commits commute records; external moves dormant loudly

Membership-record coordinates are HEAD-side (ADR 0001), so any commit
touching a file with surviving records strands those records in the old
HEAD's coordinate space and tier-2 overlap inheritance goes stale. The
repro suite (`tests/head_moves.rs`, issue 37) proved all three
consequences, every one silent: wrong-list assignment when a shifted
hunk lands inside another record's stale region, membership loss when it
lands clear, and residual-`◑` shedding whenever the commit payload
shifts lines above it — defeating ADR 0004's retention promise exactly
when it is needed. We adopt a hybrid:

- **gitchange's own commits: commutation shift.** `commit()` knows
  exactly which hunks it applied. Under its existing locked state update
  it shifts surviving same-file records' old coordinates by the
  committed deltas and **rewrites retained `◑` records against the new
  HEAD** — new coordinates *and* a re-derived anchor whose old side is
  the committed content (the repro proved shifting alone is not enough).
  It stamps the new baseline HEAD in the same update, so an own commit
  never trips the guard below.
- **External HEAD moves: visible dormancy guard.** The state file stores
  the **baseline HEAD** — the commit whose tree record coordinates
  address. When refresh finds HEAD elsewhere, it diffs the two trees
  once (paths only, no line mapping) and disables tier-2 for the paths
  the move changed: their stale live records go dormant, anchor-broken
  hunks capture to active, and a per-path notice — fired only when an
  outcome actually changed — names the changelists whose records went
  dormant. Tier-1 exact anchors are position-independent and still
  rescue untouched hunks. An unresolvable old baseline (gc'd after a
  rebase) degrades to all-paths-affected; a missing baseline
  (pre-schema state file) adopts the current HEAD once, silently.

The dividing principle: **inside gitchange, nothing unexpected happens;
when history moves outside it, we keep what can be proven and are loud
about what cannot.** Tier-2 only ever runs against records whose
coordinates provably address the current HEAD — via commutation for own
commits, via the guard everywhere else — so the silent misfile becomes
mechanically impossible rather than merely unlikely ("visible, never
silent", the spec's failure-mode philosophy).

Matcher purity holds (ADR 0001/0005): refresh computes the affected-path
set and passes it in as an explicit input, exactly as `active` and `now`
already enter the signature. HEAD identity never enters the matcher.

**This amends ADR 0004's aftermath**: "retained for overlap
re-attachment" holds for own commits only because `commit()` rewrites
the retained `◑` records; across external HEAD moves, retained records
are exact-revival-only like any other dormant record (ADR 0002).

## Considered options

- **Commutation shift alone** — rejected: covers only
  gitchange-initiated commits, leaving the silent misfile reachable
  today via external partial commits (the repro simulates exactly
  `git commit -p` elsewhere).
- **HEAD-anchored re-baselining** (map record coordinates through
  diff(old baseline↔new HEAD) before matching) — **deferred, not
  rejected** (issue 38): the best retention and one mechanism for all
  HEAD moves, but the heaviest machinery, and a wrong line mapping
  reintroduces the silent misfile by a subtler path. Revisit if
  guard-dormancy notices prove frequent in mixed git/gitchange use.
- **Visible dormancy alone** — rejected: concedes the headline flow —
  committing changelist A would shed B's anchor-broken same-file hunks
  even though `commit()` knew the applied deltas exactly.
- **Do nothing** — rejected: the repro pinned a reachable silent
  misfile, the one outcome ADRs 0001/0002 promise never happens.

## Consequences

- Schema: a baseline HEAD field in the state file, serde-defaulted so
  pre-schema files parse (`None` = adopt current HEAD, guard skipped
  once — no mass dormancy on upgrade).
- A new notice variant (per path, loss-only); rendered like
  `AmbiguousOverlap` by the CLI and the Log panel.
- The guard lands independently of commit mechanics (issue 39, consumed
  ready-made by issue 28); `tests/head_moves.rs` defect-pinning
  assertions flip with each half.
- Issue 28 gains acceptance criteria: committing changelist A's hunks
  leaves B's same-file hunks in B; a residual `◑` hunk re-attaches to
  its changelist even when the payload shifts it.
- The concession is bounded and visible: after an external commit,
  rebase, amend, or pull, anchor-broken hunks on moved paths re-sort by
  hand, guided by the notice. Untouched hunks are unaffected.
