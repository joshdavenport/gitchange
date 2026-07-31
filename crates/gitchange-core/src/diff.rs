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
    /// Binary files carry no text hunks; their change is one whole-file
    /// degenerate hunk in the universe (ADR 0009).
    pub binary: bool,
    pub hunks: Vec<DiffHunk>,
    /// Blob identity and size per side (ADR 0009): the whole-file hunk's
    /// anchor material and the diff placeholder's sizes. The changed
    /// side is the diff's new side. Worktree diffs populate it only for
    /// binary files — the on-disk content hash is the stated refresh
    /// cost; index diffs populate it for every file (two odb header
    /// reads), so a binary worktree over staged text still derives a
    /// committable whole-file payload.
    pub binary_sides: Option<BinarySides>,
}

/// The two sides of a binary file's change (ADR 0009). A `None` side
/// doesn't exist: no `head` for added/untracked files, no `changed` for
/// deletions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinarySides {
    pub head: Option<BlobInfo>,
    pub changed: Option<BlobInfo>,
}

impl BinarySides {
    /// The changed-side content hash, `None` for a deletion.
    pub fn changed_oid(&self) -> Option<String> {
        self.changed.as_ref().map(|blob| blob.oid.clone())
    }

    /// The sides as the OID pair records anchor on (ADR 0009).
    pub fn oid_anchor(&self) -> crate::state::OidAnchor {
        crate::state::OidAnchor {
            head: self.head.as_ref().map(|blob| blob.oid.clone()),
            changed: self.changed_oid(),
        }
    }
}

/// One binary blob: its content hash (`hash-object`-style) and byte size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobInfo {
    pub oid: String,
    pub size: u64,
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
