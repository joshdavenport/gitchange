use crate::state::Changelist;
use crate::universe::ChangedFile;

/// The immutable result of one refresh — the only data structure frontends
/// read. Grows membership in later tickets; for now it carries the hunk
/// universe and the changelist roster.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The hunk universe (ADR 0003), sorted by path.
    pub files: Vec<ChangedFile>,
    /// Changelists in user order.
    pub changelists: Vec<Changelist>,
    /// Name of the active changelist; `None` only when no changelists
    /// exist.
    pub active: Option<String>,
}
