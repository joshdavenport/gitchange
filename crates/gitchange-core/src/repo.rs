use std::path::Path;

use crate::backend::GitBackend;
use crate::diff::ChangeKind;
use crate::error::Error;
use crate::git2_backend::Git2Backend;
use crate::matcher::{self, Notice};
use crate::snapshot::Snapshot;
use crate::state::State;
use crate::state_file;
use crate::universe::{self, ChangedFile, Hunk, HunkStage};

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

    /// One blocking recompute pass producing a fresh snapshot: both
    /// diffs → hunk universe → matcher → persist records (ADR 0005).
    pub fn refresh(&self) -> Result<Snapshot, Error> {
        // HEAD is read before the diffs: should a commit land in between,
        // the stale stamp trips the guard on the next refresh — loud,
        // where the opposite order would stamp coordinates as newer than
        // they are, silently.
        let head = self.backend.head_oid()?;
        let mut files = universe::build(self.backend.diffs()?);
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
        Ok(Snapshot {
            files,
            changelists: state.changelists,
            active: state.active,
            notices: outcome.notices,
        })
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
/// hunk-wise (untracked files aren't in any apply preimage).
fn hunk_ops_are_file_ops(file: &ChangedFile) -> bool {
    matches!(
        file.kind,
        ChangeKind::Added | ChangeKind::Untracked | ChangeKind::Deleted
    ) && file.total_hunks() == 1
}

fn stale_notice(path: &str, hunk: &Hunk) -> Notice {
    Notice::StaleHunk {
        path: path.into(),
        new_start: hunk.new_start,
    }
}
