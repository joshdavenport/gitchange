//! Hunk-universe derivation (ADR 0003): the union of diff(HEAD↔worktree)
//! and diff(HEAD↔index), with per-hunk and per-file staging derived by
//! comparing the two. Pure — a function of the diffs alone.

use std::collections::BTreeMap;

use crate::diff::{ChangeKind, DiffHunk, FileDiff, HunkLine, RepoDiffs};

/// One file in the hunk universe, staging derived.
#[derive(Debug, Clone)]
pub struct ChangedFile {
    /// Repo-relative path, as git reports it.
    pub path: String,
    pub kind: ChangeKind,
    /// Binary files carry no text hunks until ticket 35's whole-file
    /// degenerate hunks (ADR 0009); they stay visible regardless — with
    /// the known gap that a hunk-less file derives `○ 0/0` even when its
    /// content is staged, until ticket 35 lands OID-compare derivation.
    pub binary: bool,
    /// Hunks in file order: the worktree diff's hunks plus any index-only
    /// hunks (staged then worktree-reverted).
    pub hunks: Vec<Hunk>,
}

impl ChangedFile {
    pub fn total_hunks(&self) -> usize {
        self.hunks.len()
    }

    /// Cleanly staged hunks (`●`) only — a staged-stale hunk's index
    /// content isn't what the worktree shows, so it never counts as
    /// staged; it surfaces through the `◐` file marker instead.
    pub fn staged_hunks(&self) -> usize {
        self.hunks
            .iter()
            .filter(|hunk| hunk.stage == HunkStage::Staged)
            .count()
    }

    /// Per-file marker per ADR 0003: `●` all staged, `○` all unstaged,
    /// `◐` any mix — staged-stale counts toward `◐`.
    pub fn stage(&self) -> FileStage {
        let mut all_staged = !self.hunks.is_empty();
        let mut all_unstaged = true;
        for hunk in &self.hunks {
            all_staged &= hunk.stage == HunkStage::Staged;
            all_unstaged &= hunk.stage == HunkStage::Unstaged;
        }
        if all_staged {
            FileStage::Staged
        } else if all_unstaged {
            FileStage::Unstaged
        } else {
            FileStage::PartiallyStaged
        }
    }
}

/// One hunk in the universe. Coordinates are the worktree diff's when the
/// hunk appears there, the index diff's for index-only hunks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<HunkLine>,
    pub stage: HunkStage,
    /// True for hunks only the index diff sees (staged then reverted in
    /// the worktree) — their coordinates and lines address index content,
    /// not the worktree, so staging ops treat them differently.
    pub index_only: bool,
    /// Owning changelist, written by the matcher after universe
    /// derivation; `None` is unassigned.
    pub changelist: Option<String>,
}

/// Per-hunk staging per ADR 0003's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkStage {
    /// `○` — in the worktree diff only.
    Unstaged,
    /// `●` — in both diffs with equal content.
    Staged,
    /// `◑` — the index holds an overlapping-but-different version, or an
    /// index-only hunk (staged then reverted in the worktree).
    StagedStale,
}

/// Per-file staging marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStage {
    /// `○`
    Unstaged,
    /// `◐`
    PartiallyStaged,
    /// `●`
    Staged,
}

/// Build the hunk universe from the two diffs, sorted by path.
pub(crate) fn build(diffs: RepoDiffs) -> Vec<ChangedFile> {
    let mut index_by_path: BTreeMap<String, FileDiff> = diffs
        .index
        .into_iter()
        .map(|file| (file.path.clone(), file))
        .collect();

    let mut files: Vec<ChangedFile> = diffs
        .worktree
        .into_iter()
        .map(|worktree| {
            let index = index_by_path.remove(&worktree.path);
            merge_file(Some(worktree), index)
        })
        .collect();
    // Index-only files: staged content whose worktree copy matches HEAD
    // again (or is gone) — still committable, so still in the universe.
    files.extend(
        index_by_path
            .into_values()
            .map(|index| merge_file(None, Some(index))),
    );

    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

fn merge_file(worktree: Option<FileDiff>, index: Option<FileDiff>) -> ChangedFile {
    let kind = merge_kind(
        worktree.as_ref().map(|file| file.kind),
        index.as_ref().map(|file| file.kind),
    );
    let (path, binary) = {
        let either = worktree.as_ref().or(index.as_ref()).expect("one diff side");
        (either.path.clone(), either.binary)
    };
    let hunks = pair_hunks(
        worktree.map(|file| file.hunks).unwrap_or_default(),
        index.map(|file| file.hunks).unwrap_or_default(),
    );
    ChangedFile {
        path,
        kind,
        binary,
        hunks,
    }
}

/// One kind for a file seen by either diff. Precedence mirrors the old
/// status walk: gone from the worktree reads as gone, not present.
fn merge_kind(worktree: Option<ChangeKind>, index: Option<ChangeKind>) -> ChangeKind {
    use ChangeKind::*;
    match (worktree, index) {
        (Some(Conflicted), _) | (_, Some(Conflicted)) => Conflicted,
        (Some(Deleted), _) => Deleted,
        // Index-only Added: a file absent from HEAD produces a worktree
        // delta whenever it exists on disk, so none means it was removed.
        (None, Some(Added)) => Deleted,
        (None, Some(kind)) => kind,
        (Some(Untracked), Some(Added)) => Added,
        (Some(Untracked), _) => Untracked,
        (Some(TypeChanged), _) | (_, Some(TypeChanged)) => TypeChanged,
        (Some(Added), _) | (_, Some(Added)) => Added,
        _ => Modified,
    }
}

/// Derive per-hunk staging by pairing the two diffs' hunks on their old
/// (HEAD-side) ranges — the coordinate space both diffs share.
fn pair_hunks(worktree: Vec<DiffHunk>, index: Vec<DiffHunk>) -> Vec<Hunk> {
    let mut consumed = vec![false; index.len()];
    let mut hunks: Vec<Hunk> = Vec::with_capacity(worktree.len());

    for hunk in worktree {
        let overlapping: Vec<usize> = index
            .iter()
            .enumerate()
            .filter(|(_, candidate)| old_ranges_overlap(&hunk, candidate))
            .map(|(position, _)| position)
            .collect();
        let exact = overlapping
            .iter()
            .copied()
            .find(|&position| content_equal(&hunk, &index[position]));
        let stage = if let Some(position) = exact {
            // Consume only the equal hunk: a padded-boundary neighbour
            // that merely touches must still surface as index-only.
            consumed[position] = true;
            HunkStage::Staged
        } else if overlapping.is_empty() {
            HunkStage::Unstaged
        } else {
            for position in overlapping {
                consumed[position] = true;
            }
            HunkStage::StagedStale
        };
        hunks.push(to_hunk(hunk, stage, false));
    }

    // Unpaired index hunks are committable but invisible in the worktree
    // diff — the "reverted in worktree" staged-stale flavour.
    for (position, hunk) in index.into_iter().enumerate() {
        if !consumed[position] {
            hunks.push(to_hunk(hunk, HunkStage::StagedStale, true));
        }
    }

    hunks.sort_by_key(|hunk| (hunk.old_start, hunk.new_start));
    hunks
}

fn to_hunk(hunk: DiffHunk, stage: HunkStage, index_only: bool) -> Hunk {
    Hunk {
        old_start: hunk.old_start,
        old_lines: hunk.old_lines,
        new_start: hunk.new_start,
        new_lines: hunk.new_lines,
        lines: hunk.lines,
        stage,
        index_only,
        changelist: None,
    }
}

/// Both diffs share HEAD as the old side, so old coordinates compare
/// directly; new coordinates can shift when the diffs disagree earlier in
/// the file, so equality deliberately ignores them. Context lines count
/// as content: an unstaged edit inside a staged hunk's context flips it
/// `●`→`◑` — conservative, never a false `●`.
fn content_equal(a: &DiffHunk, b: &DiffHunk) -> bool {
    a.old_start == b.old_start && a.old_lines == b.old_lines && a.lines == b.lines
}

/// Overlap on old ranges, widening empty ranges (pure insertions) to one
/// line so two insertions at the same spot pair up.
fn old_ranges_overlap(a: &DiffHunk, b: &DiffHunk) -> bool {
    let span = |hunk: &DiffHunk| (hunk.old_start, hunk.old_start + hunk.old_lines.max(1));
    let (a_start, a_end) = span(a);
    let (b_start, b_end) = span(b);
    a_start < b_end && b_start < a_end
}
