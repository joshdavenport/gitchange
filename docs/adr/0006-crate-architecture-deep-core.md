# Crate architecture: deep core, two thin frontends, one binary

gitchange is a cargo workspace of **three crates**: `gitchange-core` (lib) —
the deep module holding the domain model, ADR 0001 matcher, ADR 0002
persistence, refresh recompute, and the `GitBackend` seam with its git2
adapter; `gitchange-tui` (lib) — the ratatui frontend, exposed to the bin
as essentially `run(...) -> Result`; and `gitchange` (bin) — clap dispatch
plus the CLI handlers. One binary: no subcommand launches the TUI,
subcommands are the CLI. The driver beyond v0.1 tidiness: the CLI is the
**agent entry point** and will grow toward TUI parity, so the
core/frontend seam is the wall that matters and is enforced at the
dependency graph, not by convention.

## Boundaries

- **Core is the only crate that speaks git**: git2 appears in
  `gitchange-core`'s `Cargo.toml` and nowhere else. Frontends cannot reach
  a repository except through core's interface. The planned CLI shell-out
  fallback for apply edge cases (ticket 03) lives *inside* core, behind
  the same `GitBackend` seam.
- **No `gitchange-cli` lib crate, deliberately.** CLI parity means core's
  interface must cover everything, not that CLI code gets big. Handlers
  stay thin arg-mapping in the bin crate; when a subcommand needs real
  logic, the logic sinks into core where the TUI gets it too. Handlers
  outgrowing arg-mapping is the smell that something belongs in core —
  a cli crate would give that logic a wrong home to accumulate in.
- **`gitchange-tui` as a separate lib** walls TUI internals off from CLI
  handlers in both directions and keeps incremental compiles local.
  Prior art: gitui's asyncgit/gitui split, with a dispatch bin on top.

## Core's two-layer interface

- **Sync operations** — blocking calls (`refresh() -> Snapshot`,
  `stage_hunk(...)`, `commit(...)`, …). The CLI's layer: one blocking
  refresh per invocation.
- **`Engine`** — the threaded runtime layered on the sync ops: filesystem
  watcher, ~500ms debounce, self-loop filter, last-request-wins
  RefreshJob slot, mutation-triggered refreshes (all ADR 0005 semantics),
  emitting events over a crossbeam channel. The TUI's layer: construct
  `Engine`, receive snapshots, send requests. Engine lives in core, not
  the TUI, because its semantics are domain rules (the self-loop filter
  needs `$GIT_DIR` knowledge) and a future agent-facing long-running mode
  (e.g. a `--watch` JSON stream) gets it for free.
- **No async runtime.** ADR 0005's sync crossbeam `Select` loop stands;
  threads and channels only.

## Errors

- Core's error type is a single `thiserror` enum whose variants are carved
  by **what the caller must do about them**: `HookRejected { stderr }`
  (ADR 0004: show hook output), `LockContention` (ADR 0002: fail fast),
  `ApplyFailed` (may trigger shell-out fallback inside core),
  `Backend(source)` and `State(source)` as wrapped opaques. Core never
  uses `anyhow` — its errors are a contract, not a report.
- **`git2::Error` never appears in core's public interface** — leaking it
  through the `GitBackend` seam would make the shell-out adapter
  second-class and glue frontends to a backend detail.
- **Engine degradation is an event, not an error**: watcher death →
  polling fallback (ADR 0005) arrives on the engine channel
  (`WatcherDegraded`, alongside `RefreshComplete(Snapshot)`). Hard errors
  surface only from sync-operation calls.
- Frontends: the TUI matches variants into ticket 15's presentation
  vocabulary; the bin crate may wrap in `anyhow` for context and maps
  variants to exit codes (ticket 13).

## Considered options

- **Two crates (core + bin with tui/cli modules)** — rejected: with the
  CLI a first-class, growing frontend, module discipline is the only
  wall between frontends and between frontends and git2; three crates
  make both walls dependency-graph facts. The third `Cargo.toml` is
  cheap.
- **Four crates (+ `gitchange-cli` lib)** — rejected as above: invites
  logic to accumulate frontend-side instead of sinking into core.
- **Engine in the TUI crate (core sync-only)** — rejected: ADR 0005's
  refresh semantics are domain rules, and trapping them in the TUI would
  make any future long-running CLI/agent mode reimplement them.
- **Separate binaries for TUI and CLI** — rejected: the brief's contract
  is one `gitchange` where bare invocation is the TUI; two binaries add
  install surface for zero leverage.
