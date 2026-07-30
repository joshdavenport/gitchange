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
}
