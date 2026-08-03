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

pub use backend::{CommitPathSpec, GitBackend, HunkHeader};
pub use commit::{
    CommitOptions, CommitOutcome, CommitPayload, PayloadFile, PayloadHunk, WholeFilePayload,
};
pub use diff::{BinarySides, BlobInfo, ChangeKind, DiffHunk, FileDiff, HunkLine, RepoDiffs};
pub use engine::{Condition, Engine, EngineEvent};
pub use error::Error;
pub use matcher::Notice;
pub use repo::{Repo, StageAllOutcome};
pub use snapshot::{CommitInfo, FileGroup, GitOperation, GroupKind, Head, Snapshot};
pub use state::{Changelist, MembershipRecord, OidAnchor, RESERVED_NAMES};
pub use universe::{ChangedFile, FileStage, Hunk, HunkStage};
pub use vocabulary::{ACTIVE_MARKER, ARROW, SEPARATOR, count_noun};
