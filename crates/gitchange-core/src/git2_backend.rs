use std::path::{Path, PathBuf};

use crate::backend::GitBackend;
use crate::diff::{ChangeKind, DiffHunk, FileDiff, HunkLine, RepoDiffs};
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

    /// HEAD's tree, or `None` — the empty tree — on an unborn branch
    /// (fresh `git init`, ADR 0007).
    fn head_tree(&self) -> Result<Option<git2::Tree<'_>>, Error> {
        match self.repo.head() {
            Ok(head) => head.peel_to_tree().map(Some).map_err(backend_error),
            Err(err)
                if matches!(
                    err.code(),
                    git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
                ) =>
            {
                Ok(None)
            }
            Err(err) => Err(backend_error(err)),
        }
    }
}

impl GitBackend for Git2Backend {
    fn diffs(&self) -> Result<RepoDiffs, Error> {
        let head_tree = self.head_tree()?;

        let mut worktree_options = git2::DiffOptions::new();
        worktree_options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true)
            .include_typechange(true)
            .ignore_submodules(true);
        let worktree = self
            .repo
            .diff_tree_to_workdir(head_tree.as_ref(), Some(&mut worktree_options))
            .map_err(backend_error)?;

        let mut index_options = git2::DiffOptions::new();
        index_options
            .include_typechange(true)
            .ignore_submodules(true);
        let index = self
            .repo
            .diff_tree_to_index(head_tree.as_ref(), None, Some(&mut index_options))
            .map_err(backend_error)?;

        Ok(RepoDiffs {
            worktree: collect_file_diffs(&worktree)?,
            index: collect_file_diffs(&index)?,
        })
    }

    fn state_dir(&self) -> PathBuf {
        // `Repository::path()` is the private git dir — for a linked
        // worktree that is `.git/worktrees/<id>/`, matching git's
        // `--git-path gitchange` resolution (ADR 0002).
        self.repo.path().join("gitchange")
    }
}

fn collect_file_diffs(diff: &git2::Diff) -> Result<Vec<FileDiff>, Error> {
    let mut files = Vec::new();
    for (position, delta) in diff.deltas().enumerate() {
        let Some(kind) = change_kind(delta.status()) else {
            continue;
        };
        let path = utf8_path(&delta)?;

        // Unmerged paths are quarantined (ADR 0007): listed, excluded
        // from the hunk universe — no hunk content extracted.
        let (binary, hunks) = if kind == ChangeKind::Conflicted {
            (false, Vec::new())
        } else {
            match git2::Patch::from_diff(diff, position).map_err(backend_error)? {
                Some(patch) => (patch.delta().flags().is_binary(), patch_hunks(&patch)?),
                None => (delta.flags().is_binary(), Vec::new()),
            }
        };
        files.push(FileDiff {
            path,
            kind,
            binary,
            hunks,
        });
    }
    Ok(files)
}

fn patch_hunks(patch: &git2::Patch) -> Result<Vec<DiffHunk>, Error> {
    let mut hunks = Vec::with_capacity(patch.num_hunks());
    for hunk_index in 0..patch.num_hunks() {
        let (hunk, line_count) = patch.hunk(hunk_index).map_err(backend_error)?;
        let mut lines = Vec::with_capacity(line_count);
        for line_index in 0..line_count {
            let line = patch
                .line_in_hunk(hunk_index, line_index)
                .map_err(backend_error)?;
            lines.push(HunkLine {
                origin: line.origin(),
                // Lossy decoding is deterministic, so matching is stable
                // — but not injective: distinct invalid byte sequences
                // collide at U+FFFD, so non-UTF-8 text hunks can
                // over-match (a wrong `●` instead of `◑`). Verbatim
                // bytes replace this if that ever bites; paths get the
                // strict treatment instead (ADR 0010).
                content: String::from_utf8_lossy(line.content()).into_owned(),
            });
        }
        hunks.push(DiffHunk {
            old_start: hunk.old_start(),
            old_lines: hunk.old_lines(),
            new_start: hunk.new_start(),
            new_lines: hunk.new_lines(),
            lines,
        });
    }
    Ok(hunks)
}

/// ADR 0010: paths are UTF-8 or a loud failure — a lossy path would break
/// byte-level identity against future refreshes.
fn utf8_path(delta: &git2::DiffDelta) -> Result<String, Error> {
    let bytes = delta
        .new_file()
        .path_bytes()
        .or_else(|| delta.old_file().path_bytes())
        .unwrap_or_default();
    match std::str::from_utf8(bytes) {
        Ok(path) => Ok(path.to_owned()),
        Err(_) => Err(Error::NonUtf8Path {
            path: bytes.to_vec(),
        }),
    }
}

fn change_kind(status: git2::Delta) -> Option<ChangeKind> {
    use git2::Delta;
    match status {
        Delta::Added => Some(ChangeKind::Added),
        Delta::Deleted => Some(ChangeKind::Deleted),
        Delta::Modified => Some(ChangeKind::Modified),
        Delta::Untracked => Some(ChangeKind::Untracked),
        Delta::Typechange => Some(ChangeKind::TypeChanged),
        Delta::Conflicted => Some(ChangeKind::Conflicted),
        // Rename detection is never enabled (ADR 0011), so these are
        // unreachable — mapped defensively rather than panicking.
        Delta::Renamed | Delta::Copied => Some(ChangeKind::Modified),
        _ => None,
    }
}

fn backend_error(err: git2::Error) -> Error {
    Error::Backend(Box::new(err))
}
