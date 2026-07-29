use crate::error::Error;

/// The seam between core and any git implementation (ADR 0006). The git2
/// adapter is the only implementor in v0.1; the planned shell-out fallback
/// lives behind this same trait.
pub trait GitBackend {
    /// Every path that differs from HEAD in the worktree or the index,
    /// including untracked files.
    fn changed_files(&self) -> Result<Vec<ChangedFile>, Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    /// Repo-relative path, as git reports it.
    pub path: String,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    TypeChanged,
    Untracked,
    Conflicted,
}
