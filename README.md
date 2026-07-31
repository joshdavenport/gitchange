# gitchange

A lazygit-inspired TUI for organising uncommitted local changes into named
changelists that can be staged and committed independently.

Vocabulary, invariants, and design decisions live in [CONTEXT.md](CONTEXT.md)
and [docs/adr/](docs/adr/); agent-facing workflow docs in
[docs/agents/](docs/agents/).

## Requirements

- **git ≥ 2.5** on `PATH`. gitchange reads and writes the repository through
  libgit2, with one exception: committing shells out to `git commit` so that
  hooks run (ADR 0004). The shell-out uses only long-stable pieces (`-F`,
  `--no-verify`, `--amend`, `GIT_INDEX_FILE`); the 2.5 floor comes from
  linked-worktree discovery — inside a `git worktree` checkout, `git commit`
  must resolve the per-worktree git dir, which 2.5 introduced. If you use
  `core.hooksPath`, your git is ≥ 2.9 by definition and hooks fire from the
  configured path.
- A terminal. Linux, macOS, and Windows are all supported and CI-tested.

## Development

```sh
cargo test --workspace --all-targets   # full suite (real temp repos, no mocks)
cargo xtask sandbox make --all         # resettable manual-testing repos under .sandbox/
cargo xbench                           # RefreshJob benchmark harness (docs/agents/benchmarks.md)
```
