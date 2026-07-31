use std::path::PathBuf;

use crate::snapshot::GitOperation;

/// Core's error contract. Variants are carved by what the caller must do
/// about them (ADR 0006); `git2::Error` never appears here — backend
/// failures are wrapped opaquely in [`Error::Backend`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("not a git repository (or any of its parents): {}", path.display())]
    NotARepository { path: PathBuf },

    #[error("no changelist named '{name}'")]
    UnknownChangelist { name: String },

    #[error("'{name}' is a reserved name and cannot be a changelist")]
    ReservedName { name: String },

    #[error("a changelist named '{name}' already exists")]
    ChangelistExists { name: String },

    #[error("invalid changelist name: {reason}")]
    InvalidName { reason: String },

    #[error(
        "gitchange state is locked by another process ({}); \
         if no gitchange is running, remove the lockfile and retry",
        path.display()
    )]
    LockContention { path: PathBuf },

    /// A git operation is in progress (ADR 0007): the next commit would
    /// conclude it with one changelist's content, so commit refuses.
    /// Staging is never guarded by this.
    #[error("{} in progress — conclude or abort it first", operation.label())]
    OperationInProgress { operation: GitOperation },

    /// The commit payload is empty: the changelist has no staged hunks
    /// (ADR 0004 — the frontend answers with the stage-all-and-commit
    /// offer, never a silent auto-stage).
    #[error("nothing staged to commit")]
    NothingStaged,

    /// `git commit` exited nonzero with nothing committed. Usually a
    /// hook rejection (the dominant cause once gitchange builds the
    /// index itself), though anything git refuses lands here too — an
    /// empty message, a signing failure. `stderr` carries git's and the
    /// hook's captured output either way (ADR 0004).
    #[error("commit rejected:\n{stderr}")]
    HookRejected { stderr: String },

    /// Non-UTF-8 paths are unsupported (ADR 0010): refresh fails loudly
    /// rather than persisting a lossy path that would break identity.
    #[error(
        "unsupported non-UTF-8 path in repository: {}",
        String::from_utf8_lossy(path)
    )]
    NonUtf8Path { path: Vec<u8> },

    /// The state file could not be read, parsed, or written.
    #[error("gitchange state error: {0}")]
    State(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("git backend error: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}
