use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::backend::{CommitPathSpec, CommittedId, GitBackend, HunkHeader};
use crate::commit::CommitOptions;
use crate::diff::{BinarySides, BlobInfo, ChangeKind, DiffHunk, FileDiff, HunkLine, RepoDiffs};
use crate::error::Error;
use crate::snapshot::{CommitInfo, GitOperation, Head};
use crate::universe::ranges_overlap;

/// The state directory's name under the private git dir (ADR 0002).
/// Deliberately its own constant rather than a shared program-name
/// token: this is an on-disk identifier every existing repo already
/// carries, so renaming the program must not silently orphan it.
const STATE_DIR_NAME: &str = "gitchange";

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

    /// HEAD's commit, or `None` on an unborn branch (fresh `git init`,
    /// ADR 0007) — the one place the unborn-HEAD error codes are spelled
    /// out; `head_tree` and `head_oid` both read through it.
    fn head_commit(&self) -> Result<Option<git2::Commit<'_>>, Error> {
        match self.repo.head() {
            Ok(head) => head.peel_to_commit().map(Some).map_err(backend_error),
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

    /// HEAD's tree, or `None` — the empty tree — on an unborn branch.
    fn head_tree(&self) -> Result<Option<git2::Tree<'_>>, Error> {
        match self.head_commit()? {
            Some(commit) => commit.tree().map(Some).map_err(backend_error),
            None => Ok(None),
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
            worktree: self.collect_file_diffs(&worktree, ChangedSide::Worktree)?,
            index: self.collect_file_diffs(&index, ChangedSide::Index)?,
        })
    }

    fn state_dir(&self) -> PathBuf {
        // `Repository::path()` is the private git dir — for a linked
        // worktree that is `.git/worktrees/<id>/`, matching git's
        // `--git-path gitchange` resolution (ADR 0002).
        self.repo.path().join(STATE_DIR_NAME)
    }

    fn workdir(&self) -> Option<PathBuf> {
        self.repo.workdir().map(Path::to_path_buf)
    }

    fn head_oid(&self) -> Result<Option<String>, Error> {
        Ok(self.head_commit()?.map(|commit| commit.id().to_string()))
    }

    fn head(&self) -> Result<Head, Error> {
        match self.repo.head() {
            Ok(head) if self.repo.head_detached().map_err(backend_error)? => {
                let commit = head.peel_to_commit().map_err(backend_error)?;
                Ok(Head::Detached {
                    short_id: short_id(&commit)?,
                })
            }
            Ok(head) => Ok(Head::Branch {
                name: head.shorthand().unwrap_or("HEAD").to_owned(),
            }),
            Err(err)
                if matches!(
                    err.code(),
                    git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
                ) =>
            {
                // Unborn: HEAD is a symbolic ref to a branch that has no
                // commits yet — name it like git status does.
                let name = self
                    .repo
                    .find_reference("HEAD")
                    .ok()
                    .and_then(|head| head.symbolic_target().map(str::to_owned))
                    .map(|target| {
                        target
                            .strip_prefix("refs/heads/")
                            .unwrap_or(&target)
                            .to_owned()
                    })
                    .unwrap_or_else(|| "HEAD".to_owned());
                Ok(Head::Unborn { name })
            }
            Err(err) => Err(backend_error(err)),
        }
    }

    fn recent_commits(&self, limit: usize) -> Result<Vec<CommitInfo>, Error> {
        let Some(oid) = self.head_oid()? else {
            return Ok(Vec::new());
        };
        let mut walk = self.repo.revwalk().map_err(backend_error)?;
        walk.push(git2::Oid::from_str(&oid).map_err(backend_error)?)
            .map_err(backend_error)?;
        let mut commits = Vec::new();
        for id in walk.take(limit) {
            let commit = self
                .repo
                .find_commit(id.map_err(backend_error)?)
                .map_err(backend_error)?;
            commits.push(CommitInfo {
                short_id: short_id(&commit)?,
                author: commit.author().name().unwrap_or("").to_owned(),
                summary: commit.summary().unwrap_or("").to_owned(),
            });
        }
        Ok(commits)
    }

    fn operation(&self) -> Result<Option<GitOperation>, Error> {
        use git2::RepositoryState as State;
        Ok(match self.repo.state() {
            State::Merge => Some(GitOperation::Merge),
            State::Rebase | State::RebaseInteractive | State::RebaseMerge => {
                Some(GitOperation::Rebase)
            }
            State::CherryPick | State::CherryPickSequence => Some(GitOperation::CherryPick),
            State::Revert | State::RevertSequence => Some(GitOperation::Revert),
            State::ApplyMailbox | State::ApplyMailboxOrRebase => Some(GitOperation::Am),
            // Clean, and bisect — where committing is legitimate.
            _ => None,
        })
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
        self.apply_to_index(path, &diff, new_range)
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
        self.apply_to_index(path, &diff, old_range)
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

    fn commit_from_index_hunks(
        &self,
        payload: &[CommitPathSpec],
        message: &str,
        options: &CommitOptions,
    ) -> Result<CommittedId, Error> {
        // Temp files live under $GIT_DIR/gitchange/ so the Engine's
        // self-loop filter drops their watcher events (ADR 0005).
        let dir = self.state_dir();
        fs::create_dir_all(&dir).map_err(io_error)?;
        let pid = std::process::id();
        let index_path = dir.join(format!("commit-index-{pid}.tmp"));
        let message_path = dir.join(format!("commit-msg-{pid}.tmp"));
        let result =
            self.commit_via_temp_index(&index_path, &message_path, payload, message, options);
        // Every exit discards the temp files — success or failure, the
        // live index and worktree were never in play (ADR 0004).
        let _ = fs::remove_file(&index_path);
        let _ = fs::remove_file(&message_path);
        result
    }
}

impl Git2Backend {
    fn commit_via_temp_index(
        &self,
        index_path: &Path,
        message_path: &Path,
        payload: &[CommitPathSpec],
        message: &str,
        options: &CommitOptions,
    ) -> Result<CommittedId, Error> {
        // A stale leftover (crashed earlier run) must not seed the index.
        let _ = fs::remove_file(index_path);
        // The empty tree stands in for an unborn HEAD (ADR 0007) — one
        // base for the diff, the apply, and the temp index alike.
        let base_tree = match self.head_tree()? {
            Some(tree) => tree,
            None => {
                let oid = self
                    .repo
                    .treebuilder(None)
                    .and_then(|builder| builder.write())
                    .map_err(backend_error)?;
                self.repo.find_tree(oid).map_err(backend_error)?
            }
        };
        let mut temp = git2::Index::open(index_path).map_err(backend_error)?;
        temp.read_tree(&base_tree).map_err(backend_error)?;

        for spec in payload {
            // A binary file's whole-file selection (ADR 0009): the live
            // index entry — the staged blob — copied into the temp index
            // verbatim; no apply machinery. The OID check mirrors the
            // hunk freshness check below: an index that moved since the
            // payload was derived must not commit a silent substitute.
            if let Some(whole) = &spec.whole_file {
                let live = self.repo.index().map_err(backend_error)?;
                let entry = live.get_path(Path::new(&spec.path), 0);
                let live_oid = entry.as_ref().map(|entry| entry.id.to_string());
                if live_oid != whole.staged_oid {
                    return Err(index_moved(format_args!("{} staged blob", spec.path)));
                }
                match entry {
                    Some(entry) => temp.add(&entry).map_err(backend_error)?,
                    // The payload commits the file's staged deletion.
                    None if temp.get_path(Path::new(&spec.path), 0).is_some() => {
                        temp.remove_path(Path::new(&spec.path))
                            .map_err(backend_error)?;
                    }
                    None => {}
                }
                continue;
            }
            // Same options as `diffs()` produced the payload headers
            // from, so headers compare exactly.
            let mut diff_options = git2::DiffOptions::new();
            diff_options
                .pathspec(&spec.path)
                .disable_pathspec_match(true)
                .include_typechange(true)
                .ignore_submodules(true);
            let diff = self
                .repo
                .diff_tree_to_index(Some(&base_tree), None, Some(&mut diff_options))
                .map_err(backend_error)?;
            // Every requested hunk must still exist in the live index:
            // an index that moved since the payload was derived would
            // otherwise commit a silent subset. Failing here changes
            // nothing (ADR 0004) — the caller retries via its guards.
            let fresh = diff_hunk_headers(&diff)?;
            if let Some(missing) = spec.hunks.iter().find(|header| !fresh.contains(header)) {
                return Err(index_moved(format_args!(
                    "{} hunk at old line {}",
                    spec.path, missing.old_start
                )));
            }
            let headers = spec.hunks.clone();
            let mut apply_options = git2::ApplyOptions::new();
            apply_options.hunk_callback(move |hunk| {
                hunk.as_ref()
                    .is_some_and(|hunk| headers.contains(&hunk_header(hunk)))
            });
            // Postimage blobs land in the odb; the live index is never
            // involved. Per-path applies keep header filtering
            // unambiguous (headers are unique only within one file).
            let applied = self
                .repo
                .apply_to_tree(&base_tree, &diff, Some(&mut apply_options))
                .map_err(backend_error)?;
            match applied.get_path(Path::new(&spec.path), 0) {
                Some(entry) => temp.add(&entry).map_err(backend_error)?,
                // The payload deletes the file.
                None if temp.get_path(Path::new(&spec.path), 0).is_some() => {
                    temp.remove_path(Path::new(&spec.path))
                        .map_err(backend_error)?;
                }
                None => {}
            }
        }
        temp.write().map_err(backend_error)?;

        fs::write(message_path, message).map_err(io_error)?;
        let workdir = self
            .repo
            .workdir()
            .ok_or_else(|| Error::Backend("cannot commit in a bare repository".into()))?;
        // Shell-out is forced: git2's commit API does not run hooks
        // (ADR 0004). Discovery from the worktree resolves linked
        // worktrees' git dirs the same way a user's `git commit` would.
        let mut command = Command::new("git");
        command
            .current_dir(workdir)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env("GIT_INDEX_FILE", index_path)
            .arg("commit")
            .arg("-F")
            .arg(message_path);
        if options.no_verify {
            command.arg("--no-verify");
        }
        if options.amend {
            command.arg("--amend");
        }
        let output = command.output().map_err(io_error)?;
        if !output.status.success() {
            let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let stdout = String::from_utf8_lossy(&output.stdout);
            // Hooks write rejection reasons to either stream; surface
            // both.
            if !stdout.trim().is_empty() {
                if !stderr.is_empty() {
                    stderr.push('\n');
                }
                stderr.push_str(stdout.trim_end());
            }
            return Err(Error::HookRejected { stderr });
        }
        // Both ids read off the new HEAD commit itself. The abbreviation
        // is git's own — `core.abbrev` honoured, expanded for
        // uniqueness — so no caller has to guess a prefix length.
        let commit = self
            .head_commit()?
            .ok_or_else(|| Error::Backend("HEAD unresolved after commit".into()))?;
        Ok(CommittedId {
            oid: commit.id().to_string(),
            short_id: short_id(&commit)?,
        })
    }

    /// Apply `diff` to the live index, keeping only hunks whose change
    /// core — the +/- lines, context skirts excluded — overlaps `target`
    /// on the diff's new side. Selection must ignore context because this
    /// diff's base differs from the universe's: a padded header range can
    /// brush a neighbouring universe hunk's region and over-apply.
    /// libgit2 computes every postimage before writing, so a failure
    /// changes nothing.
    ///
    /// A refusal from the apply itself becomes [`Error::ApplyFailed`],
    /// not an opaque [`Error::Backend`] — that variant is ADR 0003's
    /// trigger for the conditional shell-out fallback, so it must be
    /// distinguishable from a locked index or a broken odb. Only the
    /// `apply` call is mapped that way; setting the call up is ordinary
    /// backend work.
    fn apply_to_index(
        &self,
        path: &str,
        diff: &git2::Diff,
        target: (u32, u32),
    ) -> Result<(), Error> {
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
            .map_err(|error| Error::ApplyFailed {
                path: path.to_owned(),
                detail: error.message().to_owned(),
            })
    }
}

fn hunk_header(hunk: &git2::DiffHunk) -> HunkHeader {
    HunkHeader {
        old_start: hunk.old_start(),
        old_lines: hunk.old_lines(),
        new_start: hunk.new_start(),
        new_lines: hunk.new_lines(),
    }
}

/// Every hunk header in `diff`, in diff order.
fn diff_hunk_headers(diff: &git2::Diff) -> Result<Vec<HunkHeader>, Error> {
    let mut headers = Vec::new();
    for delta_index in 0..diff.deltas().len() {
        let Some(patch) = git2::Patch::from_diff(diff, delta_index).map_err(backend_error)? else {
            continue;
        };
        for hunk_index in 0..patch.num_hunks() {
            let (hunk, _) = patch.hunk(hunk_index).map_err(backend_error)?;
            headers.push(hunk_header(&hunk));
        }
    }
    Ok(headers)
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

/// Which content a diff's new side addresses — decides where a binary
/// file's changed-side hash comes from (ADR 0009).
#[derive(Clone, Copy)]
enum ChangedSide {
    /// diff(HEAD↔worktree): hash the on-disk content at refresh; libgit2
    /// leaves workdir-side blob ids unset.
    Worktree,
    /// diff(HEAD↔index): the staged blob's id is already real.
    Index,
}

impl Git2Backend {
    fn collect_file_diffs(
        &self,
        diff: &git2::Diff,
        changed_side: ChangedSide,
    ) -> Result<Vec<FileDiff>, Error> {
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
            // Worktree diffs pay the anchor cost — a full content hash
            // of the on-disk file (ADR 0009's stated refresh cost) —
            // only for binary files. Index diffs carry sides for every
            // file (two odb header reads): the commit plan needs a
            // staged-blob OID even when a binary worktree change sits
            // over staged text content.
            let compute_sides = match changed_side {
                ChangedSide::Worktree => binary,
                ChangedSide::Index => kind != ChangeKind::Conflicted,
            };
            let binary_sides = if compute_sides {
                Some(self.binary_sides(&delta, &path, changed_side)?)
            } else {
                None
            };
            files.push(FileDiff {
                path,
                kind,
                binary,
                hunks,
                binary_sides,
            });
        }
        Ok(files)
    }

    /// A delta's per-side blob info (ADR 0009): the HEAD-side blob from
    /// the odb, the changed side hashed `hash-object`-style from disk
    /// (worktree diff) or read off the staged blob (index diff). A
    /// missing side — added files' HEAD, deletions' changed — is `None`.
    /// Odb sides read headers only, never content.
    ///
    /// The disk hash sees raw bytes, no clean filters: under a filter
    /// driver (Git LFS) the worktree hash can never equal the staged
    /// blob, so such a file reads permanently `◑` — a known v0.1
    /// limitation; running filters at refresh would mean odb writes.
    fn binary_sides(
        &self,
        delta: &git2::DiffDelta,
        path: &str,
        changed_side: ChangedSide,
    ) -> Result<BinarySides, Error> {
        let blob_info = |id: git2::Oid| -> Result<Option<BlobInfo>, Error> {
            if id.is_zero() {
                return Ok(None);
            }
            let (size, _) = self
                .repo
                .odb()
                .and_then(|odb| odb.read_header(id))
                .map_err(backend_error)?;
            Ok(Some(BlobInfo {
                oid: id.to_string(),
                size: size as u64,
            }))
        };
        let head = blob_info(delta.old_file().id())?;
        let changed = match changed_side {
            ChangedSide::Worktree => self.worktree_blob_info(path)?,
            ChangedSide::Index => blob_info(delta.new_file().id())?,
        };
        Ok(BinarySides { head, changed })
    }

    /// Hash the worktree content at `path` as a blob, `None` when
    /// nothing is on disk. A symlink's blob is its target string (git
    /// semantics), which also keeps a dangling link from failing the
    /// refresh.
    fn worktree_blob_info(&self, path: &str) -> Result<Option<BlobInfo>, Error> {
        let Some(file) = self.repo.workdir().map(|root| root.join(path)) else {
            return Ok(None);
        };
        let Ok(metadata) = file.symlink_metadata() else {
            return Ok(None);
        };
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(&file).map_err(io_error)?;
            let bytes = target.as_os_str().as_encoded_bytes();
            let oid =
                git2::Oid::hash_object(git2::ObjectType::Blob, bytes).map_err(backend_error)?;
            return Ok(Some(BlobInfo {
                oid: oid.to_string(),
                size: bytes.len() as u64,
            }));
        }
        let oid = git2::Oid::hash_file(git2::ObjectType::Blob, &file).map_err(backend_error)?;
        Ok(Some(BlobInfo {
            oid: oid.to_string(),
            size: metadata.len(),
        }))
    }
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

/// A commit's abbreviated id, disambiguated by libgit2 like `git log
/// --abbrev-commit` would.
fn short_id(commit: &git2::Commit<'_>) -> Result<String, Error> {
    let buf = commit.as_object().short_id().map_err(backend_error)?;
    Ok(buf.as_str().unwrap_or_default().to_owned())
}

/// The freshness-check refusal (ADR 0004): the live index moved between
/// payload derivation and commit, so committing would substitute content
/// the user never confirmed. `what` names the thing that moved — one
/// sentence, whichever check caught it.
fn index_moved(what: std::fmt::Arguments<'_>) -> Error {
    Error::Backend(format!("the index changed while committing ({what}); retry").into())
}

fn backend_error(err: git2::Error) -> Error {
    Error::Backend(Box::new(err))
}

fn io_error(err: std::io::Error) -> Error {
    Error::Backend(Box::new(err))
}
