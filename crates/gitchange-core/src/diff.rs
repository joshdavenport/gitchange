/// The raw material of one refresh: both diffs the hunk universe is built
/// from (ADR 0003) — diff(HEAD↔worktree) and diff(HEAD↔index) — as data,
/// so deriving staging stays a pure function outside the backend.
#[derive(Debug, Clone)]
pub struct RepoDiffs {
    pub worktree: Vec<FileDiff>,
    pub index: Vec<FileDiff>,
}

/// One changed file within a single diff, before universe derivation.
#[derive(Debug, Clone)]
pub struct FileDiff {
    /// Repo-relative path, as git reports it.
    pub path: String,
    pub kind: ChangeKind,
    /// Binary files carry no text hunks; whole-file degenerate hunks are
    /// ticket 35 (ADR 0009).
    pub binary: bool,
    pub hunks: Vec<DiffHunk>,
}

/// One text hunk. Old coordinates address the HEAD side in both diffs,
/// which is what makes hunks from the two diffs comparable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<HunkLine>,
}

/// One diff line, verbatim. `origin` follows git's convention: ' ', '+',
/// '-', plus the no-newline-at-EOF markers '=', '>', '<' — kept because
/// they are content-meaningful for staged/worktree equality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkLine {
    pub origin: char,
    pub content: String,
}

/// How a file changed. There is deliberately no `Renamed` variant: rename
/// detection is off in v0.1, so a rename presents as `Deleted` (old path)
/// plus `Untracked` (new path) — ADR 0011.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    TypeChanged,
    Untracked,
    Conflicted,
}
