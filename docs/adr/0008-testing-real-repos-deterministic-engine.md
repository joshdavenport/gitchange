# Testing: real repos through sync ops, deterministic Engine, corpus-gated apply

gitchange's git-touching code is tested through **core's public sync
operations against real temp-dir git repos**. The `GitBackend` seam is
**not** a test surface in v0.1: a hand-written fake adapter would encode our
assumptions about git — precisely what the tests exist to catch (libgit2's
apply quirks live below the seam). Should a second real adapter ever land,
the existing suite is parameterized across both and the seam becomes a
contract-test surface at that moment, not before. **ADR 0003's shell-out
apply fallback is not such an adapter** — it was re-scoped to a
conditional fallback inside an adapter's apply methods, so it would
parameterize only the apply corpus, not the whole suite. Every test
through the sync ops exercises the git2 adapter for
free.

## Fixtures

- **Programmatically built temp repos per test**, via a shared
  `RepoFixture` builder: init → write files → commit → edit worktree,
  plus a `with_hook()` helper that writes hook scripts into the repo
  (exercises `HookRejected { stderr }`, ADR 0004). *Amended (issue
  #59):* the builder lives in the `gitchange-test-support` workspace
  crate (`publish = false`), not core's test target, so the TUI crate's
  run-loop tests — and any future frontend's — build repos from the
  same builder instead of growing a drifting copy. ADR 0006 records why
  a test-only crate carrying git2 does not breach the git2 wall.
- **No checked-in fixture repos** — a `.git` inside the repo needs
  renaming dances, is opaque in review, and drifts from what tests claim.
- Snapshot assertions (insta-style) are allowed opportunistically where
  they read well; they are an assertion style, not a fixture source, and
  no snapshot corpus is mandated.

### States libgit2 can't reach faithfully shell out to real git

Amends the fixture rule above (issue #57). The repository states ADR
0007's commit guard reads cannot all be reached through libgit2: it has
no `git am` at all, and what the guard reads is real git's on-disk
leftovers — `.git/rebase-apply/rebasing` versus `.git/rebase-merge`, the
sequencer's todo beside `CHERRY_PICK_HEAD`. The builders that leave a
rebase, cherry-pick, revert or `am` in progress therefore drive system
git, with the host's global and system config cut out so the fixture
stays as hermetic as the libgit2-built ones (which pin the same knobs in
the repo's own config instead).

The criterion is unreachability, not convenience: a state libgit2 builds
faithfully is still built through libgit2, however tempting the command
line is. One non-operation state has since met it (issue #60) — the
**branch switch that carries dirty hunks across**, ADR 0002's scope
semantics. libgit2's safe checkout updates the index but leaves the
departing branch's files sitting in the worktree, where the hunk universe
reads them as untracked dirt and pollutes the snapshot under test; real
git removes them. Its sibling in the same ticket, an external `git reset`
of one path, stayed on libgit2 (`reset_default`) precisely because
libgit2 reaches it exactly. Fixtures shelling out stay inside the
documented git floor where the choice is free: a branch `checkout` rather
than `switch`, which needs 2.23.

This qualifies **No git version matrix** below: system git is no longer
only in the ADR 0004 commit shell-out, and the exact `RepositoryState`
these fixtures reach is version-sensitive — git 2.49 writes an
`interactive` marker for `--merge`, so libgit2 names that state
`RebaseInteractive` rather than `RebaseMerge`. The decision still holds,
because the tests assert the *operation the guard sees* and pin the
underlying state only to the on-disk shape git has used for a decade. A
builder whose operation fails to start panics where it is built rather
than leaving a guard assertion to pass over a clean repo.

## Apply-correctness corpus — a v0.1 exit criterion

Data-driven cases in `gitchange-core`'s tests: each case is data (base
content, worktree edit, hunk selection, expected index/commit result),
materialized through the builder into a fresh repo and run through the
real sync ops. Seeded from the backend research's risk list: no trailing
newline, CRLF, blank-line-only hunks, adjacent hunks, mode changes, file
creation/deletion, empty-file edges.

The corpus is a **v0.1 gate**, written into the spec's exit criteria
alongside ticket 16's benchmark harness. Rationale: write-through staging
(ADR 0003) and temp-index commits (ADR 0004) make apply correctness the
product — a wrong hunk-apply silently commits content the user didn't
select. The corpus doubles as the certification suite ADR 0003's
conditional shell-out apply fallback must pass, unchanged, to count as
behaviorally equivalent — and, being green today, it is also the evidence
that fallback is not needed.

## Engine: deterministic logic, one smoke test

- The notify watcher sits behind a small **internal event-source trait**,
  so Engine's decision logic — debounce coalescing, self-loop filter,
  last-request-wins slot (ADR 0005/0006 semantics, which ADR 0006 calls
  domain rules) — is unit-tested deterministically with synthetic events.
- **One real-fs smoke test** (touch a file, expect `RefreshComplete`
  within a generous timeout) proves the wiring, on every CI OS — watcher
  backends differ per platform and are exactly what it checks.
  - *Amended (issue #62): a second, for the git dir.* The worktree half
    and the git-dir half are different wiring — ADR 0005 watches `.git`
    on purpose so external git operations are absorbed, and a backend or
    a filter change could carry file events while dropping index and HEAD
    ones. Filtering `.git` out of the watcher fails the second test and
    leaves the first passing, which is the case for its existence. Two is
    the ceiling this ADR intends by "one": a real-fs test earns its place
    only by covering wiring no other real-fs test reaches.
- **No test asserts on real debounce timing.** Timing assertions are the
  flake source, and the constant isn't the interesting logic. What a
  refresh *produces* is carried by the sync-op suite.

## The TUI run loop: real executors, recorded requests, one smoke test

Added by issue #59; qualifies **Scope** below, which kept TUI
*rendering* outside this ADR — the run loop's wiring executes core's
sync ops and requests refreshes, and its testing method is this ADR's
to govern.

The seam is parameterization, not extraction: the loop's dispatch stays
a hand-written match (ADR 0014's precedent), and no intent type is
introduced. `event_loop` is generic over the terminal backend
(`TestBackend` in tests), takes the input receiver (the real
crossterm-reader thread moves up into `run()`), takes the app by
mutable reference, and takes the engine decomposed into what it
actually consumes: the events `Receiver` plus a `request_refresh`
closure that `run()` supplies as the real engine method. Cloning the
receiver and dropping the Engine reaches the disconnect arm
(`EngineDied`), so engine death is covered, not deviated.

The method mirrors the Engine's own pattern above — deterministic logic
tests plus one real smoke test:

- **Executor tests are real-repo end-to-end**: the op and commit-step
  executors run against a `RepoFixture` repo through core's public sync
  ops, per this ADR's method. Nothing git-touching is faked.
- **Loop-arm tests assert the outbound refresh request with a recording
  closure** — deterministic, no timeouts. This is not a fake in this
  ADR's sense: the frontend's entire contract at that arm is "emit the
  request", and what the request *does* (debounce bypass,
  last-request-wins) is Engine-test property. The stakes are real:
  ADR 0005's self-loop filter ignores gitchange's own writes, so a
  dropped request after a mutation is not a late refresh but **no
  refresh at all**.
- **One end-to-end smoke test** — real Engine over a fixture repo, a
  mutation key through the loop, `RefreshComplete` applied — proves the
  wiring the recording tests abstract, and earns its place under the
  Engine section's ceiling rule: it covers wiring no other real test
  reaches.

**Accepted deviations, recorded here so they are not silent gaps:**
`run()`'s process-edge glue (terminal init/restore, focus-change escape
codes, `Engine::spawn`/`Repo::discover` wiring, spawning the real
input-reader thread) and real terminal input/focus-event *capture*
stay uncovered — they want a live terminal and a terminal emulator.
Every behavior behind that edge is covered from the seam inward;
`FocusGained` *handling* is tested by injecting the event, only its
delivery is not.

## CI

- **Linux + macOS + Windows on stable Rust.** The OS matrix is where the
  real risk lives (notify backends, CRLF cases in the corpus, path
  handling) and is cheap on hosted runners.
- **Clippy denies warnings on the same OS matrix** (issue #80), as its own
  job. `cargo clippy --workspace --all-targets -- -D warnings` type-checks
  every target and denies rustc's own lints alongside clippy's, while
  `cargo test` prints warnings and exits 0 — so clippy is where a diagnostic
  fails, and it must compile each `cfg` the matrix claims to cover. A warning
  reachable only under `cfg(windows)` or `cfg(target_os = "macos")` is
  otherwise unreachable, which is what leaves a platform gate's dead code
  invisible. The test steps carry no `-D warnings`; matrixed clippy makes it
  redundant. Formatting compiles nothing and is platform-independent, so
  `cargo fmt --all --check` is a separate ubuntu-only job with no build cache.
- **Rustdoc denies warnings on ubuntu only** (issue #81), as its own job.
  `cargo doc --no-deps --document-private-items --workspace` under
  `RUSTDOCFLAGS: -D warnings` is the only step that reaches rustdoc's lints:
  neither `RUSTFLAGS` nor `cargo clippy --all-targets` does, so a broken
  intra-doc link survives a green clippy matrix. Private items are documented
  because gitchange is an application, not a published library — its doc
  comments exist for maintainers and agents, and most of the prose sits on
  internal items. One runner, not clippy's matrix — but the gap is real, not
  absent: an item behind `cfg(windows)` or `cfg(not(unix))` takes its doc
  comment out of ubuntu's reach, and `peak_rss_bytes` in `xtask` is already
  such an item. It carries no doc comment today, which is what makes one
  runner enough today. The gap is accepted because the risk it leaves is
  bounded by how little non-unix code this workspace has, and the failure it
  catches is otherwise silent: a doc comment naming a mechanism nothing
  answers to goes unnoticed until someone looks for an API that no longer
  exists.
- **No git version matrix.** git2 vendors libgit2, so system git enters
  the tested path only via the ADR 0004 `git commit` shell-out — a
  decades-stable interface. Runners' preinstalled git suffices; a minimum
  git version is documented, not matrixed.
- **Hooks are per-test fixtures, not CI setup** — `with_hook()` writes
  them; git-for-Windows runs hook scripts via its bundled sh, so the hook
  tests are ungated and cover all three platforms. A `#!/bin/sh` hook
  spawns, sees the temp index through `GIT_INDEX_FILE`, and its stderr
  reaches `HookRejected` (issue #63).
- **Two test families are unix-only by necessity**, which qualifies the OS
  matrix above. Neither has a Windows meaning to test: `core.fileMode` is
  off there, so the corpus's mode-change cases would assert nothing; and a
  directory mode of `0o500` has no equivalent, so `unwritable_odb` cannot
  deny the write its tests turn on. They are the whole of what the matrix
  does not reach. *Amended (issue #80):* what each gate leaves behind is
  checked, not just recorded — the clippy job compiles the non-unix side on
  Windows, where a helper or an import the gate strips of its last user
  fails the build.

## Scope

TUI rendering is not git-touching code (ADR 0006: core is the only
git-speaking crate) and is outside this decision.

## Considered options

- **`GitBackend` seam with a fake adapter as primary surface** — rejected:
  fast and deterministic, but the fake would mirror our misconceptions;
  fidelity is the point of these tests. A fake does not count as the
  second adapter.
- **Dual suites from day one (fake-backed unit + real-repo integration)** —
  rejected: two suites to keep honest for coverage the real-repo suite
  already provides at acceptable speed.
- **Checked-in fixture repos** — rejected as above.
- **Direct real-fs Engine testing with timing assertions** — rejected:
  notorious CI flake generator; the injectable event source tests the same
  logic deterministically.
- **Smoke test only for Engine** — rejected: leaves self-loop filtering
  and last-request-wins — domain rules per ADR 0006 — unverified.
- **Git version matrix in CI** — rejected: defends one very stable
  shell-out at real matrix cost.
- **Linux-only CI** — rejected: the corpus's CRLF cases would lose their
  most likely failure platform.
