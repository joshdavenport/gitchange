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
