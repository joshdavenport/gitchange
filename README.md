# gitchange

A lazygit-inspired TUI for organising uncommitted local changes into named
changelists that can be staged and committed independently.

Vocabulary, invariants, and design decisions live in [CONTEXT.md](CONTEXT.md)
and [docs/adr/](docs/adr/); agent-facing workflow docs in
[docs/agents/](docs/agents/).

## The claim model

A new hunk is claimed at a **refresh** — normally by the **active
changelist** — and never at the moment you edit a file. Editing writes no
membership, and no command guesses ahead of the refresh that decides it. Capture — the claim a refresh makes — happens in three
places:

- Continuously in the TUI. It watches the tree and refreshes as you work, so
  what you see is what has already been decided.
- At each hunk-moving CLI command. `assign`, `add`, `unstage`, and `commit`
  each refresh before they act, and their receipts report what that refresh
  decided alongside what you asked for. Commands that only write the state
  file — `switch`, and creating, deleting, or renaming a changelist — claim
  nothing.
- On demand, by name. `gitchange refresh` is the deliberate claim-now: it
  runs one refresh and reports its decisions, or prints nothing if there
  were none.

Reads never capture. `gitchange status` and `gitchange diff` recompute in
order to answer, but write nothing, so a hunk you have just written reports
as unassigned rather than being placed. `status` says capture is pending without
naming where the hunk will land, because that is not decided until the
refresh that claims it — and that refresh reports where it went.

`gitchange switch <name>` moves the marker and nothing else. It runs no
refresh, so hunks that were already unassigned stay unassigned — including
work someone else has in flight. Claiming now is the composition, and it
claims whatever is pending, not only your own edits:

```sh
gitchange switch <name>   # move the marker
gitchange refresh         # claim the pending hunks
```

`gitchange switch unassigned` turns capture off. With unassigned active,
refreshes claim nothing into a changelist until you switch back.

Manual membership is the other half: `gitchange assign` moves hunks by hand,
at the moment you run it. Capture is what happens to the hunks you never
assigned.

## Requirements

- **git ≥ 2.5** on `PATH`. gitchange reads and writes the repository through
  libgit2, with one exception: committing shells out to `git commit` so that
  hooks run (ADR 0004). The shell-out uses only long-stable pieces (`-F`,
  `--no-verify`, `--amend`, `GIT_INDEX_FILE`); the 2.5 floor comes from
  linked-worktree discovery — inside a `git worktree` checkout, `git commit`
  must resolve the per-worktree git dir, which 2.5 introduced. If you use
  `core.hooksPath`, your git is ≥ 2.9 by definition and hooks fire from the
  configured path.
- A terminal. Bare `gitchange` launches the TUI, so it needs a terminal on
  both stdin and stdout; given a pipe or a file on either, it refuses with
  exit `1` and points at `gitchange --help` for the command-line surface.
  Linux, macOS, and Windows are all supported and CI-tested.

## Development

```sh
cargo test --workspace --all-targets   # full suite (real temp repos, no mocks)
cargo xtask sandbox make --all         # resettable manual-testing repos under .sandbox/
cargo xbench                           # RefreshJob benchmark harness (docs/agents/benchmarks.md)
```
