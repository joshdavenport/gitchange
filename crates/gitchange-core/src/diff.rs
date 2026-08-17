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
    /// The filemodes git reports for the two sides — carried for every
    /// file, because a mode hunk sits beside content hunks and needs
    /// them wherever it appears (ADR 0017). They cost nothing: the diff
    /// delta names both, where [`FileDiff::sides`] pays an odb read and
    /// a content hash.
    pub modes: FileModes,
    /// Blob identity and size per side (ADR 0009): the whole-file hunk's
    /// anchor material and the diff placeholder's sizes. The changed
    /// side is the diff's new side. Worktree diffs populate it for the
    /// files that present a whole-file hunk — binary and hunk-less ones
    /// — since the on-disk content hash is the stated refresh cost;
    /// index diffs populate it for every file (two odb header reads), so
    /// a binary worktree over staged text still derives a committable
    /// whole-file payload.
    pub sides: Option<FileSides>,
}

/// The filemodes of one diff's two sides. A side is `None` where it does
/// not exist (added files' HEAD, deletions' changed) or where git reports
/// no mode for it — a platform with `core.filemode` off, where no mode
/// change is visible to compare in the first place. Comparisons treat
/// `None` as unknown rather than as `100644`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileModes {
    pub head: Option<u32>,
    pub changed: Option<u32>,
}

impl FileModes {
    /// The difference between the two sides, split by what it changes
    /// ([`ModeDelta`]) — `None` unless both sides carry a mode and those
    /// modes differ. The one definition of "a mode delta": the universe
    /// presents the `Mode` flavour as a mode hunk (ADR 0017) and
    /// frontends name both flavours from it.
    pub fn delta(&self) -> Option<ModeDelta> {
        ModeDelta::between(self.head?, self.changed?)
    }

    /// The delta this side carries only if it is a permission flip — the
    /// mode hunk's material. A `Type` delta stays with the whole-file
    /// hunk (ADR 0017, issue #100), so it reads as no flip here.
    pub(crate) fn flip(&self) -> Option<ModeDelta> {
        self.delta()
            .filter(|delta| matches!(delta, ModeDelta::Mode { .. }))
    }
}

/// The two sides of a change presenting a whole-file hunk (ADR 0009,
/// ADR 0017). A `None` side doesn't exist: no `head` for added/untracked
/// files, no `changed` for deletions. Modes are [`FileModes`]' — they are
/// carried for every file, not only these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSides {
    pub head: Option<SideInfo>,
    pub changed: Option<SideInfo>,
}

impl FileSides {
    /// The changed-side content hash, `None` for a deletion.
    pub fn changed_oid(&self) -> Option<String> {
        self.changed.as_ref().map(|side| side.oid.clone())
    }

    /// Whether both sides name the same blob — the change carries no
    /// content at all, so whatever else differs is the whole of it
    /// (ADR 0017: a mode-only change).
    pub fn same_blob(&self) -> bool {
        match (&self.head, &self.changed) {
            (Some(head), Some(changed)) => head.oid == changed.oid,
            _ => false,
        }
    }

    /// The sides as the OID pair records anchor on (ADR 0009). Modes
    /// deliberately stay out: records carry no mode bits (ADR 0017).
    pub fn oid_anchor(&self) -> crate::state::OidAnchor {
        crate::state::OidAnchor {
            head: self.head.as_ref().map(|side| side.oid.clone()),
            changed: self.changed_oid(),
        }
    }
}

/// A filemode difference between a change's two sides, told apart by
/// what it actually changes (ADR 0017). Both carry the modes git printed
/// — octal, whole, as they appear in a diff header.
///
/// The split is load-bearing twice: the universe presents a `Mode` delta
/// as a stand-alone mode hunk while a `Type` delta keeps the whole-file
/// hunk (issue #100 owns its presentation), and a placeholder that called
/// a symlink swap a mode change would misname it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeDelta {
    /// Permission bits on the same kind of object — a chmod.
    Mode { before: u32, after: u32 },
    /// The object kind itself: a file swapped for a symlink, or either
    /// for a gitlink.
    Type { before: u32, after: u32 },
}

impl ModeDelta {
    /// git's object-kind bits — the high half of a filemode.
    const KIND: u32 = 0o170000;

    /// A mode's object-kind half: what tells a regular file from a
    /// symlink or a gitlink, with the permission bits masked off. The
    /// split lives here, beside the delta flavours it defines, so no
    /// caller re-derives it from a literal.
    pub(crate) fn kind_bits(mode: u32) -> u32 {
        mode & Self::KIND
    }

    /// The delta between two modes, `None` when they agree.
    fn between(before: u32, after: u32) -> Option<Self> {
        if before == after {
            None
        } else if before & Self::KIND == after & Self::KIND {
            Some(ModeDelta::Mode { before, after })
        } else {
            Some(ModeDelta::Type { before, after })
        }
    }
}

/// One side of a changed file: its blob content hash
/// (`hash-object`-style) and byte size. The side's filemode lives in
/// [`FileModes`], which every file carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideInfo {
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

impl ChangeKind {
    /// The one-letter sigil every frontend's file rows carry (ADR 0006:
    /// shared presentation vocabulary lives in core, like
    /// `GitOperation::label`).
    pub fn sigil(&self) -> char {
        match self {
            ChangeKind::Added => 'A',
            ChangeKind::Modified => 'M',
            ChangeKind::Deleted => 'D',
            ChangeKind::TypeChanged => 'T',
            ChangeKind::Untracked => '?',
            ChangeKind::Conflicted => 'U',
        }
    }
}
