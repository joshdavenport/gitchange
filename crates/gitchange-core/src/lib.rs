mod backend;
mod commit;
mod diff;
mod engine;
mod error;
mod git2_backend;
mod hunk_id;
mod matcher;
mod repo;
mod snapshot;
mod state;
mod state_file;
mod universe;
mod vocabulary;
mod wire;

pub use backend::{CommitPathSpec, CommittedId, GitBackend, HunkHeader};
pub use commit::{
    AMEND_FLAG, CommitMessage, CommitOptions, CommitOutcome, CommitPayload, NO_EDIT_FLAG,
    NO_VERIFY_FLAG, PayloadFile, PayloadHunk, WholeFilePayload, commit_echo,
};
pub use diff::{
    ChangeKind, DiffHunk, FileDiff, FileModes, FileSides, HunkLine, ModeDelta, RepoDiffs, SideInfo,
};
pub use engine::{Condition, Engine, EngineEvent};
pub use error::{ApplySite, Error, LockHolder};
pub use hunk_id::{HunkAddress, HunkId};
pub use matcher::Advisory;
pub use repo::{
    AssignTarget, Deletion, OpOutcome, RefreshOutcome, Release, Repo, Roster, StagingTarget,
    SweepOutcome, Undeletable,
};
pub use snapshot::{CommitInfo, FileGroup, GitOperation, GroupKind, Head, Snapshot};
pub use state::{Changelist, MembershipRecord, OidAnchor, RESERVED_NAMES, RecordCounts};
pub use universe::{ChangedFile, FileStage, Hunk, HunkIdentity, HunkStage, file_stage};
pub use vocabulary::{
    ACTIVE_MARKER, ALL, ARROW, CONFLICTS, FOR_THE_NEXT_REFRESH, NO_REVIVAL, PARTIALLY_STAGED,
    RESOLVE_OUTSIDE_GITCHANGE, SEPARATOR, STAGED, STAGED_STALE, UNASSIGNED, UNSTAGED,
    conflicted_hint, count_noun, holder_label, target_line, target_named, unknown_changelist,
};
pub use wire::{HunkContent, diff_envelope, status_envelope};
