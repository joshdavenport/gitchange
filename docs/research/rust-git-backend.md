# Research: Rust git backend for gitchange

**Question:** git2 (libgit2 bindings) vs gitoxide (`gix`) vs shelling out to the
git CLI — which fits gitchange's needs (hunk-level staging, temp-index
workflows, partial-file commits, refs/notes persistence, fast status)?

**Date:** 2026-07-29. Sources checked as of this date; versions cited are
current releases. **Status: recommendation for a human to ratify in a later
session — not a decision.**

Context: `docs/kickoff/wayfinder-brief.md` §6 (interaction with the git index,
commit mechanics, persistence model, watching/refresh).

---

## 1. Capability matrix against gitchange's required operations

| Operation | git2 (libgit2) | gitoxide (gix) | git CLI shell-out |
|---|---|---|---|
| Diff worktree vs HEAD/index, hunk access, context lines | Yes — full | Partial — diffs yes, hunk API missing | Yes (`git diff -U<n>`, parse text) |
| Apply selected hunks to index (`git apply --cached`) | Yes — `apply()` + hunk callback | **No — `gix-apply` unimplemented** | Yes (craft patch text, pipe to `git apply --cached`) |
| Temp/in-memory index | Yes — `Index::new()`, `set_index`, `apply_to_tree` | Index write yes, but tree-from-index missing | Yes (`GIT_INDEX_FILE`) |
| Synthesize blobs/trees for partial-file commits | Yes — `blob()`, `treebuilder()` | Yes — object write + tree editing | Yes (`hash-object -w`, `update-index --cacheinfo`, `write-tree`, `commit-tree`) |
| Custom refs | Yes | Yes | Yes (`update-ref`) |
| Git notes | Yes — `note()`/`notes()` | **No — `gix-note` CRUD unimplemented** | Yes (`git notes`) |
| Status performance, medium-large repos | Known-slow vs git CLI | Fast, parallel; beats git CLI in benchmarks | Fast, but process-spawn + porcelain parsing per refresh |

git2 is the only in-process option covering every required operation today.

### 1.1 Diffing with hunk-level access and controllable context

- git2 `DiffOptions::context_lines(&mut self, lines: u32)` — "the number of
  unchanged lines that define the boundary of a hunk", default 3; plus
  `interhunk_lines()` and `pathspec()`. Diff sources:
  `diff_index_to_workdir`, `diff_tree_to_workdir`, `diff_tree_to_index`.
  ([docs.rs git2 0.21.0 `DiffOptions`](https://docs.rs/git2/latest/git2/struct.DiffOptions.html),
  [`Repository`](https://docs.rs/git2/latest/git2/struct.Repository.html))
- gitoxide: blob/tree/worktree diffing marked done, but "working with hunks
  of data" is an unchecked item, and tree-with-index diff is unchecked, in the
  project's own status doc
  ([gitoxide `crate-status.md`](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)).
  gitchange's whole domain model is hunks; this is a first-order gap.
- CLI: `git diff -U<n>` gives exact control, but hunks arrive as text to
  parse; lazygit does exactly this (Go, parses patch text —
  [lazygit `pkg/commands/git_commands`](https://github.com/jesseduffield/lazygit/tree/master/pkg/commands/git_commands)).

### 1.2 Applying selected hunks to the index

- git2: `Repository::apply(diff, ApplyLocation::Index, opts)` with
  `ApplyOptions::hunk_callback(FnMut(Option<DiffHunk>) -> bool)` and
  `delta_callback` to filter per-hunk/per-file — an in-process
  `git apply --cached` with hunk selection
  ([docs.rs `ApplyOptions`](https://docs.rs/git2/latest/git2/struct.ApplyOptions.html)).
  This is exactly how gitui stages/unstages hunks in production:
  `stage_hunk` builds `ApplyOptions` with a hunk callback matching hunks by
  hashed header, then `repo.apply(..., ApplyLocation::Index, ...)`;
  `unstage_hunk` applies the reversed diff the same way
  ([gitui `asyncgit/src/sync/hunks.rs`](https://github.com/gitui-org/gitui/blob/master/asyncgit/src/sync/hunks.rs)).
- gitoxide: `gix-apply` "parse and apply textual and binary patches" is
  unchecked/unimplemented ([crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)).
  **Blocker for gitchange today.**
- CLI: generate patch text for selected hunks and pipe to
  `git apply --cached` — the `git add -p` / lazygit precedent. Robust
  semantics (git handles offset drift) but requires byte-exact patch
  reconstruction.

### 1.3 Temp-index / in-memory index workflows

Needed for per-changelist staging without disturbing the real index (brief
§6, "Interaction with the git index").

- git2: `Index::new()` creates "a new in-memory index... may be used to
  perform in-memory index operations"; `Index::open(path)` opens a bare index
  file without a repo (the `GIT_INDEX_FILE` pattern);
  `Repository::set_index(&mut Index)` swaps the repo's index;
  `apply_to_tree(diff, tree, opts) -> Result<Index, Error>` applies a diff to
  a tree and hands back a fresh index without touching the real one — this
  plus `Index::write_tree_to(repo)` is a complete commit-one-changelist
  pipeline that never touches `.git/index`
  ([docs.rs `Index`](https://docs.rs/git2/latest/git2/struct.Index.html),
  [`Repository`](https://docs.rs/git2/latest/git2/struct.Repository.html)).
- gitoxide: index reading/writing done (V2/V3), but the TREE extension write
  and tree-from-index are unchecked ([crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)) —
  so the index→tree step of a partial commit isn't there via the index route.
- CLI: `GIT_INDEX_FILE=/tmp/x git read-tree HEAD && git apply --cached ... &&
  git write-tree` — the canonical scripted approach; works, at the cost of a
  process per step.

### 1.4 Synthesizing blobs/trees for partial-file commits

- git2: `Repository::blob(&[u8]) -> Oid`, `blob_writer()`,
  `treebuilder(Option<&Tree>)`, and `Index::add_frombuffer(entry, data)` (add
  an index entry straight from a memory buffer — ideal for "file content =
  HEAD content + selected hunks")
  ([docs.rs `Repository`](https://docs.rs/git2/latest/git2/struct.Repository.html),
  [`Index`](https://docs.rs/git2/latest/git2/struct.Index.html)).
- gitoxide: object writing done ("write objects and obtain id"), commit
  creation from a tree done ([crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)) —
  jj proves gix is sufficient if you build trees directly rather than via an
  index. But you'd be hand-rolling the hunk→blob synthesis gix doesn't offer.
- CLI: `git hash-object -w` + `git update-index --cacheinfo` +
  `git write-tree` + `git commit-tree`.

### 1.5 Custom refs and git notes (persistence candidates)

- git2: `reference(name, id, force, log_message)`, `reference_symbolic`, and
  full notes API — `note(notes_ref, author, committer, oid, note, force)`,
  `notes(notes_ref)`, `note_default_ref()`
  ([docs.rs `Repository`](https://docs.rs/git2/latest/git2/struct.Repository.html)).
- gitoxide: loose-ref create/update/delete done (reftable backend missing),
  but `gix-note` "CRUD for git notes" entirely unimplemented
  ([crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)).
  If notes win the persistence decision, gix can't serve it.
- CLI: full support (`git update-ref`, `git notes`).

### 1.6 Status performance (matters for the watch/refresh loop)

- libgit2 status is documented-slow on large repos:
  "`git_status_list` is slower than `git status`"
  ([libgit2#4230](https://github.com/libgit2/libgit2/issues/4230));
  `git_status_list_new` took ~3s on the rust repo, dominated by SHA1 hashing
  ([exa#28](https://github.com/ogham/exa/issues/28)); broader perf issues
  acknowledged upstream ([libgit2#5038](https://github.com/libgit2/libgit2/issues/5038)).
  Mitigations exist (`StatusOptions` pathspec/untracked tuning) but the
  ceiling is real on chromium-scale repos.
- gitoxide `gix status` runs dirwalk + index checks in parallel with rename
  tracking; the author reports it beating git itself (3% linux, 11% git, 23%
  WebKit repos) ([gitoxide discussion #1074](https://github.com/GitoxideLabs/gitoxide/discussions/1074)),
  and gix being ~8x faster than git2 for attribute queries, up to 60x
  multithreaded on WebKit ([starship PR #6476](https://github.com/starship/starship/pull/6476)).
- CLI: `git status --porcelain=v2` is fast but costs a spawn + parse per
  refresh tick.

Note gitchange's stated target is "medium-large", not chromium-scale;
libgit2 status is likely acceptable initially, and status is the single
easiest piece to later swap to gix (see gitui, below).

---

## 2. Maturity & maintenance, mid-2026

- **git2-rs**: actively maintained by the rust-lang org. 0.21.0 released
  2026-05-18 binding libgit2 1.9.3 (with experimental SHA256 repo support);
  0.20.4 2026-02-02; 0.20.3 2025-12-06 (libgit2 1.9.2)
  ([git2-rs CHANGELOG.md](https://github.com/rust-lang/git2-rs/blob/master/CHANGELOG.md)).
  Caveats: it tracks upstream libgit2, whose own pace lags git proper and
  which "only maintains support for a single version simultaneously",
  a packaging pain jj cites ([jj#5548](https://github.com/jj-vcs/jj/issues/5548)).
- **gitoxide**: very active. gix 0.86.0 / release v0.56.0 on 2026-07-23
  ([gitoxide releases](https://github.com/GitoxideLabs/gitoxide/releases)).
  Trajectory is clearly the ecosystem's future, but the specific features
  gitchange needs most (hunk APIs, apply, notes) are still unchecked in its
  own status doc ([crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md)).

## 3. What comparable tools use

- **gitui (asyncgit)**: depends on **both** — `git2 0.21` (features
  `https`) as the workhorse and `gix 0.84` (features incl. `status`,
  `max-performance`, `revision`)
  ([asyncgit/Cargo.toml](https://github.com/gitui-org/gitui/blob/master/asyncgit/Cargo.toml)).
  Hunk staging is pure git2 (`hunks.rs`, §1.2). An open tracking issue
  migrates read paths (status, diff view, log, blame) to gix incrementally
  ([gitui#2676](https://github.com/gitui-org/gitui/issues/2676),
  [gitui#2611](https://github.com/gitui-org/gitui/issues/2611)).
  gitui is the closest-shaped project to gitchange; its architecture is the
  strongest single precedent.
- **jj (jujutsu)**: gix only — `gix 0.85` with `blob-diff`, `index`,
  `max-performance-safe`; no git2 dependency
  ([jj Cargo.toml](https://github.com/jj-vcs/jj/blob/main/Cargo.toml)).
  jj is actively deprecating git2 over SSH gaps, packaging pain, and perf
  ([jj#5548](https://github.com/jj-vcs/jj/issues/5548)) — but jj **shells out
  to the git binary for fetch/push** (`git.subprocess`, now default) for
  reliable `--porcelain` output and credential handling
  ([jj changelog](https://docs.jj-vcs.dev/latest/changelog/),
  [jj#5531](https://github.com/jj-vcs/jj/issues/5531)). jj can live on gix
  because it builds trees/commits directly and has no hunk-staging or notes
  requirement.
- **cargo**: git2 with an in-progress `-Zgitoxide` migration for fetches
  ([cargo#11813](https://github.com/rust-lang/cargo/issues/11813)).
- **lazygit** (the UX model for gitchange): 100% shell-out to git CLI,
  parsing output ([lazygit pkg/commands](https://github.com/jesseduffield/lazygit/tree/master/pkg/commands)).
- **starship / onefetch**: migrated status/metrics git2→gix for perf
  ([starship#6476](https://github.com/starship/starship/pull/6476)).

**Hybrid approaches are the norm, not the exception**: gitui (git2+gix),
jj (gix+subprocess), cargo (git2+gix). Nobody serious is single-backend.

## 4. Known risks per option

- **git2**: libgit2 apply/diff has text-edge-case bugs — patches with
  "\ No newline at end of file" failing
  ([libgit2#5153](https://github.com/libgit2/libgit2/issues/5153)), blank-line
  apply failures seen via pygit2
  ([pygit2#1107](https://github.com/libgit2/pygit2/issues/1107)). Status perf
  ceiling (§1.6). C dependency (build complexity, unsafe surface, single
  supported upstream version).
- **gitoxide**: cannot do the core job today (no apply, no hunk API, no
  notes); APIs still churn (0.x, monthly breaking releases).
- **CLI shell-out**: no typed hunks — everything is patch-text generation and
  porcelain parsing; per-operation spawn cost in the refresh loop; but
  semantics are exactly git's (offset-tolerant `git apply`, real notes/refs),
  proven by lazygit.

---

## 5. Recommendation (for human ratification)

**Primary backend: git2, behind a project-owned backend trait; permit
targeted CLI shell-out for gaps; treat gitoxide as the planned future for
read paths, not a current option.**

Rationale:

1. git2 is the only in-process library covering all six required operations
   today — including the two that define gitchange (hunk-filtered
   `apply --cached` via `ApplyOptions::hunk_callback`, and in-memory
   index/`apply_to_tree` for per-changelist temp-index commits) — and gitui
   proves this exact path in a production Rust TUI.
2. gitoxide's missing pieces (hunk API, `gix-apply`, `gix-note`) are
   precisely gitchange's core; adopting it now means reimplementing apply.
3. Pure CLI (lazygit model) is viable but trades typed hunks for patch-text
   plumbing; in Rust the library route is strictly less parsing.

Concrete shape:

- Wrap all git access in one module/trait (e.g. `GitBackend`) so backends can
  be swapped per-operation — the seam every peer project ended up needing.
- Use git2 for: diff (with `context_lines`), hunk apply to real/temp index,
  blob/treebuilder synthesis, refs, notes, commit creation.
- Reserve CLI shell-out (`git apply --cached`, `GIT_INDEX_FILE`) as the
  fallback where libgit2 apply hits edge cases (no-newline-EOF, CRLF, blank
  lines) — jj's subprocess precedent shows this is respectable, and it needs
  no new abstractions if the trait exists.
- Build an apply-edge-case test corpus early (no trailing newline, CRLF,
  blank-line-only hunks, adjacent hunks, mode changes) — this is where git2
  will bite first.
- Revisit gix for `status` when the refresh loop shows latency (gitui's
  migration order: status/read paths first) — likely the first and maybe
  only gix adoption needed.

Main risks of this recommendation:

- Betting on libgit2 while the Rust ecosystem migrates away (jj deprecating,
  cargo/gitui moving read paths); mitigated by the trait seam.
- libgit2 apply correctness on text edge cases; mitigated by test corpus +
  CLI fallback.
- Status perf ceiling on very large repos; mitigated by `StatusOptions`
  tuning now, gix status later.
- C build dependency (vendored libgit2 compile time, distro packaging).

Not chosen: gix-primary (blocked on apply/hunks/notes — reassess in 6-12
months against [crate-status.md](https://github.com/GitoxideLabs/gitoxide/blob/main/crate-status.md));
CLI-primary (workable, but gives up in-process typed diffs for the tool's
hottest loop and re-derives hunk identity from text gitchange must already
model — see brief §6 "hunk identity & drift").
