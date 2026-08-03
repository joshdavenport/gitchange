use std::path::Path;

use crate::backend::{CommitPathSpec, GitBackend};
use crate::commit::{self, CommitOptions, CommitOutcome, CommitPayload};
use crate::diff::{ChangeKind, FileDiff};
use crate::error::Error;
use crate::git2_backend::Git2Backend;
use crate::matcher::{self, Notice};
use crate::snapshot::Snapshot;
use crate::state::State;
use crate::state_file;
use crate::universe::{self, ChangedFile, Hunk, HunkStage};

/// How far back the snapshot's Commits panel material reaches — a plain
/// lazygit-equivalent window, not a full history walk.
const RECENT_COMMITS_LIMIT: usize = 300;

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

    /// The per-worktree gitchange state directory — the Engine's
    /// self-loop filter root (ADR 0005).
    pub(crate) fn state_dir(&self) -> std::path::PathBuf {
        self.backend.state_dir()
    }

    /// The worktree root the Engine watches; `None` for a bare repo.
    pub(crate) fn workdir(&self) -> Option<std::path::PathBuf> {
        self.backend.workdir()
    }

    /// One blocking recompute pass producing a fresh snapshot: both
    /// diffs → hunk universe → matcher → persist records (ADR 0005).
    pub fn refresh(&self) -> Result<Snapshot, Error> {
        Ok(self.refresh_capturing_index()?.0)
    }

    /// [`Repo::refresh`], also handing back the diff(HEAD↔index) the
    /// universe was built from — the commit payload's raw material
    /// (ADR 0004), captured here so payload and snapshot describe the
    /// same instant.
    fn refresh_capturing_index(&self) -> Result<(Snapshot, Vec<FileDiff>), Error> {
        // HEAD is read before the diffs: should a commit land in between,
        // the stale stamp trips the guard on the next refresh — loud,
        // where the opposite order would stamp coordinates as newer than
        // they are, silently.
        let head = self.backend.head_oid()?;
        // Panel furniture, read adjacent to the HEAD stamp so the whole
        // snapshot describes one instant.
        let head_info = self.backend.head()?;
        let recent_commits = self.backend.recent_commits(RECENT_COMMITS_LIMIT)?;
        let operation = self.backend.operation()?;
        let diffs = self.backend.diffs()?;
        let index_diff = diffs.index.clone();
        let mut files = universe::build(diffs);
        // Reads take no lock: writers replace the file atomically, so a
        // read sees either the old or the new state, never a torn one.
        let dir = self.backend.state_dir();
        let mut state = state_file::load(&dir)?;
        // A hand-edited file may name changelists that don't exist;
        // give such records delete semantics before matching.
        state.orphan_records_of_unknown_changelists();
        let affected = self.affected_paths(&state, head.as_deref())?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let outcome = matcher::run(
            &mut files,
            state.records.clone(),
            state.active.as_deref(),
            now,
            &affected,
        );
        // Belt-and-braces for the self-loop filter (ADR 0005): the state
        // file is not rewritten when records are unchanged — except to
        // move the baseline stamp with HEAD (ADR 0012). The default-state
        // guard keeps a changelist-less repo from growing a state file
        // just to hold a stamp.
        let stamp_due = state.baseline_head != head && state != State::default();
        if outcome.records != state.records || stamp_due {
            let _lock = state_file::lock(&dir)?;
            // Reload under the lock so a write that landed since our read
            // (changelist ops) keeps its changelists; only records and
            // the baseline are ours to replace.
            let mut current = state_file::load(&dir)?;
            current.records = outcome.records;
            current.baseline_head = head;
            // A delete that landed since our read must keep its
            // orphaning: matched owners re-validate against the
            // reloaded roster before they persist.
            current.orphan_records_of_unknown_changelists();
            state_file::save(&dir, &current)?;
        }
        Ok((
            Snapshot {
                files,
                changelists: state.changelists,
                active: state.active,
                notices: outcome.notices,
                head: head_info,
                recent_commits,
                operation,
            },
            index_diff,
        ))
    }

    /// The ADR 0012 affected-path set for this refresh: empty while HEAD
    /// sits at the stored baseline (or none is stored yet — pre-baseline
    /// files adopt the current HEAD once, silently), the paths
    /// diff(baseline↔HEAD) names otherwise, and every path when the
    /// baseline no longer resolves.
    fn affected_paths(
        &self,
        state: &State,
        head: Option<&str>,
    ) -> Result<matcher::AffectedPaths, Error> {
        let Some(baseline) = state.baseline_head.as_deref() else {
            return Ok(matcher::AffectedPaths::None);
        };
        if Some(baseline) == head {
            return Ok(matcher::AffectedPaths::None);
        }
        Ok(match self.backend.paths_changed_since(baseline)? {
            Some(paths) => matcher::AffectedPaths::Some(paths.into_iter().collect()),
            None => matcher::AffectedPaths::All,
        })
    }

    /// Stage one hunk: set index := worktree for its region, a real
    /// apply-to-index (ADR 0003) — which on a staged-stale `◑` hunk means
    /// re-staging the edited version, and on an index-only one discarding
    /// it. Validates at apply against the live tree (ADR 0005): a hunk
    /// whose content is gone fails soft with a `Notice::StaleHunk` and
    /// nothing applied. Membership records are untouched — the caller's
    /// follow-up refresh re-derives staging.
    pub fn stage_hunk(&self, path: &str, hunk: &Hunk) -> Result<Vec<Notice>, Error> {
        let files = universe::build(self.backend.diffs()?);
        let Some((file, fresh)) = find_fresh(&files, path, hunk) else {
            return Ok(vec![stale_notice(path, hunk)]);
        };
        match fresh.stage {
            // Index already matches the worktree here.
            HunkStage::Staged => {}
            _ if hunk_ops_are_file_ops(file) => self.backend.stage_path(path)?,
            // Worktree matches HEAD across an index-only hunk's range, so
            // index := worktree is a reverse-apply of the staged content.
            HunkStage::StagedStale if fresh.index_only => self
                .backend
                .unstage_head_range(path, (fresh.old_start, fresh.old_lines))?,
            _ => self
                .backend
                .stage_worktree_range(path, (fresh.new_start, fresh.new_lines))?,
        }
        Ok(Vec::new())
    }

    /// Unstage one hunk: set index := HEAD for its region, a
    /// reverse-apply on the live index. Same validate-at-apply and
    /// fail-soft contract as [`Repo::stage_hunk`].
    pub fn unstage_hunk(&self, path: &str, hunk: &Hunk) -> Result<Vec<Notice>, Error> {
        let files = universe::build(self.backend.diffs()?);
        let Some((file, fresh)) = find_fresh(&files, path, hunk) else {
            return Ok(vec![stale_notice(path, hunk)]);
        };
        match fresh.stage {
            // Nothing of this hunk is in the index.
            HunkStage::Unstaged => {}
            _ if hunk_ops_are_file_ops(file) => self.backend.unstage_path(path)?,
            _ => self
                .backend
                .unstage_head_range(path, (fresh.old_start, fresh.old_lines))?,
        }
        Ok(Vec::new())
    }

    /// Stage a whole file, `git add` semantics: index := worktree,
    /// including untracked files and deletions. Unconditional — whatever
    /// the worktree holds now is what an external `git add` would stage.
    pub fn stage_file(&self, path: &str) -> Result<(), Error> {
        self.backend.stage_path(path)
    }

    /// Unstage a whole file, `git reset -- <path>` semantics: index
    /// entry := HEAD's.
    pub fn unstage_file(&self, path: &str) -> Result<(), Error> {
        self.backend.unstage_path(path)
    }

    /// The payload committing `changelist` (`None` = unassigned) would
    /// carry right now, behind a synchronous refresh — what the confirm
    /// flow shows and what [`Repo::commit`] later compares against for
    /// drift (ADR 0004's freshness guard).
    pub fn commit_payload(&self, changelist: Option<&str>) -> Result<CommitPayload, Error> {
        let (snapshot, index_diff) = self.refresh_capturing_index()?;
        validate_changelist(&snapshot, changelist)?;
        Ok(commit::plan(&snapshot.files, &index_diff, changelist).payload)
    }

    /// Commit `changelist`'s staged hunks (ADR 0004): a temporary index
    /// built as HEAD's tree plus only this payload, committed via a
    /// native `git commit` so hooks run — the live index and worktree
    /// are never touched, and any failure changes nothing. On success,
    /// one locked state update runs the ADR 0012 aftermath: consumed
    /// records removed, retained `◑` records rewritten against the new
    /// HEAD, surviving same-file records commuted, and the new baseline
    /// HEAD stamped so the external-move guard never arms for an own
    /// commit. A full refresh should follow; this method leaves the
    /// state file consistent for it.
    ///
    /// `expected` is a payload from [`Repo::commit_payload`]: when the
    /// synchronous refresh finds the live payload differs, nothing is
    /// committed and [`CommitOutcome::Drifted`] returns the fresh
    /// payload for re-confirmation.
    pub fn commit(
        &self,
        changelist: Option<&str>,
        message: &str,
        options: &CommitOptions,
        expected: Option<&CommitPayload>,
    ) -> Result<CommitOutcome, Error> {
        // The operation guard (ADR 0007), ahead of everything: git
        // honours MERGE_HEAD & co. even under GIT_INDEX_FILE, so this
        // commit would conclude the operation with one changelist's
        // payload. Checked in core, not just the TUI — every frontend
        // gets the guard.
        if let Some(operation) = self.backend.operation()? {
            return Err(Error::OperationInProgress { operation });
        }
        let (snapshot, index_diff) = self.refresh_capturing_index()?;
        validate_changelist(&snapshot, changelist)?;
        let plan = commit::plan(&snapshot.files, &index_diff, changelist);
        if plan.payload.is_empty() {
            return Err(Error::NothingStaged);
        }
        if let Some(expected) = expected
            && *expected != plan.payload
        {
            return Ok(CommitOutcome::Drifted {
                payload: plan.payload,
            });
        }

        let specs: Vec<CommitPathSpec> = plan
            .paths
            .iter()
            .map(|path| CommitPathSpec {
                path: path.path.clone(),
                hunks: path.committed.clone(),
                whole_file: path.whole_file.clone(),
            })
            .collect();
        let oid = self
            .backend
            .commit_from_index_hunks(&specs, message, options)?;

        let dir = self.backend.state_dir();
        let _lock = state_file::lock(&dir)?;
        // The residual worktree diff against the new HEAD — what retained
        // `◑` records are rewritten from (ADR 0012): the exact hunks the
        // next refresh will try to re-attach. Captured under the lock so
        // no concurrent state write can interleave between the capture
        // and the aftermath it feeds; the hold stays short (one diff).
        let residual = self.backend.diffs()?.worktree;
        let mut state = state_file::load(&dir)?;
        // A changelist-less, record-less repo has nothing to commute and
        // must not grow a state file just to hold a baseline stamp.
        if state != State::default() {
            commit::apply_aftermath(&mut state, &plan.paths, &residual);
            state.baseline_head = Some(oid.clone());
            state.orphan_records_of_unknown_changelists();
            state_file::save(&dir, &state)?;
        }
        Ok(CommitOutcome::Committed { oid })
    }

    /// The bulk align op (ADR 0004): set index := worktree for each of
    /// the changelist's staged-stale hunks — re-staging edited `◑`
    /// hunks, discarding index-only ones — so a follow-up commit carries
    /// what the worktree shows. Fail-soft per hunk like
    /// [`Repo::stage_hunk`]; returns any stale-hunk notices.
    pub fn align(&self, changelist: Option<&str>) -> Result<Vec<Notice>, Error> {
        let snapshot = self.refresh()?;
        validate_changelist(&snapshot, changelist)?;
        let mut notices = Vec::new();
        for file in &snapshot.files {
            for hunk in &file.hunks {
                if hunk.changelist.as_deref() == changelist && hunk.stage == HunkStage::StagedStale
                {
                    notices.extend(self.stage_hunk(&file.path, hunk)?);
                }
            }
        }
        Ok(notices)
    }

    /// Assign snapshot hunks of `path` to `target` (`None` =
    /// unassigned): an explicit membership op, one locked
    /// load-mutate-save cycle (ADR 0002). Validates at apply like
    /// staging (ADR 0005): each hunk is content-matched against the live
    /// tree; a vanished hunk fails soft with a `Notice::StaleHunk` while
    /// the rest are still assigned. The caller's follow-up refresh
    /// re-derives membership from the written records.
    pub fn assign_hunks(
        &self,
        path: &str,
        hunks: &[Hunk],
        target: Option<&str>,
    ) -> Result<Vec<Notice>, Error> {
        let files = universe::build(self.backend.diffs()?);
        let mut notices = Vec::new();
        let mut fresh = Vec::new();
        for hunk in hunks {
            match find_fresh(&files, path, hunk) {
                Some((_, found)) => fresh.push(found),
                None => notices.push(stale_notice(path, hunk)),
            }
        }
        if !fresh.is_empty() {
            self.update_state(|state| state.assign_records(path, &fresh, target))?;
        }
        Ok(notices)
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

/// A named changelist must exist to be committed or aligned; `None`
/// (unassigned) always exists.
fn validate_changelist(snapshot: &Snapshot, changelist: Option<&str>) -> Result<(), Error> {
    if let Some(name) = changelist
        && !snapshot.changelists.iter().any(|cl| cl.name == name)
    {
        return Err(Error::UnknownChangelist { name: name.into() });
    }
    Ok(())
}

/// Validate-at-apply (ADR 0005): find the live hunk whose verbatim lines
/// match the snapshot's. Content-matched, not coordinate-matched, so a
/// hunk merely shifted by edits above it still applies — at its fresh
/// coordinates. Identical hunks (repeated code blocks) tie-break by
/// proximity to the snapshot position, keeping the op on the hunk the
/// user pointed at. `None` is the stale case.
fn find_fresh<'a>(
    files: &'a [ChangedFile],
    path: &str,
    hunk: &Hunk,
) -> Option<(&'a ChangedFile, &'a Hunk)> {
    let file = files.iter().find(|file| file.path == path)?;
    let fresh = file
        .hunks
        .iter()
        .filter(|candidate| candidate.lines == hunk.lines)
        .min_by_key(|candidate| candidate.new_start.abs_diff(hunk.new_start))?;
    Some((file, fresh))
}

/// Added, untracked, and deleted files present their whole change as one
/// hunk, so hunk ops on them are the file ops — routed through the
/// index-entry primitives, which also cover content libgit2 won't apply
/// hunk-wise (untracked files aren't in any apply preimage). Binary
/// files are the same shape by construction (ADR 0009): one whole-file
/// hunk, `space` = whole-file index write.
fn hunk_ops_are_file_ops(file: &ChangedFile) -> bool {
    file.binary
        || (matches!(
            file.kind,
            ChangeKind::Added | ChangeKind::Untracked | ChangeKind::Deleted
        ) && file.total_hunks() == 1)
}

fn stale_notice(path: &str, hunk: &Hunk) -> Notice {
    Notice::StaleHunk {
        path: path.into(),
        new_start: hunk.new_start,
    }
}
