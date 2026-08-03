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
  `RepoFixture` builder in core's test support: init → write files →
  commit → edit worktree, plus a `with_hook()` helper that writes hook
  scripts into the repo (exercises `HookRejected { stderr }`, ADR 0004).
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
- **No test asserts on real debounce timing.** Timing assertions are the
  flake source, and the constant isn't the interesting logic. What a
  refresh *produces* is carried by the sync-op suite.

## CI

- **Linux + macOS + Windows on stable Rust.** The OS matrix is where the
  real risk lives (notify backends, CRLF cases in the corpus, path
  handling) and is cheap on hosted runners.
- **No git version matrix.** git2 vendors libgit2, so system git enters
  the tested path only via the ADR 0004 `git commit` shell-out — a
  decades-stable interface. Runners' preinstalled git suffices; a minimum
  git version is documented, not matrixed.
- **Hooks are per-test fixtures, not CI setup** — `with_hook()` writes
  them; git-for-Windows runs hook scripts via its bundled sh.

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
