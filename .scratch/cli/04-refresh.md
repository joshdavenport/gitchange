### gitchange refresh

`gitchange [-C <dir>] refresh` — mutating

- base command **[reserved]** — one persisting refresh, the CLI form of the TUI's manual refresh key (op parity; ADR 0005): capture, record re-anchoring, dormancy, baseline stamp. The deliberate-capture tool for a capture-on CLI session; the agent skill never teaches it. The receipt carries the refresh's advisories; nothing decided prints nothing, exit `0`, and a repeated refresh writes nothing. One silent write: when the stored baseline stamp differs from HEAD (an external HEAD move, or no stamp yet), the refresh restamps even with nothing decided (`repo.rs` `stamp_due`; ADR 0012) — bookkeeping, not a decision, so the receipt stays silent
- no flags **[reserved]** — the receipt is the standard text receipt (see the text-receipts decision): echo on stdout when something was decided, `notice:` lines on stderr; nothing decided prints nothing

## Decisions

[`refresh` is the deliberate persisting refresh; auto-capture stays refresh-bound](decisions/refresh-deliberate-persisting-refresh.md)
