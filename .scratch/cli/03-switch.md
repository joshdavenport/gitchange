### gitchange switch

`gitchange [-C <dir>] switch <name>` — mutating (bare marker write — no internal refresh)

- base command **[built]** — sets the **active changelist**. Mirrors `git switch` setting the current branch
- no internal refresh **[built]** — one locked state write, nothing else: the unassigned pool is claimed at the _next_ persisting refresh (the engine's in the TUI, the next mutating op or `refresh` in the CLI), so `switch unassigned` claims nothing and the agent skill's opener stays inert. The receipt is the echo alone, never advisories. Claim-now composes: `switch <name>` then `refresh`
- core-composed echo **[reserved]** — `switch` returns `OpOutcome` and core composes the echo (ADR 0006); today's CLI-composed `Switched to '<name>'` is a standing drift this cures. Core work, not built
- `unassigned` as a valid target **[built]** (ADR 0015, #52) — capture-off: capture and ambiguous-edit routing flow to **unassigned**. `status` marks the group `*`, and prints the group file-less when it holds the marker on a clean tree

## Decisions

[`switch` is a bare marker write; the claim belongs to the next persisting refresh](decisions/switch-bare-marker-right.md)
[Changelist ops return `OpOutcome`; receipts are core-composed, and none of them refreshes](decisions/changelist-ops-opoutcome.md)
[`switch unassigned` is capture-off](decisions/switch-unassigned-capture-off.md)
