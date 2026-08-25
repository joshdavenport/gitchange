//! Shared presentation vocabulary that isn't attached to one domain type
//! (ADR 0006: one phrasing, sunk into core, so the TUI and CLI can't
//! drift). Enum-attached vocabulary — `ChangeKind::sigil`,
//! `GitOperation::label`, `GroupKind::label` — stays with its enum. This
//! module holds the tokens and phrasing helpers that no single enum owns,
//! of two kinds: those with no enum to live on, and those *more than one*
//! enum's arms point back to. `UNASSIGNED`/`ALL` are the latter (with
//! `state`'s non-enum `RESERVED_NAMES`), and so is the staging set below,
//! which `HunkStage::glyph` and `FileStage::glyph` both project onto.
//!
//! ## The staging set
//!
//! Four tokens mark how much of a change is staged: `○ ● ◐ ◑`. They name
//! a staged-ness, not a level — `●` means the same thing on a hunk row
//! and a file row, which is why one set serves both and why the TUI can
//! theme `staged` once (ADR 0003: per-file markers *stay* `●◐○`).
//!
//! The level decides only which tokens are reachable. `PARTIALLY_STAGED`
//! needs parts, so it is per-file; `STAGED_STALE` is a property of one
//! hunk's index copy, so it is per-hunk and rolls up into
//! `PARTIALLY_STAGED` on the file. Neither enum can spell a token itself,
//! so a per-hunk `●` and a per-file `●` cannot drift apart.

/// Staging set: nothing staged. Reachable at both levels.
pub const UNSTAGED: char = '○';

/// Staging set: fully staged. Reachable at both levels.
pub const STAGED: char = '●';

/// Staging set: some of a file's hunks staged — per-file only, since a
/// hunk is atomic. Staged-stale hunks count toward it (ADR 0003).
/// Unreachable for a binary file, whose one whole-file hunk leaves it
/// all-or-nothing (ADR 0009).
pub const PARTIALLY_STAGED: char = '◐';

/// Staging set: staged, then the index and worktree diverged — per-hunk
/// only. Derives by OID compare on a whole-file hunk (ADR 0009).
pub const STAGED_STALE: char = '◑';

/// The from→to / repo→branch separator shared by advisories, log echoes and
/// the Status line.
pub const ARROW: char = '→';

/// The active-changelist marker.
pub const ACTIVE_MARKER: char = '*';

/// One switch target as a text listing prints it: the marker column,
/// then the label. Both of the CLI's listings compose their lines here —
/// `status`'s group headers and the bare `changelist` listing — so the
/// column they share cannot come to disagree about its width or its
/// glyph, which two spellings of `{marker} {label}` would do silently
/// (ADR 0006's one-home rule for shared tokens).
pub fn target_line(active: bool, label: &str) -> String {
    let marker = if active { ACTIVE_MARKER } else { ' ' };
    format!("{marker} {label}")
}

/// The inline separator between clauses in one-line summaries and hint
/// lines.
pub const SEPARATOR: char = '·';

/// What becomes of hunks a delete released, as every line that mentions
/// them spells it: the records guard's refusal and the forced release's
/// notice both end on this clause, so the promise a caller may read twice
/// is one sentence (ADR 0006's one-home rule).
///
/// It names the **mechanism and never a destination** (#122 §Forecasts):
/// where a released hunk lands is context-derived twice over — an
/// intervening `switch` moves it, and the index-entry-unit rule overrides
/// it hunk by hunk — so the claiming refresh's own receipt is the only
/// honest report of where it went.
pub const FOR_THE_NEXT_REFRESH: &str = "for the next persisting refresh to claim";

/// What a **dormant** record's pruning costs, as the two lines that
/// mention one spell it. A dormant record claims a hunk that has left the
/// diff, so its deletion releases nothing — there is nothing there to
/// release. What it ends is the revival that record was waiting for
/// (ADR 0002), which is the stake [`FOR_THE_NEXT_REFRESH`] would misstate.
pub const NO_REVIVAL: &str = "nothing they claimed is restored if it comes back";

/// The reserved name and display label for the unassigned pseudo-
/// changelist (`CONTEXT.md`'s "Unassigned"): hunks no changelist owns.
/// Kept as one constant so the name users are barred from and the label
/// frontends print can never drift apart.
pub const UNASSIGNED: &str = "unassigned";

/// The op target a user-typed name denotes (`CONTEXT.md`'s *Assign*:
/// "the target is any changelist or `unassigned`"): the reserved
/// `unassigned` label denotes the pseudo-changelist, which every core op
/// spells `None`; any other name denotes that changelist. Reading the
/// label back sits beside the constant that defines it, so no frontend
/// re-spells the reserved name.
pub fn target_named(name: &str) -> Option<&str> {
    (name != UNASSIGNED).then_some(name)
}

/// A hunk's holder as prose: `'feature'` for a changelist, the bare
/// reserved label for unassigned (`None`), which is not quoted because it
/// is not a name anyone chose. Shared by every line that names a holder —
/// the assign popup's provenance tail, the commit's foreign-content
/// refusal (ADR 0004) — so the two cannot spell one holder two ways.
pub fn holder_label(holder: Option<&str>) -> String {
    match holder {
        Some(name) => format!("'{name}'"),
        None => UNASSIGNED.to_owned(),
    }
}

/// `'a', 'b', 'c'` — changelist names as quoted prose, each spelled by
/// [`holder_label`] so one holder reads the same wherever it is listed
/// (ADR 0006). These are always real names, never unassigned.
pub(crate) fn quoted_list(names: &[String]) -> String {
    names
        .iter()
        .map(|name| holder_label(Some(name)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The unrecognised-name sentence the **noun command** refuses with
/// (#149): the name that matched nothing, and the changelists a retry can
/// name instead — gh's error shape (#122), so a typo costs one round trip.
///
/// Real changelists only. Unassigned is the absence of membership
/// (ADR 0016) and `all` a pseudo-view, so no mode of `changelist` has a
/// meaning for either, and both read here as simply unrecognised. That is
/// what makes this a different sentence from the CLI's `changelist_scopes`,
/// whose subject is the scopes a *verb* takes and which does include
/// unassigned.
///
/// Shared by the two modes that name an existing changelist — delete's
/// offender list and rename's `<old>` — so one repository answers one list.
pub fn unknown_changelist(name: &str, candidates: &[String]) -> String {
    let known = match candidates.is_empty() {
        true => "this repository has no changelists".to_owned(),
        false => format!("the changelists are: {}", quoted_list(candidates)),
    };
    format!("no changelist named '{name}' — {known}")
}

/// The reserved name and display label for the All pseudo-view
/// (`CONTEXT.md`'s "All"): every changed file grouped by changelist.
pub const ALL: &str = "all";

/// The display label for the Conflicts group (ADR 0007's quarantined
/// unmerged paths). Not a reserved name — the group is derived, never
/// user-created — but shared by `GroupKind::label` and the TUI's scope
/// title, which would otherwise spell it twice.
pub const CONFLICTS: &str = "conflicts";

/// "1 hunk" / "2 hunks" — the count-plus-noun shape the commit modals
/// keep needing.
pub fn count_noun(count: usize, noun: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{plural}")
}

/// The shared stem of ADR 0007's quarantine sentence: conflicted paths
/// are excluded from the hunk universe and point the user elsewhere
/// rather than offering a staging action gitchange can't complete.
pub const RESOLVE_OUTSIDE_GITCHANGE: &str = "resolve outside gitchange";

/// The full quarantine sentence naming the path — the refusal on a
/// staging attempt and the assign-popup's empty-payload reason both need
/// exactly this.
pub fn conflicted_hint(path: &str) -> String {
    format!("{path} is conflicted — {RESOLVE_OUTSIDE_GITCHANGE}")
}
