//! Shared presentation vocabulary that isn't attached to a domain type
//! (ADR 0006: one phrasing, sunk into core, so the TUI and CLI can't
//! drift). Enum-attached vocabulary — `HunkStage::glyph`,
//! `FileStage::glyph`, `ChangeKind::sigil`, `GitOperation::label`,
//! `GroupKind::label` — stays with its enum; this module holds the
//! free-standing tokens and phrasing helpers that have no enum to live
//! on, including `UNASSIGNED`/`ALL` below, which several enums' arms
//! (and `state`'s non-enum `RESERVED_NAMES`) all point back to.

/// The from→to / repo→branch separator shared by notices, log echoes and
/// the Status line.
pub const ARROW: char = '→';

/// The active-changelist marker.
pub const ACTIVE_MARKER: char = '*';

/// The inline separator between clauses in one-line summaries and hint
/// lines.
pub const SEPARATOR: char = '·';

/// The reserved name and display label for the unassigned pseudo-
/// changelist (`CONTEXT.md`'s "Unassigned"): hunks no changelist owns.
/// Kept as one constant so the name users are barred from and the label
/// frontends print can never drift apart.
pub const UNASSIGNED: &str = "unassigned";

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
