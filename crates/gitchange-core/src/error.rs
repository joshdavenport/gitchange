use std::path::PathBuf;

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
