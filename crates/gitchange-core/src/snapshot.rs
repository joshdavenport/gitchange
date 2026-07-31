use crate::diff::ChangeKind;
use crate::matcher::Notice;
use crate::state::Changelist;
use crate::universe::ChangedFile;

/// A git operation in progress (ADR 0007): each means "the next commit
/// concludes this operation", so commit is guarded while one holds and
/// the TUI pins it. Detached HEAD and bisect are deliberately absent —
/// committing there is legitimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitOperation {
    Merge,
    /// Any rebase flavour (plain, interactive, merge-backend).
    Rebase,
    CherryPick,
    Revert,
    /// `git am`.
    Am,
}

impl GitOperation {
    /// The operation as user-facing text ("merge in progress — …").
    pub fn label(&self) -> &'static str {
        match self {
            GitOperation::Merge => "merge",
            GitOperation::Rebase => "rebase",
            GitOperation::CherryPick => "cherry-pick",
            GitOperation::Revert => "revert",
            GitOperation::Am => "am",
        }
    }
}

/// Where HEAD points — the Status panel's line, snapshotted with the
/// refresh so frontends never speak git (ADR 0006).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// On a branch.
    Branch { name: String },
    /// Detached, identified by the commit's short id.
    Detached { short_id: String },
    /// Fresh `git init` (ADR 0007): HEAD names a branch with no commits.
    Unborn { name: String },
}

/// One commit for the Commits panel, newest first in
/// [`Snapshot::recent_commits`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitInfo {
    pub short_id: String,
    pub author: String,
    pub summary: String,
}

/// The immutable result of one refresh — the only data structure frontends
/// read. Hunks carry their owning changelist (written by the matcher);
/// notices record this refresh's automatic membership decisions.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The hunk universe (ADR 0003), sorted by path.
    pub files: Vec<ChangedFile>,
    /// Changelists in user order.
    pub changelists: Vec<Changelist>,
    /// Name of the active changelist; `None` only when no changelists
    /// exist.
    pub active: Option<String>,
    /// Automatic membership decisions worth spot-checking, in file
    /// order. Not persisted: a decision becomes a record, so it surfaces
    /// exactly once.
    pub notices: Vec<Notice>,
    /// Where HEAD points, for the Status panel.
    pub head: Head,
    /// Recent commits reachable from HEAD, newest first, for the Commits
    /// panel — plain lazygit-equivalent content in v0.1.
    pub recent_commits: Vec<CommitInfo>,
    /// The git operation in progress, if any (ADR 0007): the commit
    /// guard's predicate and the TUI's operation pin.
    pub operation: Option<GitOperation>,
}

impl Snapshot {
    /// The files belonging to one changelist for the All view — a file is
    /// "in" a changelist when at least one hunk is owned by it. `None`
    /// selects unassigned, which also holds hunk-less files (changed
    /// binaries until ticket 35).
    pub fn files_in(&self, changelist: Option<&str>) -> Vec<&ChangedFile> {
        self.files
            .iter()
            .filter(|file| {
                // Quarantined (ADR 0007): a conflicted file is hunk-less
                // but belongs to the Conflicts group, never unassigned.
                if file.kind == ChangeKind::Conflicted {
                    return false;
                }
                let owned = file
                    .hunks
                    .iter()
                    .any(|hunk| hunk.changelist.as_deref() == changelist);
                owned || (changelist.is_none() && file.hunks.is_empty())
            })
            .collect()
    }

    /// The quarantined unmerged paths, in path order — the Conflicts
    /// group rendered first in the file panels (ADR 0007).
    pub fn conflicted_files(&self) -> Vec<&ChangedFile> {
        self.files
            .iter()
            .filter(|file| file.kind == ChangeKind::Conflicted)
            .collect()
    }
}
