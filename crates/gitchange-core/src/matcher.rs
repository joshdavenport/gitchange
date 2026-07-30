//! The ADR 0001 matcher: membership re-derived on every refresh as a
//! pure function of (stored records, hunk universe, active changelist,
//! now). Tier 1 matches exact content anchors — position-independent, so
//! moved hunks and dormant revivals are caught. Tier 2 inherits by
//! overlap on HEAD-side old ranges: both fresh diffs and the stored
//! records address the same HEAD, so comparing old coordinates directly
//! is ADR 0001's "shift by preceding matched hunks' deltas" carried out
//! exactly — old_start *is* new_start minus the preceding deltas.

use std::collections::BTreeSet;
use std::collections::HashMap;

use crate::state::MembershipRecord;
use crate::universe::{ChangedFile, Hunk};

/// Dormant records prune after 14 days (ADR 0002).
const DORMANT_TTL_SECS: u64 = 14 * 24 * 60 * 60;

/// A hunk's owning changelist; `None` is unassigned.
type Owner = Option<String>;

/// An automatic membership decision worth spot-checking, surfaced as
/// data on the snapshot (rendered by the CLI as stderr lines, by the TUI
/// Log panel later).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// A new hunk overlapped records from two or more changelists; it was
    /// captured by the active changelist (IntelliJ's rule, ADR 0001).
    AmbiguousOverlap {
        path: String,
        /// Worktree-side line the hunk starts at.
        new_start: u32,
        /// The overlapped changelists, sorted.
        candidates: Vec<String>,
        /// Where the hunk went: the active changelist, or unassigned in
        /// the degenerate hand-edited-state case where none is active.
        assigned_to: Option<String>,
    },
    /// A stage/unstage acted on a snapshot hunk that no longer exists in
    /// the live tree (ADR 0005's stale-action rule): fail-soft, nothing
    /// was applied.
    StaleHunk {
        path: String,
        /// Line the requested hunk started at when the snapshot was
        /// taken — worktree-side, or index-side for index-only hunks.
        new_start: u32,
    },
}

pub(crate) struct MatchOutcome {
    /// The full post-refresh record set to persist: updated live records
    /// in universe order, then surviving dormant records.
    pub records: Vec<MembershipRecord>,
    pub notices: Vec<Notice>,
}

/// Match stored records against the fresh hunk universe, writing each
/// hunk's owner onto `files` and returning the records to persist.
pub(crate) fn run(
    files: &mut [ChangedFile],
    records: Vec<MembershipRecord>,
    active: Option<&str>,
    now_epoch_secs: u64,
) -> MatchOutcome {
    let mut by_path: HashMap<String, Vec<MembershipRecord>> = HashMap::new();
    let mut path_order: Vec<String> = Vec::new();
    for record in records {
        let slot = by_path.entry(record.path.clone()).or_default();
        if slot.is_empty() {
            path_order.push(record.path.clone());
        }
        slot.push(record);
    }

    let mut out = MatchOutcome {
        records: Vec::new(),
        notices: Vec::new(),
    };

    for file in files.iter_mut() {
        let stored = by_path.remove(&file.path).unwrap_or_default();
        match_file(file, stored, active, now_epoch_secs, &mut out);
    }

    // Paths that vanished from the universe entirely: every record goes
    // (or stays) dormant, pruned on expiry.
    for path in path_order {
        let Some(stored) = by_path.remove(&path) else {
            continue;
        };
        out.records.extend(
            stored
                .into_iter()
                .filter_map(|record| into_dormant(record, now_epoch_secs)),
        );
    }

    out
}

fn match_file(
    file: &mut ChangedFile,
    stored: Vec<MembershipRecord>,
    active: Option<&str>,
    now_epoch_secs: u64,
    out: &mut MatchOutcome,
) {
    let anchors: Vec<Vec<String>> = file.hunks.iter().map(anchor_of).collect();
    let mut consumed = vec![false; stored.len()];
    // Live records a tier-2 hunk evolved from: replaced by that hunk's
    // fresh record, so never carried forward as dormant.
    let mut superseded = vec![false; stored.len()];
    // Per-hunk replacement records, kept in hunk order so an unchanged
    // diff reproduces the stored order byte-for-byte (no-rewrite rule).
    let mut fresh: Vec<Option<MembershipRecord>> = vec![None; file.hunks.len()];
    // Per-hunk decision: outer None = undecided, inner None = unassigned.
    let mut owners: Vec<Option<Owner>> = vec![None; file.hunks.len()];

    // Tier 1: exact content-anchor match — live records first, then
    // dormant (revival). Position-independent: catches moved hunks.
    for pass_dormant in [false, true] {
        for (i, hunk) in file.hunks.iter().enumerate() {
            if owners[i].is_some() {
                continue;
            }
            let matched = stored.iter().enumerate().find(|(j, record)| {
                !consumed[*j] && record.is_dormant() == pass_dormant && record.anchor == anchors[i]
            });
            if let Some((j, record)) = matched {
                let owner = record.changelist.clone();
                consumed[j] = true;
                owners[i] = Some(owner.clone());
                fresh[i] = Some(record_for(&file.path, hunk, &anchors[i], owner));
            }
        }
    }

    // Tier 2: overlap inheritance on HEAD-side ranges, live records only
    // — dormant records never re-claim edited content (ADR 0002).
    for (i, hunk) in file.hunks.iter().enumerate() {
        if owners[i].is_some() {
            continue;
        }
        // Consumption is deferred to `superseded` so one record can
        // serve every fragment of a split.
        let overlapping: Vec<usize> = stored
            .iter()
            .enumerate()
            .filter(|(j, record)| {
                !consumed[*j] && !record.is_dormant() && old_ranges_overlap(hunk, record)
            })
            .map(|(j, _)| j)
            .collect();
        for &j in &overlapping {
            superseded[j] = true;
        }

        let candidates: BTreeSet<&str> = overlapping
            .iter()
            .filter_map(|&j| stored[j].changelist.as_deref())
            .collect();

        let owner = if overlapping.is_empty() {
            // Genuinely new: active changelist, or unassigned when no
            // changelists exist. No record for a hunk nobody claims —
            // a changelist-less repo must not grow a state file.
            active.map(String::from)
        } else if candidates.len() >= 2 {
            // Two or more changelists could claim it: active + notice —
            // visible, never a silent misfile.
            out.notices.push(Notice::AmbiguousOverlap {
                path: file.path.clone(),
                new_start: hunk.new_start,
                candidates: candidates.iter().map(|&name| name.into()).collect(),
                assigned_to: active.map(String::from),
            });
            active.map(String::from)
        } else {
            // One changelist (possibly alongside unassigned claims), or
            // only unassigned claims: inherit. Editing your own hunk
            // never sheds membership; splits inherit the parent's owner
            // because each fragment overlaps the parent record.
            candidates.iter().next().map(|&name| name.into())
        };

        owners[i] = Some(owner.clone());
        if !overlapping.is_empty() || owner.is_some() {
            fresh[i] = Some(record_for(&file.path, hunk, &anchors[i], owner));
        }
    }

    for (hunk, owner) in file.hunks.iter_mut().zip(&owners) {
        hunk.changelist = owner.clone().flatten();
    }
    out.records.extend(fresh.into_iter().flatten());

    // Everything neither matched nor superseded vanished from this
    // path's diff: dormant, retained for exact-match revival.
    for (j, record) in stored.into_iter().enumerate() {
        if !consumed[j] && !superseded[j] {
            out.records.extend(into_dormant(record, now_epoch_secs));
        }
    }
}

fn record_for(
    path: &str,
    hunk: &Hunk,
    anchor: &[String],
    owner: Option<String>,
) -> MembershipRecord {
    MembershipRecord {
        path: path.into(),
        old_start: hunk.old_start,
        old_lines: hunk.old_lines,
        new_start: hunk.new_start,
        new_lines: hunk.new_lines,
        changelist: owner,
        anchor: anchor.to_vec(),
        dormant_since: None,
    }
}

/// The content anchor of a fresh hunk: its verbatim lines, origin
/// included, exactly the shape records store.
fn anchor_of(hunk: &Hunk) -> Vec<String> {
    hunk.lines
        .iter()
        .map(|line| format!("{}{}", line.origin, line.content))
        .collect()
}

/// Mark a record dormant (keeping an existing dormancy timestamp), or
/// drop it once it has been dormant past the TTL.
fn into_dormant(mut record: MembershipRecord, now_epoch_secs: u64) -> Option<MembershipRecord> {
    let since = record.dormant_since.unwrap_or(now_epoch_secs);
    if now_epoch_secs.saturating_sub(since) >= DORMANT_TTL_SECS {
        return None;
    }
    record.dormant_since = Some(since);
    Some(record)
}

/// Overlap on HEAD-side ranges, widening empty ranges (pure insertions)
/// to one line so an edit to an inserted hunk still pairs with it.
fn old_ranges_overlap(hunk: &Hunk, record: &MembershipRecord) -> bool {
    let span = |start: u32, lines: u32| (start, start + lines.max(1));
    let (a_start, a_end) = span(hunk.old_start, hunk.old_lines);
    let (b_start, b_end) = span(record.old_start, record.old_lines);
    a_start < b_end && b_start < a_end
}
