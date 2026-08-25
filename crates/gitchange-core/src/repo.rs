use std::path::Path;

use crate::backend::{CommitPathSpec, CommittedId, GitBackend};
use crate::commit::{self, CommitMessage, CommitOptions, CommitOutcome, CommitPayload};
use crate::diff::{ChangeKind, FileDiff};
use crate::error::Error;
use crate::git2_backend::Git2Backend;
use crate::matcher::{self, Advisory};
use crate::snapshot::Snapshot;
use crate::state::{Changelist, RecordCounts, State};
use crate::state_file;
use crate::universe::{self, ChangedFile, Hunk, HunkStage};
use crate::vocabulary::{
    ARROW, FOR_THE_NEXT_REFRESH, NO_REVIVAL, UNASSIGNED, count_noun, holder_label,
    unknown_changelist,
};

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

/// One commit's own persisting refresh ([`Repo::commit_refresh`]): the
/// snapshot, the decisions it made, and the diff(HEAD↔index) the payload
/// is derived from — one instant, held so a frontend can ask several
/// questions of it without refreshing again.
///
/// The index diff is private: it is raw material for
/// [`Repo::prepare_commit`], not a fact about the repo a frontend has any
/// business reading (ADR 0006).
#[derive(Debug)]
pub struct CommitRefresh {
    pub snapshot: Snapshot,
    pub advisories: Vec<Advisory>,
    index: Vec<FileDiff>,
}

/// A commit derived and not yet made ([`Repo::prepare_commit`]): the
/// payload, the aftermath bookkeeping that ships with it, and the
/// staged-stale hunks a frontend's own guard may want to name.
///
/// Everything but the payload is opaque — what a frontend decides about a
/// commit, it decides from the payload and the snapshot it came out of.
#[derive(Debug)]
pub struct PreparedCommit {
    /// What this commit would carry, per file.
    pub payload: CommitPayload,
    /// The changelist it was derived for, carried rather than re-passed
    /// so no caller can commit one scope's payload under another's name.
    changelist: Option<String>,
    paths: Vec<commit::PathPlan>,
    stale: Vec<String>,
}

impl PreparedCommit {
    /// The changelist this commit was derived for (`None` is
    /// unassigned) — what a frontend's own rungs name in their refusals,
    /// read back here rather than carried alongside, so a guard cannot
    /// come to speak for a scope other than the one that would commit.
    pub fn changelist(&self) -> Option<&str> {
        self.changelist.as_deref()
    }

    /// The payload's staged-stale (`◑`) hunks — content the index holds
    /// an overlapping-but-different version of — each as the composed
    /// address (#122) a refusal names and a caller pastes back, in
    /// payload order. Empty is a payload that ships exactly what the
    /// worktree shows.
    ///
    /// The condition the TUI warns and confirms on and the CLI refuses
    /// (ADR 0015): the split is the frontends', so core reports the
    /// hunks and decides nothing.
    pub fn stale_addresses(&self) -> &[String] {
        &self.stale
    }
}

/// A repository's changelist set on its own: the names in user order
/// (creation-append) and which one holds the active marker — everything
/// [`Snapshot`] carries about changelists, and nothing it carries about
/// the change universe.
#[derive(Debug)]
pub struct Roster {
    pub changelists: Vec<Changelist>,
    /// The active changelist; `None` is unassigned — capture off
    /// (ADR 0015).
    pub active: Option<String>,
}

/// Whether a delete may release a changelist's membership records
/// (#149's records guard). Not a bool at the call site: a delete that
/// releases another actor's claims is exactly the decision worth reading
/// in the argument list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// Refuse while any records exist, live or dormant — the CLI's
    /// unforced `changelist -d`.
    Guarded,
    /// Prune the records and release the hunks, counting what went: the
    /// CLI's `-f`/`-D`, and the TUI's own delete, whose confirm dialog is
    /// what this override means there (ADR 0015's parity).
    Forced,
}

/// What a delete answered: the receipt of one that ran, or the offenders
/// of one that refused whole. Two outcomes rather than an error because
/// only the frontend can dress a refusal — the exit code, the `; `-joined
/// offender list, the override its own grammar spells — while the facts
/// behind it are core's (#149).
#[derive(Debug)]
pub enum Deletion {
    /// Every named changelist is gone; the receipt carries the echo and
    /// the notices for what the deletions decided on their own.
    Done(OpOutcome),
    /// Nothing was deleted and nothing was written. Every offender, in
    /// the order the names arrived, so one refusal is a complete
    /// instruction.
    Refused(Vec<Undeletable>),
}

/// Why one name could not be deleted (#149's two offender classes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Undeletable {
    /// No changelist by that name at the locked read. A reserved name is
    /// simply unrecognised to a delete: `unassigned` is the absence of
    /// membership (ADR 0016) and `all` a pseudo-view, so neither is
    /// something a delete could act on.
    Unrecognised {
        name: String,
        /// The real changelists, in user order — what a retry can name.
        candidates: Vec<String>,
    },
    /// It holds membership records and the delete was [`Release::Guarded`].
    HoldsRecords {
        name: String,
        /// What it holds, live and dormant — the counts the refusal
        /// names, and what decides which stake the refusal states.
        records: RecordCounts,
    },
}

impl Undeletable {
    /// The canonical sentence for this offender (ADR 0006), which a
    /// frontend joins into its own refusal and dresses.
    ///
    /// The guard's sentence names the counts and the stake, and stops
    /// there: the override is spelled differently on every surface (a flag
    /// on the CLI, a confirm dialog in the TUI), so naming one here would
    /// teach the other's caller the wrong move.
    ///
    /// The stake is not one thing. Live records hold hunks that a deletion
    /// releases recordless, for the next refresh to claim; dormant records
    /// hold nothing to release — their hunks are already out of the diff —
    /// and what deletion ends is the revival they were waiting for
    /// (ADR 0002). So a changelist with anything live states the release,
    /// and a dormant-only one states the revival it is about to lose.
    pub fn message(&self) -> String {
        match self {
            // The noun command's own unrecognised-name sentence, shared
            // with the rename mode's refused `<old>` (#168).
            Undeletable::Unrecognised { name, candidates } => unknown_changelist(name, candidates),
            Undeletable::HoldsRecords { name, records } => {
                let stake = match records.live {
                    0 => format!("deleting it prunes them, so {NO_REVIVAL}"),
                    _ => format!(
                        "deleting it prunes them and releases its hunks recordless, \
                         {FOR_THE_NEXT_REFRESH}"
                    ),
                };
                format!("'{name}' holds {} — {stake}", records.counted())
            }
        }
    }
}

/// `names` with repeats dropped, first occurrence keeping its place.
fn distinct<'a>(names: &[&'a str]) -> Vec<&'a str> {
    let mut distinct: Vec<&'a str> = Vec::new();
    for name in names {
        if !distinct.contains(name) {
            distinct.push(name);
        }
    }
    distinct
}

/// The names in `named` that cannot be deleted from `state`, in argument
/// order. Every name is asked, not just the first that fails: an
/// all-or-nothing refusal is only a complete instruction if it names
/// every offender.
fn undeletable(state: &State, named: &[&str], release: Release) -> Vec<Undeletable> {
    let mut offenders = Vec::new();
    for name in named {
        if !state.contains(name) {
            offenders.push(Undeletable::Unrecognised {
                name: (*name).to_owned(),
                candidates: state.changelist_names(),
            });
            continue;
        }
        let records = state.record_counts(name);
        if release == Release::Guarded && records.any() {
            offenders.push(Undeletable::HoldsRecords {
                name: (*name).to_owned(),
                records,
            });
        }
    }
    offenders
}

/// One echo for the whole delete, however many names it carried: the
/// invocation is one op (#122), and one op says one line.
fn deleted_echo(named: &[&str]) -> String {
    let noun = match named.len() {
        1 => "changelist",
        _ => "changelists",
    };
    let names: Vec<String> = named.iter().map(|name| holder_label(Some(name))).collect();
    format!("deleted {noun} {}", names.join(", "))
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
        let refreshed = self.commit_refresh()?;
        Ok(self.prepare_commit(&refreshed, changelist)?.payload)
    }

    /// Commit's own persisting refresh (ADR 0005's persisting set): the
    /// snapshot a commit is derived from and validated against, the
    /// decisions that refresh made for its caller to deliver once, and —
    /// held privately — the diff(HEAD↔index) the payload comes out of,
    /// captured at the same instant so payload and snapshot describe one
    /// moment.
    ///
    /// Split from [`Repo::prepare_commit`] so that one invocation runs
    /// one refresh however many questions its frontend asks: the CLI's
    /// guard stack (#151) reads this snapshot, delivers these advisories
    /// before anything can refuse, and commits what the same instant
    /// derived — where a stack built on [`Repo::commit_payload`] would
    /// refresh once per question.
    pub fn commit_refresh(&self) -> Result<CommitRefresh, Error> {
        let (refreshed, index) = self.refresh_capturing_index()?;
        Ok(CommitRefresh {
            snapshot: refreshed.snapshot,
            advisories: refreshed.advisories,
            index,
        })
    }

    /// The payload `changelist` (`None` = unassigned) would commit out of
    /// `refreshed`, with the aftermath bookkeeping that ships with it —
    /// everything [`Repo::commit_prepared`] needs, and nothing written
    /// yet.
    ///
    /// Refuses for an unrecognised changelist, and for ADR 0004's
    /// foreign-content condition — both before any temp-index work
    /// exists to abandon.
    pub fn prepare_commit(
        &self,
        refreshed: &CommitRefresh,
        changelist: Option<&str>,
    ) -> Result<PreparedCommit, Error> {
        validate_changelist(&refreshed.snapshot, changelist)?;
        let plan = commit::plan(&refreshed.snapshot.files, &refreshed.index, changelist)?;
        Ok(PreparedCommit {
            stale: stale_addresses(&refreshed.snapshot, &plan.payload, changelist),
            changelist: changelist.map(str::to_owned),
            payload: plan.payload,
            paths: plan.paths,
        })
    }

    /// Commit what [`Repo::prepare_commit`] derived (ADR 0004): a
    /// temporary index built as HEAD's tree plus only this payload,
    /// committed via a native `git commit` so hooks run — the live index
    /// and worktree are never touched, and any failure changes nothing.
    /// The receipt is core's echo naming the commit git made (ADR 0006);
    /// the refresh's own advisories are the caller's to have delivered.
    ///
    /// A full refresh should follow; this leaves the state file
    /// consistent for it.
    pub fn commit_prepared(
        &self,
        prepared: &PreparedCommit,
        message: CommitMessage<'_>,
        options: &CommitOptions,
    ) -> Result<OpOutcome, Error> {
        let committed = self.execute_commit(prepared, message, options)?;
        Ok(OpOutcome::applied(commit::committed_echo(
            &committed.short_id,
            prepared.changelist.as_deref(),
            &prepared.payload,
        )))
    }

    /// Commit `changelist`'s staged hunks behind a synchronous refresh —
    /// the whole cycle in one call, for the frontend that confirms a
    /// payload rather than guarding one.
    ///
    /// `expected` is a payload from [`Repo::commit_payload`]: when the
    /// synchronous refresh finds the live payload differs, nothing is
    /// committed and [`CommitOutcome::Drifted`] returns the fresh
    /// payload for re-confirmation.
    pub fn commit(
        &self,
        changelist: Option<&str>,
        message: CommitMessage<'_>,
        options: &CommitOptions,
        expected: Option<&CommitPayload>,
    ) -> Result<CommitOutcome, Error> {
        // The operation guard (ADR 0007), ahead of everything — ahead of
        // the refresh too, which is what this entry point has that the
        // prepared path cannot: a caller guarding a payload has already
        // refreshed to see one.
        self.refuse_mid_operation()?;
        let refreshed = self.commit_refresh()?;
        let prepared = self.prepare_commit(&refreshed, changelist)?;
        // Ahead of the drift comparison rather than left to
        // `execute_commit`, which checks it too: an empty payload is
        // nothing to commit, not a confirmation gone stale, and it should
        // say so even where a confirmed payload no longer matches.
        if prepared.payload.is_empty() {
            return Err(Error::NothingStaged);
        }
        if let Some(expected) = expected
            && *expected != prepared.payload
        {
            return Ok(CommitOutcome::Drifted {
                payload: prepared.payload,
            });
        }
        let committed = self.execute_commit(&prepared, message, options)?;
        Ok(CommitOutcome::Committed {
            oid: committed.oid,
            short_id: committed.short_id,
        })
    }

    /// The temp-index commit and the locked aftermath, shared by the two
    /// public entry points so neither can come to bookkeep differently.
    ///
    /// On success one locked state update runs the ADR 0012 aftermath:
    /// consumed records removed, retained `◑` records rewritten against
    /// the new HEAD, surviving same-file records commuted, the new
    /// baseline HEAD stamped so the external-move guard never arms for an
    /// own commit, and the commit recorded as the last one gitchange made
    /// (ADR 0004 §Aftermath).
    fn execute_commit(
        &self,
        prepared: &PreparedCommit,
        message: CommitMessage<'_>,
        options: &CommitOptions,
    ) -> Result<CommittedId, Error> {
        // Both guards again at the last moment: this is where the commit
        // becomes irreversible, and a public prepared path means the
        // caller's own stack is not the only thing standing between an
        // in-progress operation — or an empty payload — and git.
        self.refuse_mid_operation()?;
        if prepared.payload.is_empty() {
            return Err(Error::NothingStaged);
        }
        let specs: Vec<CommitPathSpec> = prepared
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
        commit::apply_aftermath(&mut state, &prepared.paths, &residual);
        state.baseline_head = Some(committed.oid.clone());
        state.prune_records_of_unknown_changelists();
        // Every commit records itself (ADR 0004 §Aftermath), which is why
        // this update is unconditional: a changelist-less repo has no
        // records to commute, but the amend that may follow still needs
        // the record to tell this commit from another actor's — so a
        // state file grows here, holding nothing it was not asked to hold.
        state.record_commit(&committed.oid, prepared.changelist.as_deref());
        state_file::save(&dir, &state)?;
        Ok(committed)
    }

    /// The operation guard (ADR 0007): git honours `MERGE_HEAD` & co.
    /// even under `GIT_INDEX_FILE`, so a commit made now would conclude
    /// that operation with one changelist's payload. Enforced in core, so
    /// every frontend has it whatever its own stack says.
    fn refuse_mid_operation(&self) -> Result<(), Error> {
        match self.backend.operation()? {
            Some(operation) => Err(Error::OperationInProgress { operation }),
            None => Ok(()),
        }
    }

    /// The bulk align op (ADR 0004): set index := worktree for each of
    /// the changelist's staged-stale hunks — re-staging edited `◑`
    /// hunks, discarding index-only ones — so a follow-up commit carries
    /// what the worktree shows. Fail-soft per hunk like
    /// [`Repo::stage_hunk`]; the echo names the whole bulk op, with any
    /// stale-hunk advisories alongside.
    pub fn align(&self, changelist: Option<&str>) -> Result<OpOutcome, Error> {
        let tally = self.bulk_apply(
            SweepScope::changelist(changelist),
            |stage| stage == HunkStage::StagedStale,
            Self::stage_hunk,
        )?;
        Ok(OpOutcome {
            echo: Some(format!(
                "aligned index to worktree — '{}'",
                changelist.unwrap_or(UNASSIGNED)
            )),
            advisories: tally.advisories,
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
        let tally = self.bulk_apply(
            scope,
            |stage| stage == HunkStage::Unstaged,
            Self::stage_hunk,
        )?;
        // Silent when nothing staged: the offer's caller is mid-flow
        // towards a dialog, and has no use for a nothing-to-do line.
        let echo =
            (tally.moved > 0).then(|| sweep_echo("stage", tally.moved, tally.skipped, scope));
        Ok(OpOutcome {
            echo,
            advisories: tally.advisories,
        })
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
        let row = [StagingTarget::Row(path.to_owned())];
        self.refreshed_sweep(SweepScope::narrowed(&row, changelist), Direction::Stage)
    }

    /// The unstage direction of `space` on a Files row: set index :=
    /// HEAD for each staged hunk of `path` that `changelist` owns — the
    /// mirror of [`Repo::stage_owned_hunks`].
    pub fn unstage_owned_hunks(
        &self,
        path: &str,
        changelist: Option<&str>,
    ) -> Result<OpOutcome, Error> {
        let row = [StagingTarget::Row(path.to_owned())];
        self.refreshed_sweep(SweepScope::narrowed(&row, changelist), Direction::Unstage)
    }

    /// The CLI's stage sweep (#145): `space`'s stage direction over a
    /// scope the caller has already validated against `snapshot` — every
    /// hunk `changelist` owns, narrowed to `targets` when any are named
    /// (an empty slice is the whole changelist). `None` is unassigned, as
    /// everywhere.
    ///
    /// Takes the snapshot rather than refreshing behind the caller's back,
    /// so one invocation runs exactly one persisting refresh: the
    /// caller's, whose advisories are the caller's to deliver. That is
    /// what separates this from [`Repo::stage_changelist`], which serves a
    /// TUI keypress and owns its own refresh.
    pub fn stage_sweep<'a>(
        &self,
        snapshot: &'a Snapshot,
        changelist: Option<&str>,
        targets: &[StagingTarget<'a>],
    ) -> Result<SweepOutcome, Error> {
        self.sweep(
            snapshot,
            SweepScope::narrowed(targets, changelist),
            Direction::Stage,
        )
    }

    /// The CLI's unstage sweep (#145): the mirror of
    /// [`Repo::stage_sweep`] over the same scope model — index := HEAD for
    /// the scope's `●` hunks, and `●` only.
    ///
    /// Each `◑` hunk the direction's filter leaves behind rides the
    /// receipt as an [`Advisory::KeptStagedStale`] naming its address, so
    /// a staged version the worktree has since moved past is kept *and*
    /// said. The advisories are raised here rather than in the shared
    /// sweep because they answer for a CLI command's whole scope: the
    /// TUI's `space` is one keypress on a row the user is looking at,
    /// where the same lines would be noise in the Log panel.
    ///
    /// An addressed target is never kept and so never named: it unstages
    /// ungated (#145), which is the discard the notice exists to prevent
    /// happening *silently*.
    pub fn unstage_sweep<'a>(
        &self,
        snapshot: &'a Snapshot,
        changelist: Option<&str>,
        targets: &[StagingTarget<'a>],
    ) -> Result<SweepOutcome, Error> {
        let scope = SweepScope::narrowed(targets, changelist);
        let mut swept = self.sweep(snapshot, scope, Direction::Unstage)?;
        swept
            .receipt
            .advisories
            .extend(kept_staged_stale(snapshot, scope));
        Ok(swept)
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
        let tally = self.apply_over(
            snapshot,
            scope,
            |stage| direction.takes(stage),
            |repo, path, hunk| direction.apply(repo, path, hunk),
        )?;
        Ok(SweepOutcome {
            receipt: OpOutcome {
                echo: Some(sweep_echo(
                    direction.verb(),
                    tally.moved,
                    tally.skipped,
                    scope,
                )),
                advisories: tally.advisories,
            },
            moved: tally.moved,
            skipped: tally.skipped,
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
    ) -> Result<Tally, Error> {
        let snapshot = self.refresh()?.snapshot;
        validate_changelist(&snapshot, scope.changelist)?;
        self.apply_over(&snapshot, scope, include, apply)
    }

    /// The body every multi-hunk staging op shares: over one snapshot, run
    /// `apply` on each swept hunk whose staging state `include` accepts —
    /// hunks another changelist owns are never swept, and conflicted files
    /// carry none at all (ADR 0007). The [`Tally`] counts what moved and
    /// what failed soft as stale, and carries the latter's advisories.
    ///
    /// Both of those clauses are the swept rows' and nothing else: an
    /// addressed target names one hunk, so `include` does not gate it —
    /// which is what makes an addressed `◑` unstage where a sweep keeps it
    /// — and neither does ownership, the caller having named the hunk
    /// itself ([`StagingTarget::Hunk`], #145). A hunk that is both swept
    /// and addressed moves once, by its address: an address narrows its
    /// file row to the named hunk, and the narrowing is what the ungated
    /// apply is for.
    fn apply_over(
        &self,
        snapshot: &Snapshot,
        scope: SweepScope<'_>,
        include: impl Fn(HunkStage) -> bool,
        apply: impl Fn(&Self, &str, &Hunk) -> Result<OpOutcome, Error>,
    ) -> Result<Tally, Error> {
        let mut tally = Tally::default();
        for file in &snapshot.files {
            if !scope.sweeps(&file.path) {
                continue;
            }
            for hunk in file.owned_hunks(scope.changelist) {
                if include(hunk.stage) && !scope.addresses(&file.path, hunk) {
                    tally.record(apply(self, &file.path, hunk)?);
                }
            }
        }
        for (path, hunk) in scope.addressed() {
            tally.record(apply(self, path, hunk)?);
        }
        Ok(tally)
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
        let tally = self.assign_over(path, hunks, target)?;
        let echo = (tally.moved > 0).then(|| match target {
            Some(name) => format!(
                "{} {} — {path} {ARROW} '{name}'",
                assign_words(target).past,
                count_noun(tally.moved, "hunk")
            ),
            None => format!(
                "{} {} — {path}",
                assign_words(target).past,
                count_noun(tally.moved, "hunk")
            ),
        });
        Ok(OpOutcome {
            echo,
            advisories: tally.advisories,
        })
    }

    /// The CLI's assign sweep (#147): the hunks `targets` name moved to
    /// `target`, over a scope the caller has already validated against
    /// `snapshot` — one counted echo for the whole invocation, whatever the
    /// argument count. A whole path takes its change universe; an address
    /// takes the one hunk it named ([`AssignTarget`]).
    ///
    /// Takes the snapshot rather than refreshing behind the caller's back,
    /// for [`Repo::stage_sweep`]'s reason: one invocation runs exactly one
    /// persisting refresh, the caller's, whose advisories are the caller's
    /// to deliver.
    ///
    /// Hunks `target` already owns are left out of the payload, so a
    /// repeated assign is *satisfied* rather than counted again — the zero
    /// it reports is the nothing-needed one, not the wholly-stale one
    /// [`SweepOutcome::moved_nothing`] splits on. One case puts such a hunk
    /// back: the index-entry unit is widened at apply (ADR 0009), so a unit
    /// split between `target` and somebody else re-writes the member
    /// `target` already had, and the count says so — the echo counts the
    /// widened payload, which is what was written. Ownership is otherwise
    /// unchecked here: a sweep takes the path's whole universe, an address
    /// moves ungated, and who may take another changelist's hunks is the
    /// caller's guard (#147's `--take-owned`), a refusal with an exit code
    /// rather than a membership decision.
    pub fn assign_sweep(
        &self,
        snapshot: &Snapshot,
        targets: &[AssignTarget<'_>],
        target: Option<&str>,
    ) -> Result<SweepOutcome, Error> {
        let mut tally = Tally::default();
        for (path, moving) in assign_payloads(snapshot, targets, target) {
            tally.absorb(self.assign_over(path, &moving, target)?);
        }
        Ok(SweepOutcome {
            receipt: OpOutcome {
                echo: Some(assign_echo(tally.moved, tally.skipped, target)),
                advisories: tally.advisories,
            },
            moved: tally.moved,
            skipped: tally.skipped,
        })
    }

    /// One path's membership write: the live hunks matching `hunks`,
    /// widened to their index-entry unit, assigned to `target` under one
    /// locked cycle — with what vanished counted and advised instead. The
    /// body both assign forms share, so the per-path op and the sweep
    /// cannot come to widen, fail soft, or count differently.
    fn assign_over(
        &self,
        path: &str,
        hunks: &[Hunk],
        target: Option<&str>,
    ) -> Result<Tally, Error> {
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
        if !fresh.is_empty() {
            self.update_state(|state| state.assign_records(path, &fresh, target))?;
        }
        Ok(Tally {
            moved: fresh.len(),
            skipped: advisories.len(),
            advisories,
        })
    }

    /// The changelist roster, read without recomputing anything: the
    /// CLI's bare listing (#149) asks only what the state file already
    /// says, so neither refresh form is run — a persisting one would
    /// capture hunks nobody asked it to, and even a read-only one would
    /// pay for both diffs and the matcher to answer from a field it
    /// carries verbatim.
    ///
    /// Like every read it takes no lock: writers replace the file
    /// atomically (ADR 0002), so a roster is some writer's before or
    /// after state, never a torn one.
    pub fn roster(&self) -> Result<Roster, Error> {
        let state = state_file::load(&self.backend.state_dir())?;
        Ok(Roster {
            changelists: state.changelists,
            active: state.active,
        })
    }

    /// Whether HEAD is the commit gitchange last made for `changelist`
    /// (`None` = unassigned) — the fact behind the CLI's foreign-head
    /// guard (ADR 0004 §Amend, spec #151), which is where the decision
    /// this answers for lives: refusing, and what overrides it, is a
    /// frontend's policy.
    ///
    /// Two readings taken together because the comparison is between
    /// commit ids, and frontends never speak git (ADR 0006): the state
    /// file's last-commit record must name this changelist *and* still be
    /// where HEAD points. `false` covers every shape of the hazard —
    /// another changelist's commit, a commit made outside gitchange, an
    /// unborn HEAD, no record at all — because an amend folds its payload
    /// into HEAD's commit regardless of which one it is.
    ///
    /// A state read like [`Repo::roster`]: no refresh, no lock.
    pub fn head_is_own_last_commit(&self, changelist: Option<&str>) -> Result<bool, Error> {
        let Some(oid) = self.backend.head_oid()? else {
            return Ok(false);
        };
        let state = state_file::load(&self.backend.state_dir())?;
        Ok(state.is_last_commit(&oid, changelist))
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

    /// Delete changelists, pruning all of their records (ADR 0016) —
    /// **all-or-nothing against one locked read** (#149): every name is
    /// validated against the same state the deletions then run on, and
    /// any offender refuses the whole call with nothing written. One
    /// locked cycle is what makes that a guarantee rather than a race:
    /// validation and deletion cannot see two different states.
    ///
    /// [`Release::Guarded`] is the records guard — a changelist holding
    /// any records, live or dormant, refuses rather than releasing its
    /// hunks recordless for the next persisting refresh to claim, which
    /// in a shared tree can land one actor's work under another's name.
    /// [`Release::Forced`] overrides that guard **only**: an unrecognised
    /// name still refuses.
    ///
    /// A bare state write: no refresh runs (ADR 0005's persisting set is
    /// unchanged), so the guard counts exactly what the records say — and
    /// recorded membership is the only membership (ADR 0016), so there is
    /// nothing for it to miss. Deleting the active changelist leaves
    /// unassigned active and says so; a forced release counts what it
    /// released. Both are decisions the caller did not ask for, so both
    /// ride the receipt as advisories.
    ///
    /// A name given twice is one delete: the redundancy is absorbed here,
    /// where the locked cycle is, so no caller has to dedupe a command
    /// line before it can be honoured.
    pub fn delete_changelists(&self, names: &[&str], release: Release) -> Result<Deletion, Error> {
        let named = distinct(names);
        let mut refused = Vec::new();
        let mut advisories = Vec::new();
        // Everything is read inside the mutation, so every fact the
        // receipt states is the one the delete acted on: a switch or a
        // refresh landing between a decision and the write would
        // otherwise make these counts and this marker a guess.
        let wrote = self.update_state(|state| {
            refused = undeletable(state, &named, release);
            if !refused.is_empty() {
                return Ok(());
            }
            for name in &named {
                let records = state.record_counts(name);
                let was_active = state.active.as_deref() == Some(*name);
                state.delete(name)?;
                if records.any() {
                    advisories.push(Advisory::RecordsReleased {
                        changelist: (*name).to_owned(),
                        records,
                    });
                }
                if was_active {
                    advisories.push(Advisory::ActiveChangelistDeleted {
                        changelist: (*name).to_owned(),
                    });
                }
            }
            Ok(())
        })?;
        if !refused.is_empty() {
            return Ok(Deletion::Refused(refused));
        }
        let mut receipt = OpOutcome::decided(wrote, || deleted_echo(&named));
        receipt.advisories = advisories;
        Ok(Deletion::Done(receipt))
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

/// The `◑` hunks an unstage sweep over `scope` leaves in the index, each
/// as the notice that names it (#145). Read off the snapshot the sweep
/// ran against, which is where the staging states it decided by live:
/// unstaging a `●` hunk cannot make one of its neighbours stale.
///
/// A `◑` hunk the scope also addresses is not kept — the address unstaged
/// it — so it is not named either, even where its file row is swept beside
/// it: the notice exists to say what stayed.
fn kept_staged_stale(snapshot: &Snapshot, scope: SweepScope<'_>) -> Vec<Advisory> {
    let mut kept = Vec::new();
    for file in snapshot
        .files
        .iter()
        .filter(|file| scope.sweeps(&file.path))
    {
        // Addresses are minted per file because the ordinal that tells
        // identical hunks apart is a file-level fact, and only where a
        // notice is actually due — no address is computed for a sweep
        // that keeps nothing.
        let addresses = file.hunk_addresses();
        for (hunk, address) in file.hunks.iter().zip(addresses) {
            if hunk.owned_by(scope.changelist)
                && hunk.stage == HunkStage::StagedStale
                && !scope.addresses(&file.path, hunk)
            {
                kept.push(Advisory::KeptStagedStale {
                    address: address.abbreviated_at(&file.path),
                    changelist: scope.changelist.map(str::to_owned),
                });
            }
        }
    }
    kept
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

/// What a membership op in this direction is called: the bare verb a
/// nothing-to-do line needs and the past tense a receipt needs. One place,
/// so the per-path op and the sweep cannot come to call a release an
/// assignment (ADR 0006) — the direction is `target`'s alone, there being
/// no third way to move a hunk's membership (ADR 0016: releasing is not
/// placing).
struct AssignWords {
    stem: &'static str,
    past: &'static str,
}

fn assign_words(target: Option<&str>) -> AssignWords {
    match target {
        Some(_) => AssignWords {
            stem: "assign",
            past: "assigned",
        },
        None => AssignWords {
            stem: "release",
            past: "released",
        },
    }
}

/// An assign sweep's echo (#147): one line for the whole invocation,
/// counting what moved and naming the target — the release direction
/// included.
///
/// Shaped like [`sweep_echo`] and for the same reasons: the skips ride the
/// count on stdout, so a harness that drops stderr still sees the apply was
/// partial, and the `of` clause appears only when something was skipped.
/// The satisfied line is its own answer rather than a zero count — nothing
/// needed to move, which is not the same fact as nothing moving.
fn assign_echo(moved: usize, skipped: usize, target: Option<&str>) -> String {
    let holder = holder_label(target);
    let AssignWords { stem, past } = assign_words(target);
    match (moved, skipped) {
        (0, 0) => format!("nothing to {stem} — every hunk named already belongs to {holder}"),
        (moved, 0) => format!("{past} {} {ARROW} {holder}", count_noun(moved, "hunk")),
        (moved, skipped) => format!(
            "{past} {moved} of {} ({skipped} skipped as stale) {ARROW} {holder}",
            count_noun(moved + skipped, "hunk")
        ),
    }
}

/// One thing a staging op moves, inside its changelist scope. The two
/// variants are the two kinds of narrowing the staging verbs have, and the
/// distinction is CONTEXT.md's §Sweep boundary: a **row** is swept, an
/// **address** is not. They mix freely in one argument list (#145), so one
/// call carries both.
#[derive(Debug, Clone)]
pub enum StagingTarget<'a> {
    /// A **file row**: every hunk the scope's changelist owns in this
    /// file, filtered by the direction. Repo-relative, as core reports
    /// paths.
    Row(String),
    /// The one hunk an address named. Not a sweep: an address has already
    /// decided which hunk moves, so the direction's filter is deliberately
    /// not applied and neither is the row's ownership scoping — the caller
    /// owns both questions, exactly as it does for [`Repo::stage_hunk`],
    /// which the TUI's per-hunk `space` calls without either. The CLI's
    /// consistency guard (#145) is its own stricter rule, and it lives
    /// there because it is a refusal with an exit code, not an index
    /// decision.
    Hunk {
        path: String,
        /// The hunk, borrowed from the snapshot the op runs against.
        /// Comparisons are by place within that snapshot, so a hunk from
        /// any other snapshot reads as a different place — at worst
        /// applying an idempotent index write twice and counting it twice
        /// in the echo.
        hunk: &'a Hunk,
        /// The composed address the caller named this hunk by, which the
        /// echo prints back — the caller's own string, like the path a
        /// row-scoped echo names.
        address: String,
    },
}

impl<'a> StagingTarget<'a> {
    /// How the echo names this target: the path for a row, the composed
    /// address for a hunk — in both cases the string the caller typed the
    /// narrowing as.
    fn label(&self) -> &str {
        match self {
            StagingTarget::Row(path) => path,
            StagingTarget::Hunk { address, .. } => address,
        }
    }

    /// Whether this target is the hunk at `path` — the same *place*,
    /// matched by identity within the snapshot the op runs against rather
    /// than by content, so two identical hunks in one file stay two
    /// targets (their addresses differ by ordinal, and so must they).
    ///
    /// Private: it is only ever asked of hunks from the traversed
    /// snapshot, which is what makes the identity match sound.
    fn same_place(&self, path: &str, hunk: &Hunk) -> bool {
        match self {
            StagingTarget::Row(_) => false,
            StagingTarget::Hunk {
                path: mine,
                hunk: addressed,
                ..
            } => mine == path && std::ptr::eq(*addressed, hunk),
        }
    }
}

/// One thing an assign moves. The boundary is `StagingTarget`'s — CONTEXT.md
/// §Sweep's, a **path** is swept and an **address** is not — and the two
/// verbs' targets stay separate types because the verbs mean different things
/// by a path: a staging row is scoped to one changelist's hunks in the file,
/// where a path here takes the file's whole change universe, membership being
/// independent of staging (ADR 0003).
///
/// They mix freely in one argument list (#164), so one call carries both.
#[derive(Debug, Clone)]
pub enum AssignTarget<'a> {
    /// A **path**: every hunk of the changed file, staged or not.
    /// Repo-relative, as core reports paths.
    Path(String),
    /// The one hunk an address named, borrowed from the snapshot the op
    /// runs against — so places are compared by identity within it, as
    /// [`StagingTarget::Hunk`] does.
    Hunk { path: String, hunk: &'a Hunk },
}

impl AssignTarget<'_> {
    /// The path this target lives in, whichever narrowing it is.
    fn path(&self) -> &str {
        match self {
            AssignTarget::Path(path) => path,
            AssignTarget::Hunk { path, .. } => path,
        }
    }

    /// Whether this target sweeps `path` whole — [`SweepScope::sweeps`]'s
    /// counterpart, minus the empty-scope arm: assign has no bare form, so
    /// there is no "everything" to fall back on.
    fn sweeps(&self, path: &str) -> bool {
        matches!(self, AssignTarget::Path(named) if named == path)
    }

    /// Whether this target is the hunk at `path` — the same *place*,
    /// matched by identity within the snapshot the op runs against, for
    /// [`StagingTarget::same_place`]'s reason.
    fn same_place(&self, path: &str, hunk: &Hunk) -> bool {
        match self {
            AssignTarget::Path(_) => false,
            AssignTarget::Hunk {
                path: mine,
                hunk: addressed,
            } => mine == path && std::ptr::eq(*addressed, hunk),
        }
    }
}

/// What each named path's membership write moves: one payload per **distinct**
/// path, in argument order, holding the hunks that path's targets named and
/// that `owner` does not already hold.
///
/// Grouping here is what makes the argument list's redundancies free: a path
/// named twice is one payload, an address named twice is one hunk, and a path
/// swept whole subsumes any address into it — every one of them a repeat that
/// would otherwise be written and counted twice, since a path's write is one
/// locked cycle over the payload it is handed.
///
/// Hunks `owner` already holds are dropped rather than rewritten, which is
/// what makes a repeated assign *satisfied* (see [`Repo::assign_sweep`]).
fn assign_payloads<'t>(
    snapshot: &Snapshot,
    targets: &'t [AssignTarget<'_>],
    owner: Option<&str>,
) -> Vec<(&'t str, Vec<Hunk>)> {
    let mut payloads: Vec<(&str, Vec<Hunk>)> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for target in targets {
        let path = target.path();
        if seen.contains(&path) {
            continue;
        }
        seen.push(path);
        // The caller validated these paths against this same snapshot, so a
        // path with no file here is a caller bug rather than a clean path —
        // an empty sweep is never the answer to one (#147).
        let Some(file) = snapshot.files.iter().find(|file| file.path == path) else {
            debug_assert!(
                false,
                "'{path}' is not in the snapshot it was validated against"
            );
            continue;
        };
        let swept = targets.iter().any(|target| target.sweeps(path));
        let named: Vec<&Hunk> = match swept {
            true => file.hunks.iter().collect(),
            false => addressed_in(targets, path),
        };
        let moving: Vec<Hunk> = named
            .into_iter()
            .filter(|hunk| !hunk.owned_by(owner))
            .cloned()
            .collect();
        if !moving.is_empty() {
            payloads.push((path, moving));
        }
    }
    payloads
}

/// The hunks `targets` address in `path`, each once: two targets naming one
/// place ([`AssignTarget::same_place`]) are one hunk.
fn addressed_in<'a>(targets: &[AssignTarget<'a>], path: &str) -> Vec<&'a Hunk> {
    let mut addressed: Vec<&Hunk> = Vec::new();
    for target in targets {
        let AssignTarget::Hunk { hunk, .. } = target else {
            continue;
        };
        if target.path() != path {
            continue;
        }
        // Asked of what is already collected, by the target's own identity
        // rule: a hunk named twice is one hunk, and two identical hunks
        // named by their ordinals stay two.
        if addressed.iter().any(|seen| target.same_place(path, seen)) {
            continue;
        }
        addressed.push(hunk);
    }
    addressed
}

/// What a staging op ranges over: one changelist's hunks, optionally
/// narrowed to some of its file rows and single hunks. Every scope `space`
/// and the staging verbs have above the hunk (ADR 0003, #145) differs only
/// in that narrowing, so they share one traversal and one echo shape.
#[derive(Debug, Clone, Copy)]
struct SweepScope<'a> {
    /// `None` is unassigned, as everywhere.
    changelist: Option<&'a str>,
    /// What this scope is narrowed to. Empty is the whole changelist.
    targets: &'a [StagingTarget<'a>],
}

impl<'a> SweepScope<'a> {
    fn changelist(changelist: Option<&'a str>) -> Self {
        Self {
            changelist,
            targets: &[],
        }
    }

    fn narrowed(targets: &'a [StagingTarget<'a>], changelist: Option<&'a str>) -> Self {
        Self {
            changelist,
            targets,
        }
    }

    /// Whether this scope sweeps `path`'s whole file row. A path reached
    /// by an address alone is not swept: the address is the narrowing.
    fn sweeps(&self, path: &str) -> bool {
        self.targets.is_empty()
            || self
                .targets
                .iter()
                .any(|target| matches!(target, StagingTarget::Row(row) if row == path))
    }

    /// Whether one of this scope's addresses names exactly this hunk.
    fn addresses(&self, path: &str, hunk: &Hunk) -> bool {
        self.targets
            .iter()
            .any(|target| target.same_place(path, hunk))
    }

    /// The addressed hunks, each with its path.
    fn addressed(&self) -> impl Iterator<Item = (&str, &'a Hunk)> {
        self.targets.iter().filter_map(|target| match target {
            StagingTarget::Row(_) => None,
            StagingTarget::Hunk { path, hunk, .. } => Some((path.as_str(), *hunk)),
        })
    }

    /// The scope as the echo names it: `'feature'`, or `a.txt in
    /// 'feature'` — a narrowed echo says which changelist it stayed
    /// inside, since that is exactly what it did not sweep past.
    fn target(&self) -> String {
        let name = self.changelist.unwrap_or(UNASSIGNED);
        match self.targets {
            [] => format!("'{name}'"),
            targets => format!(
                "{} in '{name}'",
                targets
                    .iter()
                    .map(StagingTarget::label)
                    .collect::<Vec<&str>>()
                    .join(", ")
            ),
        }
    }
}

/// A multi-hunk apply's running count: what moved, what failed soft as
/// stale, and the advisories the latter raised. One place, so every
/// traversal in [`Repo::apply_over`] classifies an outcome the same way —
/// an op that raised no advisory is an op that wrote.
#[derive(Default)]
struct Tally {
    moved: usize,
    skipped: usize,
    advisories: Vec<Advisory>,
}

impl Tally {
    /// Fold another traversal's counts into this one — the multi-path
    /// ops' running total, where one call already counted many hunks.
    fn absorb(&mut self, other: Tally) {
        self.moved += other.moved;
        self.skipped += other.skipped;
        self.advisories.extend(other.advisories);
    }

    fn record(&mut self, outcome: OpOutcome) {
        if outcome.advisories.is_empty() {
            self.moved += 1;
        } else {
            self.skipped += 1;
        }
        self.advisories.extend(outcome.advisories);
    }
}

/// The payload's staged-stale hunks as composed addresses, in payload
/// order — [`PreparedCommit::stale_addresses`]'s content, minted here
/// while the snapshot the payload came out of is still in hand.
///
/// Read off the snapshot rather than carried out of the plan because an
/// address is a fact about the file's whole hunk list (the ID's ordinal
/// disambiguates among identical hunks, ADR 0011), which the plan's
/// per-path selection no longer has. Payload files with no `◑` are
/// skipped, so the common case walks nothing.
fn stale_addresses(
    snapshot: &Snapshot,
    payload: &CommitPayload,
    changelist: Option<&str>,
) -> Vec<String> {
    let mut addresses = Vec::new();
    for file in &payload.files {
        if file.stale_hunks == 0 {
            continue;
        }
        let Some(changed) = snapshot.files.iter().find(|it| it.path == file.path) else {
            continue;
        };
        for (hunk, address) in changed.hunks.iter().zip(changed.hunk_addresses()) {
            if hunk.owned_by(changelist) && hunk.stage == HunkStage::StagedStale {
                addresses.push(address.abbreviated_at(&changed.path));
            }
        }
    }
    addresses
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
