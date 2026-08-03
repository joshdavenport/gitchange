//! Tokenised glyphs and colours (spec: replaceable, no opinionated
//! theme). Defaults stick to the terminal's own ANSI palette; swapping a
//! theme means swapping this struct, never touching the renderer.

use gitchange_core::{ACTIVE_MARKER, ARROW, FileStage, HunkStage, SEPARATOR};
use ratatui::style::Color;

pub struct Theme {
    pub colors: Colors,
    pub glyphs: Glyphs,
}

pub struct Colors {
    pub border: Color,
    pub border_focus: Color,
    pub title: Color,
    pub dim: Color,
    pub text: Color,
    /// Changelist names.
    pub changelist: Color,
    /// The unassigned warning tint.
    pub warn: Color,
    /// The active-changelist marker.
    pub active: Color,
    pub branch: Color,
    pub staged: Color,
    pub added: Color,
    pub deleted: Color,
    pub modified: Color,
    pub conflicted: Color,
    pub hunk_header: Color,
    /// Selected-row background.
    pub selection: Color,
    /// The persistent selection cursor (issue #45) — distinct from
    /// `active` and `changelist`, whose glyphs it sits beside.
    pub cursor: Color,
}

pub struct Glyphs {
    pub staged: char,
    pub partially_staged: char,
    pub unstaged: char,
    pub staged_stale: char,
    pub active: char,
    pub all: char,
    pub unassigned: char,
    pub group: char,
    pub refreshing: char,
    /// The Status panel's leading mark.
    pub ok: char,
    /// The Status panel's repo → branch separator.
    pub arrow: char,
    /// The inline separator between clauses in one-line summaries and
    /// hint lines (e.g. key hints, the ◑-stale payload tail).
    pub separator: char,
    /// Log severities (ADR 0007): their own tokens, distinct from the
    /// staging set and the unassigned marker.
    pub log_info: char,
    pub log_notice: char,
    pub log_error: char,
    /// The pinned-condition marker at the top of the Log panel.
    pub pin: char,
    /// Diff hunk-tag delimiters — the terminal stand-in for the
    /// prototype's bordered pill.
    pub tag_open: char,
    pub tag_close: char,
    /// The persistent selection cursor (issue #45).
    pub cursor: char,
    /// The keybar's key/label-pair gap: spacing, not a separator glyph
    /// (unlike `separator`, deliberately airy rather than dense) — kept
    /// distinct from the modal hint lines' dim middot. Two characters, so
    /// `&'static str` rather than `char` like the rest of this struct.
    pub keybar_gap: &'static str,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            colors: Colors {
                border: Color::DarkGray,
                border_focus: Color::Green,
                title: Color::Cyan,
                dim: Color::DarkGray,
                text: Color::Reset,
                changelist: Color::Magenta,
                warn: Color::Yellow,
                active: Color::Green,
                branch: Color::Yellow,
                staged: Color::Green,
                added: Color::Green,
                deleted: Color::Red,
                modified: Color::Yellow,
                conflicted: Color::Red,
                hunk_header: Color::Cyan,
                selection: Color::DarkGray,
                cursor: Color::Cyan,
            },
            glyphs: Glyphs {
                // Staging defaults, the active marker, the arrow and the
                // inline separator come from core (ADR 0006) so the CLI
                // and TUI can't drift; a theme may still override.
                staged: FileStage::Staged.glyph(),
                partially_staged: FileStage::PartiallyStaged.glyph(),
                unstaged: FileStage::Unstaged.glyph(),
                staged_stale: HunkStage::StagedStale.glyph(),
                active: ACTIVE_MARKER,
                all: '≡',
                unassigned: '!',
                group: '▾',
                refreshing: '⟳',
                ok: '✓',
                arrow: ARROW,
                separator: SEPARATOR,
                log_info: '·',
                log_notice: '!',
                log_error: '✗',
                pin: '▲',
                tag_open: '⟨',
                tag_close: '⟩',
                cursor: '❯',
                keybar_gap: "  ",
            },
        }
    }
}
