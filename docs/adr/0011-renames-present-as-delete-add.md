# Renames: presented as delete + add, detection off in v0.1

The hunk universe's two diffs (ADR 0003) never enable rename detection
(git2 `find_similar` / `DiffFindOptions`), and `ChangeKind` deliberately
has **no `Renamed` variant**. A rename presents as `Deleted` at the old
path plus `Untracked` at the new path (`Added` once staged) — by decision,
not accident. Staging and committing both halves still produces a rename
in history the moment git's own log-side detection sees it; nothing is
lost from the repository's point of view.

## Known consequence: membership does not follow a rename

Membership records match by path continuity, so a rename silently orphans
the old path's records into dormancy rather than following the file — the
new path's hunks are fresh changes, assigned to the active changelist. The
same applies to binary whole-file hunks, which keep membership by path
continuity (ADR 0009). This is the visible-not-silent failure shape: the
work lands in the active changelist rather than vanishing, and the matcher
(ticket 25) and whole-file hunks (ticket 35) inherit this as a stated
baseline they may later improve on.

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
