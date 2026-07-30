use std::path::PathBuf;

use crate::commit::CommitOptions;
use crate::diff::RepoDiffs;
use crate::error::Error;

/// A diff hunk's identity within one diff: (old_start, old_lines,
/// new_start, new_lines). Unique per file because hunks never overlap.
pub type HunkHeader = (u32, u32, u32, u32);

/// The seam between core and any git implementation (ADR 0006). The git2
/// adapter is the only implementor in v0.1; the planned shell-out fallback
/// lives behind this same trait. `Send` so the Engine's refresh worker can
/// own a [`crate::Repo`] on its own thread (ADR 0005).
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

    /// The paths whose tree entries differ between `baseline_oid`'s tree
    /// and the current HEAD's — the ADR 0012 guard's affected-path set,
    /// paths only, no line mapping. `Ok(None)` when the baseline no
    /// longer resolves to a commit (gc'd after a rebase, or hand-edited):
    /// the caller must degrade to treating every path as affected.
    fn paths_changed_since(&self, baseline_oid: &str) -> Result<Option<Vec<String>>, Error>;

    /// Set index := worktree within one file's worktree-side line range
    /// (start, lines): a real apply-to-index of the diff(index↔worktree)
    /// hunks overlapping it (ADR 0003). All-or-nothing — a failed apply
    /// leaves the index untouched.
    fn stage_worktree_range(&self, path: &str, new_range: (u32, u32)) -> Result<(), Error>;

    /// Set index := HEAD within one file's HEAD-side line range: a
    /// reverse-apply onto the index of the diff(HEAD↔index) hunks
    /// overlapping it. All-or-nothing, like `stage_worktree_range`.
    fn unstage_head_range(&self, path: &str, old_range: (u32, u32)) -> Result<(), Error>;

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
    /// nothing. Returns the new HEAD commit id.
    fn commit_from_index_hunks(
        &self,
        payload: &[CommitPathSpec],
        message: &str,
        options: &CommitOptions,
    ) -> Result<String, Error>;
}

/// One path's share of a commit payload: which diff(HEAD↔index) hunks
/// the temporary index carries.
#[derive(Debug, Clone)]
pub struct CommitPathSpec {
    pub path: String,
    pub hunks: Vec<HunkHeader>,
}
