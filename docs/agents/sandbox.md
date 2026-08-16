# Manual-testing sandboxes

Persistent, named repos under `.sandbox/<scenario>/` (gitignored) for
eyeballing the TUI in settled and in-flight states. Built by
`crates/xtask` replaying **real core ops** — membership records are never
hand-assembled, so the state file always matches the current schema and a
scenario failing to build is a smoke-test signal. Design record:
[issue #42](https://github.com/joshdavenport/gitchange/issues/42).

## Commands

```sh
cargo xtask sandbox make --all      # build every scenario
cargo xtask sandbox make sorted     # build one
cargo xtask sandbox reset sorted    # alias of make: nuke-and-rebuild
cargo xtask sandbox status          # missing / pristine / modified per scenario
```

Usage loop:

```sh
cd .sandbox/sorted && cargo run -p gitchange
```

This works because sandboxes deliberately contain **no Cargo.toml** —
cargo resolves upward to the gitchange workspace manifest. Don't add one
to the fake project.

## Scenarios

| Name | State |
| --- | --- |
| `fresh` | Clean repo with history, no dirty tree, no state file. First-launch experience. |
| `unassigned-only` | Dirty tree, no changelists. Pre-adoption warning-state view. |
| `sorted` | Three changelists, hunks distributed, `src/timer.rs` split across two, `fix-timeout-retry` active. |
| `mid-staging` | `sorted` plus mixed staged states: `●` config.rs, `○` main.rs/report.rs, and timer.rs split across two changelists — `●` under `debug-logging`, `◐` under `fix-timeout-retry` (its hunk is staged-stale `◑`). The two timer.rs rows disagree on purpose: a row's marker reads only the hunks its changelist owns. |
| `conflicted` | Mid-merge: `src/report.rs` quarantined in the conflicts group, commit guarded, one changelist of unrelated dirty work. |
| `binary` | Changed PNG auto-captured into an `assets-refresh` changelist as one whole-file hunk (`0/1`), next to a text change for contrast. The two revisions differ in size so the ADR 0009 placeholder has an arrow worth eyeballing; `enter` on the PNG is a deliberate no-op. |
| `large` | 45 generated modules + split timer.rs across two changelists — scroll/refresh/layout under volume. |

## Semantics

- **Reset is nuke-and-rebuild.** No snapshots: scenarios are code, so a
  rebuild is always faithful to the current definition and state-file
  schema. Intermediate exploration state is not restorable.
- **Deterministic builds.** Pinned identity (`gitchange-sandbox`) and
  stepped timestamps from 2026-01-01 make rebuilds byte-identical —
  commit OIDs survive resets, so screenshots and notes stay comparable.
  Local config pins `commit.gpgsign=false`, `core.autocrlf=false`,
  `core.hooksPath=.git/hooks` so global git config can't distort what's
  eyeballed.
- **`status` fingerprints** live in `.sandbox/.meta/<scenario>.json`
  (HEAD OID + index/worktree/state digests). "Modified" names the
  diverged layers and is informational — it's the expected steady state
  after using gitchange in a sandbox, not an anomaly.
- **Skip-with-notice.** A scenario needing a core op that doesn't exist
  yet is skipped with a notice rather than failing `make --all`; file a
  follow-up issue naming the missing op.

## Adding a scenario

Add a build fn and a `Scenario` entry in
`crates/xtask/src/sandbox/scenarios.rs`. Rules: reach gitchange state
only through `Repo`'s sync ops (create/switch/move/stage/refresh); keep
content realistic Rust (lorem-ipsum diffs flatter the UI); keep builds
deterministic (no wall-clock, no randomness); update the table above.
