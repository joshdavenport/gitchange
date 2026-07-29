use std::path::PathBuf;

/// Core's error contract. Variants are carved by what the caller must do
/// about them (ADR 0006); `git2::Error` never appears here — backend
/// failures are wrapped opaquely in [`Error::Backend`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("not a git repository (or any of its parents): {}", path.display())]
    NotARepository { path: PathBuf },

    #[error("git backend error: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}
