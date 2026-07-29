use crate::backend::ChangedFile;

/// The immutable result of one refresh — the only data structure frontends
/// read. Grows changelists and hunks in later tickets; for the tracer it
/// carries the changed-file list.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Changed files, sorted by path.
    pub files: Vec<ChangedFile>,
}
