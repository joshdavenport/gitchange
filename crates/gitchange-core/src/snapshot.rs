use crate::matcher::Notice;
use crate::state::Changelist;
use crate::universe::ChangedFile;

/// The immutable result of one refresh — the only data structure frontends
/// read. Hunks carry their owning changelist (written by the matcher);
/// notices record this refresh's automatic membership decisions.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// The hunk universe (ADR 0003), sorted by path.
    pub files: Vec<ChangedFile>,
    /// Changelists in user order.
    pub changelists: Vec<Changelist>,
    /// Name of the active changelist; `None` only when no changelists
    /// exist.
    pub active: Option<String>,
    /// Automatic membership decisions worth spot-checking, in file
    /// order. Not persisted: a decision becomes a record, so it surfaces
    /// exactly once.
    pub notices: Vec<Notice>,
}

impl Snapshot {
    /// The files belonging to one changelist for the All view — a file is
    /// "in" a changelist when at least one hunk is owned by it. `None`
    /// selects unassigned, which also holds hunk-less files (changed
    /// binaries until ticket 35).
    pub fn files_in(&self, changelist: Option<&str>) -> Vec<&ChangedFile> {
        self.files
            .iter()
            .filter(|file| {
                let owned = file
                    .hunks
                    .iter()
                    .any(|hunk| hunk.changelist.as_deref() == changelist);
                owned || (changelist.is_none() && file.hunks.is_empty())
            })
            .collect()
    }
}
