use std::path::Path;

use crate::backend::GitBackend;
use crate::error::Error;
use crate::git2_backend::Git2Backend;
use crate::snapshot::Snapshot;

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
        let mut files = self.backend.changed_files()?;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Snapshot { files })
    }
}
