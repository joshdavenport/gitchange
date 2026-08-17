//! The ADR 0001 matcher: membership re-derived on every refresh as a
//! pure function of (stored records, hunk universe, active changelist,
//! now). Tier 1 matches exact content anchors — position-independent, so
//! moved hunks and dormant revivals are caught. Tier 2 inherits by
//! overlap on HEAD-side old ranges: both fresh diffs and the stored
//! records address the same HEAD, so comparing old coordinates directly
//! is ADR 0001's "shift by preceding matched hunks' deltas" carried out
//! exactly — old_start *is* new_start minus the preceding deltas.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::diff::ChangeKind;
use crate::state::{MembershipRecord, RecordIdentity};
use crate::universe::{ChangedFile, Hunk, HunkIdentity, ranges_overlap};
use crate::vocabulary::{ARROW, count_noun};

/// Dormant records prune after 14 days (ADR 0002).
const DORMANT_TTL_SECS: u64 = 14 * 24 * 60 * 60;

/// An automatic membership decision worth spot-checking, surfaced as
/// data on the snapshot (rendered by the CLI as stderr lines, by the TUI
/// Log panel later).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Advisory {
    /// A new hunk overlapped records from two or more changelists; it was
    /// captured by the active changelist (IntelliJ's rule, ADR 0001).
    AmbiguousOverlap {
        path: String,
        /// Worktree-side line the hunk starts at.
        new_start: u32,
        /// The overlapped changelists, sorted.
        candidates: Vec<String>,
        /// Where the hunk went: the active changelist, or unassigned
        /// when unassigned is the active one (ADR 0015).
        assigned_to: Option<String>,
    },
    /// A stage/unstage/assign acted on a snapshot hunk that no longer
    /// exists in the live tree (ADR 0005's stale-action rule): fail-soft,
    /// nothing was applied for that hunk.
    StaleHunk {
        path: String,
        /// Line the requested hunk started at when the snapshot was
        /// taken — worktree-side, or index-side for index-only hunks.
        new_start: u32,
    },
    /// A genuinely new hunk was captured by the active changelist — the
    /// routine automatic membership decision (ADR 0001/0007). Severity
    /// is the presentation layer's to assign.
    AutoCaptured {
        path: String,
        /// Worktree-side line the hunk starts at.
        new_start: u32,
        /// The active changelist that captured it.
        changelist: String,
    },
    /// Dormant records revived by exact anchor match this refresh
    /// (ADR 0002) — "restored 3 hunks to api-refactor". One advisory per
    /// (path, changelist).
    DormantRevival {
        path: String,
        changelist: String,
        hunks: usize,
    },
    /// An external HEAD move stranded this path's record coordinates
    /// (ADR 0012): with tier-2 disabled its anchor-broken hunks captured
    /// to active and its stranded live records went dormant. Fired only when
    /// that actually cost something — a guarded capture happened while
    /// records went dormant; a move tier-1 rescues entirely stays quiet.
    HeadMoveDormancy {
        path: String,
        /// The changelists whose records went dormant this refresh,
        /// sorted — where the captured hunks may belong, for re-sorting
        /// by hand.
        changelists: Vec<String>,
    },
}

impl Advisory {
    /// The canonical one-line message for this advisory (ADR 0006: one
    /// phrasing, sunk into core, so frontends can't drift). Frontends add
    /// only channel dressing — the CLI a `notice:` prefix, the TUI its
    /// severity glyph; severity itself stays theirs to assign (ADR 0007).
    pub fn message(&self) -> String {
        match self {
            Advisory::AutoCaptured {
                path,
                new_start,
                changelist,
            } => {
                format!("auto-captured hunk at {path}:{new_start} {ARROW} '{changelist}'")
            }
            Advisory::AmbiguousOverlap {
                path,
                new_start,
                candidates,
                assigned_to,
            } => {
                let overlap = quoted_list(candidates);
                match assigned_to {
                    Some(name) => format!(
                        "auto-captured hunk at {path}:{new_start} {ARROW} '{name}' (ambiguous overlap: {overlap})"
                    ),
                    None => format!(
                        "hunk at {path}:{new_start} left unassigned (ambiguous overlap: {overlap})"
                    ),
                }
            }
            Advisory::DormantRevival {
                path,
                changelist,
                hunks,
            } => {
                format!(
                    "restored {} to '{changelist}' — {path}",
                    count_noun(*hunks, "hunk")
                )
            }
            Advisory::StaleHunk { path, new_start } => {
                format!(
                    "hunk at {path}:{new_start} changed since the last refresh; nothing applied"
                )
            }
            Advisory::HeadMoveDormancy { path, changelists } => {
                let list = quoted_list(changelists);
                format!(
                    "external HEAD move changed {path} — records in {list} went dormant; affected hunks captured to active"
                )
            }
        }
    }
}

/// `'a', 'b', 'c'` — changelist names as quoted prose.
fn quoted_list(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("'{name}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The paths a HEAD move changed between the stored baseline and the
/// current HEAD (ADR 0012) — computed by `refresh()`, entering the
/// matcher as an explicit input like `active` and `now`. Tier-2 overlap
/// inheritance is disabled for them: their records' HEAD-side coordinates
/// no longer address the tree fresh hunks diff against.
pub(crate) enum AffectedPaths {
    /// HEAD is at the baseline (or no baseline exists yet — the one
    /// silent adoption on pre-baseline state files).
    None,
    /// diff(baseline↔HEAD), paths only.
    Some(BTreeSet<String>),
    /// The baseline no longer resolves: nothing can prove any path
    /// unmoved.
    All,
}

impl AffectedPaths {
    fn contains(&self, path: &str) -> bool {
        match self {
            AffectedPaths::None => false,
            AffectedPaths::Some(paths) => paths.contains(path),
            AffectedPaths::All => true,
        }
    }
}

pub(crate) struct MatchOutcome {
    /// The full post-refresh record set to persist: updated live records
    /// in universe order, then surviving dormant records.
    pub records: Vec<MembershipRecord>,
    pub advisories: Vec<Advisory>,
}

/// Match stored records against the fresh hunk universe, writing each
/// hunk's owner onto `files` and returning the records to persist.
pub(crate) fn run(
    files: &mut [ChangedFile],
    records: Vec<MembershipRecord>,
    active: Option<&str>,
    now_epoch_secs: u64,
    affected: &AffectedPaths,
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
        advisories: Vec::new(),
    };

    for file in files.iter_mut() {
        let stored = by_path.remove(&file.path).unwrap_or_default();
        // Quarantine (ADR 0007): an unmerged path's records freeze —
        // passed through verbatim, no matching, no dormancy clock.
        // Post-resolution content rarely matches pre-merge content, so
        // letting them go dormant would strand the user's sorting; they
        // re-enter normal matching when the file leaves the unmerged
        // state.
        if file.kind == ChangeKind::Conflicted {
            out.records.extend(stored);
            continue;
        }
        let tier2_disabled = affected.contains(&file.path);
        match_file(
            file,
            stored,
            active,
            now_epoch_secs,
            tier2_disabled,
            &mut out,
        );
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
    tier2_disabled: bool,
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
    // Per-hunk decision, spelled out rather than hidden behind an alias
    // because both levels of `None` carry meaning: the outer says no tier
    // has decided this hunk yet, the inner says a tier decided it stays
    // unassigned — recordless (ADR 0016), under capture-off. Only the
    // outer is a real absence.
    let mut owners: Vec<Option<Option<String>>> = vec![None; file.hunks.len()];

    // Tier 1: exact content-anchor match — live records first, then
    // dormant (revival). Position-independent: catches moved hunks.
    // Revivals are automatic membership decisions (ADR 0007): counted
    // per reviving changelist and advised below.
    let mut revived: BTreeMap<String, usize> = BTreeMap::new();
    for pass_dormant in [false, true] {
        for (i, hunk) in file.hunks.iter().enumerate() {
            if owners[i].is_some() {
                continue;
            }
            let matched = stored.iter().enumerate().find(|(j, record)| {
                !consumed[*j]
                    && record.is_dormant() == pass_dormant
                    && exact_anchor_match(record, hunk, &anchors[i])
            });
            if let Some((j, record)) = matched {
                let owner = record.changelist.clone();
                consumed[j] = true;
                owners[i] = Some(Some(owner.clone()));
                if pass_dormant {
                    *revived.entry(owner.clone()).or_default() += 1;
                }
                fresh[i] = Some(record_for(&file.path, hunk, &anchors[i], owner));
            }
        }
    }
    for (changelist, hunks) in revived {
        out.advisories.push(Advisory::DormantRevival {
            path: file.path.clone(),
            changelist,
            hunks,
        });
    }

    // Whether any hunk fell through tier 1 while the guard was on — a
    // capture under the disabled tier (ADR 0012).
    let mut guarded_capture = false;

    // Tier 2: overlap inheritance on HEAD-side ranges, live records only
    // — dormant records never re-claim edited content (ADR 0002).
    for (i, hunk) in file.hunks.iter().enumerate() {
        if owners[i].is_some() {
            continue;
        }
        if tier2_disabled {
            // The ADR 0012 guard: this path's record coordinates address
            // the old baseline, not the HEAD these hunks diff against, so
            // overlap proves nothing. Capture to active; the untouched
            // stranded records go dormant below.
            guarded_capture = true;
            let owner = active.map(String::from);
            owners[i] = Some(owner.clone());
            if let Some(name) = owner {
                // A capture is a capture (ADR 0007): guarded ones get the
                // same advisory as routine ones, on top of the per-path
                // dormancy advisory when the guard cost something.
                out.advisories.push(Advisory::AutoCaptured {
                    path: file.path.clone(),
                    new_start: hunk.new_start,
                    changelist: name.clone(),
                });
                fresh[i] = Some(record_for(&file.path, hunk, &anchors[i], name));
            }
            continue;
        }
        // Consumption is deferred to `superseded` so one record can
        // serve every fragment of a split.
        let overlapping: Vec<usize> = stored
            .iter()
            .enumerate()
            .filter(|(j, record)| {
                !consumed[*j] && !record.is_dormant() && overlap_claim(record, hunk)
            })
            .map(|(j, _)| j)
            .collect();
        for &j in &overlapping {
            superseded[j] = true;
        }

        let candidates: BTreeSet<&str> = overlapping
            .iter()
            .map(|&j| stored[j].changelist.as_str())
            .collect();

        let owner = if overlapping.is_empty() {
            // Genuinely new: the active changelist, or unassigned when
            // unassigned is active (ADR 0015's capture-off). No record
            // for a hunk nobody claims — a changelist-less repo must not
            // grow a state file, and capture-off makes no claim it would
            // have to un-make later. The capture is advised (ADR 0007):
            // automatic membership decisions are visible, never silent;
            // falling through to unassigned decides nothing, so it is
            // quiet.
            if let Some(name) = active {
                out.advisories.push(Advisory::AutoCaptured {
                    path: file.path.clone(),
                    new_start: hunk.new_start,
                    changelist: name.into(),
                });
            }
            active.map(String::from)
        } else if candidates.len() >= 2 {
            // Two or more changelists could claim it: active + advisory —
            // visible, never a silent misfile.
            out.advisories.push(Advisory::AmbiguousOverlap {
                path: file.path.clone(),
                new_start: hunk.new_start,
                candidates: candidates.iter().map(|&name| name.into()).collect(),
                assigned_to: active.map(String::from),
            });
            active.map(String::from)
        } else {
            // Exactly one changelist: inherit. Editing your own hunk
            // never sheds membership; splits inherit the parent's owner
            // because each fragment overlaps the parent record.
            candidates.iter().next().map(|&name| name.into())
        };

        owners[i] = Some(owner.clone());
        // A record exactly when a changelist owns the hunk (ADR 0016):
        // a decision for unassigned — capture-off, or an ambiguous
        // overlap resolved there — writes nothing, and the superseded
        // claims above are simply dropped.
        if let Some(name) = owner {
            fresh[i] = Some(record_for(&file.path, hunk, &anchors[i], name));
        }
    }

    for (hunk, owner) in file.hunks.iter_mut().zip(&owners) {
        hunk.changelist = owner.clone().flatten();
    }
    out.records.extend(fresh.into_iter().flatten());

    // One advisory per path where the guard actually changed an outcome: a
    // hunk captured under the disabled tier while records went dormant.
    // Dormancy alone (a committed hunk's record) or a capture alone (a
    // genuinely new hunk) reads the same with the guard off — quiet.
    if guarded_capture {
        let newly_dormant: BTreeSet<&String> = stored
            .iter()
            .enumerate()
            .filter(|(j, record)| !consumed[*j] && !superseded[*j] && !record.is_dormant())
            .map(|(_, record)| &record.changelist)
            .collect();
        if !newly_dormant.is_empty() {
            out.advisories.push(Advisory::HeadMoveDormancy {
                path: file.path.clone(),
                changelists: newly_dormant.into_iter().cloned().collect(),
            });
        }
    }

    // Everything neither matched nor superseded vanished from this
    // path's diff: dormant, retained for exact-match revival.
    for (j, record) in stored.into_iter().enumerate() {
        if !consumed[j] && !superseded[j] {
            out.records.extend(into_dormant(record, now_epoch_secs));
        }
    }
}

pub(crate) fn record_for(
    path: &str,
    hunk: &Hunk,
    anchor: &[String],
    owner: String,
) -> MembershipRecord {
    let anchors = hunk.record_anchors();
    MembershipRecord {
        path: path.into(),
        old_start: hunk.old_start,
        old_lines: hunk.old_lines,
        new_start: hunk.new_start,
        new_lines: hunk.new_lines,
        changelist: owner,
        anchor: anchor.to_vec(),
        oid_anchor: anchors.oid_anchor,
        mode_change: anchors.mode_change,
        dormant_since: None,
    }
}

/// Tier-1 identity: verbatim-anchor equality for text hunks; for a
/// whole-file hunk, changed-side OID equality (ADR 0009) — the HEAD side
/// deliberately doesn't participate, so a HEAD move alone never sheds an
/// untouched binary change; for a mode hunk, path continuity alone
/// (ADR 0017), which is all its identity is, so a dormant mode record
/// revives on any re-flip at its path.
///
/// The cross-flavour arm is the load-bearing one: every degenerate
/// record carries an empty verbatim anchor, so without it a text hunk
/// and a binary or mode record at the same path would agree on emptiness
/// and match. Stated once here and once in [`overlap_claim`]; any
/// further tier has to answer it too, because the match is exhaustive
/// over the pair.
pub(crate) fn exact_anchor_match(
    record: &MembershipRecord,
    hunk: &Hunk,
    anchor: &[String],
) -> bool {
    match (record.identity(), &hunk.identity) {
        (RecordIdentity::Text { anchor: stored }, HunkIdentity::Text { .. }) => stored == anchor,
        (RecordIdentity::WholeFile { oids: stored }, HunkIdentity::WholeFile { oids }) => {
            stored.changed == oids.changed
        }
        (RecordIdentity::ModeChange, HunkIdentity::ModeChange) => true,
        _ => false,
    }
}

/// Tier-2 claim: HEAD-side range overlap for text hunks; for a
/// whole-file hunk, path continuity (`CONTEXT.md`, "Whole-file hunk") —
/// the whole file *is* the hunk, so any whole-file record at the path
/// trivially "overlaps" it and a re-export (same path, new content)
/// keeps its membership; for a mode hunk, the same path continuity, the
/// strength its identity has at both tiers (ADR 0017). See
/// [`exact_anchor_match`] on the cross arm.
pub(crate) fn overlap_claim(record: &MembershipRecord, hunk: &Hunk) -> bool {
    match (record.identity(), &hunk.identity) {
        (RecordIdentity::Text { .. }, HunkIdentity::Text { .. }) => {
            old_ranges_overlap(hunk, record)
        }
        (RecordIdentity::WholeFile { .. }, HunkIdentity::WholeFile { .. }) => true,
        (RecordIdentity::ModeChange, HunkIdentity::ModeChange) => true,
        _ => false,
    }
}

/// The content anchor of a fresh hunk: its verbatim lines, origin
/// included, exactly the shape records store — empty for a whole-file
/// hunk, whose identity rides in `oid_anchor` instead. Also how
/// `commit()` keys records to the payload hunks it consumes (ADR 0004).
pub(crate) fn anchor_of(hunk: &Hunk) -> Vec<String> {
    hunk.record_anchors().anchor
}

/// Verbatim diff lines in the anchor shape records store.
pub(crate) fn anchor_lines(lines: &[crate::diff::HunkLine]) -> Vec<String> {
    lines
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
pub(crate) fn old_ranges_overlap(hunk: &Hunk, record: &MembershipRecord) -> bool {
    ranges_overlap(
        (hunk.old_start, hunk.old_lines),
        (record.old_start, record.old_lines),
    )
}
