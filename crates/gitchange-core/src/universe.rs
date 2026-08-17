//! Hunk-universe derivation (ADR 0003): the union of diff(HEAD↔worktree)
//! and diff(HEAD↔index), with per-hunk and per-file staging derived by
//! comparing the two. Pure — a function of the diffs alone.

use std::collections::BTreeMap;

use crate::diff::{ChangeKind, DiffHunk, FileDiff, FileSides, HunkLine, ModeDelta, RepoDiffs};
use crate::state::{OidAnchor, RecordAnchors};
use crate::vocabulary;

/// One file in the hunk universe, staging derived.
#[derive(Debug, Clone)]
pub struct ChangedFile {
    /// Repo-relative path, as git reports it.
    pub path: String,
    pub kind: ChangeKind,
    /// A binary file's change is one whole-file degenerate hunk
    /// (ADR 0009): OID-anchored, staging derived by OID compare.
    pub binary: bool,
    /// Hunks in file order: the worktree diff's hunks plus any index-only
    /// hunks (staged then worktree-reverted). Never empty for a
    /// non-conflicted file — a change with no line content presents one
    /// degenerate hunk instead: a mode hunk for a mode-only change, a
    /// whole-file hunk otherwise (ADR 0017).
    pub hunks: Vec<Hunk>,
    /// Per-side blob info for a file presenting a degenerate hunk —
    /// whole-file or mode — worktree view preferred: the diff
    /// placeholder's size and mode material, and the mode hunk's staging
    /// evidence (ADR 0009, ADR 0017). `None` for a file with text hunks.
    pub sides: Option<FileSides>,
}

impl ChangedFile {
    pub fn total_hunks(&self) -> usize {
        self.hunks.len()
    }

    /// Whether this file's whole change is the one degenerate whole-file
    /// hunk — a changed binary (ADR 0009), an empty file added or
    /// deleted, or a type change (ADR 0017). False for a file with
    /// line-addressable hunks, for a mode-only change (that is a mode
    /// hunk), and for a conflicted file, which has no hunks at all.
    ///
    /// The one test for *this* shape, asked where the whole-file index
    /// write is what a hunk op means, rather than re-deriving it from
    /// `binary` or from a hunk count. Readers that treat every degenerate
    /// hunk alike — the commit planner, the diff panel — ask
    /// [`ChangedFile::presents_lone_degenerate_hunk`] instead.
    pub fn presents_whole_file_hunk(&self) -> bool {
        matches!(
            self.hunks.as_slice(),
            [hunk] if matches!(hunk.identity, HunkIdentity::WholeFile { .. })
        )
    }

    /// Whether this file's whole change is one degenerate hunk of either
    /// flavour — whole-file or mode. The shape with nothing to drill
    /// into: the diff panel renders its placeholder and hunk-mode entry
    /// is the polite no-op (ADR 0009's idiom, ADR 0017), and the commit
    /// planner hands it the whole-file payload because one index entry
    /// carries the whole change either way.
    pub fn presents_lone_degenerate_hunk(&self) -> bool {
        matches!(
            self.hunks.as_slice(),
            [hunk] if !matches!(hunk.identity, HunkIdentity::Text { .. })
        )
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

    /// Per-file marker per ADR 0003 over the whole file — what the
    /// file-level surfaces (Diff title, CLI file list) show.
    pub fn stage(&self) -> FileStage {
        file_stage(&self.hunks)
    }

    /// This file's hunks owned by `owner` ([`Hunk::owned_by`]), in file
    /// order — a Files row's scope (issue #97).
    pub fn owned_hunks(&self, owner: Option<&str>) -> impl Iterator<Item = &Hunk> {
        self.hunks.iter().filter(move |hunk| hunk.owned_by(owner))
    }
}

/// Per-file marker per ADR 0003 for an arbitrary hunk set: `●` all
/// staged, `○` all unstaged, `◐` any mix — staged-stale counts toward
/// `◐`, and an empty set is `○`. Takes a set rather than a file, because
/// a Files row derives its marker from the hunks its changelist owns
/// rather than from the whole file (issue #97).
pub fn file_stage<'a>(hunks: impl IntoIterator<Item = &'a Hunk>) -> FileStage {
    let mut any = false;
    let mut all_staged = true;
    let mut all_unstaged = true;
    for hunk in hunks {
        any = true;
        all_staged &= hunk.stage == HunkStage::Staged;
        all_unstaged &= hunk.stage == HunkStage::Unstaged;
    }
    if any && all_staged {
        FileStage::Staged
    } else if all_unstaged {
        FileStage::Unstaged
    } else {
        FileStage::PartiallyStaged
    }
}

/// What a hunk *is*, and therefore what identifies it. The flavours are
/// mutually exclusive by construction: a text hunk's identity is its
/// verbatim lines, a whole-file hunk's is its blob-OID pair (ADR 0009),
/// a mode hunk's is its path alone (ADR 0017), and none can carry
/// another's evidence. Every site that has to tell them apart matches
/// here rather than testing a field for emptiness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HunkIdentity {
    /// A text hunk: the verbatim diff lines, origin included.
    Text { lines: Vec<HunkLine> },
    /// The degenerate single hunk a changed binary file presents
    /// (`CONTEXT.md`, "Whole-file hunk"): no lines, identity carried by
    /// the OID pair instead.
    WholeFile { oids: OidAnchor },
    /// The stand-alone hunk a mode delta presents (`CONTEXT.md`, "Mode
    /// hunk"): no lines and no OID pair, because the file's blobs belong
    /// to its content records — any edit would shift a blob anchor out
    /// from under this one. Identity is path continuity alone, which is
    /// also why the variant carries no fields: the modes are snapshot
    /// data for display and stage derivation, never identity (ADR 0017).
    ModeChange,
}

impl HunkIdentity {
    /// Whether two identities name the same hunk — the "is this still the
    /// thing the user pointed at?" test that validate-at-apply
    /// (ADR 0005) and the TUI's selection survival both run.
    ///
    /// Text compares verbatim lines, so a hunk merely shifted by edits
    /// above it still counts. A whole-file hunk keeps its identity **by
    /// path continuity** (`CONTEXT.md`, "Whole-file hunk"): the whole
    /// file *is* the hunk, so a re-export — same path, new content — is
    /// still the same hunk, exactly as tier-2 membership treats it. OIDs
    /// deliberately don't participate; comparing them would make a
    /// background rewrite silently void an assign. A mode hunk has only
    /// path continuity to go on, so any two of them at a path match —
    /// including a re-flip in the other direction, which is the same
    /// predicament and belongs in the same home (ADR 0017).
    ///
    /// Cross-flavour never matches: both sides would otherwise agree on
    /// the emptiness that used to stand in for this check.
    pub fn same_hunk(&self, other: &HunkIdentity) -> bool {
        match (self, other) {
            (HunkIdentity::Text { lines }, HunkIdentity::Text { lines: candidate }) => {
                lines == candidate
            }
            (HunkIdentity::WholeFile { .. }, HunkIdentity::WholeFile { .. }) => true,
            (HunkIdentity::ModeChange, HunkIdentity::ModeChange) => true,
            _ => false,
        }
    }

    /// The verbatim lines, for the text flavour only — `None` says "this
    /// hunk has no lines to render or anchor on", where an empty slice
    /// would read as "a text hunk that happens to be empty".
    pub fn text_lines(&self) -> Option<&[HunkLine]> {
        match self {
            HunkIdentity::Text { lines } => Some(lines),
            HunkIdentity::WholeFile { .. } | HunkIdentity::ModeChange => None,
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
    pub stage: HunkStage,
    /// True for hunks only the index diff sees (staged then reverted in
    /// the worktree) — their coordinates and lines address index content,
    /// not the worktree, so staging ops treat them differently.
    pub index_only: bool,
    /// What this hunk is, and the evidence that identifies it.
    pub identity: HunkIdentity,
    /// Owning changelist, written by the matcher after universe
    /// derivation; `None` is unassigned.
    pub changelist: Option<String>,
}

impl Hunk {
    /// Whether `owner` holds this hunk; `None` is unassigned. The one
    /// ownership test — a Files row's marker and its `space` scope, the
    /// diff's foreign tagging and the assign payloads all read it, so no
    /// surface can disagree with another about who holds a hunk.
    pub fn owned_by(&self, owner: Option<&str>) -> bool {
        self.changelist.as_deref() == owner
    }

    /// Whether this hunk is a mode delta's stand-alone hunk — what routes
    /// staging to the mode-only index write (ADR 0017).
    pub fn is_mode_change(&self) -> bool {
        matches!(self.identity, HunkIdentity::ModeChange)
    }

    /// The identity projected into the fields [`MembershipRecord`] stores
    /// it in. The record is a serde type whose on-disk layout keeps them
    /// independent (ADR 0002 `cat`-debuggability), so this is the one
    /// place the sum type is flattened back out — every other reader
    /// matches [`HunkIdentity`] instead.
    ///
    /// [`MembershipRecord`]: crate::state::MembershipRecord
    pub(crate) fn record_anchors(&self) -> RecordAnchors {
        match &self.identity {
            HunkIdentity::Text { lines } => RecordAnchors {
                anchor: crate::matcher::anchor_lines(lines),
                oid_anchor: None,
                mode_change: false,
            },
            HunkIdentity::WholeFile { oids } => RecordAnchors {
                anchor: Vec::new(),
                oid_anchor: Some(oids.clone()),
                mode_change: false,
            },
            HunkIdentity::ModeChange => RecordAnchors {
                anchor: Vec::new(),
                oid_anchor: None,
                mode_change: true,
            },
        }
    }

    /// The old-side unified-diff coordinate spelling, e.g. `"-63,1"`.
    /// Callers compose the surrounding `@@` framing themselves — the TUI
    /// frames it three different ways (a log echo, the diff panel's
    /// unified header, the assign popup) that must not converge.
    pub fn old_coords(&self) -> String {
        format!("-{},{}", self.old_start, self.old_lines)
    }

    /// The new-side unified-diff coordinate spelling, e.g. `"+63,2"`. See
    /// [`Hunk::old_coords`].
    pub fn new_coords(&self) -> String {
        format!("+{},{}", self.new_start, self.new_lines)
    }
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

impl HunkStage {
    /// This state's token from the staging set (ADR 0006: one spelling,
    /// sunk into core). The TUI renders hunks through replaceable theme
    /// fields seeded from the same set, so the two levels cannot
    /// disagree about `○` and `●`.
    pub fn glyph(&self) -> char {
        match self {
            HunkStage::Unstaged => vocabulary::UNSTAGED,
            HunkStage::Staged => vocabulary::STAGED,
            HunkStage::StagedStale => vocabulary::STAGED_STALE,
        }
    }
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

impl FileStage {
    /// This state's token from the staging set, as [`HunkStage::glyph`].
    /// The CLI's file list prints it directly.
    pub fn glyph(&self) -> char {
        match self {
            FileStage::Unstaged => vocabulary::UNSTAGED,
            FileStage::PartiallyStaged => vocabulary::PARTIALLY_STAGED,
            FileStage::Staged => vocabulary::STAGED,
        }
    }
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

fn merge_file(mut worktree: Option<FileDiff>, mut index: Option<FileDiff>) -> ChangedFile {
    let kind = merge_kind(
        worktree.as_ref().map(|file| file.kind),
        index.as_ref().map(|file| file.kind),
    );
    let (path, binary) = {
        let either = worktree.as_ref().or(index.as_ref()).expect("one diff side");
        (either.path.clone(), either.binary)
    };
    // Quarantine (ADR 0007): the index side of an unmerged path reports
    // `Conflicted` with no hunks, but the worktree side still diffs as
    // `Modified` — conflict markers as content. Merged `Conflicted`
    // drops every hunk so nothing matches or stages conflict text —
    // checked before the whole-file branch, so a conflicted file is
    // quarantined, never a whole-file hunk.
    let (hunks, degenerate) = if kind == ChangeKind::Conflicted {
        (Vec::new(), false)
    } else {
        // A binary carries no text hunks by construction (ADR 0009), and
        // a zero-hunk change — mode-only, empty file added or deleted —
        // has none to pair (ADR 0017). Both are the same predicament, so
        // asking what the pairing produced covers them together: nothing
        // paired means the change has no line content to address, and it
        // presents one degenerate hunk instead — the mode hunk when the
        // mode bits are the whole change, the whole-file hunk otherwise.
        //
        // Taking the hunks leaves both diffs hunk-less from here on;
        // everything below reads their sides only, which is all a
        // degenerate hunk is made of.
        let taken = |file: &mut Option<FileDiff>| {
            file.as_mut()
                .map(|file| std::mem::take(&mut file.hunks))
                .unwrap_or_default()
        };
        let paired = pair_hunks(taken(&mut worktree), taken(&mut index));
        if !paired.is_empty() {
            (paired, false)
        } else if is_mode_only(worktree.as_ref(), index.as_ref()) {
            (vec![mode_hunk(worktree.as_ref(), index.as_ref())], true)
        } else {
            (
                vec![whole_file_hunk(worktree.as_ref(), index.as_ref())],
                true,
            )
        }
    };
    // Worktree view preferred, like text hunk coordinates; carried only
    // for a file that presents a degenerate hunk, because index diffs
    // carry sides for text files too and the staged blob is not what a
    // text file's changed side shows.
    let sides = if degenerate {
        worktree
            .as_ref()
            .and_then(|file| file.sides.clone())
            .or_else(|| index.as_ref().and_then(|file| file.sides.clone()))
    } else {
        None
    };
    ChangedFile {
        path,
        kind,
        binary,
        hunks,
        sides,
    }
}

/// Whether this change is a mode-only change — a chmod and nothing else:
/// every diff side that sees the path carries a permission-bit delta over
/// identical content. Such a change's single hunk *is* its mode hunk
/// (ADR 0017); the whole-file treatment it once received collapses into
/// the general rule.
///
/// Both halves of the test earn their keep. Identical content rules out a
/// binary rewrite, which keeps its whole-file hunk (a mode delta *beside*
/// content is issue #103's), and an empty file's add or delete, which has
/// no second side to compare. The permission-bit half rules out a
/// file↔symlink swap: git reports that with mode bits alone too, but
/// calling it a mode change would misname it, and its presentation stays
/// with issue #100.
fn is_mode_only(worktree: Option<&FileDiff>, index: Option<&FileDiff>) -> bool {
    let mode_only = |file: &&FileDiff| {
        file.sides.as_ref().is_some_and(|sides| {
            sides.same_blob() && matches!(sides.mode_delta(), Some(ModeDelta::Mode { .. }))
        })
    };
    [worktree, index].iter().flatten().all(mode_only)
}

/// The `○●◑` a degenerate hunk derives, plus whether it is index-only,
/// from which diffs see the path — the three side-presence arms, which
/// every degenerate flavour reads the same way: a diff side that doesn't
/// see the path proves the index holds HEAD's version there. Only the
/// both-sides arm differs between flavours, so each passes its own
/// `index_holds_worktree` test (blobs *and* modes for a whole-file hunk,
/// modes alone for a mode hunk, whose two sides share one blob).
fn degenerate_stage(
    worktree: Option<&FileDiff>,
    index: Option<&FileDiff>,
    index_holds_worktree: impl FnOnce(&FileDiff, &FileDiff) -> bool,
) -> (HunkStage, bool) {
    match (worktree, index) {
        // The index diff doesn't see the path: nothing staged.
        (Some(_), None) => (HunkStage::Unstaged, false),
        (Some(worktree), Some(index)) => {
            if index_holds_worktree(worktree, index) {
                (HunkStage::Staged, false)
            } else {
                // The index holds an overlapping-but-different version:
                // the edited `◑` flavour.
                (HunkStage::StagedStale, false)
            }
        }
        // Index-only: staged then worktree-reverted, the other `◑`
        // flavour.
        (None, Some(_)) => (HunkStage::StagedStale, true),
        (None, None) => unreachable!("a merged file has at least one side"),
    }
}

/// The stand-alone hunk a mode delta presents (ADR 0017). Zeroed
/// coordinates and no lines, like a whole-file hunk, with staging derived
/// by comparing the two diffs' changed-side *modes* — the only evidence a
/// mode-only change has, since its two sides share one blob.
fn mode_hunk(worktree: Option<&FileDiff>, index: Option<&FileDiff>) -> Hunk {
    let (stage, index_only) = degenerate_stage(worktree, index, |worktree, index| {
        !changed_modes_differ(worktree.sides.as_ref(), index.sides.as_ref())
    });
    Hunk {
        old_start: 0,
        old_lines: 0,
        new_start: 0,
        new_lines: 0,
        stage,
        index_only,
        identity: HunkIdentity::ModeChange,
        changelist: None,
    }
}

/// The degenerate single hunk a change with no line-addressable content
/// and no mode delta of its own presents — a changed binary file
/// (ADR 0009), an empty file added or deleted, or a type change
/// (ADR 0017). Zeroed coordinates, no lines, staging derived by comparing
/// the two diffs' changed sides instead of lines.
fn whole_file_hunk(worktree: Option<&FileDiff>, index: Option<&FileDiff>) -> Hunk {
    // `●` needs the index to hold the worktree's content *and* mode. Mode
    // participates because a change can be staged at one mode and sit in
    // the worktree at another — a mode-only change is a mode hunk of its
    // own now, but a binary rewrite staged before a chmod still lands
    // here, and the OIDs alone would read that index as `●`.
    let (stage, index_only) = degenerate_stage(worktree, index, |worktree, index| {
        let (worktree, index) = (worktree.sides.as_ref(), index.sides.as_ref());
        let same_blob =
            worktree.and_then(FileSides::changed_oid) == index.and_then(FileSides::changed_oid);
        same_blob && !changed_modes_differ(worktree, index)
    });
    // The anchor's changed side is what the universe presents: worktree
    // content when the worktree diff sees the path, the staged blob for
    // index-only files — mirroring text hunks' coordinate convention.
    let anchored = if index_only { index } else { worktree };
    Hunk {
        old_start: 0,
        old_lines: 0,
        new_start: 0,
        new_lines: 0,
        stage,
        index_only,
        identity: HunkIdentity::WholeFile {
            oids: anchored
                .and_then(|file| file.sides.as_ref())
                .map(FileSides::oid_anchor)
                .unwrap_or(OidAnchor {
                    head: None,
                    changed: None,
                }),
        },
        changelist: None,
    }
}

/// Whether the two diffs' changed sides carry different filemodes — the
/// worktree's against the index's. An unknown mode on either side (git
/// reported none) proves nothing, so it reads as "same": the conservative
/// direction, which never invents a `◑`.
fn changed_modes_differ(worktree: Option<&FileSides>, index: Option<&FileSides>) -> bool {
    matches!(
        (
            worktree.and_then(FileSides::changed_mode),
            index.and_then(FileSides::changed_mode),
        ),
        (Some(worktree), Some(index)) if worktree != index
    )
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
        stage,
        index_only,
        identity: HunkIdentity::Text { lines: hunk.lines },
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
    ranges_overlap((a.old_start, a.old_lines), (b.old_start, b.old_lines))
}

/// Overlap on (start, lines) ranges, widening empty ranges (pure
/// insertions/removals) to one line — the rule the hunk universe pairs
/// hunks with, and commit/backend matching reuse.
pub(crate) fn ranges_overlap(a: (u32, u32), b: (u32, u32)) -> bool {
    let span = |(start, lines): (u32, u32)| (start, start + lines.max(1));
    let (a_start, a_end) = span(a);
    let (b_start, b_end) = span(b);
    a_start < b_end && b_start < a_end
}
