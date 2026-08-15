# Unassigned is recordless: assign-to-unassigned forgets, never claims

A hunk belongs to unassigned exactly when it has **no membership record**.
Nothing writes a nobody-owner record: `MembershipRecord.changelist` always
names a real changelist. Assigning hunks to unassigned **deletes** their
matching records. Deleting a changelist **prunes** all of its records, live
and dormant. The hunks then flow under ADR 0001's uniform rule — recordless
means new, and new goes to the active changelist. Records are the system's
only memory; unassigned is the absence of one.

This reverses the sticky nobody-claim (`changelist: None` records) that
issue #32 introduced for explicit moves by analogy to delete-orphans. That
analogy was never stated in an ADR and never reachable from a user surface.

## Why forgetting, not filing

- **One unassigned, not two.** The sticky claim split unassigned into two
  states: recordless (capturable) and claimed-for-nobody (capture-proof).
  ADR 0015 already states the model for capture-off — "the hunks stay
  recordless … capture-off is the statement that nobody has said yet."
  Explicit assignment now says the same thing. The pseudo-changelist has
  one membership test: no record.
- **Expressiveness.** Parking a hunk safe from capture is what changelists
  are for — make one and assign to it. Under the sticky claim, returning a
  hunk to the uniform flow was expressible nowhere; a sticky orphan could
  never re-enter capture at all. Forgetting adds a power the system
  lacked and removes none it had.
- **Honest uniformity.** Assign to unassigned — or delete a changelist —
  while a real changelist is active, and the next refresh captures those
  hunks into it, with the usual auto-capture advisories. That is the
  uniform rule doing its job on hunks the user released, not a defect to
  absorb. The supported way to keep hunks loose is capture-off first:
  `switch unassigned` (ADR 0015). Prior art agrees: IntelliJ IDEA
  changelists, the system this model descends from, behave the same way
  (maintainer-tested).

## Mechanics

- Assigning to unassigned drops every record the payload hunks match or
  overlap and writes nothing back. Assigning already-recordless hunks to
  unassigned is a true no-op. The operation's echo states a release, not
  an assignment.
- Deleting a changelist prunes all of its records. The unknown-name sweep
  does the same. This amends ADR 0002's "dormant records are pruned when
  their changelist is deleted": all of the deleted changelist's records
  are pruned, live ones included.
- `MembershipRecord.changelist` narrows from `Option<String>` to `String`.
  The matcher's nobody-owner arms — the `None` overlap claim, dormancy and
  revival of nobody-claims — are removed, not kept dormant.
- Amends ADR 0001's assignment rules: unassigned holds the pre-changelist
  dirty tree and, while unassigned is active, whatever capture routes
  there. Orphans of deleted changelists and explicit moves land in
  unassigned only in that same capture-off case; otherwise they flow to
  the active changelist like any recordless hunk.
- The TUI's delete confirmation names the hunks' real destination: the
  active changelist when capture is on, unassigned when it is off.

## Considered options

- **Sticky nobody-claim (status quo from #32)** — rejected: it duplicates
  changelist parking, forks unassigned into two states, and contradicts
  ADR 0015's "claims nothing" reading of unassigned. Its one protection —
  hunks released while a real changelist is active stay put — guards a
  position the user chose against the model's grain.
- **Edit-fragile claim (sticky until content changes, then capturable)** —
  rejected: a third persistence semantic needing new matcher tiers, and it
  makes unassigned behave unlike every other target in exactly the way the
  claim was meant to avoid.
- **Forget for assigns, sticky for orphans** — rejected: same fork, one
  door over. A deleted changelist's hunks are released to the flow like
  any other recordless hunk; a never-before-seen hunk is not stickily kept
  from the active changelist, and neither are they.

## Consequences

- Core: `assign_records` on an unassigned target deletes and writes
  nothing; changelist deletion and the unknown-name sweep prune wholesale;
  the record type and matcher lose their nobody-owner arms.
- Tests asserting the sticky claim invert; fixtures that used
  assign-to-unassigned as capture-proof parking park in a changelist or
  run capture-off instead.
- The glossary's Unassigned entry simplifies to the recordless model; the
  Assign entry stops calling unassigned "a target like any other" — as an
  assign target it means release, and it can bounce.
- #51's reserved `assign --to unassigned` grammar survives with forget
  semantics.
