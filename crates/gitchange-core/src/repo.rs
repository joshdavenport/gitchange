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

/// What a mutating op did: the transparency echo for the work actually
/// executed (ADR 0007 — composed here so the TUI and CLI can't drift,
/// ADR 0006; `None` when nothing was decided or applied), plus the
/// advisories that ride its receipt — what failed soft on a fail-soft
/// op, and the decisions a bare state write made on its own.
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

    /// A bare state write's outcome: `compose`'s echo when the write
    /// decided something, silence when it did not (#122 — nothing
    /// decided says nothing, and the op still succeeded).
    fn decided(wrote: bool, compose: impl FnOnce() -> String) -> Self {
        match wrote {
            true => Self::applied(compose()),
            false => Self {
                echo: None,
                advisories: Vec::new(),
            },
        }
    }
}

/// What a staging sweep did: the receipt, plus the counts the caller
/// branches on. The counts ride out because staleness fails soft per hunk
/// (ADR 0005) and the split that follows is not the echo's to make — a
/// sweep where every hunk went stale moved nothing, which is a refusal at
/// the CLI's exit-code surface (#145), while one where some moved is a
/// success with skips counted.
#[derive(Debug)]
pub struct SweepOutcome {
    /// The counted echo and the skipped hunks' advisories.
    pub receipt: OpOutcome,
    /// Hunks whose index write went through.
    pub moved: usize,
    /// Hunks skipped as stale; each one carries an advisory on the
    /// receipt.
    pub skipped: usize,
}

impl SweepOutcome {
    /// Whether the sweep had work and none of it landed: every hunk in
    /// scope went stale. The one outcome a fail-soft sweep cannot call a
    /// success — nothing the caller asked for moved — and so the one the
    /// CLI answers with a refusal (#145). Deliberately not a satisfied
    /// scope, where there was nothing to move in the first place; the two
    /// share a zero and nothing else.
    pub fn moved_nothing(&self) -> bool {
        self.moved == 0 && self.skipped > 0
    }
}

/// What a persisting refresh produced: the snapshot, plus the automatic
/// membership decisions that refresh committed to records (ADR 0005) —
/// delivered once, to the actor who triggered it.
///
/// The advisories live here rather than on [`Snapshot`] because that is
/// the ADR 0005 filter, made structural: [`Repo::read_only_refresh`]
/// returns the snapshot alone, so no frontend has a field to leak
/// previews of decisions it never committed.
#[derive(Debug)]
pub struct RefreshOutcome {
    pub snapshot: Snapshot,
    pub advisories: Vec<Advisory>,
}

/// Which of ADR 0005's two refresh forms a recompute pass is running.
/// The pipeline is one function: the mode decides whether capture is on
/// and whether anything is written, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshMode {
    Persisting,
    ReadOnly,
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
    /// The persisting form — capture is on, and the decisions it makes
    /// come back as advisories for its caller to deliver once.
    pub fn refresh(&self) -> Result<RefreshOutcome, Error> {
        Ok(self.refresh_capturing_index()?.0)
    }

    /// The read-only refresh (ADR 0005): the same full recompute against
    /// the records as they stand, writing nothing — no capture, no record
    /// updates, no baseline stamp — and so taking no lock. Every
    /// read-only frontend invocation runs this: a glance never moves
    /// membership.
    ///
    /// Advising nothing is a filter, not a vacancy. The recompute still
    /// produces advisories; they are discarded here as previews of
    /// decisions only a persisting refresh commits and delivers, and the
    /// return type carries no field to leak them through.
    ///
    /// Ownership is what the records say, because capture-off *is* the
    /// no-active-changelist matcher run (ADR 0015): record-derived
    /// ownership — overlap inheritance, dormant revival — shows, while
    /// context-derived ownership — capture, the ADR 0009 entry-unit join
    /// — never previews, so a recordless hunk reports as unassigned.
    pub fn read_only_refresh(&self) -> Result<Snapshot, Error> {
        Ok(self.recompute(RefreshMode::ReadOnly)?.0.snapshot)
    }

    /// [`Repo::refresh`], also handing back the diff(HEAD↔index) the
    /// universe was built from — the commit payload's raw material
    /// (ADR 0004), captured here so payload and snapshot describe the
    /// same instant.
    fn refresh_capturing_index(&self) -> Result<(RefreshOutcome, Vec<FileDiff>), Error> {
        self.recompute(RefreshMode::Persisting)
    }

    /// The recompute both refresh forms run; `mode` is the only thing
    /// that differs between them (see [`RefreshMode`]).
    fn recompute(&self, mode: RefreshMode) -> Result<(RefreshOutcome, Vec<FileDiff>), Error> {
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
        // Capture is the read-only form's one behavioural difference
        // (ADR 0005): running the matcher with no active changelist *is*
        // capture-off (ADR 0015), so context-derived ownership never
        // reaches a read's snapshot while record-derived ownership does.
        let active = match mode {
            RefreshMode::Persisting => state.active.as_deref(),
            RefreshMode::ReadOnly => None,
        };
        let outcome = matcher::run(&mut files, state.records.clone(), active, now, &affected);
        if mode == RefreshMode::Persisting {
            // Belt-and-braces for the self-loop filter (ADR 0005): the
            // state file is not rewritten when records are unchanged —
            // except to move the baseline stamp with HEAD (ADR 0012). The
            // default-state guard keeps a changelist-less repo from
            // growing a state file just to hold a stamp.
            let stamp_due = state.baseline_head != head && state != State::default();
            if outcome.records != state.records || stamp_due {
                let _lock = state_file::lock(&dir)?;
                // Reload under the lock so a write that landed since our
                // read (changelist ops) keeps its changelists; only
                // records and the baseline are ours to replace.
                let mut current = state_file::load(&dir)?;
                current.records = outcome.records;
                current.baseline_head = head;
                // A delete that landed since our read must keep its
                // pruning: matched owners re-validate against the
                // reloaded roster before they persist.
                current.prune_records_of_unknown_changelists();
                state_file::save(&dir, &current)?;
            }
        }
        Ok((
            RefreshOutcome {
                snapshot: Snapshot {
                    files,
                    changelists: state.changelists,
                    active: state.active,
                    head: head_info,
                    recent_commits,
                    operation,
                },
                // Always carried this far; it is the read-only entry
                // point that drops them, having nowhere to put them.
                advisories: outcome.advisories,
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
        let (refreshed, index_diff) = self.refresh_capturing_index()?;
        let snapshot = refreshed.snapshot;
        validate_changelist(&snapshot, changelist)?;
        Ok(commit::plan(&snapshot.files, &index_diff, changelist)?.payload)
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
        let (refreshed, index_diff) = self.refresh_capturing_index()?;
        let snapshot = refreshed.snapshot;
        validate_changelist(&snapshot, changelist)?;
        let plan = commit::plan(&snapshot.files, &index_diff, changelist)?;
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
        let (_, _, advisories) = self.bulk_apply(
            SweepScope::changelist(changelist),
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
        let scope = SweepScope::changelist(changelist);
        let (staged, skipped, advisories) = self.bulk_apply(
            scope,
            |stage| stage == HunkStage::Unstaged,
            Self::stage_hunk,
        )?;
        // Silent when nothing staged: the offer's caller is mid-flow
        // towards a dialog, and has no use for a nothing-to-do line.
        let echo = (staged > 0).then(|| sweep_echo("stage", staged, skipped, scope));
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
        self.refreshed_sweep(SweepScope::changelist(changelist), Direction::Stage)
    }

    /// The unstage direction of `space` on a changelist: set index :=
    /// HEAD for each of its staged hunks — the mirror of
    /// [`Repo::stage_changelist`], with the same fail-soft, echo and
    /// membership contracts.
    pub fn unstage_changelist(&self, changelist: Option<&str>) -> Result<OpOutcome, Error> {
        self.refreshed_sweep(SweepScope::changelist(changelist), Direction::Unstage)
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
        self.refreshed_sweep(SweepScope::rows(&[path], changelist), Direction::Stage)
    }

    /// The unstage direction of `space` on a Files row: set index :=
    /// HEAD for each staged hunk of `path` that `changelist` owns — the
    /// mirror of [`Repo::stage_owned_hunks`].
    pub fn unstage_owned_hunks(
        &self,
        path: &str,
        changelist: Option<&str>,
    ) -> Result<OpOutcome, Error> {
        self.refreshed_sweep(SweepScope::rows(&[path], changelist), Direction::Unstage)
    }

    /// The CLI's stage sweep (#145): `space`'s stage direction over a
    /// scope the caller has already validated against `snapshot` — every
    /// hunk `changelist` owns, narrowed to `paths`' file rows when any are
    /// named (an empty slice is the whole changelist). `None` is
    /// unassigned, as everywhere.
    ///
    /// Takes the snapshot rather than refreshing behind the caller's back,
    /// so one invocation runs exactly one persisting refresh: the
    /// caller's, whose advisories are the caller's to deliver. That is
    /// what separates this from [`Repo::stage_changelist`], which serves a
    /// TUI keypress and owns its own refresh.
    pub fn stage_sweep(
        &self,
        snapshot: &Snapshot,
        changelist: Option<&str>,
        paths: &[&str],
    ) -> Result<SweepOutcome, Error> {
        self.sweep(
            snapshot,
            SweepScope::rows(paths, changelist),
            Direction::Stage,
        )
    }

    /// A sweep behind its own persisting refresh — the TUI's `space` at
    /// changelist and Files-row scope, where the keypress is the whole
    /// invocation. The counts are dropped: the echo already carries them,
    /// and the TUI has no exit code to split on.
    fn refreshed_sweep(
        &self,
        scope: SweepScope<'_>,
        direction: Direction,
    ) -> Result<OpOutcome, Error> {
        let snapshot = self.refresh()?.snapshot;
        validate_changelist(&snapshot, scope.changelist)?;
        Ok(self.sweep(&snapshot, scope, direction)?.receipt)
    }

    /// One staging sweep over `snapshot`: `direction`'s hunks in `scope`,
    /// each applied fail-soft, with the counted echo composed here so no
    /// frontend spells it (ADR 0006/0007).
    fn sweep(
        &self,
        snapshot: &Snapshot,
        scope: SweepScope<'_>,
        direction: Direction,
    ) -> Result<SweepOutcome, Error> {
        let (moved, skipped, advisories) = self.apply_over(
            snapshot,
            scope,
            |stage| direction.takes(stage),
            |repo, path, hunk| direction.apply(repo, path, hunk),
        )?;
        Ok(SweepOutcome {
            receipt: OpOutcome {
                echo: Some(sweep_echo(direction.verb(), moved, skipped, scope)),
                advisories,
            },
            moved,
            skipped,
        })
    }

    /// A multi-hunk staging op behind its own persisting refresh, for the ops
    /// whose hunk selection is narrower than a sweep direction — the
    /// commit flow's stage-all offer and align (ADR 0004).
    fn bulk_apply(
        &self,
        scope: SweepScope<'_>,
        include: impl Fn(HunkStage) -> bool,
        apply: impl Fn(&Self, &str, &Hunk) -> Result<OpOutcome, Error>,
    ) -> Result<(usize, usize, Vec<Advisory>), Error> {
        let snapshot = self.refresh()?.snapshot;
        validate_changelist(&snapshot, scope.changelist)?;
        self.apply_over(&snapshot, scope, include, apply)
    }

    /// The body every multi-hunk staging op shares: over one snapshot, run
    /// `apply` on each hunk in `scope` whose staging state `include`
    /// accepts — hunks another changelist owns are never touched, and
    /// conflicted files carry none at all (ADR 0007). Returns how many
    /// hunks moved and how many failed soft as stale, plus the latter's
    /// advisories.
    fn apply_over(
        &self,
        snapshot: &Snapshot,
        scope: SweepScope<'_>,
        include: impl Fn(HunkStage) -> bool,
        apply: impl Fn(&Self, &str, &Hunk) -> Result<OpOutcome, Error>,
    ) -> Result<(usize, usize, Vec<Advisory>), Error> {
        let mut moved = 0;
        let mut skipped = 0;
        let mut advisories = Vec::new();
        for file in &snapshot.files {
            if !scope.covers(&file.path) {
                continue;
            }
            for hunk in file.owned_hunks(scope.changelist) {
                if include(hunk.stage) {
                    let outcome = apply(self, &file.path, hunk)?;
                    if outcome.advisories.is_empty() {
                        moved += 1;
                    } else {
                        skipped += 1;
                    }
                    advisories.extend(outcome.advisories);
                }
            }
        }
        Ok((moved, skipped, advisories))
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
    ///
    /// Hunks sharing an index entry that commits whole move together
    /// (ADR 0009): a payload naming any of them is widened to the whole
    /// unit here, so no frontend and no scope can file one owner's blob
    /// under two. The echo counts the widened payload — what moved.
    pub fn assign_hunks(
        &self,
        path: &str,
        hunks: &[Hunk],
        target: Option<&str>,
    ) -> Result<OpOutcome, Error> {
        let files = universe::build(self.backend.diffs()?);
        let mut advisories = Vec::new();
        let mut fresh = Vec::new();
        let mut found_in = None;
        for hunk in hunks {
            match find_fresh(&files, path, hunk) {
                Some((file, found)) => {
                    found_in = Some(file);
                    fresh.push(found);
                }
                None => advisories.push(stale_advisory(path, hunk)),
            }
        }
        if let Some(file) = found_in {
            fresh = file.widen_to_entry_unit(fresh);
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
    pub fn create_changelist(&self, name: &str) -> Result<OpOutcome, Error> {
        let wrote = self.update_state(|state| state.create(name))?;
        Ok(OpOutcome::decided(wrote, || {
            format!("created changelist '{name}'")
        }))
    }

    /// Rename a changelist, carrying the active marker with it. A rename
    /// to the name it already has decides nothing and echoes nothing.
    pub fn rename_changelist(&self, from: &str, to: &str) -> Result<OpOutcome, Error> {
        let wrote = self.update_state(|state| state.rename(from, to))?;
        Ok(OpOutcome::decided(wrote, || {
            format!("renamed changelist '{from}' {ARROW} '{to}'")
        }))
    }

    /// Delete a changelist, pruning all of its records (ADR 0016).
    /// Deleting the active one leaves unassigned active, and says so:
    /// the marker moving is a decision the caller did not ask for, so it
    /// rides the receipt as an advisory.
    pub fn delete_changelist(&self, name: &str) -> Result<OpOutcome, Error> {
        let mut was_active = false;
        // Read inside the mutation, so the marker is the one the delete
        // actually took: the state is loaded under the lock, and a
        // switch that landed between our decision and the write would
        // otherwise make this advisory a guess.
        let wrote = self.update_state(|state| {
            was_active = state.active.as_deref() == Some(name);
            state.delete(name)
        })?;
        let mut outcome = OpOutcome::decided(wrote, || format!("deleted changelist '{name}'"));
        if was_active {
            outcome.advisories.push(Advisory::ActiveChangelistDeleted {
                changelist: name.into(),
            });
        }
        Ok(outcome)
    }

    /// Set the active changelist; `None` is unassigned (ADR 0015's
    /// capture-off). [`target_named`] maps a user-typed name onto
    /// this argument. Switching to the already-active target decides
    /// nothing and echoes nothing — git's "Already on" comfort line is
    /// not borrowed (#122).
    ///
    /// [`target_named`]: crate::target_named
    pub fn switch(&self, target: Option<&str>) -> Result<OpOutcome, Error> {
        let wrote = self.update_state(|state| state.switch(target))?;
        Ok(OpOutcome::decided(wrote, || {
            // Unassigned is a switch target, not a changelist, so the
            // echo names the target alone — "changelist 'unassigned'"
            // would name one that cannot exist.
            format!("switched to '{}'", target.unwrap_or(UNASSIGNED))
        }))
    }

    /// One locked load-mutate-save cycle (ADR 0002): fail-fast lock,
    /// atomic replace, nothing persisted when the mutation errors — or
    /// when it changes nothing, so a no-op (releasing already-recordless
    /// hunks, ADR 0016) never grows or rewrites the state file. Answers
    /// whether the state was written, which is exactly what separates an
    /// op that decided something from one that did not.
    fn update_state(
        &self,
        mutate: impl FnOnce(&mut State) -> Result<(), Error>,
    ) -> Result<bool, Error> {
        let dir = self.backend.state_dir();
        let _lock = state_file::lock(&dir)?;
        let state = state_file::load(&dir)?;
        let mut mutated = state.clone();
        mutate(&mut mutated)?;
        if mutated == state {
            return Ok(false);
        }
        state_file::save(&dir, &mutated)?;
        Ok(true)
    }
}

/// Which way a staging sweep moves the index, and so which hunks it
/// takes: stage takes `○` and `◑` alike — the same pair per-hunk `space`
/// stages (ADR 0003) — and unstage takes `●` only, so a staged version
/// since edited is never discarded by a sweep (#145). One enum, so the
/// TUI's sweeps and the CLI's cannot come to disagree about either half.
#[derive(Debug, Clone, Copy)]
enum Direction {
    Stage,
    Unstage,
}

impl Direction {
    fn takes(self, stage: HunkStage) -> bool {
        match self {
            Direction::Stage => matches!(stage, HunkStage::Unstaged | HunkStage::StagedStale),
            Direction::Unstage => stage == HunkStage::Staged,
        }
    }

    fn apply(self, repo: &Repo, path: &str, hunk: &Hunk) -> Result<OpOutcome, Error> {
        match self {
            Direction::Stage => repo.stage_hunk(path, hunk),
            Direction::Unstage => repo.unstage_hunk(path, hunk),
        }
    }

    /// The echo's plain verb, whose past tense is the same word plus `d`.
    fn verb(self) -> &'static str {
        match self {
            Direction::Stage => "stage",
            Direction::Unstage => "unstage",
        }
    }
}

/// A staging op's echo. A sweep always answers (ADR 0007): a count
/// when hunks moved, the quiet nothing-to-do line when none did. `verb` is
/// the plain form — `stage`/`unstage`, whose past tense is the same word
/// plus `d`.
///
/// Stale skips ride the count rather than stderr alone, so a harness that
/// drops stderr still sees on stdout that the sweep was partial and
/// re-reads (#145). The `of` clause appears only when something was
/// skipped: with nothing to compare against, `staged 3 of 3 hunks` would
/// be an arithmetic riddle where `staged 3 hunks` is the fact.
fn sweep_echo(verb: &str, moved: usize, skipped: usize, scope: SweepScope<'_>) -> String {
    let target = scope.target();
    match (moved, skipped) {
        (0, 0) => format!("nothing to {verb} — {target}"),
        (moved, 0) => format!("{verb}d {} — {target}", count_noun(moved, "hunk")),
        (moved, skipped) => format!(
            "{verb}d {moved} of {} ({skipped} skipped as stale) — {target}",
            count_noun(moved + skipped, "hunk")
        ),
    }
}

/// What a staging op ranges over: one changelist's hunks, optionally
/// narrowed to some of its file rows. Every scope `space` and the staging
/// verbs have above the hunk (ADR 0003, #145) differs only in that
/// narrowing, so they share one traversal and one echo shape.
#[derive(Debug, Clone, Copy)]
struct SweepScope<'a> {
    /// `None` is unassigned, as everywhere.
    changelist: Option<&'a str>,
    /// The Files rows — the (changelist, file) cells — this scope is
    /// narrowed to. Empty is the whole changelist.
    paths: &'a [&'a str],
}

impl<'a> SweepScope<'a> {
    fn changelist(changelist: Option<&'a str>) -> Self {
        Self {
            changelist,
            paths: &[],
        }
    }

    fn rows(paths: &'a [&'a str], changelist: Option<&'a str>) -> Self {
        Self { changelist, paths }
    }

    /// Whether this scope reaches `path` at all.
    fn covers(&self, path: &str) -> bool {
        self.paths.is_empty() || self.paths.contains(&path)
    }

    /// The scope as the echo names it: `'feature'`, or `a.txt in
    /// 'feature'` — a row-scoped echo says which changelist it stayed
    /// inside, since that is exactly what it did not sweep past.
    fn target(&self) -> String {
        let name = self.changelist.unwrap_or(UNASSIGNED);
        match self.paths {
            [] => format!("'{name}'"),
            paths => format!("{} in '{name}'", paths.join(", ")),
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
