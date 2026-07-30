use std::path::PathBuf;

use crate::diff::RepoDiffs;
use crate::error::Error;

/// The seam between core and any git implementation (ADR 0006). The git2
/// adapter is the only implementor in v0.1; the planned shell-out fallback
/// lives behind this same trait.
pub trait GitBackend {
    /// Both diffs the hunk universe is built from (ADR 0003):
    /// diff(HEAD↔worktree) including untracked content, and
    /// diff(HEAD↔index) — each against the empty tree when HEAD is unborn
    /// (ADR 0007). Rename detection is never enabled (ADR 0011).
    fn diffs(&self) -> Result<RepoDiffs, Error>;

    /// The per-worktree `gitchange` directory (ADR 0002): `gitchange`
    /// under the private git dir, so linked worktrees get independent
    /// state for free.
    fn state_dir(&self) -> PathBuf;

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
}
