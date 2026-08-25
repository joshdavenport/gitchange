use crate::diff::ChangeKind;
use crate::state::Changelist;
use crate::universe::ChangedFile;
use crate::vocabulary::{CONFLICTS, UNASSIGNED};

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

    /// The operation guard's refusal sentence (ADR 0007):
    /// `Error::OperationInProgress`'s `Display` and the TUI's soft no-op
    /// log line on `c` both say this, so the commit guard's refusal and
    /// the log explaining it can't drift apart. `Error` is
    /// `#[non_exhaustive]`, so the TUI cannot construct the variant
    /// itself to borrow its `Display` — this is the shared phrasing both
    /// sides reach instead.
    pub fn in_progress_message(&self) -> String {
        format!("{} in progress — conclude or abort it first", self.label())
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
/// read. Hunks carry their owning changelist (written by the matcher).
///
/// Deliberately advisory-free: advisories are what a refresh *decided*,
/// not what the tree *is*, so they ride [`crate::RefreshOutcome`] beside
/// this — which the read-only form never returns (ADR 0005).
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The hunk universe (ADR 0003), sorted by path.
    pub files: Vec<ChangedFile>,
    /// Changelists in user order.
    pub changelists: Vec<Changelist>,
    /// Name of the active changelist; `None` is unassigned — capture
    /// off (ADR 0015). Exactly one of {the changelists, unassigned} is
    /// active, so every reader of this field has a `*` to place.
    pub active: Option<String>,
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
    /// selects unassigned. Every non-conflicted change carries at least
    /// one hunk (ADR 0017), so ownership alone decides.
    pub fn files_in(&self, changelist: Option<&str>) -> Vec<&ChangedFile> {
        self.files
            .iter()
            .filter(|file| {
                // Quarantined (ADR 0007): a conflicted file is hunk-less
                // but belongs to the Conflicts group, never unassigned.
                if file.kind == ChangeKind::Conflicted {
                    return false;
                }
                file.hunks
                    .iter()
                    .any(|hunk| hunk.changelist.as_deref() == changelist)
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

    /// The All view's groups, in render order — the one place its
    /// grouping rules live (ADR 0006): Conflicts first when any path is
    /// unmerged (ADR 0007), changelists in user order (empty ones
    /// included, the active one flagged), unassigned last when non-empty
    /// or active. An empty unassigned group renders only to carry the
    /// `*`, which is capture-off's whole visible surface (ADR 0015).
    /// Frontends render these; they don't re-derive them.
    pub fn groups(&self) -> Vec<FileGroup<'_>> {
        let mut groups = Vec::new();
        let conflicted = self.conflicted_files();
        if !conflicted.is_empty() {
            groups.push(FileGroup {
                kind: GroupKind::Conflicts,
                files: conflicted,
            });
        }
        for changelist in &self.changelists {
            groups.push(FileGroup {
                kind: GroupKind::Changelist {
                    name: changelist.name.clone(),
                    active: self.active.as_deref() == Some(changelist.name.as_str()),
                },
                files: self.files_in(Some(&changelist.name)),
            });
        }
        let unassigned = self.files_in(None);
        let unassigned_active = self.active.is_none();
        if !unassigned.is_empty() || unassigned_active {
            groups.push(FileGroup {
                kind: GroupKind::Unassigned {
                    active: unassigned_active,
                },
                files: unassigned,
            });
        }
        groups
    }
}

/// One group of [`Snapshot::groups`]'s All-view ordering.
#[derive(Debug, Clone)]
pub struct FileGroup<'a> {
    pub kind: GroupKind,
    /// The group's files, in path order.
    pub files: Vec<&'a ChangedFile>,
}

/// What a [`FileGroup`] is — the label vocabulary frontends render from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupKind {
    /// Quarantined unmerged paths (ADR 0007).
    Conflicts,
    /// A user changelist; `active` is the `*` marker.
    Changelist { name: String, active: bool },
    /// Hunks owned by no changelist. `active` is the same `*` marker:
    /// unassigned is a switchable target, and capture flows here while
    /// it holds the marker (ADR 0015).
    Unassigned { active: bool },
}

impl GroupKind {
    /// The group as user-facing text — the All view's row label.
    pub fn label(&self) -> &str {
        match self {
            GroupKind::Conflicts => CONFLICTS,
            GroupKind::Changelist { name, .. } => name,
            GroupKind::Unassigned { .. } => UNASSIGNED,
        }
    }

    /// Whether this group wears the `*`. Two variants can, so frontends
    /// ask here rather than matching the flag themselves (ADR 0006) —
    /// otherwise a third active target would have to be found at every
    /// render site. Conflicts is derived, never a switch target.
    pub fn active(&self) -> bool {
        match self {
            GroupKind::Conflicts => false,
            GroupKind::Changelist { active, .. } | GroupKind::Unassigned { active } => *active,
        }
    }
}
