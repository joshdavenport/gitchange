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
