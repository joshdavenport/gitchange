# Research: changelist persistence options

Resolves the "Persistence model" open question in
`docs/kickoff/wayfinder-brief.md` §6: where can changelist metadata live so
it survives branch switches sanely? Survey of git-native storage options
against primary sources (git docs, tool source). No winner is picked here —
that is a later human decision. A shortlist with trade-offs closes the doc.

Scope note: this doc covers *where metadata lives*, not *what a hunk
identity is*. Hunk drift (brief §6, second bullet) is orthogonal — every
option below can store whatever identity scheme that work lands on. But the
options differ in whether referenced git objects (e.g. snapshot blobs used
as drift baselines) are protected from `git gc`, so that is evaluated.

Evaluation axes per option: durability (branch switch / rebase / clone /
gc / worktrees), visibility & interference with other git tools,
concurrency (two processes), repo pollution, shareability (push/pull vs
strictly local).

---

## Option A — custom refs (`refs/gitchange/...`) pointing at synthesized objects

Store metadata as a commit (or tag) object whose tree carries a metadata
blob (JSON/TOML) plus any content snapshots, referenced from a custom ref
namespace.

**Precedent (stgit).** StGit stores each branch's entire patch-stack state
this way: a state commit under `refs/stacks/<branch>` whose tree holds a
`stack.json` blob and a `patches/` subtree with one metadata blob per
patch. The ref name is built in
[`src/stack/stack.rs`](https://github.com/stacked-git/stgit/blob/master/src/stack/stack.rs):
`format!("refs/stacks/{branch_name}")`, and per the code comments the
"stack state representation is serialized to/from the `stack.json` blob in
the stack state tree"
([src/stack/state.rs](https://github.com/stacked-git/stgit/blob/master/src/stack/state.rs)).
State commits carry the previous state commit as a parent, so stack history
is itself versioned and gc-anchored.

**Precedent (jj).** In colocated repos, jj protects its commits from git's
gc with exactly this trick: "Commits created by `jj` have a ref starting
with `refs/jj/` to prevent GC"
([jj git-compatibility docs](https://docs.jj-vcs.dev/latest/git-compatibility/)).

- **gc:** safe. `git gc` "will keep not only objects referenced by your
  current set of branches and tags, but also objects referenced by the
  index, remote-tracking branches, reflogs … and anything else in the
  `refs/*` namespace" ([git-gc](https://git-scm.com/docs/git-gc)). A custom
  namespace is first-class gc protection — including for hunk-snapshot
  blobs placed in the state tree.
- **Branch switch / rebase:** unaffected. Checkout/rebase never touch
  unrelated refs. Whether metadata is keyed per-branch (stgit model:
  `refs/stacks/<branch>`) or global (single `refs/gitchange/state`) is a
  product decision the namespace supports either way.
- **Clone:** *not* copied. `git clone` creates remote-tracking branches for
  each branch and clones tags by default; only `--mirror` "maps all refs
  (including remote-tracking branches, notes etc.)"
  ([git-clone](https://git-scm.com/docs/git-clone)). So a fresh clone
  starts with no changelists — acceptable, since changelists describe
  uncommitted local state that a clone doesn't have either.
- **Worktrees:** refs are shared: "all refs starting with `refs/` are
  shared … exceptions: refs inside `refs/bisect`, `refs/worktree` and
  `refs/rewritten` are not shared"
  ([git-worktree](https://git-scm.com/docs/git-worktree)). Two live
  options: `refs/gitchange/*` = one changelist set visible from every
  worktree; `refs/worktree/gitchange/*` = per-worktree sets, with
  cross-worktree access via the `worktrees/<name>/refs/worktree/...`
  namespace. Since each worktree has its own working tree (and thus its own
  uncommitted changes), per-worktree (`refs/worktree/gitchange/*`) is
  probably the semantically correct default — but the mechanism supports
  both.
- **Concurrency:** best of all options. `git update-ref` does atomic
  compare-and-swap ("after verifying that the current value of the <ref>
  matches <old-oid>") and `--stdin` transactions with
  `start/prepare/commit/abort`; "if all <ref>s can be locked with matching
  <old-oid>s simultaneously, all modifications are performed. Otherwise, no
  modifications are performed"
  ([git-update-ref](https://git-scm.com/docs/git-update-ref)). Two
  gitchange processes (or gitchange vs. anything using git plumbing) can't
  silently clobber each other.
- **Visibility / interference:** invisible to normal porcelain (branch
  lists, lazygit panels). Tools that enumerate *all* refs (`git log --all`,
  `gitk --all`, `git for-each-ref`) will surface the synthesized commits —
  mild noise, same noise stgit and jj users already live with. Nothing else
  writes this namespace.
- **Pollution:** objects accumulate in the odb; superseded state commits
  become unreachable once the ref moves (if not chained as parents) and are
  pruned after the `gc.pruneExpire` grace period (default two weeks,
  [git-gc](https://git-scm.com/docs/git-gc)). Chaining parents stgit-style
  trades odb growth for free undo history.
- **Shareability:** opt-in. Any ref can be pushed/fetched with an explicit
  refspec, so "sync my changelists to my other machine" is possible later
  without redesign; default remains local because default refspecs only
  cover heads/tags ([git-clone](https://git-scm.com/docs/git-clone)).

## Option B — git notes

Notes are "blobs containing extra information about an object … taken from
notes refs", default `refs/notes/commits`, addressed *by the annotated
object's ID* ([git-notes](https://git-scm.com/docs/git-notes)).

- **Shape mismatch (disqualifying):** notes annotate *existing objects*.
  Changelist metadata describes uncommitted working-tree state, which has
  no object to annotate. We would have to synthesize an object first — at
  which point the notes layer adds indirection on top of what is already
  Option A. Notes' headline feature, following commits through rewrites
  (`notes.rewrite.rebase`/`amend`,
  [git-notes](https://git-scm.com/docs/git-notes)), is useless for
  never-committed state.
- **gc trap:** "a note (of the kind created by *git notes*) attached to an
  object does not contribute in keeping the object alive"
  ([git-gc](https://git-scm.com/docs/git-gc)). Annotating a synthesized
  object does not protect it; it still needs a real ref. Notes cannot even
  self-sufficiently anchor snapshot blobs.
- **Concurrency:** the notes ref is one commit chain; concurrent writers
  race on it. `git notes merge` strategies (`manual`, `ours`, `theirs`,
  `union`, `cat_sort_uniq`) exist but are built for *distributed* merge of
  human-readable annotations, not machine state
  ([git-notes](https://git-scm.com/docs/git-notes)).
- **Visibility:** `git log` displays notes from the default notes ref
  automatically — actively *worse* interference than a custom namespace if
  the default ref were used; a custom `refs/notes/gitchange` avoids the
  display but keeps every other drawback.
- **Clone / share:** not fetched by default (same refspec rules as A);
  pushable explicitly.
- **Verdict:** dominated by Option A in every axis that matters here.
  Include only as "considered, rejected: wrong shape".

## Option C — `.git/` sidecar file(s)

A file or directory gitchange owns, e.g. `.git/gitchange/state.json` (or
SQLite).

**Precedent.** lazygit: repo-specific config is read from
`<repo>/.git/lazygit.yml`
([docs/Config.md](https://github.com/jesseduffield/lazygit/blob/master/docs/Config.md):
"you can create repo-specific config files in `<repo>/.git/lazygit.yml`");
its mutable state (`state.yml`: recent repos, startup popup version) is
*global per-user* in the config dir, not per-repo — lazygit persists almost
nothing per-repo, which is why it gets away without any of this design.
git-branchless keeps its event-sourced undo log in
`.git/branchless/db.sqlite3` (SQLite)
([arxanas/git-branchless#209](https://github.com/arxanas/git-branchless/issues/209)).
jj is the maximal version: everything lives under a sidecar `.jj/`
directory next to `.git` — commit backend in `.jj/repo/store` (with
jj-specific metadata in `.jj/repo/store/extra/`), operation log in
`.jj/repo/op_store` + `.jj/repo/op_heads`, working-copy state tracked
separately ([jj architecture docs](https://docs.jj-vcs.dev/latest/technical/architecture/)) —
plus Option-A-style `refs/jj/*` anchor refs for gc protection when
colocated ([jj git-compatibility](https://docs.jj-vcs.dev/latest/git-compatibility/)).

- **Branch switch / rebase / gc:** trivially durable. Git neither reads nor
  deletes unknown files in `$GIT_DIR`; gc prunes *objects*, not stray
  files. The repository-layout spec documents git's own files and leaves
  tool additions alone ([gitrepository-layout](https://git-scm.com/docs/gitrepository-layout)).
- **The gc catch:** if the sidecar stores raw hunk text inline, no issue.
  If it instead references git objects (blob OIDs for snapshots — likely,
  to reuse git's storage and diffing), those objects are unreachable from
  any ref and get pruned after the grace period
  ([git-gc](https://git-scm.com/docs/git-gc)). Fix is the jj trick: pair
  the sidecar with a lightweight anchor ref (`refs/gitchange/keep`) — i.e.
  a hybrid with Option A.
- **Clone:** not copied (`.git` internals never transfer). Same
  fresh-clone-is-empty behaviour as A.
- **Worktrees:** the *most explicit* control of any option. Per-worktree
  state goes in `$GIT_DIR` (which for a linked worktree is
  `.git/worktrees/<id>/`), shared state in `$GIT_COMMON_DIR`; "path
  resolution via `git rev-parse --git-path` uses either `$GIT_DIR` or
  `$GIT_COMMON_DIR` depending on the path"
  ([git-worktree](https://git-scm.com/docs/git-worktree)). Placing
  `gitchange/` via `git rev-parse --git-path gitchange` gives the
  per-worktree behaviour that matches "changelists describe *this* working
  tree".
- **Concurrency:** roll-your-own. Need a lockfile protocol (git's own
  `*.lock` convention) or SQLite's built-in locking (git-branchless's
  choice). No atomic CAS for free; crash-safety is on us.
- **Visibility / interference:** invisible to every other tool; zero
  interference either direction. Also zero introspectability — a user
  can't `git cat-file` their way to understanding, and no other tool can
  ever cooperate.
- **Pollution:** none in the odb; one directory in `.git`.
- **Shareability:** strictly local, permanently. Sharing would require a
  separate export mechanism.

## Option D — stash-like commit objects

How stash actually works: "A stash entry is represented as a commit whose
tree records the state of the working directory, and its first parent is
the commit at `HEAD` … The tree of the second parent records the state of
the index" — the latest entry is `refs/stash`, and *older stashes exist
only in that ref's reflog* ("older stashes are found in the reflog of this
reference") ([git-stash](https://git-scm.com/docs/git-stash)).

- **Durability problem — reflog as list:** reflog entries expire:
  `gc.reflogExpire` defaults to 90 days, `gc.reflogExpireUnreachable` to 30
  days ([git-gc](https://git-scm.com/docs/git-gc)); dropped entries "will
  then be subject to pruning, and may be impossible to recover"
  ([git-stash](https://git-scm.com/docs/git-stash)). A reflog-backed
  changelist list silently decays. Disqualifying as the *listing*
  mechanism.
- **Interference:** writing into `refs/stash` itself would corrupt the
  user's actual stash list and confuse every tool with a stash panel
  (lazygit included). Must not share the namespace.
- **Clone / share:** stashes don't transfer on clone/push (reflogs are
  local; docs are silent because it simply doesn't happen —
  [git-stash](https://git-scm.com/docs/git-stash)).
- **What survives from this option:** the *commit shape*, not the storage.
  A stash-style commit (worktree-tree + index-tree parents) is a proven
  format for snapshotting uncommitted state, useful as the synthesized
  object *under an Option A ref* — one ref per changelist
  (`refs/gitchange/lists/<name>`) instead of one reflog-listed ref for all.
  Evaluated that way, it inherits all of Option A's properties.

## Option E — IntelliJ's approach (workspace.xml, outside git entirely)

IntelliJ changelists are "a set of local changes that have not yet been
committed" with one active list receiving new changes
([IntelliJ docs](https://www.jetbrains.com/help/idea/managing-changelists.html)).
Storage is the `ChangeListManager` component inside `.idea/workspace.xml` —
observable in any real workspace.xml, e.g.
[this one](https://github.com/Cognifide/IntelliJ-Shortcuts-For-AEM/blob/master/.idea/workspace.xml)
(`<component name="ChangeListManager"><list name="Default" …>` with
`<change type="MODIFICATION" beforePath=… afterPath=…/>` entries).
workspace.xml is user-local by design: since 2019.2.4 the IDE generates
`.idea/.gitignore` ignoring `/workspace.xml`
([JetBrains support threads](https://intellij-support.jetbrains.com/hc/en-us/community/posts/115000001824-I-have-idea-in-gitignore-but-it-is-still-in-local-changes)).

- **Durability:** survives branch switch/rebase/gc because git is told to
  ignore it — but it lives *in the working tree*, so durability depends on
  the ignore rule holding; mis-configured repos commit it (GitHub is full
  of accidentally-committed workspace.xml files, see cite above). Each
  IntelliJ project dir gets its own workspace.xml, so worktrees get
  per-worktree state naturally.
- **Concurrency:** none at the file level — single-IDE assumption; the IDE
  owns the file in memory and writes it out.
- **Granularity note:** the persisted model is *per-file*
  (`beforePath`/`afterPath`); IntelliJ's hunk-level tracking is done live
  against its own local-history engine, not persisted per-hunk in
  workspace.xml. gitchange's per-hunk membership requirement means we
  persist strictly more than IntelliJ does.
- **Lesson, not a candidate:** for a standalone tool, a working-tree file
  outside `.git` is the worst of both worlds (visible to git, needs ignore
  management, committable by accident). The transferable ideas are
  per-working-tree scoping and strict local-only semantics. The equivalent
  *sane* placement for us is Option C (`.git/` sidecar), which gets the
  same properties without touching the working tree.

---

## Cross-cutting: what "survives branch switches sanely" actually requires

- Git checkout carries uncommitted changes across branches (or refuses the
  switch); no storage option changes that. Metadata must therefore be keyed
  to *content* (file + hunk identity), not to the branch, or it must define
  per-branch scoping explicitly. Precedents diverge: stgit is strictly
  per-branch (`refs/stacks/<branch>`); IntelliJ changelists are global
  across branches. This is a product decision the persistence layer must
  parameterize, and options A and C both can.
- Any option that stores blob OIDs (snapshot baselines for drift tracking)
  needs a ref anchoring them, because gc protection comes only from
  refs/index/reflogs ([git-gc](https://git-scm.com/docs/git-gc)). This
  requirement alone pulls every realistic design at least partially toward
  Option A.

## Recommendation shortlist (no winner — later human decision)

1. **Custom refs, stgit-style (`refs/gitchange/*` or
   `refs/worktree/gitchange/*` → state commit with metadata blob + snapshot
   trees).** Fully git-native: gc-safe including snapshots, atomic CAS
   concurrency via update-ref transactions, free undo history via parent
   chaining, opt-in push/pull later. Costs: everything goes through git
   plumbing (slower iteration on schema), synthesized commits visible to
   `--all`-style tooling, odb growth.
2. **`.git/` sidecar (`$GIT_DIR/gitchange/`, JSON or SQLite) + minimal
   anchor ref for referenced objects (jj/git-branchless hybrid).** Simplest
   to build and evolve, per-worktree scoping for free via
   `git rev-parse --git-path`, zero visibility to other tools. Costs:
   roll-your-own locking/crash-safety, strictly local forever, still needs
   the Option-A anchor ref the moment snapshots reference git objects.
3. **Pure sidecar with inline content (no git objects at all).** Only if
   drift tracking ends up storing raw hunk text rather than blob OIDs:
   drops the anchor-ref requirement and is maximally simple, at the cost of
   duplicating content storage git already does well, and of no
   introspectability via git tooling.

Rejected outright: git notes (annotates existing objects; can't anchor
anything against gc; dominated by custom refs), literal stash mechanics
(reflog-backed lists decay under `gc.reflogExpire*`; `refs/stash` collides
with the user's stash), IntelliJ-style working-tree file (ignore-rule
fragility; solved better by the sidecar).

## Sources

- git docs: [git-gc](https://git-scm.com/docs/git-gc),
  [git-notes](https://git-scm.com/docs/git-notes),
  [git-stash](https://git-scm.com/docs/git-stash),
  [git-worktree](https://git-scm.com/docs/git-worktree),
  [git-clone](https://git-scm.com/docs/git-clone),
  [git-update-ref](https://git-scm.com/docs/git-update-ref),
  [gitrepository-layout](https://git-scm.com/docs/gitrepository-layout)
- stgit source:
  [src/stack/stack.rs](https://github.com/stacked-git/stgit/blob/master/src/stack/stack.rs)
  (`format!("refs/stacks/{branch_name}")`),
  [src/stack/state.rs](https://github.com/stacked-git/stgit/blob/master/src/stack/state.rs)
  (`stack.json` state tree)
- jj docs: [architecture](https://docs.jj-vcs.dev/latest/technical/architecture/),
  [git compatibility](https://docs.jj-vcs.dev/latest/git-compatibility/)
  (`refs/jj/` gc-protection refs)
- lazygit:
  [docs/Config.md](https://github.com/jesseduffield/lazygit/blob/master/docs/Config.md)
  (`.git/lazygit.yml` repo config; global state in user config dir)
- git-branchless:
  [issue #209](https://github.com/arxanas/git-branchless/issues/209)
  (`.git/branchless/db.sqlite3`)
- IntelliJ:
  [Group changes into changelists](https://www.jetbrains.com/help/idea/managing-changelists.html),
  example
  [workspace.xml `ChangeListManager`](https://github.com/Cognifide/IntelliJ-Shortcuts-For-AEM/blob/master/.idea/workspace.xml),
  [JetBrains support on workspace.xml/.gitignore](https://intellij-support.jetbrains.com/hc/en-us/community/posts/115000001824-I-have-idea-in-gitignore-but-it-is-still-in-local-changes)
