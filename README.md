# gitchange

A CLI+TUI for organising uncommitted local changes into named changelists that can be staged and committed independently. Based in part on Intellij's changelist feature, changelists allows you and your (potentially multiple at once) agents to work in parallel without questioning whose changes are whose. No more "the changes in the worktree aren't mine". Changelists can version at the hunk level, so agents can even edit and version changes within the same file. Co-ordinating actual change conflicts is on you though. If agents are going to touch the same block/method/line, just use worktrees.

This project's TUI is heavily [lazygit](https://github.com/jesseduffield/lazygit) inspired, please give support to that project. If you use lazygit, you should feel right at home.

Linux, macOS, and Windows are all supported and CI-tested.

> Just use worktrees

Worktrees are great, they're a long lived feature now gaining a renaissance. However, they can be annoying: what's the best placement? env files? dependencies? etc. Tooling exists to help ([workz](https://github.com/rohansx/workz/), [treeehouse](https://github.com/kunchenguid/treehouse), [wtp](https://github.com/satococoa/wtp)) but sometimes a simpler approach is better.

> Worktrees suck, long live worktrees

> [!WARNING]
> This is beta software. The CLI espeically is a first version and subject to change. Any changes that agents need to know will come with relevant skill changes.

## Installation

Wording TBD.

## Agentic use via skills

> [!WARNING]
> These skills are prototypes. The three versions exist to test which works best with agents. Ideally the smallest possible version works, I'm dogfooding this currently. Any assistance is appreciated.

Once gitchange is installed, teach your agent to use it:

```sh
# a balanced version, balanced between information and token cost
npx skills add https://github.com/joshdavenport/gitchange --skill gitchange-cli-base
# the lightest most spartan version. hopefully this one works well
npx skills add https://github.com/joshdavenport/gitchange --skill gitchange-cli-spartan
# tokens? what tokens?
npx skills add https://github.com/joshdavenport/gitchange --skill gitchange-cli-verbose
```

Invocation is up to you:

- Manually invoke
- Use plain english
- Add to your global `AGENTS.md`/`CLAUDE.md`

## How gitchange works

### The claim model

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
  both stdin and stdout; given a pipe or a file on either, it refuses with
  exit `1` and points at `gitchange --help` for the command-line surface.

## Development

```sh
cargo test --workspace --all-targets   # full suite (real temp repos, no mocks)
```

```sh
cargo xtask sandbox make --all         # resettable manual-testing repos under .sandbox/
cargo xbench                           # RefreshJob benchmark harness (docs/agents/benchmarks.md)
```
