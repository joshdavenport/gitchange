use std::path::PathBuf;

use crate::commit::{CommitOptions, WholeFilePayload};
use crate::diff::RepoDiffs;
use crate::error::Error;
use crate::snapshot::{CommitInfo, GitOperation, Head};

/// A diff hunk's identity within one diff — unique per file, because
/// hunks never overlap. Named fields rather than a `u32` quadruple: the
/// commit path builds these from two sources and compares them for
/// equality to prove the index hasn't moved (ADR 0004), and a transposed
/// pair would pass that check while committing the wrong region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HunkHeader {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
}

/// The seam between core and any git implementation (ADR 0006). The git2
/// adapter is the only implementor, and ADR 0003's conditional shell-out
/// apply fallback would not add a second one: it lives *inside* an
/// adapter's apply methods, reached when its own libgit2 apply returns
/// [`Error::ApplyFailed`]. `Send` so the Engine's refresh worker can own
/// a [`crate::Repo`] on its own thread (ADR 0005).
pub trait GitBackend: Send {
    /// Both diffs the hunk universe is built from (ADR 0003):
    /// diff(HEAD↔worktree) including untracked content, and
    /// diff(HEAD↔index) — each against the empty tree when HEAD is unborn
    /// (ADR 0007). Rename detection is never enabled (ADR 0011).
    fn diffs(&self) -> Result<RepoDiffs, Error>;

    /// The per-worktree `gitchange` directory (ADR 0002): `gitchange`
    /// under the private git dir, so linked worktrees get independent
    /// state for free.
    fn state_dir(&self) -> PathBuf;

    /// The worktree root — what the Engine's watcher observes (ADR 0005).
    /// `None` for a bare repository, which has no worktree to watch.
    fn workdir(&self) -> Option<PathBuf>;

    /// The commit id HEAD resolves to, `None` on an unborn branch
    /// (ADR 0007) — the value the state file stamps as the baseline HEAD
    /// (ADR 0012).
    fn head_oid(&self) -> Result<Option<String>, Error>;

    /// Where HEAD points — branch, detached, or unborn (ADR 0007) — for
    /// the snapshot's Status-panel line.
    fn head(&self) -> Result<Head, Error>;

    /// Up to `limit` commits reachable from HEAD, newest first; empty on
    /// an unborn branch.
    fn recent_commits(&self, limit: usize) -> Result<Vec<CommitInfo>, Error>;

    /// The git operation in progress, if any (ADR 0007) — the commit
    /// guard's one predicate. `None` covers bisect too: committing
    /// during a bisect is legitimate.
    fn operation(&self) -> Result<Option<GitOperation>, Error>;

    /// The paths whose tree entries differ between `baseline_oid`'s tree
    /// and the current HEAD's — the ADR 0012 guard's affected-path set,
    /// paths only, no line mapping. `Ok(None)` when the baseline no
    /// longer resolves to a commit (gc'd after a rebase, or hand-edited):
    /// the caller must degrade to treating every path as affected.
    fn paths_changed_since(&self, baseline_oid: &str) -> Result<Option<Vec<String>>, Error>;

    /// Set index := worktree within one file's worktree-side line range
    /// (start, lines): a real apply-to-index of the diff(index↔worktree)
    /// hunks overlapping it (ADR 0003). All-or-nothing — a failed apply
    /// leaves the index untouched, and reports [`Error::ApplyFailed`].
    fn stage_worktree_range(&self, path: &str, new_range: (u32, u32)) -> Result<(), Error>;

    /// Set index := HEAD within one file's HEAD-side line range: a
    /// reverse-apply onto the index of the diff(HEAD↔index) hunks
    /// overlapping it. All-or-nothing, like `stage_worktree_range`.
    fn unstage_head_range(&self, path: &str, old_range: (u32, u32)) -> Result<(), Error>;

    /// Set one file's index-entry mode := the worktree's, keeping its
    /// staged blob — the mode hunk's stage op (ADR 0017). No content
    /// moves with it: staging a mode delta is the whole of what this
    /// does. A path git reports no mode for, or holds no index entry
    /// for, is a no-op.
    fn stage_worktree_mode(&self, path: &str) -> Result<(), Error>;

    /// Set one file's index-entry mode := HEAD's, keeping its staged
    /// blob — the inverse of [`GitBackend::stage_worktree_mode`]. A path
    /// absent from HEAD is a no-op: an added or deleted file carries no
    /// mode hunk, its mode being part of the add/delete whole.
    fn unstage_head_mode(&self, path: &str) -> Result<(), Error>;

    /// Set the whole index entry := worktree, `git add` semantics: stages
    /// an untracked file, and stages the deletion when the file is gone.
    fn stage_path(&self, path: &str) -> Result<(), Error>;

    /// Set the whole index entry := HEAD's, `git reset -- <path>`
    /// semantics: drops the entry when HEAD doesn't have the path.
    fn unstage_path(&self, path: &str) -> Result<(), Error>;

    /// Commit `message` from a temporary index — HEAD's tree plus the
    /// named diff(HEAD↔index) hunks per path — via a native `git commit`
    /// shell-out with `GIT_INDEX_FILE`, so hooks run and see the true
    /// content (ADR 0004). The live index and worktree are never
    /// touched; every failure discards the temp file and changes
    /// nothing.
    fn commit_from_index_hunks(
        &self,
        payload: &[CommitPathSpec],
        message: &str,
        options: &CommitOptions,
    ) -> Result<CommittedId, Error>;
}

/// A commit as core and its frontends each need to name it: the full id
/// for the baseline stamp (ADR 0012) and drift checks, the abbreviation
/// for display. Both come from the backend together so display never
/// re-derives one from the other — abbreviation is git's own, honouring
/// `core.abbrev` and expanding for uniqueness, which a fixed-length
/// prefix of `oid` would silently disagree with.
#[derive(Debug, Clone)]
pub struct CommittedId {
    pub oid: String,
    pub short_id: String,
}

/// One path's share of a commit payload: which diff(HEAD↔index) hunks
/// the temporary index carries — or, for a binary file (ADR 0009), the
/// whole-file selection: the live index entry copied in verbatim, with
/// `whole_file.staged_oid` as the freshness check. `whole_file` and
/// `hunks` are mutually exclusive.
#[derive(Debug, Clone)]
pub struct CommitPathSpec {
    pub path: String,
    pub hunks: Vec<HunkHeader>,
    pub whole_file: Option<WholeFilePayload>,
}
