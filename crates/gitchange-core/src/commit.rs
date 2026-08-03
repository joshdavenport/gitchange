//! Commit-payload derivation and the record aftermath (ADR 0004,
//! ADR 0012). The payload is the changelist's staged hunks *as index
//! content*: the diff(HEAD↔index) hunks overlapping the changelist's
//! owned non-unstaged universe hunks. The aftermath — run under one
//! locked state update after the commit lands — removes fully-consumed
//! records, rewrites retained `◑` records against the new HEAD, and
//! shifts every other same-file record by the committed deltas.

use crate::backend::HunkHeader;
use crate::diff::{DiffHunk, FileDiff};
use crate::matcher::anchor_lines;
use crate::state::{MembershipRecord, OidAnchor, State};
use crate::universe::{ChangedFile, Hunk, HunkStage, ranges_overlap};
use crate::vocabulary::{UNASSIGNED, count_noun};

/// Flags for [`crate::Repo::commit`], both forwarded to the underlying
/// `git commit` (ADR 0004).
#[derive(Debug, Clone, Default)]
pub struct CommitOptions {
    pub no_verify: bool,
    pub amend: bool,
}

/// The git flag [`CommitOptions::no_verify`] drives — the commit
/// dialog's checkbox label and the shelled-out command echo name it
/// identically (ADR 0006).
pub const NO_VERIFY_FLAG: &str = "--no-verify";

/// The git flag [`CommitOptions::amend`] drives. See [`NO_VERIFY_FLAG`].
pub const AMEND_FLAG: &str = "--amend";

/// The transparency echo for one commit invocation (ADR 0007): the
/// shelled-out command's flags plus the temp-index context — one
/// phrasing, sunk into core so frontends can't drift (ADR 0006). The
/// caller logs it when git actually ran: on
/// [`CommitOutcome::Committed`], and on
/// [`crate::Error::HookRejected`] (git executed and refused); drift and
/// guard failures mean it never did.
pub fn commit_echo(
    options: &CommitOptions,
    changelist: Option<&str>,
    payload: &CommitPayload,
) -> String {
    let mut echo = String::from("git commit");
    if options.no_verify {
        echo.push(' ');
        echo.push_str(NO_VERIFY_FLAG);
    }
    if options.amend {
        echo.push(' ');
        echo.push_str(AMEND_FLAG);
    }
    let hunks = payload.staged_hunks() + payload.stale_hunks();
    echo.push_str(&format!(
        " (temp index — '{}', {})",
        changelist.unwrap_or(UNASSIGNED),
        count_noun(hunks, "hunk"),
    ));
    echo
}

/// What [`crate::Repo::commit`] produced.
#[derive(Debug)]
pub enum CommitOutcome {
    Committed {
        /// The new HEAD commit id.
        oid: String,
        /// The same commit abbreviated as git would abbreviate it —
        /// `core.abbrev` honoured, expanded for uniqueness. What the
        /// frontends echo, so a post-commit line and the Commits panel
        /// can't name one commit two different ways.
        short_id: String,
    },
    /// The refresh-before-commit found the payload changed since it was
    /// confirmed (ADR 0004's freshness guard): nothing was committed;
    /// the fresh payload is returned for re-confirmation.
    Drifted { payload: CommitPayload },
}

/// The commit payload as the confirm flow inspects it: per-file staged
/// and staged-stale counts plus the index-content hunks that would be
/// committed. Equality is the drift test — index content and staleness
/// both participate, so a re-staged hunk *and* a worktree edit flipping
/// `●`→`◑` each invalidate a confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitPayload {
    pub files: Vec<PayloadFile>,
}

impl CommitPayload {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Cleanly staged (`●`) hunks across the payload.
    pub fn staged_hunks(&self) -> usize {
        self.files.iter().map(|file| file.staged_hunks).sum()
    }

    /// Staged-stale (`◑`) hunks across the payload, both flavours — the
    /// warn-and-confirm count (ADR 0004).
    pub fn stale_hunks(&self) -> usize {
        self.files.iter().map(|file| file.stale_hunks).sum()
    }
}

/// One file's share of the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadFile {
    pub path: String,
    /// The changelist's `●` hunks here.
    pub staged_hunks: usize,
    /// The changelist's `◑` hunks here (edited and index-only flavours).
    pub stale_hunks: usize,
    /// The diff(HEAD↔index) hunks this commit carries, in file order.
    /// Empty for a binary file, which commits via `whole_file` instead.
    pub hunks: Vec<PayloadHunk>,
    /// A binary file's whole-file payload (ADR 0009); `None` for text
    /// files. Participates in equality, so a re-staged binary blob
    /// drifts a confirmation like re-staged text hunks do.
    pub whole_file: Option<WholeFilePayload>,
}

/// What a binary file commits (ADR 0009): the staged blob, whole-file —
/// the temp index receives the live index entry verbatim. `None`
/// commits the file's staged deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WholeFilePayload {
    pub staged_oid: Option<String>,
}

/// One committed index-content hunk, coordinates and verbatim lines —
/// the identity drift comparison runs on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// Verbatim lines (`origin` + content), anchor-shaped.
    pub lines: Vec<String>,
}

/// Everything `commit()` needs: the inspectable payload plus the
/// per-path aftermath bookkeeping.
pub(crate) struct CommitPlan {
    pub payload: CommitPayload,
    pub paths: Vec<PathPlan>,
}

pub(crate) struct PathPlan {
    pub path: String,
    /// Headers of the committed diff(HEAD↔index) hunks; empty for a
    /// binary file, which commits via `whole_file` instead.
    pub committed: Vec<HunkHeader>,
    /// A binary file's whole-file selection (ADR 0009).
    pub whole_file: Option<WholeFilePayload>,
    /// Records of fully-consumed hunks (`●`: index == worktree at commit
    /// time) — removed explicitly (ADR 0004).
    pub consumed: Vec<RecordKey>,
    /// Records of retained `◑` hunks — rewritten against the new HEAD.
    pub retained: Vec<Retained>,
}

/// A record's identity as the pre-commit refresh persisted it: fresh
/// live records mirror universe hunks exactly, so coordinates plus
/// anchor pin one record.
pub(crate) struct RecordKey {
    old_start: u32,
    old_lines: u32,
    anchor: Vec<String>,
    oid_anchor: Option<OidAnchor>,
}

impl RecordKey {
    fn of(hunk: &Hunk) -> Self {
        Self {
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            anchor: crate::matcher::anchor_of(hunk),
            oid_anchor: hunk.oid_anchor.clone(),
        }
    }

    fn matches(&self, record: &MembershipRecord) -> bool {
        !record.is_dormant()
            && record.old_start == self.old_start
            && record.old_lines == self.old_lines
            && record.anchor == self.anchor
            && record.oid_anchor == self.oid_anchor
    }
}

pub(crate) struct Retained {
    pub key: RecordKey,
    /// Where the committed content lands in the new HEAD (old-side
    /// range): the committed hunks' index ranges commuted past the
    /// payload's own preceding deltas — the residual hunk to rewrite
    /// from is found by overlap with this.
    pub region: (u32, u32),
}

/// Derive the payload and aftermath plan for one changelist (`None` is
/// unassigned) from a refresh's universe and its diff(HEAD↔index).
pub(crate) fn plan(
    files: &[ChangedFile],
    index: &[FileDiff],
    changelist: Option<&str>,
) -> CommitPlan {
    let mut payload_files = Vec::new();
    let mut paths = Vec::new();

    for file in files {
        let owned: Vec<&Hunk> = file
            .hunks
            .iter()
            .filter(|hunk| {
                hunk.changelist.as_deref() == changelist && hunk.stage != HunkStage::Unstaged
            })
            .collect();
        if owned.is_empty() {
            continue;
        }
        let index_file = index.iter().find(|candidate| candidate.path == file.path);
        // A binary file commits whole (ADR 0009): the staged blob copied
        // into the temp index verbatim — no hunk selection to run. An
        // owned non-unstaged hunk implies the index differs from HEAD
        // here, so the index diff sees the path and carries sides (it
        // does for text content too — the mixed binary-worktree-over-
        // staged-text case commits its staged text whole-file); the
        // `else` is defensive only.
        if file.binary {
            let Some(sides) = index_file.and_then(|file| file.binary_sides.as_ref()) else {
                continue;
            };
            let whole_file = WholeFilePayload {
                staged_oid: sides.changed_oid(),
            };
            let hunk = owned[0];
            let mut consumed = Vec::new();
            let mut retained = Vec::new();
            if hunk.stage == HunkStage::Staged {
                consumed.push(RecordKey::of(hunk));
            } else {
                retained.push(Retained {
                    key: RecordKey::of(hunk),
                    // Degenerate whole-file coordinates: the aftermath
                    // rewrites binary records from the residual diff's
                    // OIDs, not from this region.
                    region: (0, 0),
                });
            }
            payload_files.push(PayloadFile {
                path: file.path.clone(),
                staged_hunks: usize::from(hunk.stage == HunkStage::Staged),
                stale_hunks: usize::from(hunk.stage == HunkStage::StagedStale),
                hunks: Vec::new(),
                whole_file: Some(whole_file.clone()),
            });
            paths.push(PathPlan {
                path: file.path.clone(),
                committed: Vec::new(),
                whole_file: Some(whole_file),
                consumed,
                retained,
            });
            continue;
        }
        let index_hunks: &[DiffHunk] = index_file
            .map(|candidate| candidate.hunks.as_slice())
            .unwrap_or(&[]);
        // The committable atoms are index hunks; selection mirrors the
        // universe's pairing (overlap on HEAD-side ranges). An index hunk
        // straddling two changelists' hunks commits whole — the payload
        // inspection shows exactly what goes.
        let committed: Vec<&DiffHunk> = index_hunks
            .iter()
            .filter(|candidate| {
                owned.iter().any(|hunk| {
                    ranges_overlap(
                        (hunk.old_start, hunk.old_lines),
                        (candidate.old_start, candidate.old_lines),
                    )
                })
            })
            .collect();
        if committed.is_empty() {
            continue;
        }
        let headers: Vec<HunkHeader> = committed.iter().map(|hunk| header(hunk)).collect();

        let mut consumed = Vec::new();
        let mut retained = Vec::new();
        for hunk in &owned {
            if hunk.stage == HunkStage::Staged {
                consumed.push(RecordKey::of(hunk));
            } else {
                retained.push(Retained {
                    key: RecordKey::of(hunk),
                    region: committed_region(hunk, &committed, &headers),
                });
            }
        }

        payload_files.push(PayloadFile {
            path: file.path.clone(),
            staged_hunks: owned
                .iter()
                .filter(|hunk| hunk.stage == HunkStage::Staged)
                .count(),
            stale_hunks: owned
                .iter()
                .filter(|hunk| hunk.stage == HunkStage::StagedStale)
                .count(),
            hunks: committed
                .iter()
                .map(|hunk| PayloadHunk {
                    old_start: hunk.old_start,
                    old_lines: hunk.old_lines,
                    new_start: hunk.new_start,
                    new_lines: hunk.new_lines,
                    lines: anchor_lines(&hunk.lines),
                })
                .collect(),
            whole_file: None,
        });
        paths.push(PathPlan {
            path: file.path.clone(),
            committed: headers,
            whole_file: None,
            consumed,
            retained,
        });
    }

    CommitPlan {
        payload: CommitPayload {
            files: payload_files,
        },
        paths,
    }
}

/// The record aftermath (ADR 0012), run under the commit's locked state
/// update: consumed records removed, retained `◑` records rewritten from
/// the actual post-commit diff(new HEAD↔worktree) — coordinates *and*
/// anchor, exactly what the next refresh will see — and every other
/// same-file record's HEAD-side coordinates commuted past the committed
/// deltas. Worktree-side coordinates never change: the commit never
/// touches the worktree.
pub(crate) fn apply_aftermath(state: &mut State, plans: &[PathPlan], residual: &[FileDiff]) {
    for plan in plans {
        let residual_hunks: &[DiffHunk] = residual
            .iter()
            .find(|file| file.path == plan.path)
            .map(|file| file.hunks.as_slice())
            .unwrap_or(&[]);
        let mut residual_used = vec![false; residual_hunks.len()];

        state.records.retain(|record| {
            !(record.path == plan.path && plan.consumed.iter().any(|key| key.matches(record)))
        });

        for record in &mut state.records {
            if record.path != plan.path {
                continue;
            }
            if let Some(retained) = plan
                .retained
                .iter()
                .find(|retained| retained.key.matches(record))
            {
                // A retained binary `◑` record rewrites from the residual
                // diff's OIDs (ADR 0009): the whole file is the hunk, so
                // there is no region to overlap — coordinates stay
                // degenerate. No residual (the worktree moved on) leaves
                // the record for the next refresh to resolve — dormancy
                // at worst, visible per ADR 0002.
                if record.oid_anchor.is_some() {
                    let sides = residual
                        .iter()
                        .find(|file| file.path == plan.path)
                        .and_then(|file| file.binary_sides.as_ref());
                    if let Some(sides) = sides {
                        record.oid_anchor = Some(sides.oid_anchor());
                    }
                    continue;
                }
                let found = residual_hunks.iter().enumerate().find(|(i, hunk)| {
                    !residual_used[*i]
                        && ranges_overlap((hunk.old_start, hunk.old_lines), retained.region)
                });
                if let Some((i, hunk)) = found {
                    residual_used[i] = true;
                    record.old_start = hunk.old_start;
                    record.old_lines = hunk.old_lines;
                    record.new_start = hunk.new_start;
                    record.new_lines = hunk.new_lines;
                    record.anchor = anchor_lines(&hunk.lines);
                } else {
                    // No residual found (the worktree moved on between
                    // commit and now): point the record at the committed
                    // region so the next refresh resolves it — dormancy
                    // at worst, visible per ADR 0002.
                    record.old_start = retained.region.0;
                    record.old_lines = retained.region.1;
                }
            } else {
                // Commutation shift: dormant records too, so a later
                // revival leaves coordinates addressing the same HEAD as
                // every live record.
                record.old_start = shifted(record.old_start, &plan.committed);
            }
        }
    }
}

/// Where a retained `◑` hunk's committed content lands in the new HEAD:
/// the overlapping committed hunks' index-side extents, commuted past
/// the committed hunks above them.
fn committed_region(hunk: &Hunk, committed: &[&DiffHunk], headers: &[HunkHeader]) -> (u32, u32) {
    let mut start = u32::MAX;
    let mut end = 0u32;
    for candidate in committed {
        if !ranges_overlap(
            (hunk.old_start, hunk.old_lines),
            (candidate.old_start, candidate.old_lines),
        ) {
            continue;
        }
        let commuted = shifted(candidate.old_start, headers);
        start = start.min(commuted);
        end = end.max(commuted + candidate.new_lines);
    }
    if start == u32::MAX {
        (hunk.old_start, hunk.old_lines)
    } else {
        (start, end - start)
    }
}

/// A HEAD-side line commuted past the committed hunks entirely above it.
fn shifted(old_start: u32, committed: &[HunkHeader]) -> u32 {
    let delta: i64 = committed
        .iter()
        .filter(|hunk| ends_above(hunk, old_start))
        .map(|&(_, old_lines, _, new_lines)| i64::from(new_lines) - i64::from(old_lines))
        .sum();
    u32::try_from(i64::from(old_start) + delta).unwrap_or(0)
}

/// Whether a committed hunk sits entirely above `old_start`. A pure
/// insertion (zero old lines) at line N inserts *after* N, so it is
/// above only lines strictly below it.
fn ends_above(&(start, lines, _, _): &HunkHeader, old_start: u32) -> bool {
    if lines == 0 {
        start < old_start
    } else {
        start + lines <= old_start
    }
}

fn header(hunk: &DiffHunk) -> HunkHeader {
    (
        hunk.old_start,
        hunk.old_lines,
        hunk.new_start,
        hunk.new_lines,
    )
}
