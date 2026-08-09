# Renames: presented as delete + add, detection off in v0.1

The hunk universe's two diffs (ADR 0003) never enable rename detection
(git2 `find_similar` / `DiffFindOptions`), and `ChangeKind` deliberately
has **no `Renamed` variant**. A rename presents as `Deleted` at the old
path plus `Untracked` at the new path (`Added` once staged) — by decision,
not accident. Staging and committing both halves still produces a rename
in history the moment git's own log-side detection sees it; nothing is
lost from the repository's point of view.

## Known consequence: membership does not follow a rename

Membership records match by path, so a rename never carries membership to
the new path — the new path's hunks are fresh changes, captured by the
active changelist with the usual advisory. This holds for binary
whole-file hunks too: path continuity (ADR 0009) is *path* continuity, so
identical bytes at a new path are a fresh change, not the old record's.

What becomes of the old path's records depends on whether the old path
still diffs (pinned in `tests/core/matcher.rs`, issue 56):

- **Tracked file** — the old path stays in the universe as `Deleted`, so
  its records re-anchor onto the deletion hunk and keep their changelist
  (tier-2 overlap for text; for a whole-file hunk, path continuity — a
  deletion is still a binary change at that path, ADR 0009). The rename's
  two halves end up in two changelists.
- **Untracked file** — the old path leaves the universe entirely, so its
  records go dormant, retained for exact-match revival.

Either way this is the visible-not-silent failure shape: the work lands in
the active changelist or stays where it was sorted, never vanishing, and
the matcher (ticket 25) and whole-file hunks (ticket 35) inherit this as a
stated baseline they may later improve on.

## Accepted test deviation: log-side detection is not asserted

No test stages and commits both halves of a rename and asserts that git
reports one (issue #75). What such a test would pin is git's own
`--find-renames` similarity scoring: gitchange enables no detection, sets
no threshold, and passes no `-M`. That is ADR 0008's method holding — the
suite covers gitchange's behaviour through real repos, not third-party
behaviour gitchange never invokes.

The gitchange-owned half of the opening claim — the commit tree really
carries the removal at the old path and byte-identical content at the new
one — is pinned by the apply corpus (`commit_a_staged_new_file`,
`commit_a_staged_deletion_removes_the_path`). Committing both in one
changelist composes two covered halves and asserts nothing new about
gitchange. It would also fail ambiguously: git changing its heuristic and
gitchange mangling the new path's bytes are indistinguishable at the
assertion, and the byte-fidelity reading is pinned unambiguously by the
corpus.

Log-side detection is a property of whole halves committed together, not
of every rename gitchange presents. Hunk-level staging can commit part of
the new file, whose similarity to the deleted original legitimately falls
short of git's threshold — no rename in the log, and correctly so. The
opening claim is a floor on what remains recoverable when the user
commits the rename entire, not a guarantee attached to the presentation.

## Considered options

- **Enable `find_similar` on both diffs** — deferred, not rejected:
  rename pairs would need a `Renamed` kind carrying old+new paths through
  the universe, membership matching across the pair, and similarity
  thresholds tuned against false positives (a mostly-rewritten "rename"
  stealing another record's membership). None of that is needed to prove
  the v0.1 loop; revisit with evidence once the matcher exists.

## Consequences

- Anyone adding rename detection later must revisit: `ChangeKind` (add
  `Renamed { old_path }` or equivalent), universe kind-merging, membership
  matching (ticket 25), and this ADR.
- Detection cost (similarity scoring across all adds/deletes) stays out of
  the refresh path, which ADR 0005 keeps hot.
