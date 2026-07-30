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

    fn workdir(&self) -> Option<PathBuf> {
        self.repo.workdir().map(Path::to_path_buf)
    }

    fn head_oid(&self) -> Result<Option<String>, Error> {
        match self.repo.head() {
            Ok(head) => {
                let commit = head.peel_to_commit().map_err(backend_error)?;
                Ok(Some(commit.id().to_string()))
            }
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

    fn paths_changed_since(&self, baseline_oid: &str) -> Result<Option<Vec<String>>, Error> {
        // A malformed id (hand-edited state file) is as unresolvable as a
        // gc'd one.
        let Ok(oid) = git2::Oid::from_str(baseline_oid) else {
            return Ok(None);
        };
        let baseline_tree = match self.repo.find_commit(oid) {
            Ok(commit) => commit.tree().map_err(backend_error)?,
            Err(err) if err.code() == git2::ErrorCode::NotFound => return Ok(None),
            Err(err) => return Err(backend_error(err)),
        };
        let head_tree = self.head_tree()?;
        let diff = self
            .repo
            .diff_tree_to_tree(Some(&baseline_tree), head_tree.as_ref(), None)
            .map_err(backend_error)?;
        let mut paths = Vec::new();
        for delta in diff.deltas() {
            for file in [delta.old_file(), delta.new_file()] {
                let Some(bytes) = file.path_bytes() else {
                    continue;
                };
                // Membership records key on UTF-8 paths (ADR 0010), so a
                // non-UTF-8 changed path can never hit a record: skipped,
                // not a loud failure — this diff sees all of history, not
                // just gitchange-managed files.
                if let Ok(path) = std::str::from_utf8(bytes) {
                    paths.push(path.to_owned());
                }
            }
        }
        paths.sort_unstable();
        paths.dedup();
        Ok(Some(paths))
    }

    fn stage_worktree_range(&self, path: &str, new_range: (u32, u32)) -> Result<(), Error> {
        let mut options = git2::DiffOptions::new();
        options
            .pathspec(path)
            .disable_pathspec_match(true)
            .include_typechange(true)
            .ignore_submodules(true)
            // One context line, not the default three: this diff's base
            // (the index) differs from the universe's (HEAD), so an
            // index-only delta sitting between two universe hunks would
            // merge context-3 hunks across it — and hunk-wise apply is
            // all-or-nothing, staging more than the hunk asked for. One
            // line keeps such regions split while still anchoring pure
            // deletions (libgit2 anchors a hunk at `new_start`, which for
            // a zero-context deletion names the line *before* it — off by
            // one, and the apply fails).
            .context_lines(1);
        let diff = self
            .repo
            .diff_index_to_workdir(None, Some(&mut options))
            .map_err(backend_error)?;
        // Both this diff's new side and the caller's range address the
        // same worktree file, so line numbers compare directly.
        self.apply_to_index(&diff, new_range)
    }

    fn unstage_head_range(&self, path: &str, old_range: (u32, u32)) -> Result<(), Error> {
        let head_tree = self.head_tree()?;
        let mut options = git2::DiffOptions::new();
        options
            .pathspec(path)
            .disable_pathspec_match(true)
            .reverse(true)
            .include_typechange(true)
            .ignore_submodules(true)
            // One context line, same reasoning as stage_worktree_range: a
            // staged region between two universe hunks must not merge the
            // apply's hunks across it.
            .context_lines(1);
        let diff = self
            .repo
            .diff_tree_to_index(head_tree.as_ref(), None, Some(&mut options))
            .map_err(backend_error)?;
        // Reversed, so sides swap: the hunks' new side addresses HEAD —
        // the space the caller's range lives in.
        self.apply_to_index(&diff, old_range)
    }

    fn stage_path(&self, path: &str) -> Result<(), Error> {
        let mut index = self.repo.index().map_err(backend_error)?;
        let on_disk = self
            .repo
            .workdir()
            .map(|root| root.join(path).symlink_metadata().is_ok())
            .unwrap_or(false);
        if on_disk {
            index.add_path(Path::new(path)).map_err(backend_error)?;
        } else if index.get_path(Path::new(path), 0).is_some() {
            index.remove_path(Path::new(path)).map_err(backend_error)?;
        }
        index.write().map_err(backend_error)
    }

    fn unstage_path(&self, path: &str) -> Result<(), Error> {
        let mut index = self.repo.index().map_err(backend_error)?;
        let head_entry = self
            .head_tree()?
            .and_then(|tree| tree.get_path(Path::new(path)).ok());
        match head_entry {
            Some(entry) => {
                // Blob and mode from HEAD; zeroed stat fields make the
                // entry stat-dirty, exactly like `git reset -- <path>`.
                index
                    .add(&git2::IndexEntry {
                        ctime: git2::IndexTime::new(0, 0),
                        mtime: git2::IndexTime::new(0, 0),
                        dev: 0,
                        ino: 0,
                        mode: entry.filemode() as u32,
                        uid: 0,
                        gid: 0,
                        file_size: 0,
                        id: entry.id(),
                        flags: 0,
                        flags_extended: 0,
                        path: path.as_bytes().to_vec(),
                    })
                    .map_err(backend_error)?;
            }
            None if index.get_path(Path::new(path), 0).is_some() => {
                index.remove_path(Path::new(path)).map_err(backend_error)?;
            }
            None => {}
        }
        index.write().map_err(backend_error)
    }
}

impl Git2Backend {
    /// Apply `diff` to the live index, keeping only hunks whose change
    /// core — the +/- lines, context skirts excluded — overlaps `target`
    /// on the diff's new side. Selection must ignore context because this
    /// diff's base differs from the universe's: a padded header range can
    /// brush a neighbouring universe hunk's region and over-apply.
    /// libgit2 computes every postimage before writing, so a failure
    /// changes nothing.
    fn apply_to_index(&self, diff: &git2::Diff, target: (u32, u32)) -> Result<(), Error> {
        let cores = hunk_change_cores(diff)?;
        let mut options = git2::ApplyOptions::new();
        options.hunk_callback(move |hunk| {
            hunk.as_ref().is_some_and(|hunk| {
                let header = hunk_header(hunk);
                cores
                    .iter()
                    .find(|core| core.header == header)
                    .is_some_and(|core| ranges_overlap(core.range, target))
            })
        });
        self.repo
            .apply(diff, git2::ApplyLocation::Index, Some(&mut options))
            .map_err(backend_error)
    }
}

/// A hunk's identity within one diff — headers are unique because hunks
/// never overlap.
type HunkHeader = (u32, u32, u32, u32);

fn hunk_header(hunk: &git2::DiffHunk) -> HunkHeader {
    (
        hunk.old_start(),
        hunk.old_lines(),
        hunk.new_start(),
        hunk.new_lines(),
    )
}

struct HunkCore {
    header: HunkHeader,
    /// New-side (start, lines) spanned by the hunk's actual +/- lines; a
    /// pure deletion is zero-length at the gap it leaves.
    range: (u32, u32),
}

/// The change core of every hunk in `diff`: where its +/- lines land on
/// the new side, context excluded.
fn hunk_change_cores(diff: &git2::Diff) -> Result<Vec<HunkCore>, Error> {
    let mut cores = Vec::new();
    for delta_index in 0..diff.deltas().len() {
        let Some(patch) = git2::Patch::from_diff(diff, delta_index).map_err(backend_error)? else {
            continue;
        };
        for hunk_index in 0..patch.num_hunks() {
            let (hunk, line_count) = patch.hunk(hunk_index).map_err(backend_error)?;
            // The new-side line most recently passed. A change run with
            // additions is located by them; a pure-deletion run occupies
            // the gap after `last_new` and counts as touching both
            // neighbouring lines — line-before vs line-after conventions
            // differ between producers, and a gap has no width to settle
            // it.
            let mut last_new = hunk.new_start().saturating_sub(1);
            let mut start = u32::MAX;
            let mut end = 0u32;
            let mut deletion_run_anchor: Option<u32> = None;
            let settle_deletion_run = |anchor: Option<u32>, start: &mut u32, end: &mut u32| {
                if let Some(anchor) = anchor {
                    // [anchor, anchor + 2): the line before the gap and
                    // the line after it.
                    *start = (*start).min(anchor);
                    *end = (*end).max(anchor + 2);
                }
            };
            for line_index in 0..line_count {
                let line = patch
                    .line_in_hunk(hunk_index, line_index)
                    .map_err(backend_error)?;
                match line.origin() {
                    '+' => {
                        let new_line = line.new_lineno().unwrap_or(last_new + 1);
                        start = start.min(new_line);
                        end = end.max(new_line + 1);
                        last_new = new_line;
                        // The additions locate this run: it replaces, not
                        // purely deletes.
                        deletion_run_anchor = None;
                    }
                    '-' => {
                        deletion_run_anchor.get_or_insert(last_new);
                    }
                    ' ' => {
                        settle_deletion_run(deletion_run_anchor.take(), &mut start, &mut end);
                        if let Some(new_line) = line.new_lineno() {
                            last_new = new_line;
                        }
                    }
                    // EOFNL markers belong to the +/- run above them.
                    _ => {}
                }
            }
            settle_deletion_run(deletion_run_anchor, &mut start, &mut end);
            let range = if start == u32::MAX {
                (hunk.new_start(), hunk.new_lines())
            } else {
                (start, end.saturating_sub(start))
            };
            cores.push(HunkCore {
                header: hunk_header(&hunk),
                range,
            });
        }
    }
    Ok(cores)
}

/// Overlap on (start, lines) ranges, widening empty ranges (pure
/// insertions/removals) to one line — the same rule the hunk universe
/// pairs hunks with.
fn ranges_overlap(a: (u32, u32), b: (u32, u32)) -> bool {
    let span = |(start, lines): (u32, u32)| (start, start + lines.max(1));
    let (a_start, a_end) = span(a);
    let (b_start, b_end) = span(b);
    a_start < b_end && b_start < a_end
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
