### gitchange status

`gitchange [-C <dir>] status` — read-only

- base command **[built]** — the **All** view as text: changelists in user order, active marked `*`, **unassigned** group last, conflicts first
- `--json` **[reserved]** (ADR 0015) — file/changelist-level machine output; hunk detail is `diff`'s job, per the read-surface split. Speaks the JSON dialect (see Decisions): same envelope, same `schema_version` as `diff --json`
- read-only refresh **[reserved]** (#51 discussion; ADR 0005) — every read invocation refreshes read-only: no capture, no record writes, no advisories — the recompute still produces them and the read path discards them (see the reads-never-persist decision). Ownership reports what the records say: record-derived ownership (inheritance, revival) shows; context-derived (capture, entry-unit join) never previews. Built `status` persists today and changes when this lands
- capture-pending hint **[reserved]** (#51 discussion; #106) — text face only: when a real changelist is active and **unassigned** holds hunks, the unassigned group carries one line naming the rule and the resolution — `capture on: run 'gitchange refresh' to claim these, or they're claimed at your next action`. Derived from record facts (the active marker, the absence of records), naming no destination (see the mechanism-not-destination decision): the claiming refresh's receipt reports where hunks landed. `--json` adds no field — `active` plus `changelist: null` carries the same derivation

## Decisions

[Read surfaces split along git lines (ADR 0015)](decisions/read-surfaces.md)
[Reads never persist; the deciding refresh's receipt is the only advisory delivery (ADR 0005 amendment)](decisions/reads-never-persist.md)
[The JSON dialect: envelope, tagged unions, promised order](decisions/json-dialect.md)
[One `schema_version`, one written additive contract](decisions/schema_version.md)
