use std::path::Path;

use crate::backend::{ChangeKind, ChangedFile, GitBackend};
use crate::error::Error;

pub(crate) struct Git2Backend {
    repo: git2::Repository,
}

impl Git2Backend {
    pub(crate) fn discover(path: &Path) -> Result<Self, Error> {
        match git2::Repository::discover(path) {
            Ok(repo) => Ok(Self { repo }),
            Err(err) if err.code() == git2::ErrorCode::NotFound => Err(Error::NotARepository {
                path: path.to_path_buf(),
            }),
            Err(err) => Err(backend_error(err)),
        }
    }
}

impl GitBackend for Git2Backend {
    fn changed_files(&self) -> Result<Vec<ChangedFile>, Error> {
        let mut options = git2::StatusOptions::new();
        options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .exclude_submodules(true);
        let statuses = self
            .repo
            .statuses(Some(&mut options))
            .map_err(backend_error)?;

        let mut files = Vec::new();
        for entry in statuses.iter() {
            let Some(kind) = change_kind(entry.status()) else {
                continue;
            };
            let path = String::from_utf8_lossy(entry.path_bytes()).into_owned();
            files.push(ChangedFile { path, kind });
        }
        Ok(files)
    }
}

fn change_kind(status: git2::Status) -> Option<ChangeKind> {
    use git2::Status;

    // Deleted outranks Added so a staged-new file since removed from the
    // worktree reads as gone, not present.
    if status.is_conflicted() {
        Some(ChangeKind::Conflicted)
    } else if status.is_wt_new() {
        Some(ChangeKind::Untracked)
    } else if status.intersects(Status::WT_DELETED | Status::INDEX_DELETED) {
        Some(ChangeKind::Deleted)
    } else if status.is_index_new() {
        Some(ChangeKind::Added)
    } else if status.intersects(Status::WT_TYPECHANGE | Status::INDEX_TYPECHANGE) {
        Some(ChangeKind::TypeChanged)
    } else if status.intersects(Status::WT_MODIFIED | Status::INDEX_MODIFIED) {
        Some(ChangeKind::Modified)
    } else {
        None
    }
}

fn backend_error(err: git2::Error) -> Error {
    Error::Backend(Box::new(err))
}
