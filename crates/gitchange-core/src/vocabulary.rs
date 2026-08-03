//! Shared presentation vocabulary that isn't attached to a domain type
//! (ADR 0006: one phrasing, sunk into core, so the TUI and CLI can't
//! drift). Enum-attached vocabulary — `HunkStage::glyph`,
//! `FileStage::glyph`, `ChangeKind::sigil`, `GitOperation::label` — stays
//! with its enum; this module holds the free-standing tokens and
//! phrasing helpers that have no enum to live on.

/// The from→to / repo→branch separator shared by notices, log echoes and
/// the Status line.
pub const ARROW: char = '→';

/// The active-changelist marker.
pub const ACTIVE_MARKER: char = '*';

/// The inline separator between clauses in one-line summaries and hint
/// lines.
pub const SEPARATOR: char = '·';

/// "1 hunk" / "2 hunks" — the count-plus-noun shape the commit modals
/// keep needing.
pub fn count_noun(count: usize, noun: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{plural}")
}
