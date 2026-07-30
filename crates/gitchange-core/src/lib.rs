mod backend;
mod diff;
mod error;
mod git2_backend;
mod repo;
mod snapshot;
mod state;
mod state_file;
mod universe;

pub use backend::GitBackend;
pub use diff::{ChangeKind, DiffHunk, FileDiff, HunkLine, RepoDiffs};
pub use error::Error;
pub use repo::Repo;
pub use snapshot::Snapshot;
pub use state::{Changelist, RESERVED_NAMES};
pub use universe::{ChangedFile, FileStage, Hunk, HunkStage};
