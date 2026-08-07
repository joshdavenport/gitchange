mod backend;
mod commit;
mod diff;
mod engine;
mod error;
mod git2_backend;
mod matcher;
mod repo;
mod snapshot;
mod state;
mod state_file;
mod universe;
mod vocabulary;

pub use backend::{CommitPathSpec, CommittedId, GitBackend, HunkHeader};
pub use commit::{
    AMEND_FLAG, CommitOptions, CommitOutcome, CommitPayload, NO_VERIFY_FLAG, PayloadFile,
    PayloadHunk, WholeFilePayload, commit_echo,
};
pub use diff::{BinarySides, BlobInfo, ChangeKind, DiffHunk, FileDiff, HunkLine, RepoDiffs};
pub use engine::{Condition, Engine, EngineEvent};
pub use error::{ApplySite, Error};
pub use matcher::Advisory;
pub use repo::{OpOutcome, Repo};
pub use snapshot::{CommitInfo, FileGroup, GitOperation, GroupKind, Head, Snapshot};
pub use state::{Changelist, MembershipRecord, OidAnchor, RESERVED_NAMES};
pub use universe::{ChangedFile, FileStage, Hunk, HunkIdentity, HunkStage};
pub use vocabulary::{
    ACTIVE_MARKER, ALL, ARROW, CONFLICTS, PARTIALLY_STAGED, RESOLVE_OUTSIDE_GITCHANGE, SEPARATOR,
    STAGED, STAGED_STALE, UNASSIGNED, UNSTAGED, conflicted_hint, count_noun,
};
