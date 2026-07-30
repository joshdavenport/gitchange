//! Tokenised glyphs and colours (spec: replaceable, no opinionated
//! theme). Defaults stick to the terminal's own ANSI palette; swapping a
//! theme means swapping this struct, never touching the renderer.

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
    /// Error lines (the Log placeholder until #34).
    pub error: char,
    /// Diff hunk-tag delimiters — the terminal stand-in for the
    /// prototype's bordered pill.
    pub tag_open: char,
    pub tag_close: char,
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
            },
            glyphs: Glyphs {
                staged: '●',
                partially_staged: '◐',
                unstaged: '○',
                staged_stale: '◑',
                active: '*',
                all: '≡',
                unassigned: '!',
                group: '▾',
                refreshing: '⟳',
                ok: '✓',
                arrow: '→',
                error: '✗',
                tag_open: '⟨',
                tag_close: '⟩',
            },
        }
    }
}
