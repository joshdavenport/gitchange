use std::path::Path;

use crate::backend::{CommitPathSpec, GitBackend};
use crate::commit::{self, CommitOptions, CommitOutcome, CommitPayload};
use crate::diff::{ChangeKind, FileDiff};
use crate::error::Error;
use crate::git2_backend::Git2Backend;
use crate::matcher::{self, Advisory};
use crate::snapshot::Snapshot;
use crate::state::State;
use crate::state_file;
use crate::universe::{self, ChangedFile, Hunk, HunkStage};
use crate::vocabulary::{ARROW, UNASSIGNED, count_noun};

/// How far back the snapshot's Commits panel material reaches — a plain
/// lazygit-equivalent window, not a full history walk.
const RECENT_COMMITS_LIMIT: usize = 300;

/// What a fail-soft mutating op did: the transparency echo for the work
/// actually executed (ADR 0007 — composed here so the TUI and CLI can't
/// drift, ADR 0006; `None` when nothing applied), plus advisories for
/// what failed soft.
#[derive(Debug)]
pub struct OpOutcome {
    pub echo: Option<String>,
    pub advisories: Vec<Advisory>,
}

impl OpOutcome {
    fn applied(echo: String) -> Self {
        Self {
            echo: Some(echo),
            advisories: Vec::new(),
        }
    }
}

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

    /// Absolute path to this worktree's state file.
    ///
    /// Public for tooling that inspects a repo's gitchange state from
    /// outside the app — xtask's sandbox fingerprints, the benchmark
    /// harness's shape verification — so it reads the path from the one
    /// place that defines it. Spelling `.git/gitchange/state.json` by
    /// hand is also wrong for a linked worktree, where the private git
    /// dir is `.git/worktrees/<id>/` (ADR 0002).
    pub fn state_file_path(&self) -> std::path::PathBuf {
        self.state_dir().join(state_file::STATE_FILE)
    }

    /// The worktree root; `None` for a bare repo. The Engine's watch
    /// root, and the frontends' panel furniture (the Status panel's repo
    /// name, the CLI's headers) without a git edge outside core
    /// (ADR 0006) — on the sync interface both frontends hold, where the
    /// CLI (which never constructs an Engine) can reach it too.
    pub fn workdir(&self) -> Option<std::path::PathBuf> {
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
        state.prune_records_of_unknown_changelists();
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
            // pruning: matched owners re-validate against the
            // reloaded roster before they persist.
            current.prune_records_of_unknown_changelists();
            state_file::save(&dir, &current)?;
        }
        Ok((
            Snapshot {
                files,
                changelists: state.changelists,
                active: state.active,
                advisories: outcome.advisories,
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
    /// whose content is gone fails soft with a `Advisory::StaleHunk` and
    /// nothing applied. Membership records are untouched — the caller's
    /// follow-up refresh re-derives staging. The echo names the fresh
    /// coordinates the apply actually ran at (ADR 0007: the echo shows
    /// the work done, not the snapshot the caller pointed from).
    pub fn stage_hunk(&self, path: &str, hunk: &Hunk) -> Result<OpOutcome, Error> {
        let files = universe::build(self.backend.diffs()?);
        let Some((file, fresh)) = find_fresh(&files, path, hunk) else {
            return Ok(OpOutcome {
                echo: None,
                advisories: vec![stale_advisory(path, hunk)],
            });
        };
        match fresh.stage {
            // Index already matches the worktree here.
            HunkStage::Staged => {}
            // A mode hunk moves by the mode-only index write and by
            // nothing else (ADR 0017) — including the index-only
            // flavour, where index := worktree restores HEAD's mode
            // because that is the mode the worktree now holds.
            _ if fresh.is_mode_change() => self.backend.stage_worktree_mode(path)?,
            _ if hunk_op_is_a_file_op(file, fresh) => self.backend.stage_path(path)?,
            // Worktree matches HEAD across an index-only hunk's range, so
            // index := worktree is a reverse-apply of the staged content.
            HunkStage::StagedStale if fresh.index_only => self
                .backend
                .unstage_head_range(path, (fresh.old_start, fresh.old_lines))?,
            _ => self
                .backend
                .stage_worktree_range(path, (fresh.new_start, fresh.new_lines))?,
        }
        Ok(OpOutcome::applied(format!(
            "staged hunk — {path} @@ {}",
            fresh.old_coords()
        )))
    }

    /// Unstage one hunk: set index := HEAD for its region, a
    /// reverse-apply on the live index. Same validate-at-apply,
    /// fail-soft and echo contract as [`Repo::stage_hunk`].
    pub fn unstage_hunk(&self, path: &str, hunk: &Hunk) -> Result<OpOutcome, Error> {
        let files = universe::build(self.backend.diffs()?);
        let Some((file, fresh)) = find_fresh(&files, path, hunk) else {
            return Ok(OpOutcome {
                echo: None,
                advisories: vec![stale_advisory(path, hunk)],
            });
        };
        match fresh.stage {
            // Nothing of this hunk is in the index.
            HunkStage::Unstaged => {}
            _ if fresh.is_mode_change() => self.backend.unstage_head_mode(path)?,
            _ if hunk_op_is_a_file_op(file, fresh) => self.backend.unstage_path(path)?,
            _ => self
                .backend
                .unstage_head_range(path, (fresh.old_start, fresh.old_lines))?,
        }
        Ok(OpOutcome::applied(format!(
            "unstaged hunk — {path} @@ {}",
            fresh.old_coords()
        )))
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
                mode: path.mode,
            })
            .collect();
        let committed = self
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
            state.baseline_head = Some(committed.oid.clone());
            state.prune_records_of_unknown_changelists();
            state_file::save(&dir, &state)?;
        }
        Ok(CommitOutcome::Committed {
            oid: committed.oid,
            short_id: committed.short_id,
        })
    }

    /// The bulk align op (ADR 0004): set index := worktree for each of
    /// the changelist's staged-stale hunks — re-staging edited `◑`
    /// hunks, discarding index-only ones — so a follow-up commit carries
    /// what the worktree shows. Fail-soft per hunk like
    /// [`Repo::stage_hunk`]; the echo names the whole bulk op, with any
    /// stale-hunk advisories alongside.
    pub fn align(&self, changelist: Option<&str>) -> Result<OpOutcome, Error> {
        let (_, advisories) = self.bulk_apply(
            BulkScope::changelist(changelist),
            |stage| stage == HunkStage::StagedStale,
            Self::stage_hunk,
        )?;
        Ok(OpOutcome {
            echo: Some(format!(
                "aligned index to worktree — '{}'",
                changelist.unwrap_or(UNASSIGNED)
            )),
            advisories,
        })
    }

    /// The bulk stage op: set index := worktree for each of the
    /// changelist's unstaged hunks — what the commit flow's stage-all
    /// offer runs before opening the dialog. `◑` hunks are left alone:
    /// the offer stages what is not in the index, and the dialog's own
    /// align option covers the rest (ADR 0004). Fail-soft per hunk like
    /// [`Repo::stage_hunk`]; the echo counts what was staged (`None`
    /// when nothing was), with any stale-hunk advisories alongside.
    pub fn stage_all(&self, changelist: Option<&str>) -> Result<OpOutcome, Error> {
        let scope = BulkScope::changelist(changelist);
        let (staged, advisories) = self.bulk_apply(
            scope,
            |stage| stage == HunkStage::Unstaged,
            Self::stage_hunk,
        )?;
        // Silent when nothing staged: the offer's caller is mid-flow
        // towards a dialog, and has no use for a nothing-to-do line.
        let echo = (staged > 0).then(|| bulk_echo("stage", staged, scope));
        Ok(OpOutcome { echo, advisories })
    }

    /// The stage direction of `space` on a changelist: set index :=
    /// worktree for every hunk of it the index does not already hold —
    /// `○` unstaged and `◑` staged-stale alike, the same pair per-hunk
    /// `space` stages (ADR 0003). Wider than [`Repo::stage_all`], which
    /// serves the commit flow's offer. Fail-soft per hunk like
    /// [`Repo::stage_hunk`]; the echo counts what was staged, and says
    /// so when that is nothing.
    pub fn stage_changelist(&self, changelist: Option<&str>) -> Result<OpOutcome, Error> {
        self.stage_scope(BulkScope::changelist(changelist))
    }

    /// The unstage direction of `space` on a changelist: set index :=
    /// HEAD for each of its staged hunks — the mirror of
    /// [`Repo::stage_changelist`], with the same fail-soft, echo and
    /// membership contracts.
    pub fn unstage_changelist(&self, changelist: Option<&str>) -> Result<OpOutcome, Error> {
        self.unstage_scope(BulkScope::changelist(changelist))
    }

    /// The stage direction of `space` on a Files row (issue #97): the
    /// changelist scope narrowed to one file — set index := worktree for
    /// each hunk of `path` that `changelist` owns and the index does not
    /// already hold. Hunks of the same file owned elsewhere stay out of
    /// the index, so a row can never stage another changelist's work.
    /// gitchange has no whole-file stage of its own; `git add <path>` is
    /// that op and refresh absorbs it (ADR 0003).
    pub fn stage_owned_hunks(
        &self,
        path: &str,
        changelist: Option<&str>,
    ) -> Result<OpOutcome, Error> {
        self.stage_scope(BulkScope::file(path, changelist))
    }

    /// The unstage direction of `space` on a Files row: set index :=
    /// HEAD for each staged hunk of `path` that `changelist` owns — the
    /// mirror of [`Repo::stage_owned_hunks`].
    pub fn unstage_owned_hunks(
        &self,
        path: &str,
        changelist: Option<&str>,
    ) -> Result<OpOutcome, Error> {
        self.unstage_scope(BulkScope::file(path, changelist))
    }

    /// `space`'s stage direction over any bulk scope: `○` unstaged and
    /// `◑` staged-stale alike, the same pair per-hunk `space` stages
    /// (ADR 0003). Wider than [`Repo::stage_all`], which serves the
    /// commit flow's offer. Fail-soft per hunk like [`Repo::stage_hunk`];
    /// the echo counts what was staged, and says so when that is nothing.
    fn stage_scope(&self, scope: BulkScope<'_>) -> Result<OpOutcome, Error> {
        let (staged, advisories) = self.bulk_apply(
            scope,
            |stage| matches!(stage, HunkStage::Unstaged | HunkStage::StagedStale),
            Self::stage_hunk,
        )?;
        Ok(OpOutcome {
            echo: Some(bulk_echo("stage", staged, scope)),
            advisories,
        })
    }

    /// `space`'s unstage direction over any bulk scope: the mirror of
    /// [`Repo::stage_scope`], over its staged hunks.
    fn unstage_scope(&self, scope: BulkScope<'_>) -> Result<OpOutcome, Error> {
        let (unstaged, advisories) = self.bulk_apply(
            scope,
            |stage| stage == HunkStage::Staged,
            Self::unstage_hunk,
        )?;
        Ok(OpOutcome {
            echo: Some(bulk_echo("unstage", unstaged, scope)),
            advisories,
        })
    }

    /// The body every bulk staging op shares: over one snapshot, run
    /// `apply` on each hunk in `scope` whose staging state `include`
    /// accepts — hunks another changelist owns are never touched, and
    /// conflicted files carry none at all (ADR 0007). Returns how many
    /// hunks moved, plus the advisories of those that failed soft.
    fn bulk_apply(
        &self,
        scope: BulkScope<'_>,
        include: impl Fn(HunkStage) -> bool,
        apply: impl Fn(&Self, &str, &Hunk) -> Result<OpOutcome, Error>,
    ) -> Result<(usize, Vec<Advisory>), Error> {
        let snapshot = self.refresh()?;
        validate_changelist(&snapshot, scope.changelist)?;
        let mut moved = 0;
        let mut advisories = Vec::new();
        for file in &snapshot.files {
            if scope.path.is_some_and(|path| path != file.path) {
                continue;
            }
            for hunk in file.owned_hunks(scope.changelist) {
                if include(hunk.stage) {
                    let outcome = apply(self, &file.path, hunk)?;
                    if outcome.advisories.is_empty() {
                        moved += 1;
                    }
                    advisories.extend(outcome.advisories);
                }
            }
        }
        Ok((moved, advisories))
    }

    /// Assign snapshot hunks of `path` to `target`: an explicit
    /// membership op, one locked load-mutate-save cycle (ADR 0002).
    /// `target: None` releases to unassigned — records are deleted,
    /// nothing is written back (ADR 0016), and the echo states the
    /// release. Validates at apply like staging (ADR 0005): each hunk is
    /// content-matched against the live tree; a vanished hunk fails soft
    /// with an `Advisory::StaleHunk` while the rest are still assigned.
    /// The caller's follow-up refresh re-derives membership from the
    /// written records. The echo counts what was acted on (`None` when
    /// nothing was).
    pub fn assign_hunks(
        &self,
        path: &str,
        hunks: &[Hunk],
        target: Option<&str>,
    ) -> Result<OpOutcome, Error> {
        let files = universe::build(self.backend.diffs()?);
        let mut advisories = Vec::new();
        let mut fresh = Vec::new();
        for hunk in hunks {
            match find_fresh(&files, path, hunk) {
                Some((_, found)) => fresh.push(found),
                None => advisories.push(stale_advisory(path, hunk)),
            }
        }
        let echo = (!fresh.is_empty()).then(|| match target {
            Some(name) => format!(
                "assigned {} — {path} {ARROW} '{name}'",
                count_noun(fresh.len(), "hunk")
            ),
            None => format!("released {} — {path}", count_noun(fresh.len(), "hunk")),
        });
        if !fresh.is_empty() {
            self.update_state(|state| state.assign_records(path, &fresh, target))?;
        }
        Ok(OpOutcome { echo, advisories })
    }

    /// Create a changelist. The active marker stays where it is
    /// (ADR 0015): a caller that wants the new changelist active says so
    /// with [`Repo::switch`].
    pub fn create_changelist(&self, name: &str) -> Result<(), Error> {
        self.update_state(|state| state.create(name))
    }

    /// Rename a changelist, carrying the active marker with it.
    pub fn rename_changelist(&self, from: &str, to: &str) -> Result<(), Error> {
        self.update_state(|state| state.rename(from, to))
    }

    /// Delete a changelist, pruning all of its records (ADR 0016).
    /// Deleting the active one leaves unassigned active.
    pub fn delete_changelist(&self, name: &str) -> Result<(), Error> {
        self.update_state(|state| state.delete(name))
    }

    /// Set the active changelist; `None` is unassigned (ADR 0015's
    /// capture-off). [`target_named`] maps a user-typed name onto
    /// this argument.
    ///
    /// [`target_named`]: crate::target_named
    pub fn switch(&self, target: Option<&str>) -> Result<(), Error> {
        self.update_state(|state| state.switch(target))
    }

    /// One locked load-mutate-save cycle (ADR 0002): fail-fast lock,
    /// atomic replace, nothing persisted when the mutation errors — or
    /// when it changes nothing, so a no-op (releasing already-recordless
    /// hunks, ADR 0016) never grows or rewrites the state file.
    fn update_state(
        &self,
        mutate: impl FnOnce(&mut State) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let dir = self.backend.state_dir();
        let _lock = state_file::lock(&dir)?;
        let state = state_file::load(&dir)?;
        let mut mutated = state.clone();
        mutate(&mut mutated)?;
        if mutated == state {
            return Ok(());
        }
        state_file::save(&dir, &mutated)
    }
}

/// A changelist-scoped staging op's echo. `space` on a changelist
/// always answers (ADR 0007): a count when hunks moved, the quiet
/// nothing-to-do line when none did. `verb` is the plain form —
/// `stage`/`unstage`, whose past tense is the same word plus `d`.
fn bulk_echo(verb: &str, moved: usize, scope: BulkScope<'_>) -> String {
    let target = scope.target();
    match moved {
        0 => format!("nothing to {verb} — {target}"),
        moved => format!("{verb}d {} — {target}", count_noun(moved, "hunk")),
    }
}

/// What a bulk staging op ranges over: one changelist's hunks, optionally
/// narrowed to a single file. The two scopes `space` has above the hunk
/// (ADR 0003) differ only in that narrowing, so they share one traversal
/// and one echo shape.
#[derive(Debug, Clone, Copy)]
struct BulkScope<'a> {
    /// `None` is unassigned, as everywhere.
    changelist: Option<&'a str>,
    /// `Some` narrows to a Files row — the (changelist, file) cell.
    path: Option<&'a str>,
}

impl<'a> BulkScope<'a> {
    fn changelist(changelist: Option<&'a str>) -> Self {
        Self {
            changelist,
            path: None,
        }
    }

    fn file(path: &'a str, changelist: Option<&'a str>) -> Self {
        Self {
            changelist,
            path: Some(path),
        }
    }

    /// The scope as the echo names it: `'feature'`, or `a.txt in
    /// 'feature'` — a file-scoped echo says which changelist it stayed
    /// inside, since that is exactly what it did not sweep past.
    fn target(&self) -> String {
        let name = self.changelist.unwrap_or(UNASSIGNED);
        match self.path {
            None => format!("'{name}'"),
            Some(path) => format!("{path} in '{name}'"),
        }
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

/// Validate-at-apply (ADR 0005): find the live hunk whose identity
/// matches the snapshot's. Content-matched, not coordinate-matched, so a
/// hunk merely shifted by edits above it still applies — at its fresh
/// coordinates. Identical hunks (repeated code blocks) tie-break by
/// proximity to the snapshot position, keeping the op on the hunk the
/// user pointed at. `None` is the stale case.
///
/// A whole-file hunk matches by path continuity rather than content
/// ([`crate::HunkIdentity::same_hunk`]), so a binary rewritten between
/// refresh and keypress still resolves — reachable in practice, since
/// assign (`ctrl+a`/`A`) routes a binary's whole-file hunk through here.
fn find_fresh<'a>(
    files: &'a [ChangedFile],
    path: &str,
    hunk: &Hunk,
) -> Option<(&'a ChangedFile, &'a Hunk)> {
    let file = files.iter().find(|file| file.path == path)?;
    let fresh = file
        .hunks
        .iter()
        .filter(|candidate| candidate.identity.same_hunk(&hunk.identity))
        .min_by_key(|candidate| candidate.new_start.abs_diff(hunk.new_start))?;
    Some((file, fresh))
}

/// Whether this hunk moves by a whole index-entry write rather than an
/// apply. A whole-file hunk is that shape by construction (ADR 0009,
/// ADR 0017): `space` on a changed binary, an empty file's add or delete,
/// or a type change is a whole-file index write. So is the lone hunk of
/// an added, untracked or deleted file, whose content libgit2 won't apply
/// hunk-wise (an untracked file is in no apply preimage). A mode hunk is
/// *not* — it has its own mode-only write, checked before this, so
/// staging one never writes an entry whole (ADR 0017).
///
/// Asked per hunk, not per file: a chmod'd binary presents a mode hunk
/// beside its whole-file one, and an index-only text hunk can sit beside
/// a whole-file hunk when the worktree went binary over staged text —
/// each moves by its own op.
///
/// Neither these whole-entry writes nor the range apply below carries a
/// mode with its content (ADR 0017: no ride-along) — the backend puts
/// the index entry's permission bits back after the write, so a flip
/// staged by its own hunk survives a neighbouring content stage. Only a
/// type change moves the entry's mode without a mode hunk, the object
/// kind being the whole-file hunk's own delta.
fn hunk_op_is_a_file_op(file: &ChangedFile, hunk: &Hunk) -> bool {
    hunk.is_whole_file()
        || (matches!(
            file.kind,
            ChangeKind::Added | ChangeKind::Untracked | ChangeKind::Deleted
        ) && file.total_hunks() == 1)
}

fn stale_advisory(path: &str, hunk: &Hunk) -> Advisory {
    Advisory::StaleHunk {
        path: path.into(),
        new_start: hunk.new_start,
    }
}
