use std::path::Path;

use crate::backend::GitBackend;
use crate::error::Error;
use crate::git2_backend::Git2Backend;
use crate::snapshot::Snapshot;
use crate::state::State;
use crate::state_file;
use crate::universe;

/// A handle on one git repository, holding the backend behind the
/// `GitBackend` seam. Frontends reach git only through this type.
pub struct Repo {
    backend: Box<dyn GitBackend>,
}

impl Repo {
    /// Open the repository containing `path`, searching upward like git.
    pub fn discover(path: &Path) -> Result<Self, Error> {
        let backend = Git2Backend::discover(path)?;
        Ok(Self {
            backend: Box::new(backend),
        })
    }

    /// One blocking recompute pass producing a fresh snapshot.
    pub fn refresh(&self) -> Result<Snapshot, Error> {
        let files = universe::build(self.backend.diffs()?);
        // Reads take no lock: writers replace the file atomically, so a
        // read sees either the old or the new state, never a torn one.
        let state = state_file::load(&self.backend.state_dir())?;
        Ok(Snapshot {
            files,
            changelists: state.changelists,
            active: state.active,
        })
    }

    /// Create a changelist. The first one created becomes active.
    pub fn create_changelist(&self, name: &str) -> Result<(), Error> {
        self.update_state(|state| state.create(name))
    }

    /// Rename a changelist, carrying the active marker with it.
    pub fn rename_changelist(&self, from: &str, to: &str) -> Result<(), Error> {
        self.update_state(|state| state.rename(from, to))
    }

    /// Delete a changelist. Deleting the active one promotes the first
    /// remaining changelist.
    pub fn delete_changelist(&self, name: &str) -> Result<(), Error> {
        self.update_state(|state| state.delete(name))
    }

    /// Set the active changelist.
    pub fn switch(&self, name: &str) -> Result<(), Error> {
        self.update_state(|state| state.switch(name))
    }

    /// One locked load-mutate-save cycle (ADR 0002): fail-fast lock,
    /// atomic replace, nothing persisted when the mutation errors.
    fn update_state(
        &self,
        mutate: impl FnOnce(&mut State) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let dir = self.backend.state_dir();
        let _lock = state_file::lock(&dir)?;
        let mut state = state_file::load(&dir)?;
        mutate(&mut state)?;
        state_file::save(&dir, &state)
    }
}
