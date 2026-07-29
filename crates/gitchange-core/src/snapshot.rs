use crate::backend::ChangedFile;
use crate::state::Changelist;

/// The immutable result of one refresh — the only data structure frontends
/// read. Grows hunks and membership in later tickets; for now it carries
/// the changed-file list and the changelist roster.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Changed files, sorted by path.
    pub files: Vec<ChangedFile>,
    /// Changelists in user order.
    pub changelists: Vec<Changelist>,
    /// Name of the active changelist; `None` only when no changelists
    /// exist.
    pub active: Option<String>,
}
